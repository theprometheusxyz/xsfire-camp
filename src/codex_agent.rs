use agent_client_protocol::{
    AuthMethod, AuthMethodId, AuthenticateRequest, AuthenticateResponse, CancelNotification,
    ClientCapabilities, CreateTerminalRequest, Error, ForkSessionRequest, ForkSessionResponse,
    KillTerminalCommandRequest, ListSessionsRequest, ListSessionsResponse, LoadSessionRequest,
    LoadSessionResponse, McpServer, McpServerHttp, McpServerStdio, NewSessionRequest,
    NewSessionResponse, PromptRequest, PromptResponse, ReleaseTerminalRequest,
    ResumeSessionRequest, ResumeSessionResponse, SessionId, SessionInfo,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, SetSessionModeRequest,
    SetSessionModeResponse, SetSessionModelRequest, SetSessionModelResponse, TerminalOutputRequest,
    WaitForTerminalExitRequest,
};
use codex_core::{
    ExecCommandRequest, NewThread, ResponseItem, RolloutRecorder, ThreadManager, ThreadSortKey,
    UnifiedExecContext, UnifiedExecDelegate, UnifiedExecDelegateFactory, UnifiedExecError,
    UnifiedExecResponse, WriteStdinRequest,
    auth::{AuthManager, read_codex_api_key_from_env, read_openai_api_key_from_env},
    config::{
        Config,
        types::{McpServerConfig, McpServerTransportConfig},
    },
    find_thread_path_by_id_str, parse_cursor, parse_turn_item,
    protocol::{InitialHistory, SessionSource},
};
use codex_login::{AuthMode, CODEX_API_KEY_ENV_VAR, OPENAI_API_KEY_ENV_VAR};
use codex_protocol::{
    ThreadId,
    protocol::{RolloutItem, SessionMetaLine},
};
use std::{
    cell::RefCell,
    collections::HashMap,
    io::{self, Write},
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Mutex},
};
use tracing::{debug, info};
use unicode_segmentation::UnicodeSegmentation;
use uuid::Uuid;

use crate::{
    acp_create_terminal, acp_kill_terminal_command, acp_release_terminal, acp_terminal_output,
    acp_wait_for_terminal_exit,
    backend::{BackendDriver, BackendKind},
    local_spawner::{AcpFs, LocalSpawner},
    resolve_session_alias,
    session_store::{GlobalSessionIndex, SessionStore},
    thread::Thread,
};

/// Codex backend driver for ACP.
///
/// This bridges the ACP protocol with the existing codex-rs infrastructure,
/// allowing Codex to be used as an ACP backend behind a stable driver interface.
pub struct CodexDriver {
    /// Handle to the current authentication
    auth_manager: Arc<AuthManager>,
    /// Capabilities of the connected client
    client_capabilities: Arc<Mutex<ClientCapabilities>>,
    /// The underlying codex configuration
    config: Config,
    /// Thread manager for handling sessions
    thread_manager: ThreadManager,
    /// Active sessions mapped by `SessionId`
    sessions: Rc<RefCell<HashMap<SessionId, Rc<Thread>>>>,
    /// Session working directories for filesystem sandboxing
    session_roots: Arc<Mutex<HashMap<SessionId, PathBuf>>>,
    /// Optional global canonical session store, for cross-backend continuity.
    ///
    /// If `ACP_HOME` (or `$HOME`) can't be resolved, this stays disabled.
    global_session_index: Option<Arc<Mutex<GlobalSessionIndex>>>,
}

const SESSION_LIST_PAGE_SIZE: usize = 25;
const SESSION_TITLE_MAX_GRAPHEMES: usize = 120;
const XSFIRE_CODEX_OPEN_BROWSER_ENV_VAR: &str = "XSFIRE_CODEX_OPEN_BROWSER";
const ACP_TERMINAL_OUTPUT_BYTE_LIMIT_DEFAULT: u64 = 32 * 1024;
const ACP_TERMINAL_OUTPUT_BYTE_LIMIT_MIN: u64 = 4 * 1024;
const ACP_TERMINAL_OUTPUT_BYTE_LIMIT_MAX: u64 = 512 * 1024;

#[derive(Clone, Debug)]
struct AcpTerminalHandle {
    session_id: SessionId,
    terminal_id: String,
}

