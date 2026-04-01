use agent_client_protocol::{
    AuthMethod, AuthenticateRequest, AuthenticateResponse, CancelNotification, Error,
    ListSessionsRequest, ListSessionsResponse, LoadSessionRequest, LoadSessionResponse,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, SessionConfigOption,
    SessionConfigOptionCategory, SessionConfigSelectOption, SessionId, SessionInfo,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, SetSessionModeRequest,
    SetSessionModeResponse, SetSessionModelRequest, SetSessionModelResponse, StopReason,
};
use std::sync::{Arc, Mutex};
use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
    process::{Command as StdCommand, Stdio},
    rc::Rc,
};
use tokio::{io::AsyncReadExt, process::Command as TokioCommand, sync::watch};
use tracing::{debug, info};
use uuid::Uuid;

use crate::{
    backend::{BackendDriver, BackendKind},
    cli_common::{prompt_blocks_to_text, send_agent_text},
    session_store::{GlobalSessionIndex, SessionStore},
};

#[derive(serde::Deserialize)]
struct ClaudeAuthStatus {
    #[serde(rename = "loggedIn")]
    logged_in: bool,
}

struct ClaudeSession {
    cwd: PathBuf,
    model: Option<String>,
    history: Vec<(String, String)>,
    session_store: Option<SessionStore>,
    active_prompt: Option<watch::Sender<bool>>,
}

struct CommandRunResult {
    stdout: String,
    exit_code: Option<i32>,
    cancelled: bool,
}

/// Claude Code backend driver (shells out to the `claude` CLI).
///
/// This is intentionally minimal:
/// - `new_session` creates an in-memory session ID
/// - `prompt` runs `claude --print` and streams the response as a single ACP message chunk
pub struct ClaudeCodeDriver {
    sessions: Rc<RefCell<HashMap<SessionId, ClaudeSession>>>,
    global_session_index: Option<Arc<Mutex<GlobalSessionIndex>>>,
}

impl ClaudeCodeDriver {
    const AUTH_METHOD_ID: &'static str = "claude-cli";

    pub fn new() -> Self {
        Self {
            sessions: Rc::default(),
            global_session_index: GlobalSessionIndex::load().map(|idx| Arc::new(Mutex::new(idx))),
        }
    }

    fn bin() -> String {
        std::env::var("XSFIRE_CLAUDE_BIN").unwrap_or_else(|_| "claude".to_string())
    }

    fn extra_args() -> Vec<String> {
        std::env::var("XSFIRE_CLAUDE_ARGS")
            .ok()
            .and_then(|s| shlex::split(&s))
            .unwrap_or_default()
    }

    fn default_model() -> Option<String> {
        std::env::var("XSFIRE_CLAUDE_MODEL").ok()
    }

    fn help_text() -> String {
        format!(
            "Claude commands:\n- /status\n- /model <name>\n- /reset\n\n{}",
            BackendKind::ClaudeCode
                .work_orchestration_profile()
                .render_summary(),
        )
    }

    fn status_text(model: &str, history_turns: usize) -> String {
        let profile = BackendKind::ClaudeCode.work_orchestration_profile();
        format!(
            "Claude session status:\n- model: {model}\n- history_turns: {history_turns}\n- task_orchestration: {}\n- task_monitoring: {}\n- progress_vector_checks: {}\n- preempt_on_new_prompt: {}\n- acp_bridge: {}\n- work_orchestration_sequence: {} ({})\n- operator_hint: {}",
            profile.task_orchestration,
            profile.task_monitoring,
            profile.vector_checks_value(),
            profile.preempt_value(),
            profile.bridge_summary(),
            crate::backend::WorkOrchestrationProfile::SEQUENCE,
            crate::backend::WorkOrchestrationProfile::GLOSSARY,
            profile.operator_hint,
        )
    }

    fn validate_auth_method(method_id: &str) -> Result<(), Error> {
        if method_id == Self::AUTH_METHOD_ID {
            return Ok(());
        }

        Err(Error::invalid_params().data(format!(
            "unsupported auth method for claude backend: {method_id}"
        )))
    }

