use std::{
    collections::HashMap,
    io::Cursor,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use agent_client_protocol::{
    AgentSideConnection, Client, ClientCapabilities, ReadTextFileRequest, SessionId,
    WriteTextFileRequest,
};
use codex_apply_patch::StdFs;
use tokio::sync::mpsc;

use crate::ACP_CLIENT;

#[derive(Debug)]
pub enum FsTask {
    ReadFile {
        session_id: SessionId,
        path: PathBuf,
        tx: std::sync::mpsc::Sender<std::io::Result<String>>,
    },
    ReadFileLimit {
        session_id: SessionId,
        path: PathBuf,
        limit: usize,
        tx: tokio::sync::oneshot::Sender<std::io::Result<String>>,
    },
    WriteFile {
        session_id: SessionId,
        path: PathBuf,
        content: String,
        tx: std::sync::mpsc::Sender<std::io::Result<()>>,
    },
}

impl FsTask {
    async fn run(self) {
        match self {
            FsTask::ReadFile {
                session_id,
                path,
                tx,
            } => {
                let read_text_file =
                    Self::client().read_text_file(ReadTextFileRequest::new(session_id, path));
                let response = read_text_file
                    .await
                    .map(|response| response.content)
                    .map_err(|e| std::io::Error::other(e.to_string()));
                tx.send(response).ok();
            }
            FsTask::ReadFileLimit {
                session_id,
                path,
                limit,
                tx,
            } => {
                let read_text_file = Self::client().read_text_file(
                    ReadTextFileRequest::new(session_id, path)
                        .limit(limit.try_into().unwrap_or(u32::MAX)),
                );
                let response = read_text_file
                    .await
                    .map(|response| response.content)
                    .map_err(|e| std::io::Error::other(e.to_string()));
                tx.send(response).ok();
            }
            FsTask::WriteFile {
                session_id,
                path,
                content,
                tx,
            } => {
                let response = Self::client()
                    .write_text_file(WriteTextFileRequest::new(session_id, path, content))
                    .await
                    .map(|_| ())
                    .map_err(|e| std::io::Error::other(e.to_string()));
                tx.send(response).ok();
            }
        }
    }

    fn client() -> &'static AgentSideConnection {
        ACP_CLIENT.get().expect("Missing ACP client")
    }
}

pub struct AcpFs {
    client_capabilities: Arc<Mutex<ClientCapabilities>>,
    local_spawner: LocalSpawner,
    session_id: SessionId,
    session_roots: Arc<Mutex<HashMap<SessionId, PathBuf>>>,
}

impl AcpFs {
    pub fn new(
        session_id: SessionId,
        client_capabilities: Arc<Mutex<ClientCapabilities>>,
        local_spawner: LocalSpawner,
        session_roots: Arc<Mutex<HashMap<SessionId, PathBuf>>>,
    ) -> Self {
        Self {
            client_capabilities,
            local_spawner,
            session_id,
            session_roots,
        }
    }

    fn session_root(&self) -> std::io::Result<PathBuf> {
        self.session_roots
            .lock()
            .unwrap()
            .get(&self.session_id)
            .cloned()
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "session root not registered",
                )
            })
    }

    fn ensure_within_root(&self, path: &std::path::Path) -> std::io::Result<PathBuf> {
        let root = std::path::absolute(self.session_root()?)?;
        let abs_path = std::path::absolute(path)?;
        if abs_path.starts_with(&root) {
            Ok(abs_path)
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "access to {} denied (outside session root {})",
                    abs_path.display(),
                    root.display()
                ),
            ))
        }
    }
}

impl codex_apply_patch::Fs for AcpFs {
    fn read_to_string(&self, path: &std::path::Path) -> std::io::Result<String> {
        if !self.client_capabilities.lock().unwrap().fs.read_text_file {
            return StdFs.read_to_string(path);
        }
        let path = self.ensure_within_root(path)?;
        let (tx, rx) = std::sync::mpsc::channel();
        self.local_spawner.spawn(FsTask::ReadFile {
            session_id: self.session_id.clone(),
            path,
            tx,
        });
        rx.recv()
            .map_err(|e| std::io::Error::other(e.to_string()))
            .flatten()
    }

    fn write(&self, path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
        if !self.client_capabilities.lock().unwrap().fs.write_text_file {
            return StdFs.write(path, contents);
        }
        let path = self.ensure_within_root(path)?;
        let (tx, rx) = std::sync::mpsc::channel();
        self.local_spawner.spawn(FsTask::WriteFile {
            session_id: self.session_id.clone(),
            path,
            content: String::from_utf8(contents.to_vec())
                .map_err(|e| std::io::Error::other(e.to_string()))?,
            tx,
        });
        rx.recv()
            .map_err(|e| std::io::Error::other(e.to_string()))
            .flatten()
    }
}