struct AcpUnifiedExecDelegate {
    session_id: SessionId,
    client_capabilities: Arc<Mutex<ClientCapabilities>>,
    terminals: Arc<Mutex<HashMap<String, AcpTerminalHandle>>>,
}

impl AcpUnifiedExecDelegate {
    fn new(session_id: SessionId, client_capabilities: Arc<Mutex<ClientCapabilities>>) -> Self {
        Self {
            session_id,
            client_capabilities,
            terminals: Arc::default(),
        }
    }

    fn routed_session_id(&self) -> SessionId {
        resolve_session_alias(&self.session_id)
    }

    fn supports_standard_terminal(&self) -> bool {
        self.client_capabilities.lock().unwrap().terminal
    }

    fn output_byte_limit(max_output_tokens: Option<usize>) -> u64 {
        max_output_tokens
            .map(|tokens| tokens.saturating_mul(4) as u64)
            .unwrap_or(ACP_TERMINAL_OUTPUT_BYTE_LIMIT_DEFAULT)
            .clamp(
                ACP_TERMINAL_OUTPUT_BYTE_LIMIT_MIN,
                ACP_TERMINAL_OUTPUT_BYTE_LIMIT_MAX,
            )
    }

    fn exit_code_from_terminal_status(
        exit_status: &Option<agent_client_protocol::TerminalExitStatus>,
    ) -> Option<i32> {
        exit_status
            .as_ref()
            .and_then(|status| status.exit_code)
            .and_then(|code| i32::try_from(code).ok())
    }
}

#[async_trait::async_trait]
impl UnifiedExecDelegate for AcpUnifiedExecDelegate {
    async fn exec_command(
        &self,
        request: ExecCommandRequest,
        context: &UnifiedExecContext,
    ) -> Result<UnifiedExecResponse, UnifiedExecError> {
        if !self.supports_standard_terminal() {
            return Err(UnifiedExecError::create_process(
                "ACP client does not advertise terminal support".to_string(),
            ));
        }
        let Some((command, args)) = request.command.split_first() else {
            return Err(UnifiedExecError::MissingCommandLine);
        };

        let session_id = self.routed_session_id();
        let create_request = CreateTerminalRequest::new(session_id.clone(), command.clone())
            .args(args.to_vec())
            .cwd(request.workdir.clone())
            .output_byte_limit(Self::output_byte_limit(request.max_output_tokens));
        let create_response = acp_create_terminal(create_request)
            .await
            .map_err(|err| UnifiedExecError::create_process(err.to_string()))?;
        let terminal_id = create_response.terminal_id.0.as_ref().to_string();
        let output_response = acp_terminal_output(TerminalOutputRequest::new(
            session_id.clone(),
            create_response.terminal_id.clone(),
        ))
        .await
        .map_err(|err| UnifiedExecError::create_process(err.to_string()))?;

        let still_running = output_response.exit_status.is_none();
        if still_running {
            self.terminals.lock().unwrap().insert(
                request.process_id.clone(),
                AcpTerminalHandle {
                    session_id,
                    terminal_id: terminal_id.clone(),
                },
            );
        } else {
            acp_release_terminal(ReleaseTerminalRequest::new(
                session_id,
                create_response.terminal_id,
            ))
            .await
            .map_err(|err| UnifiedExecError::create_process(err.to_string()))?;
        }

        let output = output_response.output;
        let raw_output = output.as_bytes().to_vec();
        let exit_status = output_response.exit_status;
        Ok(UnifiedExecResponse {
            event_call_id: context.call_id().to_string(),
            chunk_id: Uuid::new_v4().to_string(),
            wall_time: std::time::Duration::from_millis(0),
            output,
            raw_output,
            process_id: still_running.then(|| request.process_id.clone()),
            exit_code: Self::exit_code_from_terminal_status(&exit_status),
            original_token_count: None,
            terminal_id: Some(terminal_id),
            session_command: Some(request.command.clone()),
            session_cwd: request
                .workdir
                .clone()
                .or_else(|| Some(context.cwd().to_path_buf())),
        })
    }