    fn check_auth_status(bin: &str) -> Result<(), Error> {
        let output = StdCommand::new(bin)
            .args(["auth", "status"])
            .output()
            .map_err(|e| {
            Error::invalid_params().data(format!(
                "failed to execute Claude CLI ({bin}). Install it or set XSFIRE_CLAUDE_BIN. Error: {e}"
            ))
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if output.status.success() {
            if let Ok(status) = serde_json::from_str::<ClaudeAuthStatus>(&stdout)
                && !status.logged_in
            {
                return Err(Error::invalid_params().data(
                    "Claude CLI is installed but not authenticated. Run `claude auth login` first.",
                ));
            }
            return Ok(());
        }

        Err(Error::invalid_params().data(format!(
            "Claude CLI is installed but not ready for ACP authenticate. Run `claude auth login` first. exit={:?} stdout={} stderr={}",
            output.status.code(),
            stdout.trim(),
            stderr.trim(),
        )))
    }

    fn config_options(current_model: Option<String>) -> Vec<SessionConfigOption> {
        let current_value = current_model.unwrap_or_else(|| "default".to_string());
        vec![
            SessionConfigOption::select(
                "model",
                "Model",
                current_value,
                vec![
                    SessionConfigSelectOption::new("default", "Default"),
                    SessionConfigSelectOption::new("opus", "Opus"),
                    SessionConfigSelectOption::new("sonnet", "Sonnet"),
                    SessionConfigSelectOption::new("haiku", "Haiku"),
                ],
            )
            .category(SessionConfigOptionCategory::Model)
            .description("Model used by Claude CLI for this session"),
        ]
    }

    async fn run_claude(
        &self,
        cwd: PathBuf,
        model: Option<String>,
        prompt: String,
        mut cancel_rx: watch::Receiver<bool>,
    ) -> Result<CommandRunResult, Error> {
        let bin = Self::bin();
        let bin_display = bin.clone();
        let extra_args = Self::extra_args();

        let mut cmd = TokioCommand::new(&bin);
        cmd.arg("--print");
        cmd.arg("--cwd");
        cmd.arg(&cwd);
        if let Some(model) = model {
            cmd.arg("--model");
            cmd.arg(model);
        }
        cmd.args(extra_args);
        cmd.arg(prompt);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| {
            Error::invalid_params().data(format!(
                "failed to execute Claude CLI ({bin_display}). Install it or set XSFIRE_CLAUDE_BIN. Error: {e}"
            ))
        })?;

        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::internal_error().data("Claude CLI stdout pipe missing"))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::internal_error().data("Claude CLI stderr pipe missing"))?;

        let stdout_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            stdout
                .read_to_end(&mut buf)
                .await
                .map(|_| buf)
                .map_err(|e| Error::internal_error().data(e.to_string()))
        });
        let stderr_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            stderr
                .read_to_end(&mut buf)
                .await
                .map(|_| buf)
                .map_err(|e| Error::internal_error().data(e.to_string()))
        });

        let (status, cancelled) = tokio::select! {
            result = child.wait() => (
                result.map_err(|e| Error::internal_error().data(e.to_string()))?,
                false,
            ),
            changed = cancel_rx.changed() => {
                if changed.is_ok() && *cancel_rx.borrow() {
                    drop(child.start_kill());
                    (
                        child.wait().await.map_err(|e| Error::internal_error().data(e.to_string()))?,
                        true,
                    )
                } else {
                    (
                        child.wait().await.map_err(|e| Error::internal_error().data(e.to_string()))?,
                        false,
                    )
                }
            }
        };

        let stdout_bytes = stdout_task
            .await
            .map_err(|e| Error::internal_error().data(e.to_string()))??;
        let stderr_bytes = stderr_task
            .await
            .map_err(|e| Error::internal_error().data(e.to_string()))??;
        let stdout = String::from_utf8_lossy(&stdout_bytes).to_string();
        let stderr = String::from_utf8_lossy(&stderr_bytes).to_string();

        if !cancelled && !status.success() {
            return Err(Error::internal_error().data(format!(
                "Claude CLI failed (exit {:?}). stderr:\n{stderr}",
                status.code()
            )));
        }

        Ok(CommandRunResult {
            stdout,
            exit_code: status.code(),
            cancelled,
        })
    }
}

enum ClaudeCommand {
    Help,
    Status,
    Reset,
    SetModel(String),
}

