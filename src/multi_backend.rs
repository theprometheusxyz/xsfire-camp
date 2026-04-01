use agent_client_protocol::{
    AuthMethod, AuthenticateRequest, AuthenticateResponse, CancelNotification, Error,
    ForkSessionRequest, ForkSessionResponse, ListSessionsRequest, ListSessionsResponse,
    LoadSessionRequest, LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest,
    PromptResponse, ResumeSessionRequest, ResumeSessionResponse, SessionConfigOption,
    SessionConfigOptionCategory, SessionConfigSelectOption, SessionId, SessionInfo,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, SetSessionModeRequest,
    SetSessionModeResponse, SetSessionModelRequest, SetSessionModelResponse, StopReason,
};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};
use uuid::Uuid;

use crate::{
    backend::{BackendDriver, BackendKind},
    cli_common::{prompt_blocks_to_text, send_agent_text},
    register_session_alias,
};

struct RoutedSession {
    active_backend: BackendKind,
    backend_sessions: HashMap<BackendKind, SessionId>,
    backend_config_options: HashMap<BackendKind, Vec<SessionConfigOption>>,
    cwd: std::path::PathBuf,
    mcp_servers: Vec<agent_client_protocol::McpServer>,
    meta: Option<agent_client_protocol::Meta>,
}

impl RoutedSession {
    fn single_backend(
        active_backend: BackendKind,
        child_session_id: SessionId,
        child_config_options: Vec<SessionConfigOption>,
        cwd: std::path::PathBuf,
        mcp_servers: Vec<agent_client_protocol::McpServer>,
        meta: Option<agent_client_protocol::Meta>,
    ) -> Self {
        let mut backend_sessions = HashMap::new();
        backend_sessions.insert(active_backend, child_session_id);
        let mut backend_config_options = HashMap::new();
        backend_config_options.insert(active_backend, child_config_options);
        Self {
            active_backend,
            backend_sessions,
            backend_config_options,
            cwd,
            mcp_servers,
            meta,
        }
    }
}

pub struct MultiBackendDriver {
    codex: Rc<dyn BackendDriver>,
    claude: Rc<dyn BackendDriver>,
    gemini: Rc<dyn BackendDriver>,
    sessions: RefCell<HashMap<SessionId, RoutedSession>>,
}

const MULTI_CODEX_CURSOR_PREFIX: &str = "multi:codex:";
const MULTI_ROUTED_CURSOR: &str = "multi:routed";

impl MultiBackendDriver {
    pub fn new(
        codex: Rc<dyn BackendDriver>,
        claude: Rc<dyn BackendDriver>,
        gemini: Rc<dyn BackendDriver>,
    ) -> Self {
        Self {
            codex,
            claude,
            gemini,
            sessions: RefCell::new(HashMap::new()),
        }
    }

    fn default_backend() -> BackendKind {
        std::env::var("XSFIRE_DEFAULT_BACKEND")
            .ok()
            .and_then(|v| BackendKind::parse(v.trim()))
            .filter(|k| *k != BackendKind::Multi)
            .unwrap_or(BackendKind::Codex)
    }

    fn driver_for(&self, backend: BackendKind) -> Rc<dyn BackendDriver> {
        match backend {
            BackendKind::Codex => self.codex.clone(),
            BackendKind::ClaudeCode => self.claude.clone(),
            BackendKind::Gemini => self.gemini.clone(),
            BackendKind::Multi => self.codex.clone(),
        }
    }

    fn parse_backend_selector(raw: &str) -> Option<BackendKind> {
        let trimmed = raw.trim();
        let mut parts = trimmed.split_whitespace();
        let cmd = parts.next()?;
        if cmd != "/backend" {
            return None;
        }
        parts.next().and_then(BackendKind::parse)
    }

    fn is_switch_backend_command(raw: &str) -> bool {
        raw.trim().starts_with("/backend")
    }

    fn backend_config_option(active_backend: BackendKind) -> SessionConfigOption {
        SessionConfigOption::select(
            "backend",
            "Backend",
            active_backend.as_str(),
            vec![
                SessionConfigSelectOption::new("codex", "Codex"),
                SessionConfigSelectOption::new("claude-code", "Claude Code"),
                SessionConfigSelectOption::new("gemini", "Gemini"),
            ],
        )
        .category(SessionConfigOptionCategory::Other)
        .description("Choose which backend handles this thread and apply its evidence-backed work-orchestration profile")
    }

    fn backend_usage_message() -> String {
        format!(
            "Usage: /backend <codex|claude-code|gemini>\n- codex: {}\n- claude-code: {}\n- gemini: {}",
            BackendKind::Codex
                .work_orchestration_profile()
                .bridge_summary(),
            BackendKind::ClaudeCode
                .work_orchestration_profile()
                .bridge_summary(),
            BackendKind::Gemini
                .work_orchestration_profile()
                .bridge_summary(),
        )
    }