    async fn write_stdin(
        &self,
        request: WriteStdinRequest,
        context: &UnifiedExecContext,
    ) -> Result<UnifiedExecResponse, UnifiedExecError> {
        if !request.input.is_empty() {
            return Err(UnifiedExecError::StdinClosed);
        }
        let handle = self
            .terminals
            .lock()
            .unwrap()
            .get(&request.process_id)
            .cloned()
            .ok_or_else(|| UnifiedExecError::UnknownProcessId {
                process_id: request.process_id.clone(),
            })?;
        let output_response = acp_terminal_output(TerminalOutputRequest::new(
            handle.session_id.clone(),
            agent_client_protocol::TerminalId::new(handle.terminal_id.clone()),
        ))
        .await
        .map_err(|err| UnifiedExecError::create_process(err.to_string()))?;

        let still_running = output_response.exit_status.is_none();
        if !still_running {
            self.terminals.lock().unwrap().remove(&request.process_id);
            acp_release_terminal(ReleaseTerminalRequest::new(
                handle.session_id.clone(),
                agent_client_protocol::TerminalId::new(handle.terminal_id.clone()),
            ))
            .await
            .map_err(|err| UnifiedExecError::create_process(err.to_string()))?;
        }

        let output = output_response.output;
        let raw_output = output.as_bytes().to_vec();
        let exit_status = output_response.exit_status;
        Ok(UnifiedExecResponse {
            event_call_id: context.call_id().to_string(),
            chunk_id: Uuid::new_v4().to_string(),
            wall_time: std::time::Duration::from_millis(0),
            output,
            raw_output,
            process_id: still_running.then(|| request.process_id.clone()),
            exit_code: Self::exit_code_from_terminal_status(&exit_status),
            original_token_count: None,
            terminal_id: Some(handle.terminal_id),
            session_command: Some(Vec::new()),
            session_cwd: Some(context.cwd().to_path_buf()),
        })
    }

    async fn terminate_all_processes(&self) -> Result<(), UnifiedExecError> {
        let handles = {
            let mut terminals = self.terminals.lock().unwrap();
            terminals
                .drain()
                .map(|(_, handle)| handle)
                .collect::<Vec<_>>()
        };
        for handle in handles {
            let terminal_id = agent_client_protocol::TerminalId::new(handle.terminal_id.clone());
            acp_kill_terminal_command(KillTerminalCommandRequest::new(
                handle.session_id.clone(),
                terminal_id.clone(),
            ))
            .await
            .map_err(|err| UnifiedExecError::create_process(err.to_string()))?;
            acp_wait_for_terminal_exit(WaitForTerminalExitRequest::new(
                handle.session_id.clone(),
                terminal_id.clone(),
            ))
            .await
            .map_err(|err| UnifiedExecError::create_process(err.to_string()))?;
            acp_release_terminal(ReleaseTerminalRequest::new(handle.session_id, terminal_id))
                .await
                .map_err(|err| UnifiedExecError::create_process(err.to_string()))?;
        }
        Ok(())
    }
}

impl CodexDriver {
    /// Create a new `CodexDriver` with the given configuration.
    pub fn new(config: Config, client_capabilities: Arc<Mutex<ClientCapabilities>>) -> Self {
        let auth_manager = AuthManager::shared(
            config.codex_home.clone(),
            false,
            config.cli_auth_credentials_store_mode,
        );

        let local_spawner = LocalSpawner::new();
        let capabilities_clone = client_capabilities.clone();
        let unified_exec_client_capabilities = client_capabilities.clone();
        let session_roots: Arc<Mutex<HashMap<SessionId, PathBuf>>> = Arc::default();
        let session_roots_clone = session_roots.clone();
        let thread_manager = ThreadManager::new_with_fs(
            config.codex_home.clone(),
            auth_manager.clone(),
            // Match Codex CLI session source so ACP sessions share the same metadata.
            SessionSource::Cli,
            Box::new(move |thread_id| {
                Arc::new(AcpFs::new(
                    Self::session_id_from_thread_id(thread_id),
                    capabilities_clone.clone(),
                    local_spawner.clone(),
                    session_roots_clone.clone(),
                ))
            }),
            Some(Arc::new(move |thread_id| {
                Arc::new(AcpUnifiedExecDelegate::new(
                    Self::session_id_from_thread_id(thread_id),
                    unified_exec_client_capabilities.clone(),
                )) as Arc<dyn UnifiedExecDelegate>
            }) as UnifiedExecDelegateFactory),
        );

        let global_session_index = GlobalSessionIndex::load().map(|idx| Arc::new(Mutex::new(idx)));
        Self {
            auth_manager,
            client_capabilities,
            config,
            thread_manager,
            sessions: Rc::default(),
            session_roots,
            global_session_index,
        }
    }

