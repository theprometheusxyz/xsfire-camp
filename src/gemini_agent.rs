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
    fs,
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

struct GeminiSession {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GeminiAuthType {
    LoginWithGoogle,
    GeminiApiKey,
    VertexAi,
    ComputeDefaultCredentials,
}

impl GeminiAuthType {
    const fn as_str(self) -> &'static str {
        match self {
            Self::LoginWithGoogle => "oauth-personal",
            Self::GeminiApiKey => "gemini-api-key",
            Self::VertexAi => "vertex-ai",
            Self::ComputeDefaultCredentials => "compute-default-credentials",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "oauth-personal" => Some(Self::LoginWithGoogle),
            "gemini-api-key" => Some(Self::GeminiApiKey),
            "vertex-ai" => Some(Self::VertexAi),
            "compute-default-credentials" => Some(Self::ComputeDefaultCredentials),
            _ => None,
        }
    }
}

struct GeminiAuthConfig {
    selected_type: Option<GeminiAuthType>,
    enforced_type: Option<GeminiAuthType>,
    settings_path: Option<PathBuf>,
    oauth_creds_path: Option<PathBuf>,
}

/// Gemini CLI backend driver (shells out to the `gemini` CLI).
///
/// Minimal implementation:
/// - `new_session` creates an in-memory session ID
/// - `prompt` runs `gemini --output-format text --approval-mode plan -p "<prompt>"`
///   to avoid interactive approvals and streams the response as a single ACP message chunk
pub struct GeminiCliDriver {
    sessions: Rc<RefCell<HashMap<SessionId, GeminiSession>>>,
    global_session_index: Option<Arc<Mutex<GlobalSessionIndex>>>,
}

impl GeminiCliDriver {
    const AUTH_METHOD_ID: &'static str = "gemini-cli";

    pub fn new() -> Self {
        Self {
            sessions: Rc::default(),
            global_session_index: GlobalSessionIndex::load().map(|idx| Arc::new(Mutex::new(idx))),
        }
    }

    fn bin() -> String {
        std::env::var("XSFIRE_GEMINI_BIN").unwrap_or_else(|_| "gemini".to_string())
    }

    fn extra_args() -> Vec<String> {
        std::env::var("XSFIRE_GEMINI_ARGS")
            .ok()
            .and_then(|s| shlex::split(&s))
            .unwrap_or_default()
    }

    fn approval_mode() -> String {
        std::env::var("XSFIRE_GEMINI_APPROVAL_MODE").unwrap_or_else(|_| "plan".to_string())
    }

    fn default_model() -> Option<String> {
        std::env::var("XSFIRE_GEMINI_MODEL").ok()
    }

    fn help_text() -> String {
        format!(
            "Gemini commands:\n- /status\n- /model <name>\n- /reset\n\n{}",
            BackendKind::Gemini
                .work_orchestration_profile()
                .render_summary(),
        )
    }