    fn backend_switch_message(target_backend: BackendKind) -> String {
        format!(
            "Switched backend to `{}` for this thread.\n{}",
            target_backend.as_str(),
            target_backend.work_orchestration_profile().render_summary(),
        )
    }

    fn with_backend_option(
        existing: Option<Vec<SessionConfigOption>>,
        active_backend: BackendKind,
    ) -> Vec<SessionConfigOption> {
        let mut options = existing.unwrap_or_default();
        options.retain(|opt| opt.id.0.as_ref() != "backend");
        options.push(Self::backend_config_option(active_backend));
        options
    }

    fn merge_active_options(
        active_backend: BackendKind,
        mut options: Vec<SessionConfigOption>,
    ) -> Vec<SessionConfigOption> {
        options.retain(|opt| opt.id.0.as_ref() != "backend");
        options.push(Self::backend_config_option(active_backend));
        options
    }

    async fn ensure_backend_session(
        &self,
        session_id: &SessionId,
        backend: BackendKind,
    ) -> Result<SessionId, Error> {
        if let Some(existing) = self
            .sessions
            .borrow()
            .get(session_id)
            .and_then(|s| s.backend_sessions.get(&backend))
            .cloned()
        {
            return Ok(existing);
        }

        let (cwd, mcp_servers, meta) = {
            let sessions = self.sessions.borrow();
            let Some(session) = sessions.get(session_id) else {
                return Err(Error::resource_not_found(None));
            };
            (
                session.cwd.clone(),
                session.mcp_servers.clone(),
                session.meta.clone(),
            )
        };

        let request = NewSessionRequest::new(cwd)
            .mcp_servers(mcp_servers)
            .meta(meta.clone());
        let response = self.driver_for(backend).new_session(request).await?;
        let child = response.session_id;
        let child_options = response.config_options;

        {
            let mut sessions = self.sessions.borrow_mut();
            let Some(session) = sessions.get_mut(session_id) else {
                return Err(Error::resource_not_found(None));
            };
            session.backend_sessions.insert(backend, child.clone());
            session
                .backend_config_options
                .insert(backend, child_options.unwrap_or_default());
        }

        register_session_alias(&child, session_id);
        Ok(child)
    }

    fn resolve_routed(
        &self,
        session_id: &SessionId,
    ) -> Result<(BackendKind, Rc<dyn BackendDriver>, SessionId), Error> {
        let sessions = self.sessions.borrow();
        let Some(route) = sessions.get(session_id) else {
            return Err(Error::resource_not_found(None));
        };
        let backend = route.active_backend;
        let Some(child) = route.backend_sessions.get(&backend).cloned() else {
            return Err(Error::resource_not_found(None));
        };
        Ok((backend, self.driver_for(backend), child))
    }

    fn is_synthetic_routed_session_id(session_id: &SessionId) -> bool {
        session_id.0.as_ref().starts_with("multi:")
    }

    fn wrap_codex_cursor(cursor: String) -> String {
        format!("{MULTI_CODEX_CURSOR_PREFIX}{cursor}")
    }

    fn unwrap_codex_cursor(cursor: &str) -> Option<String> {
        cursor
            .strip_prefix(MULTI_CODEX_CURSOR_PREFIX)
            .map(str::to_string)
    }

    fn listable_routed_sessions(&self, cwd: Option<&std::path::PathBuf>) -> Vec<SessionInfo> {
        self.sessions
            .borrow()
            .iter()
            .filter(|(id, session)| {
                Self::is_synthetic_routed_session_id(id)
                    && cwd.map(|cwd| session.cwd == *cwd).unwrap_or(true)
            })
            .map(|(id, session)| {
                SessionInfo::new(id.clone(), session.cwd.clone()).title(format!(
                    "Unified session [{}]",
                    session.active_backend.as_str()
                ))
            })
            .collect()
    }

    fn register_routed_session(&self, session_id: SessionId, routed_session: RoutedSession) {
        let child_session_id = routed_session
            .backend_sessions
            .get(&routed_session.active_backend)
            .cloned();
        self.sessions
            .borrow_mut()
            .insert(session_id.clone(), routed_session);
        if let Some(child_session_id) = child_session_id {
            register_session_alias(&child_session_id, &session_id);
        }
    }