    fn session_id_from_thread_id(thread_id: ThreadId) -> SessionId {
        SessionId::new(thread_id.to_string())
    }

    fn get_thread(&self, session_id: &SessionId) -> Result<Rc<Thread>, Error> {
        Ok(self
            .sessions
            .borrow()
            .get(session_id)
            .ok_or_else(|| Error::resource_not_found(None))?
            .clone())
    }

    fn should_open_chatgpt_browser() -> bool {
        std::env::var(XSFIRE_CODEX_OPEN_BROWSER_ENV_VAR)
            .map(|value| is_truthy_env_value(&value))
            .unwrap_or(false)
    }

    async fn check_auth(&self) -> Result<(), Error> {
        if self.config.model_provider_id == "openai" && self.auth_manager.auth().await.is_none() {
            return Err(Error::auth_required());
        }
        Ok(())
    }

    /// Build a session config from base config, working directory, and MCP servers.
    /// This is shared between `new_session` and `load_session`.
    fn build_session_config(
        &self,
        cwd: &PathBuf,
        mcp_servers: Vec<McpServer>,
    ) -> Result<Config, Error> {
        let mut config = self.config.clone();
        config.include_apply_patch_tool = true;
        config.cwd.clone_from(cwd);

        // Propagate any client-provided MCP servers that codex-rs supports.
        let mut new_mcp_servers = config.mcp_servers.get().clone();
        for mcp_server in mcp_servers {
            match mcp_server {
                // Not supported in codex
                McpServer::Sse(..) => {}
                McpServer::Http(McpServerHttp {
                    name, url, headers, ..
                }) => {
                    new_mcp_servers.insert(
                        name,
                        McpServerConfig {
                            transport: McpServerTransportConfig::StreamableHttp {
                                url,
                                bearer_token_env_var: None,
                                http_headers: if headers.is_empty() {
                                    None
                                } else {
                                    Some(headers.into_iter().map(|h| (h.name, h.value)).collect())
                                },
                                env_http_headers: None,
                            },
                            enabled: true,
                            startup_timeout_sec: None,
                            tool_timeout_sec: None,
                            disabled_tools: None,
                            enabled_tools: None,
                            disabled_reason: None,
                        },
                    );
                }
                McpServer::Stdio(McpServerStdio {
                    name,
                    command,
                    args,
                    env,
                    ..
                }) => {
                    new_mcp_servers.insert(
                        name,
                        McpServerConfig {
                            transport: McpServerTransportConfig::Stdio {
                                command: command.display().to_string(),
                                args,
                                env: if env.is_empty() {
                                    None
                                } else {
                                    Some(env.into_iter().map(|env| (env.name, env.value)).collect())
                                },
                                env_vars: vec![],
                                cwd: Some(cwd.clone()),
                            },
                            enabled: true,
                            startup_timeout_sec: None,
                            tool_timeout_sec: None,
                            disabled_tools: None,
                            enabled_tools: None,
                            disabled_reason: None,
                        },
                    );
                }
                _ => {}
            }
        }

        config
            .mcp_servers
            .set(new_mcp_servers)
            .map_err(|e| anyhow::anyhow!(e))?;

        Ok(config)
    }

    async fn resolve_rollout_path(&self, session_id: &SessionId) -> Result<PathBuf, Error> {
        find_thread_path_by_id_str(&self.config.codex_home, session_id.0.as_ref())
            .await
            .map_err(|e| Error::internal_error().data(e.to_string()))?
            .ok_or_else(|| Error::resource_not_found(None))
    }

    fn build_session_store(
        &self,
        session_id: &SessionId,
        config: &Config,
        thread_id: &ThreadId,
    ) -> Option<SessionStore> {
        let mut index = self.global_session_index.as_ref()?.lock().ok()?;
        let global_id = index.get_or_create(&format!("codex:{}", session_id.0))?;
        SessionStore::init(
            global_id,
            "codex",
            session_id.0.as_ref().to_string(),
            thread_id.to_string(),
            Some(&config.cwd),
        )
    }