    fn status_text(model: &str, history_turns: usize) -> String {
        let profile = BackendKind::Gemini.work_orchestration_profile();
        format!(
            "Gemini session status:\n- model: {model}\n- history_turns: {history_turns}\n- task_orchestration: {}\n- task_monitoring: {}\n- progress_vector_checks: {}\n- preempt_on_new_prompt: {}\n- acp_bridge: {}\n- work_orchestration_sequence: {} ({})\n- operator_hint: {}",
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
            "unsupported auth method for gemini backend: {method_id}"
        )))
    }

    fn check_cli_available(bin: &str) -> Result<(), Error> {
        let output = StdCommand::new(bin).arg("--version").output().map_err(|e| {
            Error::invalid_params().data(format!(
                "failed to execute Gemini CLI ({bin}). Install it or set XSFIRE_GEMINI_BIN. Error: {e}"
            ))
        })?;

        if output.status.success() {
            return Ok(());
        }

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(Error::invalid_params().data(format!(
            "Gemini CLI is installed but not ready for ACP authenticate. Authenticate with the `gemini` CLI first. exit={:?} stdout={} stderr={}",
            output.status.code(),
            stdout,
            stderr,
        )))
    }

    fn validate_auth_configuration() -> Result<(), Error> {
        let config = Self::load_auth_config()?;
        let settings_path = Self::display_settings_path(config.settings_path.as_deref());
        let effective_type = config.selected_type.or_else(Self::auth_type_from_env);

        if let Some(enforced_type) = config.enforced_type {
            match effective_type {
                Some(current_type) if current_type == enforced_type => {}
                Some(current_type) => {
                    return Err(Error::invalid_params().data(format!(
                        "Gemini CLI enforces auth type `{}` in {}, but ACP authenticate resolved `{}`. Re-authenticate or update your environment before starting.",
                        enforced_type.as_str(),
                        settings_path,
                        current_type.as_str(),
                    )));
                }
                None => {
                    return Err(Error::invalid_params().data(format!(
                        "Gemini CLI enforces auth type `{}` in {}, but no Gemini auth configuration is ready. Configure the matching auth method before starting ACP.",
                        enforced_type.as_str(),
                        settings_path,
                    )));
                }
            }
        }

        let Some(auth_type) = effective_type else {
            return Err(Error::invalid_params().data(format!(
                "Gemini CLI is installed but no auth method is configured. Set an auth method in {} or export one of: GEMINI_API_KEY, GOOGLE_GENAI_USE_VERTEXAI=true, GOOGLE_GENAI_USE_GCA=true.",
                settings_path,
            )));
        };

        match auth_type {
            GeminiAuthType::LoginWithGoogle => {
                let oauth_creds_path =
                    Self::display_oauth_creds_path(config.oauth_creds_path.as_deref());
                let has_oauth_creds = config
                    .oauth_creds_path
                    .as_ref()
                    .is_some_and(|path| path.is_file());
                if !has_oauth_creds {
                    return Err(Error::invalid_params().data(format!(
                        "Gemini CLI is configured for `oauth-personal`, but {} is missing. Run `gemini` and complete login before starting ACP.",
                        oauth_creds_path,
                    )));
                }
            }
            GeminiAuthType::GeminiApiKey => {
                if !Self::has_env_var("GEMINI_API_KEY") {
                    return Err(Error::invalid_params().data(
                        "Gemini CLI is configured for `gemini-api-key`, but GEMINI_API_KEY is not set."
                            .to_string(),
                    ));
                }
            }
            GeminiAuthType::VertexAi => {
                let has_vertex_project_location = Self::has_env_var("GOOGLE_CLOUD_PROJECT")
                    && Self::has_env_var("GOOGLE_CLOUD_LOCATION");
                let has_google_api_key = Self::has_env_var("GOOGLE_API_KEY");
                if !has_vertex_project_location && !has_google_api_key {
                    return Err(Error::invalid_params().data(
                        "Gemini CLI is configured for `vertex-ai`, but neither GOOGLE_API_KEY nor the GOOGLE_CLOUD_PROJECT/GOOGLE_CLOUD_LOCATION pair is set."
                            .to_string(),
                    ));
                }
            }
            GeminiAuthType::ComputeDefaultCredentials => {}
        }

        Ok(())
    }

    fn load_auth_config() -> Result<GeminiAuthConfig, Error> {
        let gemini_dir = Self::gemini_dir();
        let settings_path = gemini_dir.as_ref().map(|path| path.join("settings.json"));
        let oauth_creds_path = gemini_dir
            .as_ref()
            .map(|path| path.join("oauth_creds.json"));
        let mut selected_type = None;
        let mut enforced_type = None;

        if let Some(settings_path) = settings_path.as_ref()
            && settings_path.is_file()
        {
            let contents = fs::read_to_string(settings_path).map_err(|e| {
                Error::invalid_params().data(format!(
                    "failed to read Gemini settings from {}: {e}",
                    settings_path.display()
                ))
            })?;
            let settings: serde_json::Value = serde_json::from_str(&contents).map_err(|e| {
                Error::invalid_params().data(format!(
                    "failed to parse Gemini settings from {}: {e}",
                    settings_path.display()
                ))
            })?;
            selected_type = Self::settings_auth_type(
                &settings,
                &["security", "auth", "selectedType"],
                settings_path,
            )?;
            enforced_type = Self::settings_auth_type(
                &settings,
                &["security", "auth", "enforcedType"],
                settings_path,
            )?;
        }

        Ok(GeminiAuthConfig {
            selected_type,
            enforced_type,
            settings_path,
            oauth_creds_path,
        })
    }

    fn settings_auth_type(
        value: &serde_json::Value,
        path: &[&str],
        settings_path: &Path,
    ) -> Result<Option<GeminiAuthType>, Error> {
        let mut current = value;
        for segment in path {
            let Some(next) = current.get(*segment) else {
                return Ok(None);
            };
            current = next;
        }

        if current.is_null() {
            return Ok(None);
        }

        let Some(raw_value) = current.as_str() else {
            return Err(Error::invalid_params().data(format!(
                "Gemini settings {} contains a non-string value at {}.",
                settings_path.display(),
                path.join("."),
            )));
        };

        GeminiAuthType::from_str(raw_value)
            .map(Some)
            .ok_or_else(|| {
                Error::invalid_params().data(format!(
                    "Gemini settings {} contains unsupported auth value `{}` at {}.",
                    settings_path.display(),
                    raw_value,
                    path.join("."),
                ))
            })
    }

    fn auth_type_from_env() -> Option<GeminiAuthType> {
        if matches!(std::env::var("GOOGLE_GENAI_USE_GCA").as_deref(), Ok("true")) {
            return Some(GeminiAuthType::LoginWithGoogle);
        }
        if matches!(
            std::env::var("GOOGLE_GENAI_USE_VERTEXAI").as_deref(),
            Ok("true")
        ) {
            return Some(GeminiAuthType::VertexAi);
        }
        if Self::has_env_var("GEMINI_API_KEY") {
            return Some(GeminiAuthType::GeminiApiKey);
        }
        None
    }

    fn has_env_var(key: &str) -> bool {
        std::env::var_os(key).is_some_and(|value| !value.is_empty())
    }

    fn gemini_dir() -> Option<PathBuf> {
        Self::home_dir().map(|path| path.join(".gemini"))
    }

    fn home_dir() -> Option<PathBuf> {
        std::env::var_os("USERPROFILE")
            .filter(|path| !path.is_empty())
            .or_else(|| std::env::var_os("HOME").filter(|path| !path.is_empty()))
            .map(PathBuf::from)
    }

    fn display_settings_path(path: Option<&Path>) -> String {
        path.map(|path| path.display().to_string())
            .unwrap_or_else(|| "~/.gemini/settings.json".to_string())
    }

    fn display_oauth_creds_path(path: Option<&Path>) -> String {
        path.map(|path| path.display().to_string())
            .unwrap_or_else(|| "~/.gemini/oauth_creds.json".to_string())
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
                    SessionConfigSelectOption::new("gemini-2.5-pro", "Gemini 2.5 Pro"),
                    SessionConfigSelectOption::new("gemini-2.5-flash", "Gemini 2.5 Flash"),
                    SessionConfigSelectOption::new("gemini-2.0-flash", "Gemini 2.0 Flash"),
                ],
            )
            .category(SessionConfigOptionCategory::Model)
            .description("Model used by Gemini CLI for this session"),
        ]
    }

    async fn run_gemini(
        &self,
        cwd: PathBuf,
        model: Option<String>,
        prompt: String,
        mut cancel_rx: watch::Receiver<bool>,
    ) -> Result<CommandRunResult, Error> {
        let bin = Self::bin();
        let bin_display = bin.clone();
        let extra_args = Self::extra_args();
        let approval_mode = Self::approval_mode();

        let mut cmd = TokioCommand::new(&bin);
        cmd.current_dir(&cwd);
        cmd.arg("--output-format");
        cmd.arg("text");
        cmd.arg("--approval-mode");
        cmd.arg(approval_mode);
        if let Some(model) = model {
            cmd.arg("--model");
            cmd.arg(model);
        }
        cmd.args(extra_args);
        cmd.arg("--prompt");
        cmd.arg(prompt);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| {
            Error::invalid_params().data(format!(
                "failed to execute Gemini CLI ({bin_display}). Install it or set XSFIRE_GEMINI_BIN. Error: {e}"
            ))
        })?;

        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::internal_error().data("Gemini CLI stdout pipe missing"))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::internal_error().data("Gemini CLI stderr pipe missing"))?;

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
                "Gemini CLI failed (exit {:?}). stderr:\n{stderr}",
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