    fn codex_backing_session_for(
        &self,
        session_id: &SessionId,
        operation: &str,
    ) -> Result<SessionId, Error> {
        let sessions = self.sessions.borrow();
        if let Some(route) = sessions.get(session_id) {
            if route.active_backend != BackendKind::Codex {
                return Err(Error::invalid_params().data(format!(
                    "multi session/{operation} is only supported for codex-backed routed sessions"
                )));
            }
            return route
                .backend_sessions
                .get(&BackendKind::Codex)
                .cloned()
                .ok_or_else(|| Error::resource_not_found(None));
        }
        if Self::is_synthetic_routed_session_id(session_id) {
            return Err(Error::resource_not_found(None));
        }
        Ok(session_id.clone())
    }
}

#[async_trait::async_trait(?Send)]
impl BackendDriver for MultiBackendDriver {
    fn backend_kind(&self) -> BackendKind {
        BackendKind::Multi
    }

    fn supports_load_session(&self) -> bool {
        true
    }

    fn supports_fork_session(&self) -> bool {
        self.codex.supports_fork_session()
    }

    fn supports_resume_session(&self) -> bool {
        self.codex.supports_resume_session()
    }

    fn auth_methods(&self) -> Vec<AuthMethod> {
        let mut out = Vec::new();
        out.extend(self.codex.auth_methods());
        out.extend(self.claude.auth_methods());
        out.extend(self.gemini.auth_methods());
        out
    }

    async fn authenticate(
        &self,
        request: AuthenticateRequest,
    ) -> Result<AuthenticateResponse, Error> {
        let method = request.method_id.to_string();
        if matches!(
            method.as_str(),
            "chatgpt" | "codex-api-key" | "openai-api-key"
        ) {
            return self.codex.authenticate(request).await;
        }
        if method == "claude-cli" {
            return self.claude.authenticate(request).await;
        }
        if method == "gemini-cli" {
            return self.gemini.authenticate(request).await;
        }
        Err(Error::invalid_params().data(format!("unsupported auth method: {method}")))
    }

    async fn new_session(&self, request: NewSessionRequest) -> Result<NewSessionResponse, Error> {
        let backend = Self::default_backend();
        let child_response = self
            .driver_for(backend)
            .new_session(request.clone())
            .await?;
        let child_session_id = child_response.session_id.clone();
        let routed_session_id = SessionId::new(format!("multi:{}", Uuid::new_v4()));
        let child_backend_options = child_response.config_options.clone().unwrap_or_default();

        self.register_routed_session(
            routed_session_id.clone(),
            RoutedSession::single_backend(
                backend,
                child_session_id,
                child_backend_options.clone(),
                request.cwd,
                request.mcp_servers,
                request.meta,
            ),
        );

        let mut response = NewSessionResponse::new(routed_session_id);
        response.modes = child_response.modes;
        response.models = child_response.models;
        response.config_options = Some(Self::merge_active_options(backend, child_backend_options));
        response.meta = child_response.meta;
        Ok(response)
    }

    async fn load_session(
        &self,
        request: LoadSessionRequest,
    ) -> Result<LoadSessionResponse, Error> {
        let mut response = self.codex.load_session(request.clone()).await?;
        self.register_routed_session(
            request.session_id.clone(),
            RoutedSession::single_backend(
                BackendKind::Codex,
                request.session_id.clone(),
                response.config_options.clone().unwrap_or_default(),
                request.cwd,
                request.mcp_servers,
                request.meta,
            ),
        );
        response.config_options = Some(Self::with_backend_option(
            response.config_options,
            BackendKind::Codex,
        ));
        Ok(response)
    }

    async fn fork_session(
        &self,
        request: ForkSessionRequest,
    ) -> Result<ForkSessionResponse, Error> {
        let source_session_id = self.codex_backing_session_for(&request.session_id, "fork")?;
        let ForkSessionRequest {
            cwd,
            mcp_servers,
            meta,
            ..
        } = request.clone();
        let child_response = self
            .codex
            .fork_session(
                ForkSessionRequest::new(source_session_id, cwd.clone())
                    .mcp_servers(mcp_servers.clone())
                    .meta(meta.clone()),
            )
            .await?;

        let routed_session_id = SessionId::new(format!("multi:{}", Uuid::new_v4()));
        let child_session_id = child_response.session_id.clone();
        let child_backend_options = child_response.config_options.clone().unwrap_or_default();
        self.register_routed_session(
            routed_session_id.clone(),
            RoutedSession::single_backend(
                BackendKind::Codex,
                child_session_id,
                child_backend_options.clone(),
                cwd,
                mcp_servers,
                meta,
            ),
        );

        Ok(ForkSessionResponse::new(routed_session_id)
            .modes(child_response.modes)
            .models(child_response.models)
            .config_options(Some(Self::merge_active_options(
                BackendKind::Codex,
                child_backend_options,
            )))
            .meta(child_response.meta))
    }

