use regex_lite::Regex;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};
use tracing::{error, warn};
use uuid::Uuid;

static SECRET_REDACTION_RE: OnceLock<Regex> = OnceLock::new();
#[cfg(test)]
pub(crate) static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[cfg(test)]
pub(crate) fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn secret_redaction_re() -> &'static Regex {
    SECRET_REDACTION_RE.get_or_init(|| {
        // Very rough: redact common "sk-..." style tokens.
        Regex::new(r"\b(sk-[A-Za-z0-9]{20,})\b").unwrap_or_else(|_| std::process::abort())
    })
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn redact_string(s: &str) -> String {
    secret_redaction_re()
        .replace_all(s, "sk-REDACTED")
        .into_owned()
}

fn redact_json(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::String(s) => serde_json::Value::String(redact_string(&s)),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(redact_json).collect())
        }
        serde_json::Value::Object(map) => {
            serde_json::Value::Object(map.into_iter().map(|(k, v)| (k, redact_json(v))).collect())
        }
        other => other,
    }
}

pub struct AcpHome;

impl AcpHome {
    pub fn resolve() -> Option<PathBuf> {
        if let Ok(v) = std::env::var("ACP_HOME")
            && !v.trim().is_empty()
        {
            return Some(PathBuf::from(v));
        }
        let home = std::env::var("HOME").ok()?;
        if home.trim().is_empty() {
            return None;
        }
        Some(PathBuf::from(home).join(".acp"))
    }
}

#[derive(Debug, Clone)]
pub struct GlobalSessionIndex {
    path: PathBuf,
    map: BTreeMap<String, String>,
}

impl GlobalSessionIndex {
    pub fn load() -> Option<Self> {
        let root = AcpHome::resolve()?;
        let path = root.join("index.json");
        let map = match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => {
                warn!("Failed to read ACP session index {}: {}", path.display(), e);
                BTreeMap::new()
            }
        };
        Some(Self { path, map })
    }

    pub fn get_or_create(&mut self, key: &str) -> Option<String> {
        if let Some(existing) = self.map.get(key) {
            return Some(existing.clone());
        }
        let id = Uuid::new_v4().to_string();
        self.map.insert(key.to_string(), id.clone());
        if let Err(e) = self.save() {
            warn!("Failed to save ACP session index: {e}");
        }
        Some(id)
    }

    fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let data = serde_json::to_string_pretty(&self.map).unwrap_or_else(|_| "{}".to_string());
        std::fs::write(&tmp, data)?;
        std::fs::rename(tmp, &self.path)?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct SessionStore {
    inner: Arc<SessionStoreInner>,
}

struct SessionStoreInner {
    global_session_id: String,
    backend: String,
    acp_session_id: String,
    backend_session_id: String,
    #[allow(dead_code)]
    root: PathBuf,
    canonical_file: Mutex<File>,
}

#[derive(Serialize)]
struct SessionState {
    schema_version: u32,
    created_at_ms: u64,
    global_session_id: String,
    backend: String,
    acp_session_id: String,
    backend_session_id: String,
    cwd: Option<String>,
}

#[derive(Serialize)]
struct CanonicalEvent {
    schema_version: u32,
    ts_ms: u64,
    global_session_id: String,
    backend: String,
    acp_session_id: String,
    backend_session_id: String,
    kind: String,
    data: serde_json::Value,
}

impl SessionStore {
    pub fn init(
        global_session_id: String,
        backend: impl Into<String>,
        acp_session_id: impl Into<String>,
        backend_session_id: impl Into<String>,
        cwd: Option<&Path>,
    ) -> Option<Self> {
        let acp_home = AcpHome::resolve()?;
        let backend = backend.into();
        let acp_session_id = acp_session_id.into();
        let backend_session_id = backend_session_id.into();

        let root = acp_home.join("sessions").join(&global_session_id);
        if let Err(e) = std::fs::create_dir_all(root.join("backends").join(&backend)) {
            warn!(
                "Failed to create session store directory {}: {}",
                root.display(),
                e
            );
            return None;
        }

        let state_path = root.join("state.json");
        if !state_path.exists() {
            let state = SessionState {
                schema_version: 1,
                created_at_ms: now_unix_ms(),
                global_session_id: global_session_id.clone(),
                backend: backend.clone(),
                acp_session_id: acp_session_id.clone(),
                backend_session_id: backend_session_id.clone(),
                cwd: cwd.map(|p| p.display().to_string()),
            };
            if let Ok(data) = serde_json::to_string_pretty(&state)
                && let Err(e) = std::fs::write(&state_path, data)
            {
                warn!(
                    "Failed to write session state {}: {}",
                    state_path.display(),
                    e
                );
            }
        }

        let canonical_path = root.join("canonical.jsonl");
        let canonical_file = match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&canonical_path)
        {
            Ok(f) => f,
            Err(e) => {
                warn!(
                    "Failed to open canonical session log {}: {}",
                    canonical_path.display(),
                    e
                );
                return None;
            }
        };

        Some(Self {
            inner: Arc::new(SessionStoreInner {
                global_session_id,
                backend,
                acp_session_id,
                backend_session_id,
                root,
                canonical_file: Mutex::new(canonical_file),
            }),
        })
    }

    #[allow(dead_code)]
    pub fn global_session_id(&self) -> &str {
        &self.inner.global_session_id
    }

    #[allow(dead_code)]
    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    pub fn log(&self, kind: &str, data: serde_json::Value) {
        let event = CanonicalEvent {
            schema_version: 1,
            ts_ms: now_unix_ms(),
            global_session_id: self.inner.global_session_id.clone(),
            backend: self.inner.backend.clone(),
            acp_session_id: self.inner.acp_session_id.clone(),
            backend_session_id: self.inner.backend_session_id.clone(),
            kind: kind.to_string(),
            data: redact_json(data),
        };

        let Ok(line) = serde_json::to_string(&event) else {
            return;
        };

        let mut file = match self.inner.canonical_file.lock() {
            Ok(f) => f,
            Err(_) => {
                error!("SessionStore lock poisoned");
                return;
            }
        };

        if let Err(e) = writeln!(file, "{line}") {
            error!("Failed to append canonical event: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn writes_canonical_log_and_redacts_secrets() {
        let _guard = lock_env();

        let root = std::env::temp_dir().join(format!("acp-session-store-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();

        // Safe within this test due to ENV_LOCK serialization.
        unsafe {
            std::env::set_var("ACP_HOME", &root);
        }

        let mut idx = GlobalSessionIndex::load().expect("ACP_HOME should be resolvable");
        let global_id = idx.get_or_create("codex:test-session").unwrap();

        let store = SessionStore::init(
            global_id.clone(),
            "codex",
            "acp-session-id",
            "backend-session-id",
            None,
        )
        .expect("SessionStore should init");

        store.log(
            "test.event",
            json!({
                "token": "sk-1234567890123456789012345",
                "nested": { "t": "sk-aaaaaaaaaaaaaaaaaaaaaa" }
            }),
        );

        let canonical_path = root
            .join("sessions")
            .join(&global_id)
            .join("canonical.jsonl");
        let s = std::fs::read_to_string(&canonical_path).unwrap();
        assert!(s.contains("sk-REDACTED"), "expected redaction in: {s}");

        drop(std::fs::remove_dir_all(&root));
        // Safe within this test due to ENV_LOCK serialization.
        unsafe {
            std::env::remove_var("ACP_HOME");
        }
    }
}