fn parse_claude_command(input: &str) -> Option<ClaudeCommand> {
    let trimmed = input.trim();
    if trimmed == "/help" {
        return Some(ClaudeCommand::Help);
    }
    if trimmed == "/status" {
        return Some(ClaudeCommand::Status);
    }
    if trimmed == "/reset" {
        return Some(ClaudeCommand::Reset);
    }
    if let Some(value) = trimmed.strip_prefix("/model ") {
        let model = value.trim();
        if !model.is_empty() {
            return Some(ClaudeCommand::SetModel(model.to_string()));
        }
    }
    None
}

#[async_trait::async_trait(?Send)]
impl BackendDriver for ClaudeCodeDriver {
    fn backend_kind(&self) -> BackendKind {
        BackendKind::ClaudeCode
    }

    fn auth_methods(&self) -> Vec<AuthMethod> {
        vec![AuthMethod::new(
            Self::AUTH_METHOD_ID,
            "Claude CLI (pre-authenticated)",
        )
        .description("Authenticate using the `claude` CLI before starting. ACP authenticate validates `claude auth status`, but does not launch an interactive login flow.")]
    }

    async fn authenticate(
        &self,
        request: AuthenticateRequest,
    ) -> Result<AuthenticateResponse, Error> {
        Self::validate_auth_method(request.method_id.0.as_ref())?;
        Self::check_auth_status(&Self::bin())?;
        Ok(AuthenticateResponse::new())
    }

    async fn new_session(&self, request: NewSessionRequest) -> Result<NewSessionResponse, Error> {
        let session_id = SessionId::new(format!("claude:{}", Uuid::new_v4()));
        let cwd = request.cwd;
        let session_store = self.init_session_store(&session_id, &cwd);

        self.sessions.borrow_mut().insert(
            session_id.clone(),
            ClaudeSession {
                cwd,
                model: Self::default_model(),
                history: Vec::new(),
                session_store,
                active_prompt: None,
            },
        );

        info!("Created Claude session: {session_id:?}");
        let model = self
            .sessions
            .borrow()
            .get(&session_id)
            .and_then(|s| s.model.clone());
        Ok(NewSessionResponse::new(session_id).config_options(Self::config_options(model)))
    }

    async fn load_session(
        &self,
        _request: LoadSessionRequest,
    ) -> Result<LoadSessionResponse, Error> {
        Err(Error::invalid_params().data(
            "load_session is not supported for --backend=claude-code yet (sessions are in-memory).",
        ))
    }

    async fn list_sessions(
        &self,
        _request: ListSessionsRequest,
    ) -> Result<ListSessionsResponse, Error> {
        let sessions = self
            .sessions
            .borrow()
            .iter()
            .map(|(id, s)| {
                SessionInfo::new(id.clone(), s.cwd.clone()).title("Claude Code (in-memory)")
            })
            .collect::<Vec<_>>();
        Ok(ListSessionsResponse::new(sessions))
    }