impl codex_core::codex::Fs for AcpFs {
    fn file_buffer(
        &self,
        path: &std::path::Path,
        limit: usize,
    ) -> std::pin::Pin<
        Box<
            dyn Future<Output = std::io::Result<Box<dyn tokio::io::AsyncBufRead + Unpin + Send>>>
                + Send,
        >,
    > {
        if !self.client_capabilities.lock().unwrap().fs.read_text_file {
            return StdFs.file_buffer(path, limit);
        }
        let path = match self.ensure_within_root(path) {
            Ok(path) => path,
            Err(e) => return Box::pin(async move { Err(e) }),
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.local_spawner.spawn(FsTask::ReadFileLimit {
            session_id: self.session_id.clone(),
            path,
            limit,
            tx,
        });
        Box::pin(async move {
            let file = rx
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))
                .flatten()?;

            Ok(Box::new(tokio::io::BufReader::new(Cursor::new(file.into_bytes()))) as _)
        })
    }
}

#[derive(Clone)]
pub struct LocalSpawner {
    send: mpsc::UnboundedSender<FsTask>,
}

impl LocalSpawner {
    pub fn new() -> Self {
        let (send, mut recv) = mpsc::unbounded_channel::<FsTask>();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        std::thread::spawn(move || {
            let local = tokio::task::LocalSet::new();

            local.spawn_local(async move {
                while let Some(new_task) = recv.recv().await {
                    tokio::task::spawn_local(new_task.run());
                }
                // If the while loop returns, then all the LocalSpawner
                // objects have been dropped.
            });

            // This will return once all senders are dropped and all
            // spawned tasks have returned.
            rt.block_on(local);
        });

        Self { send }
    }

    pub fn spawn(&self, task: FsTask) {
        self.send
            .send(task)
            .expect("Thread with LocalSet has shut down.");
    }
}

#[cfg(test)]
mod tests {
    use super::{AcpFs, LocalSpawner};
    use agent_client_protocol::{ClientCapabilities, FileSystemCapability, SessionId};
    use codex_apply_patch::Fs as ApplyPatchFs;
    use std::{
        collections::HashMap,
        fs,
        path::PathBuf,
        sync::{Arc, Mutex},
    };
    use uuid::Uuid;

    struct TempDirGuard {
        path: PathBuf,
    }

    impl TempDirGuard {
        fn new(prefix: &str) -> Self {
            let path = std::env::temp_dir().join(format!("{prefix}-{}", Uuid::new_v4()));
            fs::create_dir_all(&path).expect("failed to create temp dir");
            Self { path }
        }

        fn path(&self) -> &std::path::Path {
            &self.path
        }
    }

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _cleanup_result = fs::remove_dir_all(&self.path);
        }
    }

    fn build_fs(client_capabilities: ClientCapabilities, root: &std::path::Path) -> AcpFs {
        let session_id = SessionId::new(format!("local-spawner-test-{}", Uuid::new_v4()));
        let session_roots = Arc::new(Mutex::new(HashMap::from([(
            session_id.clone(),
            root.to_path_buf(),
        )])));
        AcpFs::new(
            session_id,
            Arc::new(Mutex::new(client_capabilities)),
            LocalSpawner::new(),
            session_roots,
        )
    }

    #[test]
    fn acp_fs_denies_out_of_root_reads_and_writes_when_client_fs_is_available() {
        let root = TempDirGuard::new("xsfire-camp-root");
        let outside = TempDirGuard::new("xsfire-camp-outside");
        let outside_file = outside.path().join("outside.txt");
        fs::write(&outside_file, "outside").expect("failed to write outside file");

        let acp_fs = build_fs(
            ClientCapabilities::new().fs(FileSystemCapability::new()
                .read_text_file(true)
                .write_text_file(true)),
            root.path(),
        );

        let read_error = ApplyPatchFs::read_to_string(&acp_fs, &outside_file)
            .expect_err("expected out-of-root read to be denied");
        assert_eq!(read_error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(
            read_error.to_string().contains("outside session root"),
            "unexpected read error: {read_error}"
        );

        let write_error = ApplyPatchFs::write(&acp_fs, &outside_file, b"mutated")
            .expect_err("expected out-of-root write to be denied");
        assert_eq!(write_error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(
            write_error.to_string().contains("outside session root"),
            "unexpected write error: {write_error}"
        );
    }

    #[test]
    fn acp_fs_falls_back_to_local_fs_when_client_fs_capability_is_disabled() {
        let root = TempDirGuard::new("xsfire-camp-root");
        let outside = TempDirGuard::new("xsfire-camp-outside");
        let outside_file = outside.path().join("outside.txt");
        fs::write(&outside_file, "outside").expect("failed to write outside file");

        let acp_fs = build_fs(ClientCapabilities::new(), root.path());

        let contents = ApplyPatchFs::read_to_string(&acp_fs, &outside_file)
            .expect("expected local fs fallback read to succeed");
        assert_eq!(contents, "outside");

        let rewritten = outside.path().join("rewritten.txt");
        ApplyPatchFs::write(&acp_fs, &rewritten, b"rewritten")
            .expect("expected local fs fallback write to succeed");
        assert_eq!(
            fs::read_to_string(&rewritten).expect("failed to read rewritten file"),
            "rewritten"
        );
    }
}