enum GeminiCommand {
    Help,
    Status,
    Reset,
    SetModel(String),
}

fn parse_gemini_command(input: &str) -> Option<GeminiCommand> {
    let trimmed = input.trim();
    if trimmed == "/help" {
        return Some(GeminiCommand::Help);
    }
    if trimmed == "/status" {
        return Some(GeminiCommand::Status);
    }
    if trimmed == "/reset" {
        return Some(GeminiCommand::Reset);
    }
    if let Some(value) = trimmed.strip_prefix("/model ") {
        let model = value.trim();
        if !model.is_empty() {
            return Some(GeminiCommand::SetModel(model.to_string()));
        }
    }
    None
}

#[async_trait::async_trait(?Send)]
impl BackendDriver for GeminiCliDriver {
    fn backend_kind(&self) -> BackendKind {
        BackendKind::Gemini
    }

    fn auth_methods(&self) -> Vec<AuthMethod> {
        vec![AuthMethod::new(
            Self::AUTH_METHOD_ID,
            "Gemini CLI (pre-authenticated)",
        )
        .description("Authenticate using the `gemini` CLI before starting. ACP authenticate validates CLI availability plus Gemini auth configuration readiness, but cannot launch or verify the interactive login flow.")]
    }

    async fn authenticate(
        &self,
        request: AuthenticateRequest,
    ) -> Result<AuthenticateResponse, Error> {
        Self::validate_auth_method(request.method_id.0.as_ref())?;
        Self::check_cli_available(&Self::bin())?;
        Self::validate_auth_configuration()?;
        Ok(AuthenticateResponse::new())
    }