    async fn register_thread(
        &self,
        session_id: SessionId,
        config: Config,
        new_thread: NewThread,
        replay_history: Option<Vec<RolloutItem>>,
    ) -> Result<LoadSessionResponse, Error> {
        let NewThread {
            thread_id,
            thread,
            session_configured: _,
        } = new_thread;

        self.session_roots
            .lock()
            .unwrap()
            .insert(session_id.clone(), config.cwd.clone());

        let thread = Rc::new(Thread::new(
            session_id.clone(),
            thread,
            self.auth_manager.clone(),
            self.thread_manager.get_models_manager(),
            self.client_capabilities.clone(),
            config.clone(),
            self.build_session_store(&session_id, &config, &thread_id),
        ));

        if let Some(history) = replay_history.filter(|history| !history.is_empty()) {
            thread.replay_history(history).await?;
        }

        let load = thread.load().await?;
        self.sessions.borrow_mut().insert(session_id, thread);
        Ok(load)
    }
}

#[async_trait::async_trait(?Send)]
impl BackendDriver for CodexDriver {
    fn backend_kind(&self) -> BackendKind {
        BackendKind::Codex
    }

    fn supports_load_session(&self) -> bool {
        true
    }

    fn supports_fork_session(&self) -> bool {
        true
    }

    fn supports_resume_session(&self) -> bool {
        true
    }

    fn auth_methods(&self) -> Vec<AuthMethod> {
        let mut auth_methods = vec![
            CodexAuthMethod::ChatGpt.into(),
            CodexAuthMethod::CodexApiKey.into(),
            CodexAuthMethod::OpenAiApiKey.into(),
        ];
        // Until codex device code auth works, we can't use this in remote ssh projects.
        if std::env::var("NO_BROWSER").is_ok() {
            auth_methods.remove(0);
        }
        auth_methods
    }

    async fn authenticate(
        &self,
        request: AuthenticateRequest,
    ) -> Result<AuthenticateResponse, Error> {
        let auth_method = CodexAuthMethod::try_from(request.method_id)?;

        // Check before starting login flow if already authenticated with the same method
        if let Some(auth) = self.auth_manager.auth().await {
            match (auth.mode, auth_method) {
                (
                    AuthMode::ApiKey,
                    CodexAuthMethod::CodexApiKey | CodexAuthMethod::OpenAiApiKey,
                )
                | (AuthMode::ChatGPT, CodexAuthMethod::ChatGpt) => {
                    return Ok(AuthenticateResponse::new());
                }
                _ => {}
            }
        }

        match auth_method {
            CodexAuthMethod::ChatGpt => {
                // ACP runs inside IDE hosts where OS-level browser launches can fail noisily.
                let mut opts = codex_login::ServerOptions::new(
                    self.config.codex_home.clone(),
                    codex_core::auth::CLIENT_ID.to_string(),
                    None,
                    self.config.cli_auth_credentials_store_mode,
                );
                let open_browser = Self::should_open_chatgpt_browser();
                opts.open_browser = open_browser;

                let server =
                    codex_login::run_login_server(opts).map_err(Error::into_internal_error)?;
                emit_chatgpt_login_instructions(server.actual_port, &server.auth_url, open_browser);

                server
                    .block_until_done()
                    .await
                    .map_err(Error::into_internal_error)?;

                self.auth_manager.reload();
            }
            CodexAuthMethod::CodexApiKey => {
                let api_key = read_codex_api_key_from_env().ok_or_else(|| {
                    Error::internal_error().data(format!("{CODEX_API_KEY_ENV_VAR} is not set"))
                })?;
                codex_login::login_with_api_key(
                    &self.config.codex_home,
                    &api_key,
                    self.config.cli_auth_credentials_store_mode,
                )
                .map_err(Error::into_internal_error)?;
            }
            CodexAuthMethod::OpenAiApiKey => {
                let api_key = read_openai_api_key_from_env().ok_or_else(|| {
                    Error::internal_error().data(format!("{OPENAI_API_KEY_ENV_VAR} is not set"))
                })?;
                codex_login::login_with_api_key(
                    &self.config.codex_home,
                    &api_key,
                    self.config.cli_auth_credentials_store_mode,
                )
                .map_err(Error::into_internal_error)?;
            }
        }

        self.auth_manager.reload();

        Ok(AuthenticateResponse::new())
    }