    async fn prompt(&self, request: PromptRequest) -> Result<PromptResponse, Error> {
        let session_id = request.session_id.clone();
        let user_text = prompt_blocks_to_text(&request.prompt);
        debug!(
            "Claude prompt (session={session_id:?}) chars={}",
            user_text.len()
        );

        {
            let sessions = self.sessions.borrow();
            if let Some(session) = sessions.get(&session_id)
                && let Some(store) = &session.session_store
            {
                store.log("acp.prompt", serde_json::json!({ "text": user_text }));
            }
        }

        if let Some(command) = parse_claude_command(&user_text) {
            let message = {
                let mut sessions = self.sessions.borrow_mut();
                let Some(session) = sessions.get_mut(&session_id) else {
                    return Err(Error::resource_not_found(None));
                };
                match command {
                    ClaudeCommand::Help => Self::help_text(),
                    ClaudeCommand::Status => {
                        let model = session.model.as_deref().unwrap_or("default");
                        Self::status_text(model, session.history.len())
                    }
                    ClaudeCommand::Reset => {
                        session.history.clear();
                        "Claude session history has been reset.".to_string()
                    }
                    ClaudeCommand::SetModel(model) => {
                        let normalized = if model == "default" {
                            None
                        } else {
                            Some(model)
                        };
                        session.model = normalized.clone();
                        let model_text = normalized.as_deref().unwrap_or("default");
                        format!("Claude model set to `{model_text}`.")
                    }
                }
            };
            {
                let sessions = self.sessions.borrow();
                if let Some(session) = sessions.get(&session_id)
                    && let Some(store) = &session.session_store
                {
                    store.log(
                        "acp.agent_message_chunk",
                        serde_json::json!({ "text": message }),
                    );
                }
            }
            send_agent_text(&session_id, message).await;
            return Ok(PromptResponse::new(StopReason::EndTurn));
        }

        let (cwd, model, full_prompt, cancel_rx) = {
            let mut sessions = self.sessions.borrow_mut();
            let Some(session) = sessions.get_mut(&session_id) else {
                return Err(Error::resource_not_found(None));
            };
            if session.active_prompt.is_some() {
                return Err(Error::invalid_params().data(format!(
                    "a Claude prompt is already running for session {}",
                    session_id
                )));
            }

            // Keep a short running transcript to preserve basic continuity.
            // If needed, users can override this behavior by embedding their own context in the prompt.
            let mut full_prompt = String::new();
            for (user, assistant) in session.history.iter().rev().take(6).rev() {
                full_prompt.push_str("User:\n");
                full_prompt.push_str(user);
                full_prompt.push_str("\n\nAssistant:\n");
                full_prompt.push_str(assistant);
                full_prompt.push_str("\n\n");
            }
            full_prompt.push_str("User:\n");
            full_prompt.push_str(&user_text);

            let (cancel_tx, cancel_rx) = watch::channel(false);
            session.active_prompt = Some(cancel_tx);

            (
                session.cwd.clone(),
                session.model.clone(),
                full_prompt,
                cancel_rx,
            )
        };

        let output = self.run_claude(cwd, model, full_prompt, cancel_rx).await;
        {
            let mut sessions = self.sessions.borrow_mut();
            if let Some(session) = sessions.get_mut(&session_id) {
                session.active_prompt = None;
            }
        }
        let output = output?;
        if output.cancelled {
            debug!(
                "Claude prompt cancelled for session {} (exit {:?})",
                session_id, output.exit_code
            );
            return Ok(PromptResponse::new(StopReason::Cancelled));
        }

        let output_text = output.stdout.trim_end_matches('\n').to_string();
        {
            let sessions = self.sessions.borrow();
            if let Some(session) = sessions.get(&session_id)
                && let Some(store) = &session.session_store
            {
                store.log(
                    "acp.agent_message_chunk",
                    serde_json::json!({ "text": output_text }),
                );
            }
        }
        send_agent_text(&session_id, &output_text).await;

        {
            let mut sessions = self.sessions.borrow_mut();
            let Some(session) = sessions.get_mut(&session_id) else {
                return Err(Error::resource_not_found(None));
            };
            session.history.push((user_text, output_text));
        }

        Ok(PromptResponse::new(StopReason::EndTurn))
    }

    async fn cancel(&self, args: CancelNotification) -> Result<(), Error> {
        let sessions = self.sessions.borrow();
        let Some(session) = sessions.get(&args.session_id) else {
            return Err(Error::resource_not_found(None));
        };
        if let Some(cancel_tx) = &session.active_prompt {
            let _ = cancel_tx.send(true);
        }
        Ok(())
    }

    async fn set_session_mode(
        &self,
        _args: SetSessionModeRequest,
    ) -> Result<SetSessionModeResponse, Error> {
        Err(Error::invalid_params()
            .data("set_session_mode is not supported for --backend=claude-code yet."))
    }

    async fn set_session_model(
        &self,
        args: SetSessionModelRequest,
    ) -> Result<SetSessionModelResponse, Error> {
        let mut sessions = self.sessions.borrow_mut();
        let Some(session) = sessions.get_mut(&args.session_id) else {
            return Err(Error::resource_not_found(None));
        };
        session.model = if args.model_id.0.as_ref() == "default" {
            None
        } else {
            Some(args.model_id.0.to_string())
        };
        Ok(SetSessionModelResponse::new())
    }

    async fn set_session_config_option(
        &self,
        args: SetSessionConfigOptionRequest,
    ) -> Result<SetSessionConfigOptionResponse, Error> {
        if args.config_id.0.as_ref() != "model" {
            return Err(Error::invalid_params().data(format!(
                "unsupported config option for claude backend: {}",
                args.config_id
            )));
        }
        let model = if args.value.0.as_ref() == "default" {
            None
        } else {
            Some(args.value.0.to_string())
        };
        let mut sessions = self.sessions.borrow_mut();
        let Some(session) = sessions.get_mut(&args.session_id) else {
            return Err(Error::resource_not_found(None));
        };
        session.model = model;
        Ok(SetSessionConfigOptionResponse::new(Self::config_options(
            session.model.clone(),
        )))
    }
}