    async fn new_session(&self, request: NewSessionRequest) -> Result<NewSessionResponse, Error> {
        let session_id = SessionId::new(format!("gemini:{}", Uuid::new_v4()));
        let cwd = request.cwd;
        let session_store = self.init_session_store(&session_id, &cwd);

        self.sessions.borrow_mut().insert(
            session_id.clone(),
            GeminiSession {
                cwd,
                model: Self::default_model(),
                history: Vec::new(),
                session_store,
                active_prompt: None,
            },
        );

        info!("Created Gemini session: {session_id:?}");
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
            "load_session is not supported for --backend=gemini yet (sessions are in-memory).",
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
            .map(|(id, s)| SessionInfo::new(id.clone(), s.cwd.clone()).title("Gemini (in-memory)"))
            .collect::<Vec<_>>();
        Ok(ListSessionsResponse::new(sessions))
    }

    async fn prompt(&self, request: PromptRequest) -> Result<PromptResponse, Error> {
        let session_id = request.session_id.clone();
        let user_text = prompt_blocks_to_text(&request.prompt);
        debug!(
            "Gemini prompt (session={session_id:?}) chars={}",
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

        if let Some(command) = parse_gemini_command(&user_text) {
            let message = {
                let mut sessions = self.sessions.borrow_mut();
                let Some(session) = sessions.get_mut(&session_id) else {
                    return Err(Error::resource_not_found(None));
                };
                match command {
                    GeminiCommand::Help => Self::help_text(),
                    GeminiCommand::Status => {
                        let model = session.model.as_deref().unwrap_or("default");
                        Self::status_text(model, session.history.len())
                    }
                    GeminiCommand::Reset => {
                        session.history.clear();
                        "Gemini session history has been reset.".to_string()
                    }
                    GeminiCommand::SetModel(model) => {
                        let normalized = if model == "default" {
                            None
                        } else {
                            Some(model)
                        };
                        session.model = normalized.clone();
                        let model_text = normalized.as_deref().unwrap_or("default");
                        format!("Gemini model set to `{model_text}`.")
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
                    "a Gemini prompt is already running for session {}",
                    session_id
                )));
            }

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

        let output = self.run_gemini(cwd, model, full_prompt, cancel_rx).await;
        {
            let mut sessions = self.sessions.borrow_mut();
            if let Some(session) = sessions.get_mut(&session_id) {
                session.active_prompt = None;
            }
        }
        let output = output?;
        if output.cancelled {
            debug!(
                "Gemini prompt cancelled for session {} (exit {:?})",
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
            .data("set_session_mode is not supported for --backend=gemini yet."))
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
                "unsupported config option for gemini backend: {}",
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

impl GeminiCliDriver {
    fn init_session_store(&self, session_id: &SessionId, cwd: &Path) -> Option<SessionStore> {
        let idx = self.global_session_index.as_ref()?;
        let global_id = idx
            .lock()
            .ok()
            .and_then(|mut i| i.get_or_create(&format!("gemini:{}", session_id.0)))?;
        SessionStore::init(
            global_id,
            "gemini",
            session_id.0.to_string(),
            session_id.0.to_string(),
            Some(cwd),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::GeminiCliDriver;
    use crate::backend::BackendDriver;
    use agent_client_protocol::{
        AuthenticateRequest, CancelNotification, Error, NewSessionRequest, PromptRequest,
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
        cleanup_dir: Option<PathBuf>,
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
            if let Some(dir) = &self.cleanup_dir {
                drop(fs::remove_dir_all(dir));
            }
        }
    }

    fn install_fake_gemini_bin() -> TempEnvVar {
        let temp_dir = std::env::temp_dir().join(format!("xsfire-gemini-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&temp_dir).unwrap();
        let script_path = write_fake_gemini_script(&temp_dir);
        let previous = std::env::var("XSFIRE_GEMINI_BIN").ok();
        // Safe in tests when serialized by ENV_LOCK.
        unsafe {
            std::env::set_var("XSFIRE_GEMINI_BIN", &script_path);
        }
        TempEnvVar {
            key: "XSFIRE_GEMINI_BIN",
            previous,
            cleanup_dir: Some(temp_dir),
        }
    }

    fn set_temp_env_var(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> TempEnvVar {
        let previous = std::env::var(key).ok();
        // Safe in tests when serialized by ENV_LOCK.
        unsafe {
            std::env::set_var(key, value);
        }
        TempEnvVar {
            key,
            previous,
            cleanup_dir: None,
        }
    }

    fn unset_temp_env_var(key: &'static str) -> TempEnvVar {
        let previous = std::env::var(key).ok();
        // Safe in tests when serialized by ENV_LOCK.
        unsafe {
            std::env::remove_var(key);
        }
        TempEnvVar {
            key,
            previous,
            cleanup_dir: None,
        }
    }

    fn clear_gemini_auth_env() -> Vec<TempEnvVar> {
        vec![
            unset_temp_env_var("GEMINI_API_KEY"),
            unset_temp_env_var("GOOGLE_API_KEY"),
            unset_temp_env_var("GOOGLE_CLOUD_PROJECT"),
            unset_temp_env_var("GOOGLE_CLOUD_LOCATION"),
            unset_temp_env_var("GOOGLE_GENAI_USE_GCA"),
            unset_temp_env_var("GOOGLE_GENAI_USE_VERTEXAI"),
        ]
    }

    struct FakeGeminiHome {
        _home: TempEnvVar,
        _userprofile: TempEnvVar,
    }

    fn install_fake_gemini_home(
        selected_type: Option<&str>,
        enforced_type: Option<&str>,
        with_oauth_creds: bool,
    ) -> FakeGeminiHome {
        let home_dir = std::env::temp_dir().join(format!("xsfire-gemini-home-{}", Uuid::new_v4()));
        let gemini_dir = home_dir.join(".gemini");
        fs::create_dir_all(&gemini_dir).unwrap();

        if selected_type.is_some() || enforced_type.is_some() {
            let mut auth = serde_json::Map::new();
            if let Some(selected_type) = selected_type {
                auth.insert(
                    "selectedType".to_string(),
                    serde_json::Value::String(selected_type.to_string()),
                );
            }
            if let Some(enforced_type) = enforced_type {
                auth.insert(
                    "enforcedType".to_string(),
                    serde_json::Value::String(enforced_type.to_string()),
                );
            }

            let settings = serde_json::json!({
                "security": {
                    "auth": auth,
                }
            });
            fs::write(
                gemini_dir.join("settings.json"),
                serde_json::to_vec_pretty(&settings).unwrap(),
            )
            .unwrap();
        }

        if with_oauth_creds {
            fs::write(gemini_dir.join("oauth_creds.json"), "{}").unwrap();
        }

        let previous_home = std::env::var("HOME").ok();
        // Safe in tests when serialized by ENV_LOCK.
        unsafe {
            std::env::set_var("HOME", &home_dir);
        }
        let home_guard = TempEnvVar {
            key: "HOME",
            previous: previous_home,
            cleanup_dir: Some(home_dir.clone()),
        };

        let userprofile_guard = set_temp_env_var("USERPROFILE", &home_dir);

        FakeGeminiHome {
            _home: home_guard,
            _userprofile: userprofile_guard,
        }
    }

    fn write_fake_gemini_script(temp_dir: &Path) -> PathBuf {
        #[cfg(windows)]
        let script_path = temp_dir.join("gemini.cmd");
        #[cfg(not(windows))]
        let script_path = temp_dir.join("gemini");

        #[cfg(windows)]
        let script = r#"@echo off
if "%1"=="--version" (
  echo gemini fake 1.0
  exit /b 0
)
powershell -NoProfile -Command "Start-Sleep -Seconds 2; Write-Output 'fake gemini output'"
exit /b 0
"#;

        #[cfg(not(windows))]
        let script = r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  printf '%s\n' 'gemini fake 1.0'
  exit 0
fi
exec sleep 2
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
        let err = GeminiCliDriver::validate_auth_method("wrong-method").unwrap_err();
        let message = serde_json::to_value(&err)
            .unwrap()
            .get("data")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        assert!(message.contains("unsupported auth method"));
    }

    #[test]
    fn missing_gemini_binary_returns_helpful_error() {
        let err = GeminiCliDriver::check_cli_available("/definitely/missing/gemini").unwrap_err();
        let message = serde_json::to_value(&err)
            .unwrap()
            .get("data")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        assert!(message.contains("Install it or set XSFIRE_GEMINI_BIN"));
    }

    #[test]
    fn authenticate_request_uses_supported_method() {
        let request = AuthenticateRequest::new("gemini-cli");
        assert_eq!(
            request.method_id.0.as_ref(),
            GeminiCliDriver::AUTH_METHOD_ID
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "current_thread")]
    async fn authenticate_rejects_missing_auth_configuration() {
        let _guard = crate::session_store::lock_env();
        let _bin = install_fake_gemini_bin();
        let _home = install_fake_gemini_home(None, None, false);
        let _auth_env = clear_gemini_auth_env();

        let driver = GeminiCliDriver::new();
        let error = driver
            .authenticate(AuthenticateRequest::new("gemini-cli"))
            .await
            .unwrap_err();
        let message = error_message(&error);
        assert!(message.contains("no auth method is configured"));
        assert!(message.contains("GEMINI_API_KEY"));
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "current_thread")]
    async fn authenticate_accepts_gemini_api_key_env() {
        let _guard = crate::session_store::lock_env();
        let _bin = install_fake_gemini_bin();
        let _home = install_fake_gemini_home(None, None, false);
        let _auth_env = clear_gemini_auth_env();
        let _api_key = set_temp_env_var("GEMINI_API_KEY", "test-key");

        let driver = GeminiCliDriver::new();
        let response = driver
            .authenticate(AuthenticateRequest::new("gemini-cli"))
            .await;
        assert!(response.is_ok());
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "current_thread")]
    async fn authenticate_requires_oauth_credentials_file() {
        let _guard = crate::session_store::lock_env();
        let _bin = install_fake_gemini_bin();
        let _home = install_fake_gemini_home(Some("oauth-personal"), None, false);
        let _auth_env = clear_gemini_auth_env();

        let driver = GeminiCliDriver::new();
        let error = driver
            .authenticate(AuthenticateRequest::new("gemini-cli"))
            .await
            .unwrap_err();
        let message = error_message(&error);
        assert!(message.contains("oauth-personal"));
        assert!(message.contains("oauth_creds.json"));
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "current_thread")]
    async fn authenticate_accepts_oauth_credentials_file() {
        let _guard = crate::session_store::lock_env();
        let _bin = install_fake_gemini_bin();
        let _home = install_fake_gemini_home(Some("oauth-personal"), None, true);
        let _auth_env = clear_gemini_auth_env();

        let driver = GeminiCliDriver::new();
        let response = driver
            .authenticate(AuthenticateRequest::new("gemini-cli"))
            .await;
        assert!(response.is_ok());
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "current_thread")]
    async fn authenticate_accepts_vertex_ai_env() {
        let _guard = crate::session_store::lock_env();
        let _bin = install_fake_gemini_bin();
        let _home = install_fake_gemini_home(Some("vertex-ai"), None, false);
        let _auth_env = clear_gemini_auth_env();
        let _google_api_key = set_temp_env_var("GOOGLE_API_KEY", "vertex-key");

        let driver = GeminiCliDriver::new();
        let response = driver
            .authenticate(AuthenticateRequest::new("gemini-cli"))
            .await;
        assert!(response.is_ok());
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "current_thread")]
    async fn authenticate_rejects_enforced_auth_mismatch() {
        let _guard = crate::session_store::lock_env();
        let _bin = install_fake_gemini_bin();
        let _home = install_fake_gemini_home(None, Some("oauth-personal"), false);
        let _auth_env = clear_gemini_auth_env();
        let _vertex = set_temp_env_var("GOOGLE_GENAI_USE_VERTEXAI", "true");

        let driver = GeminiCliDriver::new();
        let error = driver
            .authenticate(AuthenticateRequest::new("gemini-cli"))
            .await
            .unwrap_err();
        let message = error_message(&error);
        assert!(message.contains("enforces auth type `oauth-personal`"));
        assert!(message.contains("resolved `vertex-ai`"));
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "current_thread")]
    async fn authenticate_prefers_selected_type_over_env() {
        let _guard = crate::session_store::lock_env();
        let _bin = install_fake_gemini_bin();
        let _home = install_fake_gemini_home(Some("oauth-personal"), Some("oauth-personal"), true);
        let _auth_env = clear_gemini_auth_env();
        let _vertex = set_temp_env_var("GOOGLE_GENAI_USE_VERTEXAI", "true");

        let driver = GeminiCliDriver::new();
        let response = driver
            .authenticate(AuthenticateRequest::new("gemini-cli"))
            .await;
        assert!(response.is_ok());
    }

    #[test]
    fn auth_type_from_env_prefers_explicit_google_flags() {
        let _guard = crate::session_store::lock_env();
        let _auth_env = clear_gemini_auth_env();
        let _api_key = set_temp_env_var("GEMINI_API_KEY", "test-key");
        let _vertex = set_temp_env_var("GOOGLE_GENAI_USE_VERTEXAI", "true");

        assert_eq!(
            GeminiCliDriver::auth_type_from_env(),
            Some(super::GeminiAuthType::VertexAi)
        );
    }

    fn error_message(error: &Error) -> String {
        serde_json::to_value(error)
            .unwrap()
            .get("data")
            .and_then(|value| value.as_str())
            .unwrap()
            .to_string()
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "current_thread")]
    async fn cancel_stops_running_prompt() {
        let _guard = crate::session_store::lock_env();
        let _bin = install_fake_gemini_bin();

        let driver = GeminiCliDriver::new();
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
    async fn set_session_model_updates_gemini_session() {
        let driver = GeminiCliDriver::new();
        let cwd = std::env::current_dir().unwrap();
        let session = driver
            .new_session(NewSessionRequest::new(cwd))
            .await
            .unwrap();
        let session_id = session.session_id.clone();

        driver
            .set_session_model(SetSessionModelRequest::new(
                session_id.clone(),
                "gemini-2.5-pro",
            ))
            .await
            .unwrap();

        let sessions = driver.sessions.borrow();
        let session = sessions.get(&session_id).unwrap();
        assert_eq!(session.model.as_deref(), Some("gemini-2.5-pro"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn set_session_config_option_updates_model_and_rejects_other_options() {
        let driver = GeminiCliDriver::new();
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
                "gemini-2.5-flash",
            ))
            .await
            .unwrap();

        {
            let sessions = driver.sessions.borrow();
            let session = sessions.get(&session_id).unwrap();
            assert_eq!(session.model.as_deref(), Some("gemini-2.5-flash"));
        }
        let selected_model = response
            .config_options
            .iter()
            .find(|option| option.id.0.as_ref() == "model")
            .map(|option| match &option.kind {
                SessionConfigKind::Select(select) => select.current_value.0.to_string(),
                _ => panic!("model config option should be a select"),
            });
        assert_eq!(selected_model.as_deref(), Some("gemini-2.5-flash"));

        let error = driver
            .set_session_config_option(SetSessionConfigOptionRequest::new(
                session_id,
                "temperature",
                "0.2",
            ))
            .await
            .unwrap_err();
        let message = error_message(&error);
        assert!(message.contains("unsupported config option for gemini backend"));
    }

    #[test]
    fn gemini_help_text_exposes_work_orchestration_profile() {
        let help = GeminiCliDriver::help_text();
        assert!(help.contains("Gemini commands:"));
        assert!(help.contains("Work orchestration profile: Gemini CLI"));
        assert!(help.contains("Goal/Rubric/Next Action"));
        assert!(help.contains("single ACP message chunk only"));
    }

    #[test]
    fn gemini_status_text_exposes_sequential_profile_defaults() {
        let status = GeminiCliDriver::status_text("gemini-2.5-pro", 3);
        assert!(status.contains("- model: gemini-2.5-pro"));
        assert!(status.contains("- history_turns: 3"));
        assert!(status.contains("- task_orchestration: sequential"));
        assert!(status.contains("- task_monitoring: status-only"));
        assert!(status.contains("- progress_vector_checks: off"));
        assert!(status.contains("- preempt_on_new_prompt: off"));
        assert!(status.contains("- work_orchestration_sequence: R->P->M->W->A"));
    }
}