    async fn new_session(&self, request: NewSessionRequest) -> Result<NewSessionResponse, Error> {
        // Check before sending if authentication was successful or not
        self.check_auth().await?;

        let NewSessionRequest {
            cwd, mcp_servers, ..
        } = request;
        info!("Creating new session with cwd: {}", cwd.display());

        let config = self.build_session_config(&cwd, mcp_servers)?;
        let num_mcp_servers = config.mcp_servers.len();

        let new_thread = Box::pin(self.thread_manager.start_thread(config.clone()))
            .await
            .map_err(|_e| Error::internal_error())?;
        let session_id = Self::session_id_from_thread_id(new_thread.thread_id);
        let load = self
            .register_thread(session_id.clone(), config, new_thread, None)
            .await?;

        debug!("Created new session with {} MCP servers", num_mcp_servers);

        Ok(NewSessionResponse::new(session_id)
            .modes(load.modes)
            .models(load.models)
            .config_options(load.config_options)
            .meta(load.meta))
    }

    async fn load_session(
        &self,
        request: LoadSessionRequest,
    ) -> Result<LoadSessionResponse, Error> {
        info!("Loading session: {}", request.session_id);
        // Check before sending if authentication was successful or not
        self.check_auth().await?;

        let LoadSessionRequest {
            session_id,
            cwd,
            mcp_servers,
            ..
        } = request;

        let rollout_path = self.resolve_rollout_path(&session_id).await?;

        let history = RolloutRecorder::get_rollout_history(&rollout_path)
            .await
            .map_err(|e| Error::internal_error().data(e.to_string()))?;

        let rollout_items = match &history {
            InitialHistory::Resumed(resumed) => resumed.history.clone(),
            InitialHistory::Forked(items) => items.clone(),
            InitialHistory::New => Vec::new(),
        };

        let config = self.build_session_config(&cwd, mcp_servers)?;

        let new_thread = Box::pin(self.thread_manager.resume_thread_from_rollout(
            config.clone(),
            rollout_path,
            self.auth_manager.clone(),
        ))
        .await
        .map_err(|e| Error::internal_error().data(e.to_string()))?;

        self.register_thread(session_id, config, new_thread, Some(rollout_items))
            .await
    }

    async fn fork_session(
        &self,
        request: ForkSessionRequest,
    ) -> Result<ForkSessionResponse, Error> {
        self.check_auth().await?;

        let ForkSessionRequest {
            session_id,
            cwd,
            mcp_servers,
            ..
        } = request;
        let rollout_path = self.resolve_rollout_path(&session_id).await?;
        let config = self.build_session_config(&cwd, mcp_servers)?;
        let new_thread = Box::pin(self.thread_manager.fork_thread(
            usize::MAX,
            config.clone(),
            rollout_path,
        ))
        .await
        .map_err(|e| Error::internal_error().data(e.to_string()))?;

        let forked_session_id = Self::session_id_from_thread_id(new_thread.thread_id);
        let load = self
            .register_thread(forked_session_id.clone(), config, new_thread, None)
            .await?;

        Ok(ForkSessionResponse::new(forked_session_id)
            .modes(load.modes)
            .models(load.models)
            .config_options(load.config_options)
            .meta(load.meta))
    }

    async fn resume_session(
        &self,
        request: ResumeSessionRequest,
    ) -> Result<ResumeSessionResponse, Error> {
        self.check_auth().await?;

        let ResumeSessionRequest {
            session_id,
            cwd,
            mcp_servers,
            ..
        } = request;
        let rollout_path = self.resolve_rollout_path(&session_id).await?;
        let config = self.build_session_config(&cwd, mcp_servers)?;
        let new_thread = Box::pin(self.thread_manager.resume_thread_from_rollout(
            config.clone(),
            rollout_path,
            self.auth_manager.clone(),
        ))
        .await
        .map_err(|e| Error::internal_error().data(e.to_string()))?;

        let load = self
            .register_thread(session_id, config, new_thread, None)
            .await?;

        Ok(ResumeSessionResponse::new()
            .modes(load.modes)
            .models(load.models)
            .config_options(load.config_options)
            .meta(load.meta))
    }