    async fn resume_session(
        &self,
        request: ResumeSessionRequest,
    ) -> Result<ResumeSessionResponse, Error> {
        let child_session_id = self.codex_backing_session_for(&request.session_id, "resume")?;
        let ResumeSessionRequest {
            session_id,
            cwd,
            mcp_servers,
            meta,
            ..
        } = request.clone();
        let child_response = self
            .codex
            .resume_session(
                ResumeSessionRequest::new(child_session_id.clone(), cwd.clone())
                    .mcp_servers(mcp_servers.clone())
                    .meta(meta.clone()),
            )
            .await?;
        let child_backend_options = child_response.config_options.clone().unwrap_or_default();
        self.register_routed_session(
            session_id,
            RoutedSession::single_backend(
                BackendKind::Codex,
                child_session_id,
                child_backend_options.clone(),
                cwd,
                mcp_servers,
                meta,
            ),
        );

        Ok(ResumeSessionResponse::new()
            .modes(child_response.modes)
            .models(child_response.models)
            .config_options(Some(Self::merge_active_options(
                BackendKind::Codex,
                child_backend_options,
            )))
            .meta(child_response.meta))
    }

    async fn list_sessions(
        &self,
        request: ListSessionsRequest,
    ) -> Result<ListSessionsResponse, Error> {
        if request.cursor.as_deref() == Some(MULTI_ROUTED_CURSOR) {
            return Ok(ListSessionsResponse::new(
                self.listable_routed_sessions(request.cwd.as_ref()),
            ));
        }

        let mut codex_request = request.clone();
        codex_request.cursor = request
            .cursor
            .as_deref()
            .and_then(Self::unwrap_codex_cursor)
            .or_else(|| request.cursor.clone());

        let mut response = self.codex.list_sessions(codex_request).await?;
        if let Some(next_cursor) = response.next_cursor.take() {
            response.next_cursor = Some(Self::wrap_codex_cursor(next_cursor));
            return Ok(response);
        }

        let routed_sessions = self.listable_routed_sessions(request.cwd.as_ref());
        if request.cursor.is_some() {
            if !routed_sessions.is_empty() {
                response.next_cursor = Some(MULTI_ROUTED_CURSOR.to_string());
            }
            return Ok(response);
        }

        let mut seen = response
            .sessions
            .iter()
            .map(|session| session.session_id.0.to_string())
            .collect::<HashSet<_>>();
        for session in routed_sessions {
            if seen.insert(session.session_id.0.to_string()) {
                response.sessions.push(session);
            }
        }

        Ok(response)
    }

    async fn prompt(&self, request: PromptRequest) -> Result<PromptResponse, Error> {
        let session_id = request.session_id.clone();
        let prompt_text = prompt_blocks_to_text(&request.prompt);

        if let Some(target_backend) = Self::parse_backend_selector(&prompt_text) {
            self.ensure_backend_session(&session_id, target_backend)
                .await?;
            {
                let mut sessions = self.sessions.borrow_mut();
                let Some(session) = sessions.get_mut(&session_id) else {
                    return Err(Error::resource_not_found(None));
                };
                session.active_backend = target_backend;
            }

            send_agent_text(&session_id, Self::backend_switch_message(target_backend)).await;
            return Ok(PromptResponse::new(StopReason::EndTurn));
        }

        if Self::is_switch_backend_command(&prompt_text) {
            send_agent_text(&session_id, Self::backend_usage_message()).await;
            return Ok(PromptResponse::new(StopReason::EndTurn));
        }

        let (backend, driver, child_session_id) = match self.resolve_routed(&session_id) {
            Ok(v) => v,
            Err(_) => {
                let target_backend = Self::default_backend();
                self.ensure_backend_session(&session_id, target_backend)
                    .await?;
                {
                    let mut sessions = self.sessions.borrow_mut();
                    let Some(session) = sessions.get_mut(&session_id) else {
                        return Err(Error::resource_not_found(None));
                    };
                    session.active_backend = target_backend;
                }
                self.resolve_routed(&session_id)?
            }
        };

        let mut routed = request;
        routed.session_id = child_session_id;
        let _ = backend; // Reserved for backend-specific behavior.
        driver.prompt(routed).await
    }

    async fn cancel(&self, args: CancelNotification) -> Result<(), Error> {
        let (_, driver, child_session_id) = self.resolve_routed(&args.session_id)?;
        let mut routed = args;
        routed.session_id = child_session_id;
        driver.cancel(routed).await
    }

    async fn set_session_mode(
        &self,
        args: SetSessionModeRequest,
    ) -> Result<SetSessionModeResponse, Error> {
        let (_, driver, child_session_id) = self.resolve_routed(&args.session_id)?;
        let mut routed = args;
        routed.session_id = child_session_id;
        driver.set_session_mode(routed).await
    }