impl ClaudeCodeDriver {
    fn init_session_store(&self, session_id: &SessionId, cwd: &Path) -> Option<SessionStore> {
        let idx = self.global_session_index.as_ref()?;
        let global_id = idx
            .lock()
            .ok()
            .and_then(|mut i| i.get_or_create(&format!("claude:{}", session_id.0)))?;
        SessionStore::init(
            global_id,
            "claude-code",
            session_id.0.to_string(),
            session_id.0.to_string(),
            Some(cwd),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::ClaudeCodeDriver;
    use crate::backend::BackendDriver;
    use agent_client_protocol::{
        AuthenticateRequest, CancelNotification, NewSessionRequest, PromptRequest,
        SessionConfigKind, SetSessionConfigOptionRequest, SetSessionModelRequest, StopReason,
    };
    use std::{
        fs,
        path::{Path, PathBuf},
        time::Duration,
    };
    use uuid::Uuid;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    struct TempEnvVar {
        key: &'static str,
        previous: Option<String>,
        temp_dir: PathBuf,
    }

    impl Drop for TempEnvVar {
        fn drop(&mut self) {
            // Safe in tests when serialized by ENV_LOCK.
            unsafe {
                match &self.previous {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
            drop(fs::remove_dir_all(&self.temp_dir));
        }
    }

    fn install_fake_claude_bin() -> TempEnvVar {
        let temp_dir = std::env::temp_dir().join(format!("xsfire-claude-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&temp_dir).unwrap();
        let script_path = write_fake_claude_script(&temp_dir);
        let previous = std::env::var("XSFIRE_CLAUDE_BIN").ok();
        // Safe in tests when serialized by ENV_LOCK.
        unsafe {
            std::env::set_var("XSFIRE_CLAUDE_BIN", &script_path);
        }
        TempEnvVar {
            key: "XSFIRE_CLAUDE_BIN",
            previous,
            temp_dir,
        }
    }

    fn write_fake_claude_script(temp_dir: &Path) -> PathBuf {
        #[cfg(windows)]
        let script_path = temp_dir.join("claude.cmd");
        #[cfg(not(windows))]
        let script_path = temp_dir.join("claude");

        #[cfg(windows)]
        let script = r#"@echo off
if "%1"=="auth" if "%2"=="status" (
  echo {"loggedIn": true}
  exit /b 0
)
powershell -NoProfile -Command "Start-Sleep -Seconds 10; Write-Output 'fake claude output'"
exit /b 0
"#;

        #[cfg(not(windows))]
        let script = r#"#!/bin/sh
if [ "$1" = "auth" ] && [ "$2" = "status" ]; then
  printf '%s\n' '{"loggedIn": true}'
  exit 0
fi
sleep 10
printf '%s\n' 'fake claude output'
"#;

        fs::write(&script_path, script).unwrap();
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&script_path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&script_path, permissions).unwrap();
        }
        script_path
    }

    #[test]
    fn rejects_unknown_auth_method() {
        let err = ClaudeCodeDriver::validate_auth_method("wrong-method").unwrap_err();
        let message = serde_json::to_value(&err)
            .unwrap()
            .get("data")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        assert!(message.contains("unsupported auth method"));
    }

    #[test]
    fn missing_claude_binary_returns_helpful_error() {
        let err = ClaudeCodeDriver::check_auth_status("/definitely/missing/claude").unwrap_err();
        let message = serde_json::to_value(&err)
            .unwrap()
            .get("data")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        assert!(message.contains("Install it or set XSFIRE_CLAUDE_BIN"));
    }

    #[test]
    fn authenticate_request_uses_supported_method() {
        let request = AuthenticateRequest::new("claude-cli");
        assert_eq!(
            request.method_id.0.as_ref(),
            ClaudeCodeDriver::AUTH_METHOD_ID
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "current_thread")]
    async fn authenticate_checks_claude_status() {
        let _guard = crate::session_store::ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();
        let _bin = install_fake_claude_bin();

        let driver = ClaudeCodeDriver::new();
        let response = driver
            .authenticate(AuthenticateRequest::new("claude-cli"))
            .await;
        assert!(response.is_ok());
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "current_thread")]
    async fn cancel_stops_running_prompt() {
        let _guard = crate::session_store::ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();
        let _bin = install_fake_claude_bin();

        let driver = ClaudeCodeDriver::new();
        let cwd = std::env::current_dir().unwrap();
        let session = driver
            .new_session(NewSessionRequest::new(cwd))
            .await
            .unwrap();
        let session_id = session.session_id.clone();

        let prompt = driver.prompt(PromptRequest::new(session_id.clone(), vec!["hello".into()]));
        let cancel = async {
            for _ in 0..100 {
                let active = {
                    let sessions = driver.sessions.borrow();
                    sessions
                        .get(&session_id)
                        .and_then(|session| session.active_prompt.as_ref())
                        .is_some()
                };
                if active {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            driver
                .cancel(CancelNotification::new(session_id.clone()))
                .await
                .unwrap();
        };

        let response = tokio::time::timeout(Duration::from_secs(5), async {
            let (response, ()) = tokio::join!(prompt, cancel);
            response
        })
        .await
        .expect("prompt should stop after cancel")
        .unwrap();

        assert_eq!(response.stop_reason, StopReason::Cancelled);

        let sessions = driver.sessions.borrow();
        let session = sessions.get(&session_id).unwrap();
        assert!(session.history.is_empty());
        assert!(session.active_prompt.is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn set_session_model_updates_claude_session() {
        let driver = ClaudeCodeDriver::new();
        let cwd = std::env::current_dir().unwrap();
        let session = driver
            .new_session(NewSessionRequest::new(cwd))
            .await
            .unwrap();
        let session_id = session.session_id.clone();

        driver
            .set_session_model(SetSessionModelRequest::new(
                session_id.clone(),
                "claude-3-7-sonnet",
            ))
            .await
            .unwrap();

        let sessions = driver.sessions.borrow();
        let session = sessions.get(&session_id).unwrap();
        assert_eq!(session.model.as_deref(), Some("claude-3-7-sonnet"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn set_session_config_option_updates_model_and_rejects_other_options() {
        let driver = ClaudeCodeDriver::new();
        let cwd = std::env::current_dir().unwrap();
        let session = driver
            .new_session(NewSessionRequest::new(cwd))
            .await
            .unwrap();
        let session_id = session.session_id.clone();

        let response = driver
            .set_session_config_option(SetSessionConfigOptionRequest::new(
                session_id.clone(),
                "model",
                "claude-3-5-haiku",
            ))
            .await
            .unwrap();

        {
            let sessions = driver.sessions.borrow();
            let session = sessions.get(&session_id).unwrap();
            assert_eq!(session.model.as_deref(), Some("claude-3-5-haiku"));
        }
        let selected_model = response
            .config_options
            .iter()
            .find(|option| option.id.0.as_ref() == "model")
            .map(|option| match &option.kind {
                SessionConfigKind::Select(select) => select.current_value.0.to_string(),
                _ => panic!("model config option should be a select"),
            });
        assert_eq!(selected_model.as_deref(), Some("claude-3-5-haiku"));

        let error = driver
            .set_session_config_option(SetSessionConfigOptionRequest::new(
                session_id,
                "temperature",
                "0.2",
            ))
            .await
            .unwrap_err();
        let message = serde_json::to_value(&error)
            .unwrap()
            .get("data")
            .and_then(|value| value.as_str())
            .unwrap()
            .to_string();
        assert!(message.contains("unsupported config option for claude backend"));
    }

    #[test]
    fn claude_help_text_exposes_work_orchestration_profile() {
        let help = ClaudeCodeDriver::help_text();
        assert!(help.contains("Claude commands:"));
        assert!(help.contains("Work orchestration profile: Claude Code"));
        assert!(help.contains("Goal/Rubric/Next Action"));
        assert!(help.contains("single ACP message chunk only"));
    }

    #[test]
    fn claude_status_text_exposes_sequential_profile_defaults() {
        let status = ClaudeCodeDriver::status_text("claude-3-7-sonnet", 2);
        assert!(status.contains("- model: claude-3-7-sonnet"));
        assert!(status.contains("- history_turns: 2"));
        assert!(status.contains("- task_orchestration: sequential"));
        assert!(status.contains("- task_monitoring: status-only"));
        assert!(status.contains("- progress_vector_checks: off"));
        assert!(status.contains("- preempt_on_new_prompt: off"));
        assert!(status.contains("- work_orchestration_sequence: R->P->M->W->A"));
    }
}