    async fn list_sessions(
        &self,
        request: ListSessionsRequest,
    ) -> Result<ListSessionsResponse, Error> {
        self.check_auth().await?;

        let ListSessionsRequest { cwd, cursor, .. } = request;
        let cursor_obj = cursor.as_deref().and_then(parse_cursor);

        let page = RolloutRecorder::list_threads(
            &self.config.codex_home,
            SESSION_LIST_PAGE_SIZE,
            cursor_obj.as_ref(),
            ThreadSortKey::UpdatedAt,
            &[
                SessionSource::Cli,
                SessionSource::VSCode,
                SessionSource::Unknown,
            ],
            None,
            self.config.model_provider_id.as_str(),
        )
        .await
        .map_err(|err| Error::internal_error().data(format!("failed to list sessions: {err}")))?;

        let sessions = page
            .items
            .into_iter()
            .filter_map(|item| {
                // Codex rollout summaries put the SessionMetaLine first in the head.
                let session_meta_line = item.head.first().and_then(|first| {
                    serde_json::from_value::<SessionMetaLine>(first.clone()).ok()
                })?;

                if let Some(filter_cwd) = cwd.as_ref()
                    && session_meta_line.meta.cwd != *filter_cwd
                {
                    return None;
                }

                let mut title = None;
                for value in item.head {
                    if let Ok(response_item) = serde_json::from_value::<ResponseItem>(value)
                        && let Some(turn_item) = parse_turn_item(&response_item)
                        && let codex_protocol::items::TurnItem::UserMessage(user) = turn_item
                    {
                        if let Some(formatted) = format_session_title(&user.message()) {
                            title = Some(formatted);
                        }
                        break;
                    }
                }

                let updated_at = item.updated_at.clone().or(item.created_at.clone());

                Some(
                    SessionInfo::new(
                        SessionId::new(session_meta_line.meta.id.to_string()),
                        session_meta_line.meta.cwd.clone(),
                    )
                    .title(title)
                    .updated_at(updated_at),
                )
            })
            .collect::<Vec<_>>();

        let next_cursor = page
            .next_cursor
            .as_ref()
            .and_then(|next_cursor| serde_json::to_value(next_cursor).ok())
            .and_then(|value| value.as_str().map(str::to_owned));

        Ok(ListSessionsResponse::new(sessions).next_cursor(next_cursor))
    }

    async fn prompt(&self, request: PromptRequest) -> Result<PromptResponse, Error> {
        info!("Processing prompt for session: {}", request.session_id);
        // Check before sending if authentication was successful or not
        self.check_auth().await?;

        // Get the session state
        let thread = self.get_thread(&request.session_id)?;
        let stop_reason = thread.prompt(request).await?;

        Ok(PromptResponse::new(stop_reason))
    }

    async fn cancel(&self, args: CancelNotification) -> Result<(), Error> {
        info!("Cancelling operations for session: {}", args.session_id);
        self.get_thread(&args.session_id)?.cancel().await?;
        Ok(())
    }

    async fn set_session_mode(
        &self,
        args: SetSessionModeRequest,
    ) -> Result<SetSessionModeResponse, Error> {
        info!("Setting session mode for session: {}", args.session_id);
        self.get_thread(&args.session_id)?
            .set_mode(args.mode_id)
            .await?;
        Ok(SetSessionModeResponse::default())
    }

    async fn set_session_model(
        &self,
        args: SetSessionModelRequest,
    ) -> Result<SetSessionModelResponse, Error> {
        info!("Setting session model for session: {}", args.session_id);

        self.get_thread(&args.session_id)?
            .set_model(args.model_id)
            .await?;

        Ok(SetSessionModelResponse::default())
    }