    async fn set_session_model(
        &self,
        args: SetSessionModelRequest,
    ) -> Result<SetSessionModelResponse, Error> {
        let (_, driver, child_session_id) = self.resolve_routed(&args.session_id)?;
        let mut routed = args;
        routed.session_id = child_session_id;
        driver.set_session_model(routed).await
    }

    async fn set_session_config_option(
        &self,
        args: SetSessionConfigOptionRequest,
    ) -> Result<SetSessionConfigOptionResponse, Error> {
        if args.config_id.0.as_ref() == "backend" {
            let target_backend = BackendKind::parse(args.value.0.as_ref()).ok_or_else(|| {
                Error::invalid_params().data("backend must be one of: codex|claude-code|gemini")
            })?;
            if target_backend == BackendKind::Multi {
                return Err(Error::invalid_params()
                    .data("backend must be one of: codex|claude-code|gemini"));
            }

            self.ensure_backend_session(&args.session_id, target_backend)
                .await?;
            let merged_options = {
                let sessions = self.sessions.borrow();
                let Some(session) = sessions.get(&args.session_id) else {
                    return Err(Error::resource_not_found(None));
                };
                Self::merge_active_options(
                    target_backend,
                    session
                        .backend_config_options
                        .get(&target_backend)
                        .cloned()
                        .unwrap_or_default(),
                )
            };
            {
                let mut sessions = self.sessions.borrow_mut();
                let Some(session) = sessions.get_mut(&args.session_id) else {
                    return Err(Error::resource_not_found(None));
                };
                session.active_backend = target_backend;
            }
            send_agent_text(
                &args.session_id,
                Self::backend_switch_message(target_backend),
            )
            .await;
            return Ok(SetSessionConfigOptionResponse::new(merged_options));
        }

        let parent_session_id = args.session_id.clone();
        let (backend, driver, child_session_id) = self.resolve_routed(&parent_session_id)?;
        let mut routed = args;
        routed.session_id = child_session_id;
        let response = driver.set_session_config_option(routed).await?;
        let merged_options = Self::merge_active_options(backend, response.config_options.clone());
        {
            let mut sessions = self.sessions.borrow_mut();
            if let Some(session) = sessions.get_mut(&parent_session_id) {
                session
                    .backend_config_options
                    .insert(backend, response.config_options.clone());
            }
        }
        Ok(SetSessionConfigOptionResponse::new(merged_options).meta(response.meta))
    }
}

#[cfg(test)]
mod tests {
    use super::MultiBackendDriver;
    use crate::backend::{BackendDriver, BackendKind};
    use agent_client_protocol::{
        AuthMethod, AuthenticateRequest, AuthenticateResponse, CancelNotification, Error,
        ForkSessionRequest, ForkSessionResponse, ListSessionsRequest, ListSessionsResponse,
        LoadSessionRequest, LoadSessionResponse, NewSessionRequest, NewSessionResponse,
        PromptRequest, PromptResponse, ResumeSessionRequest, ResumeSessionResponse,
        SessionConfigOption, SessionId, SessionInfo, SetSessionConfigOptionRequest,
        SetSessionConfigOptionResponse, SetSessionModeRequest, SetSessionModeResponse,
        SetSessionModelRequest, SetSessionModelResponse, StopReason,
    };
    use std::{cell::RefCell, collections::HashMap, path::PathBuf, rc::Rc};

    struct StubDriver {
        backend: BackendKind,
        sessions: RefCell<Vec<SessionInfo>>,
        list_responses: RefCell<HashMap<Option<String>, ListSessionsResponse>>,
        supports_load_session: bool,
        supports_fork_session: bool,
        supports_resume_session: bool,
        fork_response: RefCell<Option<ForkSessionResponse>>,
        resume_response: RefCell<Option<ResumeSessionResponse>>,
        fork_requests: RefCell<Vec<ForkSessionRequest>>,
        resume_requests: RefCell<Vec<ResumeSessionRequest>>,
    }

    impl StubDriver {
        fn new(
            backend: BackendKind,
            sessions: Vec<SessionInfo>,
            supports_load_session: bool,
        ) -> Self {
            Self {
                backend,
                sessions: RefCell::new(sessions),
                list_responses: RefCell::new(HashMap::new()),
                supports_load_session,
                supports_fork_session: false,
                supports_resume_session: false,
                fork_response: RefCell::new(None),
                resume_response: RefCell::new(None),
                fork_requests: RefCell::new(Vec::new()),
                resume_requests: RefCell::new(Vec::new()),
            }
        }