    async fn set_session_config_option(
        &self,
        args: SetSessionConfigOptionRequest,
    ) -> Result<SetSessionConfigOptionResponse, Error> {
        info!(
            "Setting session config option for session: {} (config_id: {}, value: {})",
            args.session_id, args.config_id.0, args.value.0
        );

        let thread = self.get_thread(&args.session_id)?;

        thread.set_config_option(args.config_id, args.value).await?;

        let config_options = thread.config_options().await?;

        Ok(SetSessionConfigOptionResponse::new(config_options))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodexAuthMethod {
    ChatGpt,
    CodexApiKey,
    OpenAiApiKey,
}

impl From<CodexAuthMethod> for AuthMethodId {
    fn from(method: CodexAuthMethod) -> Self {
        Self::new(match method {
            CodexAuthMethod::ChatGpt => "chatgpt",
            CodexAuthMethod::CodexApiKey => "codex-api-key",
            CodexAuthMethod::OpenAiApiKey => "openai-api-key",
        })
    }
}

impl From<CodexAuthMethod> for AuthMethod {
    fn from(method: CodexAuthMethod) -> Self {
        match method {
            CodexAuthMethod::ChatGpt => Self::new(method, "ChatGPT (Browser login)").description(
                "Starts a local login server and prints the auth URL to stderr.\nBrowser auto-open is disabled by default in ACP to avoid OS open failures; set XSFIRE_CODEX_OPEN_BROWSER=1 to re-enable it.\nTip: set NO_BROWSER=1 for headless/SSH workflows and use an API key method.",
            ),
            CodexAuthMethod::CodexApiKey => {
                Self::new(method, format!("API key ({CODEX_API_KEY_ENV_VAR})")).description(
                    format!(
                        "Requires setting the `{CODEX_API_KEY_ENV_VAR}` environment variable.\nIn Zed, set this in your agent server `env` configuration."
                    ),
                )
            }
            CodexAuthMethod::OpenAiApiKey => {
                Self::new(method, format!("API key ({OPENAI_API_KEY_ENV_VAR})")).description(
                    format!(
                        "Requires setting the `{OPENAI_API_KEY_ENV_VAR}` environment variable.\nIn Zed, set this in your agent server `env` configuration."
                    ),
                )
            }
        }
    }
}

impl TryFrom<AuthMethodId> for CodexAuthMethod {
    type Error = Error;

    fn try_from(value: AuthMethodId) -> Result<Self, Self::Error> {
        match value.0.as_ref() {
            "chatgpt" => Ok(CodexAuthMethod::ChatGpt),
            "codex-api-key" => Ok(CodexAuthMethod::CodexApiKey),
            "openai-api-key" => Ok(CodexAuthMethod::OpenAiApiKey),
            _ => Err(Error::invalid_params().data("unsupported authentication method")),
        }
    }
}

fn is_truthy_env_value(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn emit_chatgpt_login_instructions(actual_port: u16, auth_url: &str, open_browser: bool) {
    let browser_message = if open_browser {
        format!("If your browser did not open, navigate to this URL to authenticate:\n\n{auth_url}")
    } else {
        format!(
            "Browser auto-open is disabled in ACP to avoid OS open failures.\nOpen this URL to authenticate:\n\n{auth_url}\n\nSet {XSFIRE_CODEX_OPEN_BROWSER_ENV_VAR}=1 to re-enable browser auto-open."
        )
    };

    let mut stderr = io::stderr().lock();
    if writeln!(
        stderr,
        "Starting local login server on http://localhost:{actual_port}.\n{browser_message}"
    )
    .is_err()
    {
        // Stderr may be unavailable in some ACP hosts; login can still continue with the local server.
    }
}

fn truncate_graphemes(text: &str, max_graphemes: usize) -> String {
    let mut graphemes = text.grapheme_indices(true);

    if let Some((byte_index, _)) = graphemes.nth(max_graphemes) {
        if max_graphemes >= 3 {
            let mut truncate_graphemes = text.grapheme_indices(true);
            if let Some((truncate_byte_index, _)) = truncate_graphemes.nth(max_graphemes - 3) {
                let truncated = &text[..truncate_byte_index];
                format!("{truncated}...")
            } else {
                text.to_string()
            }
        } else {
            let truncated = &text[..byte_index];
            truncated.to_string()
        }
    } else {
        text.to_string()
    }
}

fn format_session_title(message: &str) -> Option<String> {
    let normalized = message.replace(['\r', '\n'], " ");
    let trimmed = normalized.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(truncate_graphemes(trimmed, SESSION_TITLE_MAX_GRAPHEMES))
    }
}

#[cfg(test)]
mod tests {
    use super::is_truthy_env_value;

    #[test]
    fn parses_truthy_env_values() {
        for value in ["1", "true", "TRUE", " yes ", "On"] {
            assert!(is_truthy_env_value(value), "{value} should be truthy");
        }
    }

    #[test]
    fn rejects_non_truthy_env_values() {
        for value in ["", "0", "false", "off", "no", "random"] {
            assert!(!is_truthy_env_value(value), "{value} should be false");
        }
    }
}