        fn with_list_responses(
            backend: BackendKind,
            sessions: Vec<SessionInfo>,
            list_responses: HashMap<Option<String>, ListSessionsResponse>,
            supports_load_session: bool,
        ) -> Self {
            Self {
                backend,
                sessions: RefCell::new(sessions),
                list_responses: RefCell::new(list_responses),
                supports_load_session,
                supports_fork_session: false,
                supports_resume_session: false,
                fork_response: RefCell::new(None),
                resume_response: RefCell::new(None),
                fork_requests: RefCell::new(Vec::new()),
                resume_requests: RefCell::new(Vec::new()),
            }
        }

        fn with_fork_response(mut self, response: ForkSessionResponse) -> Self {
            self.supports_fork_session = true;
            self.fork_response = RefCell::new(Some(response));
            self
        }

        fn with_resume_response(mut self, response: ResumeSessionResponse) -> Self {
            self.supports_resume_session = true;
            self.resume_response = RefCell::new(Some(response));
            self
        }
    }

    #[async_trait::async_trait(?Send)]
    impl BackendDriver for StubDriver {
        fn backend_kind(&self) -> BackendKind {
            self.backend
        }

        fn supports_load_session(&self) -> bool {
            self.supports_load_session
        }

        fn supports_fork_session(&self) -> bool {
            self.supports_fork_session
        }

        fn supports_resume_session(&self) -> bool {
            self.supports_resume_session
        }

        fn auth_methods(&self) -> Vec<AuthMethod> {
            Vec::new()
        }

        async fn authenticate(
            &self,
            _request: AuthenticateRequest,
        ) -> Result<AuthenticateResponse, Error> {
            Ok(AuthenticateResponse::new())
        }

        async fn new_session(
            &self,
            request: NewSessionRequest,
        ) -> Result<NewSessionResponse, Error> {
            let session_id = SessionId::new(format!("{}:stub", self.backend.as_str()));
            self.sessions.borrow_mut().push(
                SessionInfo::new(session_id.clone(), request.cwd).title(self.backend.as_str()),
            );
            Ok(NewSessionResponse::new(session_id))
        }

        async fn load_session(
            &self,
            _request: LoadSessionRequest,
        ) -> Result<LoadSessionResponse, Error> {
            Err(Error::invalid_params().data("unsupported"))
        }

        async fn fork_session(
            &self,
            request: ForkSessionRequest,
        ) -> Result<ForkSessionResponse, Error> {
            self.fork_requests.borrow_mut().push(request);
            self.fork_response
                .borrow()
                .clone()
                .ok_or_else(|| Error::invalid_params().data("unsupported"))
        }

        async fn resume_session(
            &self,
            request: ResumeSessionRequest,
        ) -> Result<ResumeSessionResponse, Error> {
            self.resume_requests.borrow_mut().push(request);
            self.resume_response
                .borrow()
                .clone()
                .ok_or_else(|| Error::invalid_params().data("unsupported"))
        }

        async fn list_sessions(
            &self,
            request: ListSessionsRequest,
        ) -> Result<ListSessionsResponse, Error> {
            if let Some(response) = self.list_responses.borrow().get(&request.cursor).cloned() {
                return Ok(response);
            }
            Ok(ListSessionsResponse::new(self.sessions.borrow().clone()))
        }

        async fn prompt(&self, _request: PromptRequest) -> Result<PromptResponse, Error> {
            Ok(PromptResponse::new(StopReason::EndTurn))
        }

        async fn cancel(&self, _args: CancelNotification) -> Result<(), Error> {
            Ok(())
        }

        async fn set_session_mode(
            &self,
            _args: SetSessionModeRequest,
        ) -> Result<SetSessionModeResponse, Error> {
            Err(Error::invalid_params().data("unsupported"))
        }

        async fn set_session_model(
            &self,
            _args: SetSessionModelRequest,
        ) -> Result<SetSessionModelResponse, Error> {
            Err(Error::invalid_params().data("unsupported"))
        }

        async fn set_session_config_option(
            &self,
            _args: SetSessionConfigOptionRequest,
        ) -> Result<SetSessionConfigOptionResponse, Error> {
            Err(Error::invalid_params().data("unsupported"))
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_sessions_merges_codex_and_routed_sessions() {
        let cwd = PathBuf::from("/tmp/xsfire-camp-test");
        let codex_session = SessionInfo::new("codex:existing", cwd.clone()).title("Codex stored");
        let driver = MultiBackendDriver::new(
            Rc::new(StubDriver::new(
                BackendKind::Codex,
                vec![codex_session],
                true,
            )),
            Rc::new(StubDriver::new(BackendKind::ClaudeCode, Vec::new(), false)),
            Rc::new(StubDriver::new(BackendKind::Gemini, Vec::new(), false)),
        );

        let created = driver
            .new_session(NewSessionRequest::new(cwd.clone()))
            .await
            .unwrap();
        let response = driver
            .list_sessions(ListSessionsRequest::new())
            .await
            .unwrap();

        let ids = response
            .sessions
            .iter()
            .map(|session| session.session_id.0.to_string())
            .collect::<Vec<_>>();

        assert!(ids.iter().any(|id| id == "codex:existing"));
        assert!(ids.iter().any(|id| id == created.session_id.0.as_ref()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_sessions_dedupes_loaded_codex_session_ids() {
        let cwd = PathBuf::from("/tmp/xsfire-camp-test");
        let codex_id = SessionId::new("codex:existing");
        let codex_session = SessionInfo::new(codex_id.clone(), cwd.clone()).title("Codex stored");
        let driver = MultiBackendDriver::new(
            Rc::new(StubDriver::new(
                BackendKind::Codex,
                vec![codex_session],
                true,
            )),
            Rc::new(StubDriver::new(BackendKind::ClaudeCode, Vec::new(), false)),
            Rc::new(StubDriver::new(BackendKind::Gemini, Vec::new(), false)),
        );

        driver.sessions.borrow_mut().insert(
            codex_id.clone(),
            super::RoutedSession {
                active_backend: BackendKind::Codex,
                backend_sessions: HashMap::from([(BackendKind::Codex, codex_id.clone())]),
                backend_config_options: HashMap::new(),
                cwd: cwd.clone(),
                mcp_servers: Vec::new(),
                meta: None,
            },
        );

        let response = driver
            .list_sessions(ListSessionsRequest::new())
            .await
            .unwrap();
        let count = response
            .sessions
            .iter()
            .filter(|session| session.session_id == codex_id)
            .count();

        assert_eq!(count, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_sessions_defers_routed_sessions_until_codex_pages_finish() {
        let cwd = PathBuf::from("/tmp/xsfire-camp-test");
        let first_page = ListSessionsResponse::new(vec![
            SessionInfo::new("codex:page-1", cwd.clone()).title("Codex page 1"),
        ])
        .next_cursor("page-2");
        let second_page = ListSessionsResponse::new(vec![
            SessionInfo::new("codex:page-2", cwd.clone()).title("Codex page 2"),
        ]);

        let driver = MultiBackendDriver::new(
            Rc::new(StubDriver::with_list_responses(
                BackendKind::Codex,
                Vec::new(),
                HashMap::from([
                    (None, first_page),
                    (Some("page-2".to_string()), second_page),
                ]),
                true,
            )),
            Rc::new(StubDriver::new(BackendKind::ClaudeCode, Vec::new(), false)),
            Rc::new(StubDriver::new(BackendKind::Gemini, Vec::new(), false)),
        );

        let created = driver
            .new_session(NewSessionRequest::new(cwd.clone()))
            .await
            .unwrap();

        let first = driver
            .list_sessions(ListSessionsRequest::new())
            .await
            .unwrap();
        let first_ids = first
            .sessions
            .iter()
            .map(|session| session.session_id.0.to_string())
            .collect::<Vec<_>>();
        assert_eq!(first_ids, vec!["codex:page-1"]);
        assert_eq!(first.next_cursor.as_deref(), Some("multi:codex:page-2"));

        let second = driver
            .list_sessions(
                ListSessionsRequest::new()
                    .cursor(first.next_cursor.clone().expect("cursor should exist")),
            )
            .await
            .unwrap();
        let second_ids = second
            .sessions
            .iter()
            .map(|session| session.session_id.0.to_string())
            .collect::<Vec<_>>();
        assert_eq!(second_ids, vec!["codex:page-2"]);
        assert_eq!(second.next_cursor.as_deref(), Some("multi:routed"));

        let routed = driver
            .list_sessions(
                ListSessionsRequest::new().cursor(
                    second
                        .next_cursor
                        .clone()
                        .expect("routed cursor should exist"),
                ),
            )
            .await
            .unwrap();
        let routed_ids = routed
            .sessions
            .iter()
            .map(|session| session.session_id.0.to_string())
            .collect::<Vec<_>>();
        assert_eq!(routed_ids, vec![created.session_id.0.to_string()]);
        assert_eq!(routed.next_cursor, None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fork_session_wraps_codex_child_in_synthetic_multi_session() {
        let cwd = PathBuf::from("/tmp/xsfire-camp-test");
        let codex = Rc::new(
            StubDriver::new(BackendKind::Codex, Vec::new(), true).with_fork_response(
                ForkSessionResponse::new("codex:forked").config_options(vec![
                    SessionConfigOption::select(
                        "model",
                        "Model",
                        "gpt-5",
                        vec![agent_client_protocol::SessionConfigSelectOption::new(
                            "gpt-5", "GPT-5",
                        )],
                    ),
                ]),
            ),
        );
        let driver = MultiBackendDriver::new(
            codex.clone(),
            Rc::new(StubDriver::new(BackendKind::ClaudeCode, Vec::new(), false)),
            Rc::new(StubDriver::new(BackendKind::Gemini, Vec::new(), false)),
        );

        let response = driver
            .fork_session(ForkSessionRequest::new("codex:source", cwd.clone()))
            .await
            .unwrap();

        assert!(response.session_id.0.as_ref().starts_with("multi:"));
        assert_eq!(
            codex.fork_requests.borrow()[0].session_id.0.as_ref(),
            "codex:source"
        );
        let route = driver.sessions.borrow();
        let stored = route.get(&response.session_id).unwrap();
        assert_eq!(stored.active_backend, BackendKind::Codex);
        assert_eq!(
            stored
                .backend_sessions
                .get(&BackendKind::Codex)
                .unwrap()
                .0
                .as_ref(),
            "codex:forked"
        );
        let option_ids = response
            .config_options
            .unwrap()
            .into_iter()
            .map(|option| option.id.0.to_string())
            .collect::<Vec<_>>();
        assert!(option_ids.iter().any(|id| id == "backend"));
        assert!(option_ids.iter().any(|id| id == "model"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fork_session_rejects_non_codex_routed_session() {
        let cwd = PathBuf::from("/tmp/xsfire-camp-test");
        let driver = MultiBackendDriver::new(
            Rc::new(StubDriver::new(BackendKind::Codex, Vec::new(), true)),
            Rc::new(StubDriver::new(BackendKind::ClaudeCode, Vec::new(), false)),
            Rc::new(StubDriver::new(BackendKind::Gemini, Vec::new(), false)),
        );
        let session_id = SessionId::new("multi:test");
        driver.sessions.borrow_mut().insert(
            session_id.clone(),
            super::RoutedSession {
                active_backend: BackendKind::ClaudeCode,
                backend_sessions: HashMap::from([(
                    BackendKind::ClaudeCode,
                    SessionId::new("claude-code:child"),
                )]),
                backend_config_options: HashMap::new(),
                cwd: cwd.clone(),
                mcp_servers: Vec::new(),
                meta: None,
            },
        );

        let result = driver
            .fork_session(ForkSessionRequest::new(session_id, cwd))
            .await;

        assert!(result.is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn resume_session_registers_requested_session_id_as_codex_route() {
        let cwd = PathBuf::from("/tmp/xsfire-camp-test");
        let codex = Rc::new(
            StubDriver::new(BackendKind::Codex, Vec::new(), true).with_resume_response(
                ResumeSessionResponse::new().config_options(vec![SessionConfigOption::select(
                    "model",
                    "Model",
                    "gpt-5",
                    vec![agent_client_protocol::SessionConfigSelectOption::new(
                        "gpt-5", "GPT-5",
                    )],
                )]),
            ),
        );
        let driver = MultiBackendDriver::new(
            codex.clone(),
            Rc::new(StubDriver::new(BackendKind::ClaudeCode, Vec::new(), false)),
            Rc::new(StubDriver::new(BackendKind::Gemini, Vec::new(), false)),
        );
        let session_id = SessionId::new("codex:resume");

        let response = driver
            .resume_session(ResumeSessionRequest::new(session_id.clone(), cwd.clone()))
            .await
            .unwrap();

        assert_eq!(
            codex.resume_requests.borrow()[0].session_id.0.as_ref(),
            "codex:resume"
        );
        let route = driver.sessions.borrow();
        let stored = route.get(&session_id).unwrap();
        assert_eq!(stored.active_backend, BackendKind::Codex);
        assert_eq!(
            stored
                .backend_sessions
                .get(&BackendKind::Codex)
                .unwrap()
                .0
                .as_ref(),
            "codex:resume"
        );
        let option_ids = response
            .config_options
            .unwrap()
            .into_iter()
            .map(|option| option.id.0.to_string())
            .collect::<Vec<_>>();
        assert!(option_ids.iter().any(|id| id == "backend"));
        assert!(option_ids.iter().any(|id| id == "model"));
    }

    #[test]
    fn backend_switch_message_includes_profile_summary() {
        let message = MultiBackendDriver::backend_switch_message(BackendKind::ClaudeCode);
        assert!(message.contains("Switched backend to `claude-code`"));
        assert!(message.contains("Work orchestration profile: Claude Code"));
        assert!(message.contains("Goal/Rubric/Next Action"));
        assert!(message.contains("single ACP message chunk only"));
    }

    #[test]
    fn backend_usage_message_lists_backend_bridge_modes() {
        let message = MultiBackendDriver::backend_usage_message();
        assert!(message.contains("codex: live ACP plan/tool updates available"));
        assert!(message.contains("claude-code: single ACP message chunk only"));
        assert!(message.contains("gemini: single ACP message chunk only"));
    }
}
