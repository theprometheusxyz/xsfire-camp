use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    ops::DerefMut,
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use codex_apply_patch::parse_patch;

use agent_client_protocol::{
    Annotations, AudioContent, AvailableCommand, AvailableCommandInput, AvailableCommandsUpdate,
    BlobResourceContents, Client, ClientCapabilities, ConfigOptionUpdate, Content, ContentBlock,
    ContentChunk, Diff, EmbeddedResource, EmbeddedResourceResource, Error, ImageContent,
    LoadSessionResponse, Meta, ModelId, ModelInfo, PermissionOption, PermissionOptionKind, Plan,
    PlanEntry, PlanEntryPriority, PlanEntryStatus, PromptRequest, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, ResourceLink, SelectedPermissionOutcome,
    SessionConfigId, SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption,
    SessionConfigValueId, SessionId, SessionMode, SessionModeId, SessionModeState,
    SessionModelState, SessionNotification, SessionUpdate, StopReason, Terminal, TextContent,
    TextResourceContents, ToolCall, ToolCallContent, ToolCallId, ToolCallLocation, ToolCallStatus,
    ToolCallUpdate, ToolCallUpdateFields, ToolKind, UnstructuredCommandInput,
};
use codex_common::approval_presets::{ApprovalPreset, builtin_approval_presets};
use codex_core::{
    AuthManager, CodexThread, RolloutRecorder, ThreadSortKey,
    config::{
        Config,
        edit::{ConfigEdit, ConfigEditsBuilder},
        set_project_trust_level,
    },
    error::CodexErr,
    features::{FEATURES, Feature},
    models_manager::manager::{ModelsManager, RefreshStrategy},
    parse_command::parse_command,
    parse_turn_item,
    protocol::{
        AgentMessageContentDeltaEvent, AgentMessageEvent, AgentReasoningEvent,
        AgentReasoningRawContentEvent, AgentReasoningSectionBreakEvent,
        ApplyPatchApprovalRequestEvent, ElicitationAction, ErrorEvent, Event, EventMsg,
        ExecApprovalRequestEvent, ExecCommandBeginEvent, ExecCommandEndEvent,
        ExecCommandOutputDeltaEvent, ExitedReviewModeEvent, FileChange, ItemCompletedEvent,
        ItemStartedEvent, ListCustomPromptsResponseEvent, McpInvocation, McpStartupCompleteEvent,
        McpStartupUpdateEvent, McpToolCallBeginEvent, McpToolCallEndEvent, Op,
        PatchApplyBeginEvent, PatchApplyEndEvent, ReasoningContentDeltaEvent,
        ReasoningRawContentDeltaEvent, ReviewDecision, ReviewOutputEvent, ReviewRequest,
        ReviewTarget, SandboxPolicy, SessionSource, StreamErrorEvent, TerminalInteractionEvent,
        TokenCountEvent, TurnAbortedEvent, TurnCompleteEvent, TurnStartedEvent, UserMessageEvent,
        ViewImageToolCallEvent, WarningEvent, WebSearchBeginEvent, WebSearchEndEvent,
    },
    review_format::format_review_findings_block,
    review_prompts::user_facing_hint,
};
use codex_protocol::{
    approvals::ElicitationRequestEvent,
    config_types::{Personality, TrustLevel},
    custom_prompts::CustomPrompt,
    items::TurnItem,
    models::ResponseItem,
    openai_models::{ModelPreset, ReasoningEffort},
    parse_command::ParsedCommand,
    plan_tool::{PlanItemArg, StepStatus, UpdatePlanArgs},
    protocol::{RolloutItem, SessionMetaLine},
    user_input::UserInput,
};
use heck::ToTitleCase;
use itertools::Itertools;
use mcp_types::{CallToolResult, RequestId};
use serde_json::json;
use tokio::sync::{mpsc, oneshot};
use tracing::{error, info, warn};
use unicode_segmentation::UnicodeSegmentation;
use uuid::Uuid;

use crate::{
    ACP_CLIENT,
    backend::{BackendKind, WorkOrchestrationProfile},
    current_client_info,
    link_paths::normalize_outgoing_local_markdown_links,
    prompt_args::{expand_custom_prompt, parse_slash_name},
    session_store::SessionStore,
};

static APPROVAL_PRESETS: LazyLock<Vec<ApprovalPreset>> = LazyLock::new(builtin_approval_presets);
const INIT_COMMAND_PROMPT: &str = include_str!("./prompt_for_init_command.md");
const SESSION_LIST_PAGE_SIZE: usize = 25;
const SESSION_TITLE_MAX_GRAPHEMES: usize = 120;
const CONTEXT_OPT_TRIGGER_PERCENT_DEFAULT: i64 = 90;
const CONTEXT_OPT_TRIGGER_PERCENT_OPTIONS: [i64; 5] = [75, 80, 85, 90, 95];
const FEATURE_CONFIG_ID_PREFIX: &str = "beta_feature.";
const MONITOR_PANEL_WIDTH_DEFAULT: usize = 92;
const MONITOR_PANEL_WIDTH_MIN: usize = 56;
const MONITOR_PANEL_WIDTH_MAX: usize = 132;
const MONITOR_PROGRESS_BAR_MIN_WIDTH: usize = 18;
const MONITOR_PROGRESS_BAR_MAX_WIDTH: usize = 40;
const CONFIG_OPTIONS_DENSITY_ENV: &str = "XSFIRE_CONFIG_OPTIONS_DENSITY";
const CONFIG_OPTIONS_INLINE_FULL_MIN_COLUMNS: usize = 140;
const EXEC_OUTPUT_MAX_BYTES_ENV: &str = "ACP_EXEC_OUTPUT_MAX_BYTES";
const EXEC_OUTPUT_MAX_BYTES_DEFAULT: usize = 64 * 1024;
const EXEC_OUTPUT_MAX_BYTES_MIN: usize = 4 * 1024;
const EXEC_OUTPUT_MAX_BYTES_MAX: usize = 2 * 1024 * 1024;
const UI_TEXT_CHUNK_MAX_CHARS_ENV: &str = "ACP_UI_TEXT_CHUNK_MAX_CHARS";
const UI_TEXT_CHUNK_MAX_CHARS_DEFAULT: usize = 12_000;
const UI_TEXT_CHUNK_MAX_CHARS_MIN: usize = 512;
const UI_TEXT_CHUNK_MAX_CHARS_MAX: usize = 200_000;
const TOOL_RAW_OUTPUT_MAX_CHARS_ENV: &str = "ACP_TOOL_RAW_OUTPUT_MAX_CHARS";
const TOOL_RAW_OUTPUT_MAX_CHARS_DEFAULT: usize = 32_000;
const TOOL_RAW_OUTPUT_MAX_CHARS_MIN: usize = 2_048;
const TOOL_RAW_OUTPUT_MAX_CHARS_MAX: usize = 500_000;
const TOOL_CALL_WATCHDOG_SECONDS_DEFAULT: u64 = 90;
const TOOL_CALL_WATCHDOG_SECONDS_MIN: u64 = 60;
const TOOL_CALL_WATCHDOG_SECONDS_MAX: u64 = 120;
const TOOL_CALL_WATCHDOG_TICK_SECONDS: u64 = 1;
const DIAGNOSTICS_AUTO_LOG_ENV: &str = "ACP_DIAGNOSTICS_AUTO_LOG";
const DIAGNOSTICS_LOG_EVERY_ENV: &str = "ACP_DIAGNOSTICS_LOG_EVERY";
const DIAGNOSTICS_LOG_EVERY_DEFAULT: u64 = 50;
const DIAGNOSTICS_LOG_EVERY_MIN: u64 = 5;
const DIAGNOSTICS_LOG_EVERY_MAX: u64 = 5_000;
const MONITOR_BOTTLENECK_SLOW_SECS: u64 = 20;
const MONITOR_REPEAT_STREAK_WARN: usize = 3;
const MONITOR_PHASE_DOMINANCE_MIN_EVENTS: usize = 8;
const MONITOR_PHASE_DOMINANCE_WARN_PERCENT: usize = 75;
const MONITOR_STALL_PLAN_UPDATE_WARN: usize = 2;
const MONITOR_STALL_NO_PROGRESS_WARN_SECS: u64 = 45;

#[derive(Clone, Debug)]
struct SessionListEntry {
    id: SessionId,
    title: Option<String>,
    updated_at: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContextOptimizationMode {
    Off,
    Monitor,
    Auto,
}

impl ContextOptimizationMode {
    fn from_config_value(raw: &str) -> Result<Self, Error> {
        match raw {
            "off" => Ok(Self::Off),
            "monitor" => Ok(Self::Monitor),
            "auto" => Ok(Self::Auto),
            _ => Err(Error::invalid_params()
                .data("Unsupported context optimization mode (expected: off, monitor, auto)")),
        }
    }

    fn as_config_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Monitor => "monitor",
            Self::Auto => "auto",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TaskOrchestrationMode {
    Parallel,
    Sequential,
}

impl TaskOrchestrationMode {
    fn from_config_value(raw: &str) -> Result<Self, Error> {
        match raw {
            "parallel" => Ok(Self::Parallel),
            "sequential" => Ok(Self::Sequential),
            _ => Err(Error::invalid_params()
                .data("Unsupported task orchestration mode (expected: parallel, sequential)")),
        }
    }

    fn as_config_value(self) -> &'static str {
        match self {
            Self::Parallel => "parallel",
            Self::Sequential => "sequential",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TaskMonitoringMode {
    On,
    Auto,
    Off,
}

impl TaskMonitoringMode {
    fn from_config_value(raw: &str) -> Result<Self, Error> {
        match raw {
            "on" => Ok(Self::On),
            "auto" => Ok(Self::Auto),
            "off" => Ok(Self::Off),
            _ => Err(Error::invalid_params()
                .data("Task Monitoring values must be one of: on, auto, off")),
        }
    }

    fn as_config_value(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Auto => "auto",
            Self::Off => "off",
        }
    }

    fn is_enabled(self) -> bool {
        !matches!(self, Self::Off)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UiVisibilityMode {
    Full,
    FinalOnly,
}

impl UiVisibilityMode {
    fn from_env() -> Self {
        match std::env::var("ACP_UI_VISIBILITY_MODE")
            .ok()
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("final_only") => Self::FinalOnly,
            _ => Self::Full,
        }
    }

    fn hides_internal_updates(self) -> bool {
        matches!(self, Self::FinalOnly)
    }

    fn as_config_value(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::FinalOnly => "final_only",
        }
    }
}

#[derive(Debug)]
struct RuntimeDiagnosticsState {
    started_at: Instant,
    notifications_sent: AtomicU64,
    notification_errors: AtomicU64,
    user_message_chunks: AtomicU64,
    user_message_chars: AtomicU64,
    agent_message_chunks: AtomicU64,
    agent_message_chars: AtomicU64,
    agent_thought_chunks: AtomicU64,
    agent_thought_chars: AtomicU64,
    tool_calls: AtomicU64,
    tool_call_payload_chars: AtomicU64,
    tool_call_updates: AtomicU64,
    tool_call_update_payload_chars: AtomicU64,
    plan_updates: AtomicU64,
    permission_requests: AtomicU64,
    auto_log_notification_watermark: AtomicU64,
}

impl RuntimeDiagnosticsState {
    fn new() -> Self {
        Self {
            started_at: Instant::now(),
            notifications_sent: AtomicU64::new(0),
            notification_errors: AtomicU64::new(0),
            user_message_chunks: AtomicU64::new(0),
            user_message_chars: AtomicU64::new(0),
            agent_message_chunks: AtomicU64::new(0),
            agent_message_chars: AtomicU64::new(0),
            agent_thought_chunks: AtomicU64::new(0),
            agent_thought_chars: AtomicU64::new(0),
            tool_calls: AtomicU64::new(0),
            tool_call_payload_chars: AtomicU64::new(0),
            tool_call_updates: AtomicU64::new(0),
            tool_call_update_payload_chars: AtomicU64::new(0),
            plan_updates: AtomicU64::new(0),
            permission_requests: AtomicU64::new(0),
            auto_log_notification_watermark: AtomicU64::new(0),
        }
    }
}

#[derive(Debug, serde::Serialize)]
struct RuntimeDiagnosticsSnapshot {
    reason: String,
    client_info: Option<String>,
    ui_visibility_mode: String,
    uptime_secs: u64,
    active_tasks: usize,
    notifications_sent: u64,
    notification_errors: u64,
    current_rss_bytes: Option<u64>,
    user_message_chunks: u64,
    user_message_chars: u64,
    agent_message_chunks: u64,
    agent_message_chars: u64,
    agent_thought_chunks: u64,
    agent_thought_chars: u64,
    tool_calls: u64,
    tool_call_payload_chars: u64,
    tool_call_updates: u64,
    tool_call_update_payload_chars: u64,
    plan_updates: u64,
    permission_requests: u64,
}

impl RuntimeDiagnosticsSnapshot {
    fn total_payload_chars(&self) -> u64 {
        self.user_message_chars
            + self.agent_message_chars
            + self.agent_thought_chars
            + self.tool_call_payload_chars
            + self.tool_call_update_payload_chars
    }

    fn render(&self) -> String {
        let current_rss = self
            .current_rss_bytes
            .map(format_bytes)
            .unwrap_or_else(|| "unavailable".to_string());
        let client_info = self.client_info.as_deref().unwrap_or("unknown");
        let lines = [
            format!(
                "ACP client: {client_info} | visibility={} | uptime={}s | active_tasks={}",
                self.ui_visibility_mode, self.uptime_secs, self.active_tasks
            ),
            format!(
                "Process RSS: {current_rss} | notifications_sent={} | notification_errors={}",
                self.notifications_sent, self.notification_errors
            ),
            format!(
                "User chunks: {} ({}), agent text chunks: {} ({})",
                self.user_message_chunks,
                format_bytes(self.user_message_chars),
                self.agent_message_chunks,
                format_bytes(self.agent_message_chars)
            ),
            format!(
                "Thought chunks: {} ({}), tool calls: {} ({}), tool updates: {} ({})",
                self.agent_thought_chunks,
                format_bytes(self.agent_thought_chars),
                self.tool_calls,
                format_bytes(self.tool_call_payload_chars),
                self.tool_call_updates,
                format_bytes(self.tool_call_update_payload_chars)
            ),
            format!(
                "Plan updates: {} | permission requests: {} | total tracked payload={}",
                self.plan_updates,
                self.permission_requests,
                format_bytes(self.total_payload_chars())
            ),
        ];
        lines.join("\n")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MonitorMode {
    Standard,
    Detail,
    Retrospective,
}

impl MonitorMode {
    fn from_rest(rest: &str) -> Self {
        let rest_lower = rest.to_lowercase();
        if rest_lower.contains("retro") || rest_lower.contains("retrospective") {
            Self::Retrospective
        } else if rest_lower.contains("detail") || rest_lower.contains("details") {
            Self::Detail
        } else {
            Self::Standard
        }
    }

    fn is_detail(&self) -> bool {
        matches!(self, Self::Detail)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConfigOptionsDensity {
    Compact,
    Full,
}

impl ConfigOptionsDensity {
    fn from_env() -> Self {
        match std::env::var(CONFIG_OPTIONS_DENSITY_ENV)
            .ok()
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("full") => Self::Full,
            _ => Self::Compact,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AdvancedOptionsPanel {
    Context,
    Tasks,
    Beta,
}

impl AdvancedOptionsPanel {
    fn from_config_value(raw: &str) -> Result<Self, Error> {
        match raw {
            "context" => Ok(Self::Context),
            "tasks" => Ok(Self::Tasks),
            "beta" => Ok(Self::Beta),
            _ => Err(Error::invalid_params()
                .data("Advanced Panel values must be one of: context, tasks, beta")),
        }
    }

    fn as_config_value(self) -> &'static str {
        match self {
            Self::Context => "context",
            Self::Tasks => "tasks",
            Self::Beta => "beta",
        }
    }
}

fn parse_on_off_toggle(raw: &str, option_name: &str) -> Result<bool, Error> {
    match raw {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => {
            Err(Error::invalid_params()
                .data(format!("{option_name} values must be one of: on, off")))
        }
    }
}

fn parse_on_off_env(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "1" => Some(true),
        "off" | "false" | "0" => Some(false),
        _ => None,
    }
}

fn parse_bounded_usize_env(var_name: &str, default: usize, min: usize, max: usize) -> usize {
    std::env::var(var_name)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .unwrap_or(default)
        .clamp(min, max)
}

fn parse_bounded_u64_env(var_name: &str, default: u64, min: u64, max: u64) -> u64 {
    std::env::var(var_name)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(default)
        .clamp(min, max)
}

fn diagnostics_auto_log_enabled_from_env() -> bool {
    std::env::var(DIAGNOSTICS_AUTO_LOG_ENV)
        .ok()
        .as_deref()
        .and_then(parse_on_off_env)
        .unwrap_or(false)
}

fn diagnostics_log_every_from_env() -> u64 {
    parse_bounded_u64_env(
        DIAGNOSTICS_LOG_EVERY_ENV,
        DIAGNOSTICS_LOG_EVERY_DEFAULT,
        DIAGNOSTICS_LOG_EVERY_MIN,
        DIAGNOSTICS_LOG_EVERY_MAX,
    )
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;

    let bytes_f64 = bytes as f64;
    if bytes_f64 >= GIB {
        format!("{:.2} GiB", bytes_f64 / GIB)
    } else if bytes_f64 >= MIB {
        format!("{:.2} MiB", bytes_f64 / MIB)
    } else if bytes_f64 >= KIB {
        format!("{:.2} KiB", bytes_f64 / KIB)
    } else {
        format!("{bytes} B")
    }
}

fn current_process_rss_bytes() -> Option<u64> {
    #[cfg(unix)]
    {
        let pid = std::process::id().to_string();
        let output = Command::new("ps")
            .args(["-o", "rss=", "-p", &pid])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let stdout = String::from_utf8(output.stdout).ok()?;
        let rss_kib = stdout.trim().parse::<u64>().ok()?;
        Some(rss_kib.saturating_mul(1024))
    }

    #[cfg(not(unix))]
    {
        None
    }
}

fn json_payload_chars(value: &serde_json::Value) -> u64 {
    serde_json::to_string(value)
        .map(|s| s.chars().count() as u64)
        .unwrap_or_default()
}

fn ui_text_chunk_max_chars_from_env() -> usize {
    parse_bounded_usize_env(
        UI_TEXT_CHUNK_MAX_CHARS_ENV,
        UI_TEXT_CHUNK_MAX_CHARS_DEFAULT,
        UI_TEXT_CHUNK_MAX_CHARS_MIN,
        UI_TEXT_CHUNK_MAX_CHARS_MAX,
    )
}

fn truncate_to_char_limit(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    text.chars().take(max_chars).collect::<String>()
}

fn truncate_to_byte_limit(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

fn cap_json_payload_for_ui(payload: serde_json::Value, payload_name: &str) -> serde_json::Value {
    let max_chars = parse_bounded_usize_env(
        TOOL_RAW_OUTPUT_MAX_CHARS_ENV,
        TOOL_RAW_OUTPUT_MAX_CHARS_DEFAULT,
        TOOL_RAW_OUTPUT_MAX_CHARS_MIN,
        TOOL_RAW_OUTPUT_MAX_CHARS_MAX,
    );

    let rendered = serde_json::to_string(&payload).unwrap_or_else(|_| "<unserializable>".into());
    if rendered.chars().count() <= max_chars {
        return payload;
    }

    json!({
        "truncated": true,
        "payload": payload_name,
        "original_chars": rendered.chars().count(),
        "limit_chars": max_chars,
        "preview": truncate_to_char_limit(&rendered, max_chars),
        "hint": format!("Set {TOOL_RAW_OUTPUT_MAX_CHARS_ENV} to raise/lower this limit"),
    })
}

fn cap_tool_call_payloads_for_ui(mut tool_call: ToolCall) -> ToolCall {
    if let Some(raw_input) = tool_call.raw_input.take() {
        tool_call.raw_input = Some(cap_json_payload_for_ui(raw_input, "tool_call.raw_input"));
    }
    if let Some(raw_output) = tool_call.raw_output.take() {
        tool_call.raw_output = Some(cap_json_payload_for_ui(raw_output, "tool_call.raw_output"));
    }
    tool_call
}

fn cap_tool_call_update_payloads_for_ui(mut update: ToolCallUpdate) -> ToolCallUpdate {
    if let Some(raw_input) = update.fields.raw_input.take() {
        update.fields.raw_input = Some(cap_json_payload_for_ui(
            raw_input,
            "tool_call_update.raw_input",
        ));
    }
    if let Some(raw_output) = update.fields.raw_output.take() {
        update.fields.raw_output = Some(cap_json_payload_for_ui(
            raw_output,
            "tool_call_update.raw_output",
        ));
    }
    update
}

fn split_text_for_ui_chunks(text: &str, max_chars: usize) -> Vec<String> {
    let max_chars = max_chars.max(1);
    if text.is_empty() {
        return vec![String::new()];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_chars = 0usize;
    for ch in text.chars() {
        current.push(ch);
        current_chars += 1;
        if current_chars >= max_chars {
            chunks.push(std::mem::take(&mut current));
            current_chars = 0;
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn decode_utf8_streaming(pending_bytes: &mut Vec<u8>, chunk: &[u8], flush: bool) -> String {
    pending_bytes.extend_from_slice(chunk);
    if pending_bytes.is_empty() {
        return String::new();
    }

    let mut output = String::new();
    let mut cursor = 0usize;
    while cursor < pending_bytes.len() {
        match std::str::from_utf8(&pending_bytes[cursor..]) {
            Ok(valid) => {
                output.push_str(valid);
                cursor = pending_bytes.len();
                break;
            }
            Err(err) => {
                let valid_up_to = err.valid_up_to();
                if valid_up_to > 0 {
                    let end = cursor + valid_up_to;
                    if let Ok(valid_prefix) = std::str::from_utf8(&pending_bytes[cursor..end]) {
                        output.push_str(valid_prefix);
                    }
                    cursor = end;
                }

                match err.error_len() {
                    Some(error_len) => {
                        output.push('\u{FFFD}');
                        cursor = cursor.saturating_add(error_len);
                    }
                    None => break,
                }
            }
        }
    }

    if flush {
        if cursor < pending_bytes.len() {
            output.push_str(&String::from_utf8_lossy(&pending_bytes[cursor..]));
        }
        pending_bytes.clear();
    } else if cursor == pending_bytes.len() {
        pending_bytes.clear();
    } else {
        let trailing = pending_bytes[cursor..].to_vec();
        *pending_bytes = trailing;
    }

    output
}

fn sanitize_internal_frame_text(text: &str) -> String {
    text.lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !(trimmed.starts_with("assistant to=")
                || trimmed.starts_with("assistant  to=")
                || trimmed.starts_with("assistant to =")
                || trimmed.starts_with("tool_call("))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Clone, Debug, Default)]
struct PromptTokenEstimate {
    text_tokens: i64,
    embedded_context_tokens: i64,
    resource_link_tokens: i64,
    image_tokens: i64,
    audio_tokens: i64,
    total_tokens: i64,
}

impl PromptTokenEstimate {
    fn to_json(&self) -> serde_json::Value {
        json!({
            "text_tokens": self.text_tokens,
            "embedded_context_tokens": self.embedded_context_tokens,
            "resource_link_tokens": self.resource_link_tokens,
            "image_tokens_assumed": self.image_tokens,
            "audio_tokens_assumed": self.audio_tokens,
            "total_tokens": self.total_tokens,
            "notes": [
                "This is a rough estimate to monitor context pressure",
                "Image/audio costs are assumption-based and may differ per model",
            ],
        })
    }
}

#[derive(Clone, Debug)]
struct PendingAutoCompact {
    submission_id: String,
    total_tokens: i64,
    context_window: Option<i64>,
    used_percent: Option<i64>,
}

#[derive(Clone, Debug)]
struct PlanSnapshotEntry {
    step: String,
    status: StepStatus,
}

#[derive(Clone, Debug, Default)]
struct PlanSnapshot {
    explanation: Option<String>,
    items: Vec<PlanSnapshotEntry>,
}

#[derive(Clone, Debug, Default)]
struct FlowVectorState {
    analysis: i64,
    execution: i64,
    validation: i64,
    coordination: i64,
    path: Vec<char>,
    recent_actions: VecDeque<String>,
    last_plan: PlanSnapshot,
    last_plan_update_at: Option<Instant>,
    last_progress_at: Option<Instant>,
    stalled_plan_updates: usize,
    last_completed_steps: usize,
    last_plan_total_steps: usize,
}

impl FlowVectorState {
    fn plan_status_counts(&self) -> (usize, usize, usize, usize) {
        if self.last_plan.items.is_empty() {
            return (0, 0, 0, 0);
        }

        let pending = self
            .last_plan
            .items
            .iter()
            .filter(|item| matches!(item.status, StepStatus::Pending))
            .count();
        let in_progress = self
            .last_plan
            .items
            .iter()
            .filter(|item| matches!(item.status, StepStatus::InProgress))
            .count();
        let completed = self
            .last_plan
            .items
            .iter()
            .filter(|item| matches!(item.status, StepStatus::Completed))
            .count();
        (completed, in_progress, pending, self.last_plan.items.len())
    }

    fn render_progress_bar(completed: usize, total: usize, width: usize) -> String {
        let safe_width = width.max(8);
        let filled = if total == 0 {
            0
        } else {
            completed.saturating_mul(safe_width) / total
        }
        .min(safe_width);
        let empty = safe_width.saturating_sub(filled);
        format!("[{}{}]", "#".repeat(filled), ".".repeat(empty))
    }

    fn record_phase(&mut self, phase: char, detail: impl Into<String>) {
        match phase {
            'A' => self.analysis += 1,
            'E' => self.execution += 1,
            'V' => self.validation += 1,
            'C' => self.coordination += 1,
            _ => {}
        }

        self.path.push(phase);
        if self.path.len() > 48 {
            let drop_count = self.path.len().saturating_sub(48);
            self.path.drain(..drop_count);
        }

        self.recent_actions.push_back(detail.into());
        if self.recent_actions.len() > 24 {
            self.recent_actions.pop_front();
        }
    }

    fn record_plan_update(&mut self, explanation: Option<String>, plan: &[PlanItemArg]) {
        let now = Instant::now();
        let completed = plan
            .iter()
            .filter(|item| matches!(item.status, StepStatus::Completed))
            .count();
        let total = plan.len();

        if self.last_plan_update_at.is_none()
            || completed != self.last_completed_steps
            || total != self.last_plan_total_steps
        {
            self.stalled_plan_updates = 0;
            self.last_progress_at = Some(now);
        } else {
            self.stalled_plan_updates = self.stalled_plan_updates.saturating_add(1);
        }
        self.last_completed_steps = completed;
        self.last_plan_total_steps = total;
        self.last_plan_update_at = Some(now);

        self.last_plan = PlanSnapshot {
            explanation,
            items: plan
                .iter()
                .map(|item| PlanSnapshotEntry {
                    step: item.step.clone(),
                    status: item.status.clone(),
                })
                .collect(),
        };
        self.record_phase('C', "plan updated");
    }

    fn trailing_action_streak(&self) -> Option<(String, usize)> {
        let mut actions = self.recent_actions.iter().rev();
        let latest = actions.next()?.trim().to_string();
        if latest.is_empty() {
            return None;
        }

        let mut streak = 1;
        for action in actions {
            if action.trim() == latest {
                streak += 1;
            } else {
                break;
            }
        }
        Some((latest, streak))
    }

    fn dominant_recent_phase(&self, max_events: usize) -> Option<(char, usize, usize)> {
        let mut analysis = 0;
        let mut execution = 0;
        let mut validation = 0;
        let mut coordination = 0;
        let recent = self
            .path
            .iter()
            .rev()
            .take(max_events)
            .copied()
            .collect::<Vec<_>>();
        if recent.is_empty() {
            return None;
        }

        for phase in &recent {
            match phase {
                'A' => analysis += 1,
                'E' => execution += 1,
                'V' => validation += 1,
                'C' => coordination += 1,
                _ => {}
            }
        }

        let mut dominant = ('A', analysis);
        for (phase, count) in [('E', execution), ('V', validation), ('C', coordination)] {
            if count > dominant.1 {
                dominant = (phase, count);
            }
        }
        Some((dominant.0, dominant.1, recent.len()))
    }

    fn render_repeat_signal(&self) -> String {
        if let Some((latest_action, streak)) = self.trailing_action_streak()
            && streak >= MONITOR_REPEAT_STREAK_WARN
        {
            return format!("Repeat loop: `{latest_action}` repeated {streak}x in a row");
        }

        if let Some((phase, hits, total)) = self.dominant_recent_phase(12)
            && total >= MONITOR_PHASE_DOMINANCE_MIN_EVENTS
        {
            let dominant_percent = hits.saturating_mul(100) / total;
            if dominant_percent >= MONITOR_PHASE_DOMINANCE_WARN_PERCENT {
                return format!(
                    "Repeat loop: recent flow is {}-heavy ({hits}/{total}, {dominant_percent}%)",
                    flow_phase_name(phase)
                );
            }
        }

        "Repeat loop: no strong repetition signal".to_string()
    }

    fn render_stall_signal(&self, active_task_count: usize) -> String {
        let Some(last_plan_update_at) = self.last_plan_update_at else {
            return "Progress stall: no plan updates yet".to_string();
        };

        let no_progress_age = self
            .last_progress_at
            .map(|moment| moment.elapsed())
            .unwrap_or_default();
        let plan_update_age = last_plan_update_at.elapsed();

        if active_task_count > 0 && self.stalled_plan_updates >= MONITOR_STALL_PLAN_UPDATE_WARN {
            return format!(
                "Progress stall: {} consecutive plan updates without completed-step gain (last completion {} ago)",
                self.stalled_plan_updates,
                format_monitor_duration(no_progress_age)
            );
        }

        if active_task_count > 0 && no_progress_age.as_secs() >= MONITOR_STALL_NO_PROGRESS_WARN_SECS
        {
            return format!(
                "Progress stall: no completed step for {} while {} task(s) are active",
                format_monitor_duration(no_progress_age),
                active_task_count
            );
        }

        if active_task_count == 0 {
            return format!(
                "Progress stall: idle (last plan update {} ago)",
                format_monitor_duration(plan_update_age)
            );
        }

        format!(
            "Progress stall: healthy (last completed-step change {} ago)",
            format_monitor_duration(no_progress_age)
        )
    }

    fn path_string(&self) -> String {
        if self.path.is_empty() {
            return "(no flow data yet)".to_string();
        }
        self.path
            .iter()
            .map(char::to_string)
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn resultant_vector(&self) -> (i64, i64, f64, &'static str, &'static str) {
        // x-axis: execution (+) <-> coordination (-)
        // y-axis: analysis (+) <-> validation (-)
        let x = self.execution - self.coordination;
        let y = self.analysis - self.validation;

        let magnitude = ((x.pow(2) + y.pow(2)) as f64).sqrt();
        let direction = flow_direction_from_xy(x, y);
        let semantic = flow_semantic_from_direction(direction);
        (x, y, magnitude, direction, semantic)
    }

    fn render_compass(&self) -> String {
        let (x, y, magnitude, direction, semantic) = self.resultant_vector();
        let mut lines = Vec::new();
        lines.push("Flow compass".to_string());
        lines.push("  N: Analysis".to_string());
        lines.push("W: Coordination   +   E: Execution".to_string());
        lines.push("  S: Validation".to_string());
        lines.push(format!(
            "Vector = ({x}, {y}), |v|={magnitude:.2}, heading={direction}"
        ));
        lines.push(format!("Semantic heading = {semantic}"));
        lines.join("\n")
    }

    fn render_plan_progress(&self) -> String {
        let (completed, in_progress, pending, total) = self.plan_status_counts();
        let percent = if total == 0 {
            0
        } else {
            completed.saturating_mul(100) / total
        };
        let bar_width = monitor_progress_bar_width(monitor_panel_width());
        let progress_bar = Self::render_progress_bar(completed, total, bar_width);

        let mut lines = Vec::new();
        lines.push("Plan progress".to_string());
        lines.push(format!(
            "Progress: {progress_bar} {percent}% ({completed}/{total} completed, {in_progress} in_progress, {pending} pending)"
        ));

        if self.last_plan.items.is_empty() {
            lines.push("Plan: no plan updates received yet.".to_string());
            return lines.join("\n");
        }

        if let Some(explanation) = &self.last_plan.explanation
            && !explanation.trim().is_empty()
        {
            lines.push(format!("Plan note: {}", explanation.trim()));
        }
        for item in &self.last_plan.items {
            let status = match item.status {
                StepStatus::Pending => "pending",
                StepStatus::InProgress => "in_progress",
                StepStatus::Completed => "completed",
            };
            lines.push(format!("- [{status}] {}", item.step));
        }
        lines.join("\n")
    }

    fn render_recent_actions(&self, detail: bool) -> String {
        if self.recent_actions.is_empty() {
            return "Recent actions: (none yet)".to_string();
        }
        let actions = if detail {
            self.recent_actions.iter().cloned().collect::<Vec<_>>()
        } else {
            self.recent_actions
                .iter()
                .rev()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
        };
        let mut lines = vec!["Recent actions:".to_string()];
        lines.extend(actions.into_iter().map(|line| format!("- {line}")));
        lines.join("\n")
    }
}

fn flow_direction_from_xy(x: i64, y: i64) -> &'static str {
    if x == 0 && y == 0 {
        return "CENTER";
    }

    let angle = (y as f64).atan2(x as f64).to_degrees();
    if (-22.5..22.5).contains(&angle) {
        "E"
    } else if (22.5..67.5).contains(&angle) {
        "NE"
    } else if (67.5..112.5).contains(&angle) {
        "N"
    } else if (112.5..157.5).contains(&angle) {
        "NW"
    } else if !(-157.5..157.5).contains(&angle) {
        "W"
    } else if (-157.5..-112.5).contains(&angle) {
        "SW"
    } else if (-112.5..-67.5).contains(&angle) {
        "S"
    } else {
        "SE"
    }
}

fn flow_semantic_from_direction(direction: &str) -> &'static str {
    match direction {
        "N" => "analysis-heavy: deep reasoning / problem framing",
        "NE" => "analysis -> execution: validated implementation momentum",
        "E" => "execution-heavy: delivery and tool throughput",
        "SE" => "execution + validation: stabilization and hardening",
        "S" => "validation-heavy: review, checks, and correctness focus",
        "SW" => "validation -> coordination: rollback/replan posture",
        "W" => "coordination-heavy: planning, scoping, and alignment",
        "NW" => "coordination + analysis: strategy and architecture shaping",
        _ => "neutral: balanced or insufficient signal",
    }
}

fn flow_phase_name(phase: char) -> &'static str {
    match phase {
        'A' => "analysis",
        'E' => "execution",
        'V' => "validation",
        'C' => "coordination",
        _ => "unknown",
    }
}

fn format_monitor_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs < 1 {
        return "<1s".to_string();
    }

    if secs < 60 {
        return format!("{secs}s");
    }

    let mins = secs / 60;
    let rem_secs = secs % 60;
    if mins < 60 {
        if rem_secs == 0 {
            return format!("{mins}m");
        }
        return format!("{mins}m {rem_secs}s");
    }

    let hours = mins / 60;
    let rem_mins = mins % 60;
    if rem_mins == 0 {
        format!("{hours}h")
    } else {
        format!("{hours}h {rem_mins}m")
    }
}

#[derive(Clone, Debug)]
struct ContextOptimizationState {
    mode: ContextOptimizationMode,
    trigger_percent: i64,
    last_prompt_estimate: Option<PromptTokenEstimate>,
    last_token_info: Option<codex_core::protocol::TokenUsageInfo>,
    pending_auto_compact: Option<PendingAutoCompact>,
    auto_compact_submission_id: Option<String>,
    auto_compact_count: usize,
}

impl Default for ContextOptimizationState {
    fn default() -> Self {
        let mode = std::env::var("ACP_CONTEXT_OPT_MODE")
            .ok()
            .and_then(|raw| ContextOptimizationMode::from_config_value(raw.trim()).ok())
            .unwrap_or(ContextOptimizationMode::Monitor);

        let trigger_percent = std::env::var("ACP_CONTEXT_OPT_TRIGGER_PERCENT")
            .ok()
            .and_then(|raw| raw.parse::<i64>().ok())
            .unwrap_or(CONTEXT_OPT_TRIGGER_PERCENT_DEFAULT)
            .clamp(50, 99);

        Self {
            mode,
            trigger_percent,
            last_prompt_estimate: None,
            last_token_info: None,
            pending_auto_compact: None,
            auto_compact_submission_id: None,
            auto_compact_count: 0,
        }
    }
}

#[derive(Clone, Debug)]
struct TaskMonitoringState {
    orchestration_mode: TaskOrchestrationMode,
    monitor_mode: TaskMonitoringMode,
    vector_check_enabled: bool,
    preempt_on_new_prompt: bool,
}

impl Default for TaskMonitoringState {
    fn default() -> Self {
        let profile = BackendKind::Codex.work_orchestration_profile();
        let preempt_on_new_prompt = std::env::var("ACP_PREEMPT_ON_NEW_PROMPT")
            .ok()
            .as_deref()
            .and_then(parse_on_off_env)
            .unwrap_or(profile.preempt_on_new_prompt);
        Self {
            orchestration_mode: TaskOrchestrationMode::from_config_value(
                profile.task_orchestration,
            )
            .expect("codex orchestration profile must stay ACP-compatible"),
            monitor_mode: TaskMonitoringMode::from_config_value(profile.task_monitoring)
                .expect("codex monitoring profile must stay ACP-compatible"),
            vector_check_enabled: profile.vector_checks,
            preempt_on_new_prompt,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct SetupWizardProgressState {
    status_checked: bool,
    monitor_checked: bool,
    vector_checked: bool,
}

impl SetupWizardProgressState {
    const TOTAL_VERIFICATION_STEPS: usize = 3;

    fn completed_count(self) -> usize {
        usize::from(self.status_checked)
            + usize::from(self.monitor_checked)
            + usize::from(self.vector_checked)
    }

    fn progress_percent(self) -> usize {
        (self.completed_count() * 100) / Self::TOTAL_VERIFICATION_STEPS
    }

    fn verification_status(self) -> StepStatus {
        match self.completed_count() {
            0 => StepStatus::Pending,
            Self::TOTAL_VERIFICATION_STEPS => StepStatus::Completed,
            _ => StepStatus::InProgress,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ExperimentalFeatureSpec {
    id: Feature,
    key: &'static str,
    name: &'static str,
    description: &'static str,
    default_enabled: bool,
}

fn experimental_feature_specs() -> Vec<ExperimentalFeatureSpec> {
    FEATURES
        .iter()
        .filter_map(|spec| {
            let name = spec.stage.beta_menu_name()?;
            let description = spec.stage.beta_menu_description()?;
            Some(ExperimentalFeatureSpec {
                id: spec.id,
                key: spec.key,
                name,
                description,
                default_enabled: spec.default_enabled,
            })
        })
        .collect()
}

fn experimental_feature_config_id(key: &str) -> String {
    format!("{FEATURE_CONFIG_ID_PREFIX}{key}")
}

fn parse_experimental_feature_config_id(config_id: &str) -> Option<ExperimentalFeatureSpec> {
    let key = config_id.strip_prefix(FEATURE_CONFIG_ID_PREFIX)?;
    experimental_feature_specs()
        .into_iter()
        .find(|spec| spec.key == key)
}

/// Trait for abstracting over the `CodexThread` to make testing easier.
#[async_trait::async_trait]
pub trait CodexThreadImpl {
    async fn submit(&self, op: Op) -> Result<String, CodexErr>;
    async fn next_event(&self) -> Result<Event, CodexErr>;
}

#[async_trait::async_trait]
impl CodexThreadImpl for CodexThread {
    async fn submit(&self, op: Op) -> Result<String, CodexErr> {
        self.submit(op).await
    }

    async fn next_event(&self) -> Result<Event, CodexErr> {
        self.next_event().await
    }
}

#[async_trait::async_trait]
pub trait ModelsManagerImpl {
    async fn get_model(&self, model_id: &Option<String>, config: &Config) -> String;
    async fn list_models(&self, config: &Config) -> Vec<ModelPreset>;
}

#[async_trait::async_trait]
impl ModelsManagerImpl for ModelsManager {
    async fn get_model(&self, model_id: &Option<String>, config: &Config) -> String {
        self.get_default_model(model_id, config, RefreshStrategy::OnlineIfUncached)
            .await
    }

    async fn list_models(&self, config: &Config) -> Vec<ModelPreset> {
        self.list_models(config, RefreshStrategy::OnlineIfUncached)
            .await
    }
}

pub trait Auth {
    fn logout(&self) -> Result<bool, Error>;
}

impl Auth for Arc<AuthManager> {
    fn logout(&self) -> Result<bool, Error> {
        self.as_ref()
            .logout()
            .map_err(|e| Error::internal_error().data(e.to_string()))
    }
}

enum ThreadMessage {
    Load {
        response_tx: oneshot::Sender<Result<LoadSessionResponse, Error>>,
    },
    GetConfigOptions {
        response_tx: oneshot::Sender<Result<Vec<SessionConfigOption>, Error>>,
    },
    Prompt {
        request: PromptRequest,
        response_tx: oneshot::Sender<Result<oneshot::Receiver<Result<StopReason, Error>>, Error>>,
    },
    SetMode {
        mode: SessionModeId,
        response_tx: oneshot::Sender<Result<(), Error>>,
    },
    SetModel {
        model: ModelId,
        response_tx: oneshot::Sender<Result<(), Error>>,
    },
    SetConfigOption {
        config_id: SessionConfigId,
        value: SessionConfigValueId,
        response_tx: oneshot::Sender<Result<(), Error>>,
    },
    Cancel {
        response_tx: oneshot::Sender<Result<(), Error>>,
    },
    ReplayHistory {
        history: Vec<RolloutItem>,
        response_tx: oneshot::Sender<Result<(), Error>>,
    },
}

pub struct Thread {
    /// A sender for interacting with the thread.
    message_tx: mpsc::UnboundedSender<ThreadMessage>,
    /// A handle to the spawned task.
    _handle: tokio::task::JoinHandle<()>,
}

impl Thread {
    pub fn new(
        session_id: SessionId,
        thread: Arc<dyn CodexThreadImpl>,
        auth: Arc<AuthManager>,
        models_manager: Arc<dyn ModelsManagerImpl>,
        client_capabilities: Arc<Mutex<ClientCapabilities>>,
        config: Config,
        session_store: Option<SessionStore>,
    ) -> Self {
        let (message_tx, message_rx) = mpsc::unbounded_channel();

        let actor = ThreadActor::new(
            auth,
            SessionClient::new(session_id, client_capabilities, session_store),
            thread,
            models_manager,
            config,
            message_rx,
        );
        let handle = tokio::task::spawn_local(actor.spawn());

        Self {
            message_tx,
            _handle: handle,
        }
    }

    pub async fn load(&self) -> Result<LoadSessionResponse, Error> {
        let (response_tx, response_rx) = oneshot::channel();

        let message = ThreadMessage::Load { response_tx };
        drop(self.message_tx.send(message));

        response_rx
            .await
            .map_err(|e| Error::internal_error().data(e.to_string()))?
    }

    pub async fn config_options(&self) -> Result<Vec<SessionConfigOption>, Error> {
        let (response_tx, response_rx) = oneshot::channel();

        let message = ThreadMessage::GetConfigOptions { response_tx };
        drop(self.message_tx.send(message));

        response_rx
            .await
            .map_err(|e| Error::internal_error().data(e.to_string()))?
    }

    pub async fn prompt(&self, request: PromptRequest) -> Result<StopReason, Error> {
        let (response_tx, response_rx) = oneshot::channel();

        let message = ThreadMessage::Prompt {
            request,
            response_tx,
        };
        drop(self.message_tx.send(message));

        response_rx
            .await
            .map_err(|e| Error::internal_error().data(e.to_string()))??
            .await
            .map_err(|e| Error::internal_error().data(e.to_string()))?
    }

    pub async fn set_mode(&self, mode: SessionModeId) -> Result<(), Error> {
        let (response_tx, response_rx) = oneshot::channel();

        let message = ThreadMessage::SetMode { mode, response_tx };
        drop(self.message_tx.send(message));

        response_rx
            .await
            .map_err(|e| Error::internal_error().data(e.to_string()))?
    }

    pub async fn set_model(&self, model: ModelId) -> Result<(), Error> {
        let (response_tx, response_rx) = oneshot::channel();

        let message = ThreadMessage::SetModel { model, response_tx };
        drop(self.message_tx.send(message));

        response_rx
            .await
            .map_err(|e| Error::internal_error().data(e.to_string()))?
    }

    pub async fn set_config_option(
        &self,
        config_id: SessionConfigId,
        value: SessionConfigValueId,
    ) -> Result<(), Error> {
        let (response_tx, response_rx) = oneshot::channel();

        let message = ThreadMessage::SetConfigOption {
            config_id,
            value,
            response_tx,
        };
        drop(self.message_tx.send(message));

        response_rx
            .await
            .map_err(|e| Error::internal_error().data(e.to_string()))?
    }

    pub async fn cancel(&self) -> Result<(), Error> {
        let (response_tx, response_rx) = oneshot::channel();

        let message = ThreadMessage::Cancel { response_tx };
        drop(self.message_tx.send(message));

        response_rx
            .await
            .map_err(|e| Error::internal_error().data(e.to_string()))?
    }

    pub async fn replay_history(&self, history: Vec<RolloutItem>) -> Result<(), Error> {
        let (response_tx, response_rx) = oneshot::channel();

        let message = ThreadMessage::ReplayHistory {
            history,
            response_tx,
        };
        drop(self.message_tx.send(message));

        response_rx
            .await
            .map_err(|e| Error::internal_error().data(e.to_string()))?
    }
}

enum SubmissionState {
    /// Loading custom prompts from the project
    CustomPrompts(CustomPromptsState),
    /// User prompts + some slash commands like /init or /review
    Prompt(PromptState),
    /// Subtask, like /compact
    Task(TaskState),
    /// One-shot slash commands that return a single response event.
    OneShot(OneShotCommandState),
}

impl SubmissionState {
    fn is_active(&self) -> bool {
        match self {
            Self::CustomPrompts(state) => state.is_active(),
            Self::Prompt(state) => state.is_active(),
            Self::Task(state) => state.is_active(),
            Self::OneShot(state) => state.is_active(),
        }
    }

    async fn handle_event(&mut self, client: &SessionClient, event: EventMsg) {
        match self {
            Self::CustomPrompts(state) => state.handle_event(event),
            Self::Prompt(state) => state.handle_event(client, event).await,
            Self::Task(state) => state.handle_event(client, event).await,
            Self::OneShot(state) => state.handle_event(client, event).await,
        }
    }

    async fn handle_watchdog_tick(&mut self, client: &SessionClient) {
        match self {
            Self::Prompt(state) => state.handle_watchdog_tick(client).await,
            Self::Task(state) => state.handle_watchdog_tick(client).await,
            Self::CustomPrompts(_) | Self::OneShot(_) => {}
        }
    }

    async fn fail_open_tool_calls(&mut self, client: &SessionClient, reason: &str) {
        match self {
            Self::Prompt(state) => state.fail_open_tool_calls(client, reason).await,
            Self::Task(state) => state.fail_open_tool_calls(client, reason).await,
            Self::CustomPrompts(_) | Self::OneShot(_) => {}
        }
    }

    async fn force_cancelled(&mut self, client: &SessionClient, reason: &str) {
        match self {
            Self::Prompt(state) => state.force_cancelled(client, reason).await,
            Self::Task(state) => state.force_cancelled(client, reason).await,
            Self::OneShot(state) => {
                if let Some(tx) = state.response_tx.take() {
                    drop(tx.send(Ok(StopReason::Cancelled)));
                }
            }
            Self::CustomPrompts(state) => {
                if let Some(tx) = state.response_tx.take() {
                    drop(tx.send(Err(
                        Error::internal_error().data("custom prompt listing cancelled"),
                    )));
                }
            }
        }
    }

    fn monitor_label(&self) -> Option<&'static str> {
        match self {
            Self::CustomPrompts(_) => None,
            Self::Prompt(_) => Some("prompt"),
            Self::Task(_) => Some("task"),
            Self::OneShot(state) => Some(match state.kind {
                OneShotKind::McpTools => "oneshot:mcp",
                OneShotKind::Skills => "oneshot:skills",
            }),
        }
    }

    fn longest_open_tool_call_runtime(&self) -> Option<OpenToolCallRuntime> {
        match self {
            Self::Prompt(state) => state.longest_open_tool_call_runtime(),
            Self::Task(state) => state.longest_open_tool_call_runtime(),
            Self::CustomPrompts(_) | Self::OneShot(_) => None,
        }
    }
}

struct CustomPromptsState {
    response_tx: Option<oneshot::Sender<Result<Vec<CustomPrompt>, Error>>>,
}

impl CustomPromptsState {
    fn new(response_tx: oneshot::Sender<Result<Vec<CustomPrompt>, Error>>) -> Self {
        Self {
            response_tx: Some(response_tx),
        }
    }

    fn is_active(&self) -> bool {
        let Some(response_tx) = &self.response_tx else {
            return false;
        };
        !response_tx.is_closed()
    }

    fn handle_event(&mut self, event: EventMsg) {
        match event {
            EventMsg::ListCustomPromptsResponse(ListCustomPromptsResponseEvent {
                custom_prompts,
            }) => {
                if let Some(tx) = self.response_tx.take() {
                    drop(tx.send(Ok(custom_prompts)));
                }
            }
            e => {
                warn!("Unexpected event: {e:?}");
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OneShotKind {
    McpTools,
    Skills,
}

#[derive(Clone, Debug, Default)]
struct SkillsCommandOptions {
    force_reload: bool,
    enabled: Option<bool>,
    scope: Option<String>,
    query: Option<String>,
}

impl SkillsCommandOptions {
    fn has_filters(&self) -> bool {
        self.enabled.is_some() || self.scope.is_some() || self.query.is_some()
    }
}

struct OneShotCommandState {
    kind: OneShotKind,
    response_tx: Option<oneshot::Sender<Result<StopReason, Error>>>,
    skills_options: SkillsCommandOptions,
}

impl OneShotCommandState {
    fn new(kind: OneShotKind, response_tx: oneshot::Sender<Result<StopReason, Error>>) -> Self {
        Self {
            kind,
            response_tx: Some(response_tx),
            skills_options: SkillsCommandOptions::default(),
        }
    }

    fn new_skills(
        response_tx: oneshot::Sender<Result<StopReason, Error>>,
        skills_options: SkillsCommandOptions,
    ) -> Self {
        Self {
            kind: OneShotKind::Skills,
            response_tx: Some(response_tx),
            skills_options,
        }
    }

    fn is_active(&self) -> bool {
        let Some(response_tx) = &self.response_tx else {
            return false;
        };
        !response_tx.is_closed()
    }

    async fn handle_event(&mut self, client: &SessionClient, event: EventMsg) {
        match (self.kind, event) {
            (OneShotKind::McpTools, EventMsg::McpListToolsResponse(event)) => {
                client
                    .send_agent_text(format_mcp_tools_message(&event))
                    .await;
                if let Some(tx) = self.response_tx.take() {
                    drop(tx.send(Ok(StopReason::EndTurn)));
                }
            }
            (OneShotKind::Skills, EventMsg::ListSkillsResponse(event)) => {
                client
                    .send_agent_text(format_skills_message(&event, &self.skills_options))
                    .await;
                if let Some(tx) = self.response_tx.take() {
                    drop(tx.send(Ok(StopReason::EndTurn)));
                }
            }
            (_, EventMsg::Error(err)) => {
                if let Some(tx) = self.response_tx.take() {
                    drop(tx.send(Err(Error::internal_error().data(err.message))));
                }
            }
            _ => {}
        }
    }
}

struct ActiveCommand {
    call_id: String,
    tool_call_id: ToolCallId,
    terminal_id: Option<String>,
    terminal_output: bool,
    output: String,
    visible_output_bytes: usize,
    output_limit_bytes: usize,
    output_truncated: bool,
    truncation_notice_sent: bool,
    pending_utf8_bytes: Vec<u8>,
    file_extension: Option<String>,
}

impl ActiveCommand {
    fn display_terminal_id(&self) -> &str {
        self.terminal_id.as_deref().unwrap_or(&self.call_id)
    }
}

#[derive(Clone, Debug)]
struct OpenToolCall {
    kind: &'static str,
    started_at: Instant,
}

#[derive(Clone, Debug)]
struct OpenToolCallRuntime {
    kind: &'static str,
    elapsed: Duration,
}

struct PromptState {
    active_command: Option<ActiveCommand>,
    active_web_search: Option<String>,
    open_tool_calls: HashMap<String, OpenToolCall>,
    thread: Arc<dyn CodexThreadImpl>,
    event_count: usize,
    response_tx: Option<oneshot::Sender<Result<StopReason, Error>>>,
    submission_id: String,
    seen_message_deltas: bool,
    seen_reasoning_deltas: bool,
    buffered_agent_text: String,
    completed: bool,
    run_started_logged: bool,
    awaiting_model_resume: bool,
    tool_watchdog_timeout: Duration,
}

impl PromptState {
    fn tool_watchdog_timeout_from_env() -> Duration {
        let parsed = std::env::var("ACP_TOOL_WATCHDOG_SECONDS")
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .unwrap_or(TOOL_CALL_WATCHDOG_SECONDS_DEFAULT);
        let bounded = parsed.clamp(
            TOOL_CALL_WATCHDOG_SECONDS_MIN,
            TOOL_CALL_WATCHDOG_SECONDS_MAX,
        );
        Duration::from_secs(bounded)
    }

    fn exec_output_limit_bytes_from_env() -> usize {
        parse_bounded_usize_env(
            EXEC_OUTPUT_MAX_BYTES_ENV,
            EXEC_OUTPUT_MAX_BYTES_DEFAULT,
            EXEC_OUTPUT_MAX_BYTES_MIN,
            EXEC_OUTPUT_MAX_BYTES_MAX,
        )
    }

    fn clamp_exec_output_for_ui(
        active_command: &mut ActiveCommand,
        data: String,
    ) -> Option<String> {
        if data.is_empty() {
            return None;
        }

        let mut bounded = String::new();
        if !active_command.output_truncated {
            let remaining = active_command
                .output_limit_bytes
                .saturating_sub(active_command.visible_output_bytes);
            if remaining == 0 {
                active_command.output_truncated = true;
            } else if data.len() <= remaining {
                active_command.visible_output_bytes += data.len();
                bounded.push_str(&data);
            } else {
                let allowed = truncate_to_byte_limit(&data, remaining);
                active_command.visible_output_bytes += allowed.len();
                bounded.push_str(&allowed);
                active_command.output_truncated = true;
            }
        }

        if active_command.output_truncated && !active_command.truncation_notice_sent {
            if !bounded.is_empty() && !bounded.ends_with('\n') {
                bounded.push('\n');
            }
            bounded.push_str(&format!(
                "[xsfire-camp] Output truncated after {} bytes to keep the session stable. Set {} to adjust this limit.\n",
                active_command.output_limit_bytes,
                EXEC_OUTPUT_MAX_BYTES_ENV
            ));
            active_command.truncation_notice_sent = true;
        }

        if bounded.is_empty() {
            None
        } else {
            Some(bounded)
        }
    }

    fn new(
        thread: Arc<dyn CodexThreadImpl>,
        response_tx: oneshot::Sender<Result<StopReason, Error>>,
        submission_id: String,
    ) -> Self {
        Self {
            active_command: None,
            active_web_search: None,
            open_tool_calls: HashMap::new(),
            thread,
            event_count: 0,
            response_tx: Some(response_tx),
            submission_id,
            seen_message_deltas: false,
            seen_reasoning_deltas: false,
            buffered_agent_text: String::new(),
            completed: false,
            run_started_logged: false,
            awaiting_model_resume: false,
            tool_watchdog_timeout: Self::tool_watchdog_timeout_from_env(),
        }
    }

    fn new_background(thread: Arc<dyn CodexThreadImpl>, submission_id: String) -> Self {
        Self {
            active_command: None,
            active_web_search: None,
            open_tool_calls: HashMap::new(),
            thread,
            event_count: 0,
            response_tx: None,
            submission_id,
            seen_message_deltas: false,
            seen_reasoning_deltas: false,
            buffered_agent_text: String::new(),
            completed: false,
            run_started_logged: false,
            awaiting_model_resume: false,
            tool_watchdog_timeout: Self::tool_watchdog_timeout_from_env(),
        }
    }

    fn is_active(&self) -> bool {
        if self.completed {
            return false;
        }
        self.response_tx.as_ref().is_none_or(|tx| !tx.is_closed())
    }

    fn longest_open_tool_call_runtime(&self) -> Option<OpenToolCallRuntime> {
        self.open_tool_calls
            .values()
            .map(|open_call| OpenToolCallRuntime {
                kind: open_call.kind,
                elapsed: open_call.started_at.elapsed(),
            })
            .max_by_key(|runtime| runtime.elapsed)
    }

    fn finish_with_result(&mut self, result: Result<StopReason, Error>) {
        if let Some(response_tx) = self.response_tx.take() {
            drop(response_tx.send(result));
        }
        self.completed = true;
    }

    async fn emit_agent_text_for_ui(&mut self, client: &SessionClient, text: String) {
        if text.is_empty() {
            return;
        }

        if client.ui_visibility_mode().hides_internal_updates() {
            let sanitized = sanitize_internal_frame_text(&text);
            if !sanitized.is_empty() {
                if !self.buffered_agent_text.is_empty() {
                    self.buffered_agent_text.push('\n');
                }
                self.buffered_agent_text.push_str(&sanitized);
            }
            return;
        }

        client.send_agent_text(text).await;
    }

    async fn flush_final_agent_text_if_needed(&mut self, client: &SessionClient) {
        if !client.ui_visibility_mode().hides_internal_updates() {
            return;
        }
        if self.buffered_agent_text.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.buffered_agent_text);
        client.send_agent_text(text).await;
    }

    fn ensure_run_started_logged(&mut self, client: &SessionClient) {
        if self.run_started_logged {
            return;
        }
        self.run_started_logged = true;
        client.log_canonical(
            "acp.bridge.run_started",
            json!({
                "submission_id": self.submission_id,
            }),
        );
    }

    fn mark_tool_call_started(
        &mut self,
        client: &SessionClient,
        call_id: &str,
        kind: &'static str,
    ) {
        self.open_tool_calls.insert(
            call_id.to_string(),
            OpenToolCall {
                kind,
                started_at: Instant::now(),
            },
        );
        client.log_canonical(
            "acp.bridge.tool_call_received",
            json!({
                "submission_id": self.submission_id,
                "tool_call_id": call_id,
                "tool_kind": kind,
            }),
        );
        client.log_canonical(
            "acp.bridge.tool_exec_started",
            json!({
                "submission_id": self.submission_id,
                "tool_call_id": call_id,
                "tool_kind": kind,
            }),
        );
    }

    fn mark_tool_exec_finished(
        &self,
        client: &SessionClient,
        call_id: &str,
        kind: &str,
        status: ToolCallStatus,
        exit_code: Option<i32>,
        reason: Option<&str>,
    ) {
        client.log_canonical(
            "acp.bridge.tool_exec_finished",
            json!({
                "submission_id": self.submission_id,
                "tool_call_id": call_id,
                "tool_kind": kind,
                "status": format!("{status:?}"),
                "exit_code": exit_code,
                "reason": reason,
            }),
        );
    }

    fn mark_tool_result_sent(
        &mut self,
        client: &SessionClient,
        call_id: &str,
        status: ToolCallStatus,
        reason: Option<&str>,
    ) {
        self.awaiting_model_resume = true;
        client.log_canonical(
            "acp.bridge.tool_result_sent",
            json!({
                "submission_id": self.submission_id,
                "tool_call_id": call_id,
                "status": format!("{status:?}"),
                "reason": reason,
            }),
        );
    }

    fn maybe_log_model_resumed(&mut self, client: &SessionClient) {
        if !self.awaiting_model_resume {
            return;
        }
        self.awaiting_model_resume = false;
        client.log_canonical(
            "acp.bridge.model_resumed",
            json!({
                "submission_id": self.submission_id,
            }),
        );
    }

    fn mark_final_emitted(&self, client: &SessionClient, stop_reason: &str) {
        client.log_canonical(
            "acp.bridge.final_emitted",
            json!({
                "submission_id": self.submission_id,
                "stop_reason": stop_reason,
            }),
        );
    }

    async fn fail_tool_call_with_reason(
        &mut self,
        client: &SessionClient,
        call_id: &str,
        reason: &str,
    ) {
        let call_id_owned = call_id.to_string();
        let Some(open_call) = self.open_tool_calls.remove(call_id) else {
            return;
        };

        self.mark_tool_exec_finished(
            client,
            call_id,
            open_call.kind,
            ToolCallStatus::Failed,
            None,
            Some(reason),
        );
        client
            .send_tool_call_update(ToolCallUpdate::new(
                call_id_owned.clone(),
                ToolCallUpdateFields::new()
                    .status(ToolCallStatus::Failed)
                    .raw_output(json!({
                        "error": reason,
                        "tool_kind": open_call.kind,
                    })),
            ))
            .await;
        self.mark_tool_result_sent(client, &call_id_owned, ToolCallStatus::Failed, Some(reason));

        if self
            .active_command
            .as_ref()
            .is_some_and(|active| active.call_id == call_id)
        {
            self.active_command = None;
        }
        if self.active_web_search.as_deref() == Some(call_id) {
            self.active_web_search = None;
        }
    }

    async fn fail_open_tool_calls(&mut self, client: &SessionClient, reason: &str) {
        let call_ids = self.open_tool_calls.keys().cloned().collect::<Vec<_>>();
        for call_id in call_ids {
            self.fail_tool_call_with_reason(client, &call_id, reason)
                .await;
        }
    }

    async fn force_cancelled(&mut self, client: &SessionClient, reason: &str) {
        self.fail_open_tool_calls(client, reason).await;
        if !self.completed {
            self.mark_final_emitted(client, "cancelled_preempted");
            self.finish_with_result(Ok(StopReason::Cancelled));
        }
    }

    async fn handle_watchdog_tick(&mut self, client: &SessionClient) {
        if self.completed {
            return;
        }
        let now = Instant::now();
        let timed_out = self
            .open_tool_calls
            .iter()
            .filter_map(|(call_id, open_call)| {
                if now.duration_since(open_call.started_at) >= self.tool_watchdog_timeout {
                    Some(call_id.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        for call_id in timed_out {
            self.fail_tool_call_with_reason(client, &call_id, "watchdog_timeout")
                .await;
        }
    }

    async fn emit_exec_output_update(
        client: &SessionClient,
        active_command: &mut ActiveCommand,
        _call_id: &str,
        data: String,
    ) {
        let Some(data) = Self::clamp_exec_output_for_ui(active_command, data) else {
            return;
        };

        let update = if client.supports_embedded_terminal_output(active_command) {
            ToolCallUpdate::new(
                active_command.tool_call_id.clone(),
                ToolCallUpdateFields::new(),
            )
            .meta(Meta::from_iter([(
                "terminal_output".to_owned(),
                serde_json::json!({
                    "terminal_id": active_command.display_terminal_id(),
                    "data": data
                }),
            )]))
        } else if client.supports_standard_terminal() && active_command.terminal_id.is_some() {
            return;
        } else {
            active_command.output.push_str(&data);
            let content = match active_command.file_extension.as_deref() {
                Some("md") => active_command.output.clone(),
                Some(ext) => format!(
                    "```{ext}\n{}\n```\n",
                    active_command.output.trim_end_matches('\n')
                ),
                None => format!(
                    "```sh\n{}\n```\n",
                    active_command.output.trim_end_matches('\n')
                ),
            };
            ToolCallUpdate::new(
                active_command.tool_call_id.clone(),
                ToolCallUpdateFields::new().content(vec![content.into()]),
            )
        };

        client.send_tool_call_update(update).await;
    }

    #[expect(clippy::too_many_lines)]
    async fn handle_event(&mut self, client: &SessionClient, event: EventMsg) {
        self.event_count += 1;
        self.ensure_run_started_logged(client);
        self.handle_watchdog_tick(client).await;

        // Complete any previous web search before starting a new one
        match &event {
            EventMsg::WebSearchBegin(..)
            | EventMsg::UserMessage(..)
            | EventMsg::ExecApprovalRequest(..)
            | EventMsg::ExecCommandBegin(..)
            | EventMsg::ExecCommandOutputDelta(..)
            | EventMsg::ExecCommandEnd(..)
            | EventMsg::McpToolCallBegin(..)
            | EventMsg::McpToolCallEnd(..)
            | EventMsg::ApplyPatchApprovalRequest(..)
            | EventMsg::PatchApplyBegin(..)
            | EventMsg::PatchApplyEnd(..)
            | EventMsg::TurnStarted(..)
            | EventMsg::TurnComplete(..)
            | EventMsg::TokenCount(..)
            | EventMsg::TurnDiff(..)
            | EventMsg::EnteredReviewMode(..)
            | EventMsg::ExitedReviewMode(..) => {
                self.complete_web_search(client).await;
            }
            _ => {}
        }

        match event {
            EventMsg::TurnStarted(TurnStartedEvent {
                model_context_window,
            }) => {
                info!("Task started with context window of {model_context_window:?}");
            }
            EventMsg::ItemStarted(ItemStartedEvent { thread_id, turn_id, item }) => {

                info!("Item started with thread_id: {thread_id}, turn_id: {turn_id}, item: {item:?}");
            }
            EventMsg::UserMessage(UserMessageEvent {
                message,
                images: _,
                text_elements: _,
                local_images: _,
            }) => {
                info!("User message: {message:?}");
            }
            EventMsg::AgentMessageContentDelta(AgentMessageContentDeltaEvent { thread_id, turn_id, item_id, delta }) => {
                info!("Agent message content delta received: thread_id: {thread_id}, turn_id: {turn_id}, item_id: {item_id}, delta: {delta:?}");
                self.seen_message_deltas = true;
                self.maybe_log_model_resumed(client);
                self.emit_agent_text_for_ui(client, delta).await;
            }
            EventMsg::ReasoningContentDelta(ReasoningContentDeltaEvent { thread_id, turn_id, item_id, delta, summary_index: index })
            | EventMsg::ReasoningRawContentDelta(ReasoningRawContentDeltaEvent { thread_id, turn_id, item_id, delta, content_index: index }) => {
                info!("Agent reasoning content delta received: thread_id: {thread_id}, turn_id: {turn_id}, item_id: {item_id}, index: {index}, delta: {delta:?}");
                self.seen_reasoning_deltas = true;
                self.maybe_log_model_resumed(client);
                client.send_agent_thought(delta).await;
            }
            EventMsg::AgentReasoningSectionBreak(AgentReasoningSectionBreakEvent { item_id, summary_index}) => {
                info!("Agent reasoning section break received:  item_id: {item_id}, index: {summary_index}");
                // Make sure the section heading actually get spacing
                self.seen_reasoning_deltas = true;
                self.maybe_log_model_resumed(client);
                client.send_agent_thought("\n\n").await;
            }
            EventMsg::AgentMessage(AgentMessageEvent { message }) => {
                info!("Agent message (non-delta) received: {message:?}");
                // We didn't receive this message via streaming
                if !std::mem::take(&mut self.seen_message_deltas) {
                    self.maybe_log_model_resumed(client);
                    self.emit_agent_text_for_ui(client, message).await;
                }
            }
            EventMsg::AgentReasoning(AgentReasoningEvent { text }) => {
                info!("Agent reasoning (non-delta) received: {text:?}");
                // We didn't receive this message via streaming
                if !std::mem::take(&mut self.seen_reasoning_deltas) {
                    self.maybe_log_model_resumed(client);
                    client.send_agent_thought(text).await;
                }
            }
            EventMsg::PlanUpdate(UpdatePlanArgs { explanation, plan }) => {
                // Send this to the client via session/update notification
                info!("Agent plan updated. Explanation: {:?}", explanation);
                client.update_plan(plan, explanation).await;
            }
            EventMsg::WebSearchBegin(WebSearchBeginEvent { call_id }) => {
                info!("Web search started: call_id={}", call_id);
                // Create a ToolCall notification for the search beginning
                self.start_web_search(client, call_id).await;
            }
            EventMsg::WebSearchEnd(WebSearchEndEvent { call_id, query }) => {
                info!("Web search query received: call_id={call_id}, query={query}");
                // Send update that the search is in progress with the query
                // (WebSearchEnd just means we have the query, not that results are ready)
                self.update_web_search_query(client, call_id, query).await;
                // The actual search results will come through AgentMessage events
                // We mark as completed when a new tool call begins
            }
            EventMsg::ExecApprovalRequest(event) => {
                info!("Command execution started: call_id={}, command={:?}", event.call_id, event.command);
                if let Err(err) = self.exec_approval(client, event).await {
                    self.finish_with_result(Err(err));
                }
            }
            EventMsg::ExecCommandBegin(event) => {
                info!(
                    "Command execution started: call_id={}, command={:?}",
                    event.call_id, event.command
                );
                self.exec_command_begin(client, event).await;
            }
            EventMsg::ExecCommandOutputDelta(delta_event) => {
                self.exec_command_output_delta(client, delta_event).await;
            }
            EventMsg::ExecCommandEnd(end_event) => {
                info!(
                    "Command execution ended: call_id={}, exit_code={}",
                    end_event.call_id, end_event.exit_code
                );
                self.exec_command_end(client, end_event).await;
            }
            EventMsg::TerminalInteraction(event) => {
                info!(
                    "Terminal interaction: call_id={}, process_id={}, stdin={}",
                    event.call_id, event.process_id, event.stdin
                );
                self.terminal_interaction(client, event).await;
            }
            EventMsg::McpToolCallBegin(McpToolCallBeginEvent { call_id, invocation }) => {
                info!("MCP tool call begin: call_id={call_id}, invocation={} {}", invocation.server, invocation.tool);
                self.start_mcp_tool_call(client, call_id, invocation).await;
            }
            EventMsg::McpToolCallEnd(McpToolCallEndEvent { call_id, invocation, duration, result }) => {
                info!("MCP tool call ended: call_id={call_id}, invocation={} {}, duration={duration:?}", invocation.server, invocation.tool);
                self.end_mcp_tool_call(client, call_id, result).await;
            }
            EventMsg::ApplyPatchApprovalRequest(event) => {
                info!("Apply patch approval request: call_id={}, reason={:?}", event.call_id, event.reason);
                if let Err(err) = self.patch_approval(client, event).await {
                    self.finish_with_result(Err(err));
                }
            }
            EventMsg::PatchApplyBegin(event) => {
                info!("Patch apply begin: call_id={}, auto_approved={}", event.call_id,event.auto_approved);
                self.start_patch_apply(client, event).await;
            }
            EventMsg::PatchApplyEnd(event) => {
                info!("Patch apply end: call_id={}, success={}", event.call_id, event.success);
                self.end_patch_apply(client, event).await;
            }
            EventMsg::ItemCompleted(ItemCompletedEvent { thread_id, turn_id, item }) => {
                info!("Item completed: thread_id={}, turn_id={}, item={:?}", thread_id, turn_id, item);
            }
            EventMsg::TurnComplete(TurnCompleteEvent { last_agent_message}) => {
                info!(
                    "Task completed successfully after {} events. Last agent message: {last_agent_message:?}", self.event_count
                );
                self.flush_final_agent_text_if_needed(client).await;
                self.fail_open_tool_calls(client, "turn_complete_cleanup").await;
                self.mark_final_emitted(client, "end_turn");
                self.finish_with_result(Ok(StopReason::EndTurn));
            }
            EventMsg::UndoStarted(event) => {
                client
                    .send_agent_text(
                        event
                            .message
                            .unwrap_or_else(|| "Undo in progress...".to_string()),
                    )
                    .await;
            }
            EventMsg::UndoCompleted(event) => {
                let fallback = if event.success {
                    "Undo completed.".to_string()
                } else {
                    "Undo failed.".to_string()
                };
                client.send_agent_text(event.message.unwrap_or(fallback)).await;
            }
            EventMsg::StreamError(StreamErrorEvent { message , codex_error_info, additional_details }) => {
                error!("Handled error during turn: {message} {codex_error_info:?} {additional_details:?}");
                self.fail_open_tool_calls(client, "stream_error").await;
                self.mark_final_emitted(client, "cancelled_stream_error");
                self.finish_with_result(Ok(StopReason::Cancelled));
            }
            EventMsg::Error(ErrorEvent { message, codex_error_info }) => {
                error!("Unhandled error during turn: {message} {codex_error_info:?}");
                self.fail_open_tool_calls(client, "error_event").await;
                self.mark_final_emitted(client, "error");
                self.finish_with_result(Err(
                    Error::internal_error()
                        .data(json!({ "message": message, "codex_error_info": codex_error_info })),
                ));
            }
            EventMsg::TurnAborted(TurnAbortedEvent { reason }) => {
                info!("Turn aborted: {reason:?}");
                self.fail_open_tool_calls(client, "turn_aborted").await;
                self.mark_final_emitted(client, "cancelled_turn_aborted");
                self.finish_with_result(Ok(StopReason::Cancelled));
            }
            EventMsg::ShutdownComplete => {
                info!("Agent shutting down");
                self.fail_open_tool_calls(client, "shutdown_complete").await;
                self.mark_final_emitted(client, "cancelled_shutdown");
                self.finish_with_result(Ok(StopReason::Cancelled));
            }
            EventMsg::ViewImageToolCall(ViewImageToolCallEvent { call_id, path }) => {
                info!("ViewImageToolCallEvent received");
                let display_path = path.display().to_string();
                client
                    .send_tool_call(
                        ToolCall::new(call_id, format!("View Image {display_path}"))
                            .kind(ToolKind::Read)
                            .status(ToolCallStatus::Completed)
                            .content(vec![ToolCallContent::Content(Content::new(
                                ContentBlock::ResourceLink(ResourceLink::new(
                                    display_path.clone(),
                                    display_path.clone(),
                                )),
                            ))])
                            .locations(vec![ToolCallLocation::new(path)]),
                    )
                    .await;
            }
            EventMsg::EnteredReviewMode(review_request) => {
                info!("Review begin: request={review_request:?}");
            }
            EventMsg::ExitedReviewMode(event) => {
                info!("Review end: output={event:?}");
                if let Err(err) = self.review_mode_exit(client, event).await {
                    self.finish_with_result(Err(err));
                }
            }
            EventMsg::Warning(WarningEvent { message }) => {
                warn!("Warning: {message}");
            }
            EventMsg::McpStartupUpdate(McpStartupUpdateEvent { server, status }) => {
                info!("MCP startup update: server={server}, status={status:?}");
            }
            EventMsg::McpStartupComplete(McpStartupCompleteEvent {
                ready,
                failed,
                cancelled,
            }) => {
                info!(
                    "MCP startup complete: ready={ready:?}, failed={failed:?}, cancelled={cancelled:?}"
                );
            }
            EventMsg::ElicitationRequest(event) => {
                info!("Elicitation request: server={}, id={:?}, message={}", event.server_name, event.id, event.message);
                if let Err(err) = self.mcp_elicitation(client, event).await {
                    self.finish_with_result(Err(err));
                }
            }

            // Ignore these events
            EventMsg::AgentReasoningRawContent(..)
            | EventMsg::ThreadRolledBack(..)
            // In the future we can use this to update usage stats
            | EventMsg::TokenCount(..)
            // we already have a way to diff the turn, so ignore
            | EventMsg::TurnDiff(..)
            // Revisit when we can emit status updates
            | EventMsg::BackgroundEvent(..)
            | EventMsg::ContextCompacted(..)
            | EventMsg::SkillsUpdateAvailable
            // Old events
            | EventMsg::AgentMessageDelta(..) | EventMsg::AgentReasoningDelta(..) | EventMsg::AgentReasoningRawContentDelta(..)
            | EventMsg::RawResponseItem(..)
            | EventMsg::SessionConfigured(..)
            // TODO: Subagent UI?
            | EventMsg::CollabAgentSpawnBegin(..)
            | EventMsg::CollabAgentSpawnEnd(..)
            | EventMsg::CollabAgentInteractionBegin(..)
            | EventMsg::CollabAgentInteractionEnd(..)
            | EventMsg::CollabWaitingBegin(..)
            | EventMsg::CollabWaitingEnd(..)
            | EventMsg::CollabCloseBegin(..)
            | EventMsg::CollabCloseEnd(..) => {},
            e @ (EventMsg::McpListToolsResponse(..)
            // returned from Op::ListCustomPrompts, ignore
            | EventMsg::ListCustomPromptsResponse(..)
            | EventMsg::ListSkillsResponse(..)
            // Used for returning a single history entry
            | EventMsg::GetHistoryEntryResponse(..)
            | EventMsg::DeprecationNotice(..)
            |  EventMsg::RequestUserInput(..)
            | EventMsg::DynamicToolCallRequest(..)
            ) => {
                warn!("Unexpected event: {:?}", e);
            }
        }
    }

    async fn mcp_elicitation(
        &self,
        client: &SessionClient,
        event: ElicitationRequestEvent,
    ) -> Result<(), Error> {
        let raw_input = serde_json::json!(&event);
        let ElicitationRequestEvent {
            server_name,
            id,
            message,
        } = event;
        let tool_call_id = ToolCallId::new(match &id {
            RequestId::String(s) => s.clone(),
            RequestId::Integer(i) => i.to_string(),
        });
        let response = client
            .request_permission(
                ToolCallUpdate::new(
                    tool_call_id.clone(),
                    ToolCallUpdateFields::new()
                        .title(server_name.clone())
                        .status(ToolCallStatus::Pending)
                        .content(vec![message.into()])
                        .raw_input(raw_input),
                ),
                vec![
                    PermissionOption::new(
                        "approved",
                        "Yes, provide the requested info",
                        PermissionOptionKind::AllowOnce,
                    ),
                    PermissionOption::new(
                        "abort",
                        "No, but continue without it",
                        PermissionOptionKind::RejectOnce,
                    ),
                    PermissionOption::new(
                        "cancel",
                        "Cancel this request",
                        PermissionOptionKind::RejectOnce,
                    ),
                ],
            )
            .await?;

        let decision = match response.outcome {
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome { option_id, .. }) => {
                match option_id.0.as_ref() {
                    "approved" => ElicitationAction::Accept,
                    "abort" => ElicitationAction::Decline,
                    _ => ElicitationAction::Cancel,
                }
            }
            RequestPermissionOutcome::Cancelled | _ => ElicitationAction::Cancel,
        };

        self.thread
            .submit(Op::ResolveElicitation {
                server_name,
                request_id: id,
                decision,
            })
            .await
            .map_err(|e| Error::from(anyhow::anyhow!(e)))?;

        client
            .send_tool_call_update(ToolCallUpdate::new(
                tool_call_id,
                ToolCallUpdateFields::new().status(if decision == ElicitationAction::Accept {
                    ToolCallStatus::Completed
                } else {
                    ToolCallStatus::Failed
                }),
            ))
            .await;

        Ok(())
    }

    async fn review_mode_exit(
        &self,
        client: &SessionClient,
        event: ExitedReviewModeEvent,
    ) -> Result<(), Error> {
        let ExitedReviewModeEvent { review_output } = event;
        let Some(ReviewOutputEvent {
            findings,
            overall_correctness: _,
            overall_explanation,
            overall_confidence_score: _,
        }) = review_output
        else {
            return Ok(());
        };

        let text = if findings.is_empty() {
            let explanation = overall_explanation.trim();
            if explanation.is_empty() {
                "Reviewer failed to output a response"
            } else {
                explanation
            }
            .to_string()
        } else {
            format_review_findings_block(&findings, None)
        };

        client.send_agent_text(&text).await;
        Ok(())
    }

    async fn patch_approval(
        &self,
        client: &SessionClient,
        event: ApplyPatchApprovalRequestEvent,
    ) -> Result<(), Error> {
        let raw_input = serde_json::json!(&event);
        let ApplyPatchApprovalRequestEvent {
            call_id,
            changes,
            reason,
            // grant_root doesn't seem to be set anywhere on the codex side
            grant_root: _,
            turn_id: _,
        } = event;
        let (title, locations, content) = extract_tool_call_content_from_changes(changes);
        let response = client
            .request_permission(
                ToolCallUpdate::new(
                    call_id,
                    ToolCallUpdateFields::new()
                        .kind(ToolKind::Edit)
                        .status(ToolCallStatus::Pending)
                        .title(title)
                        .locations(locations)
                        .content(content.chain(reason.map(|r| r.into())).collect::<Vec<_>>())
                        .raw_input(raw_input),
                ),
                vec![
                    PermissionOption::new("approved", "Yes", PermissionOptionKind::AllowOnce),
                    PermissionOption::new(
                        "abort",
                        "No, provide feedback",
                        PermissionOptionKind::RejectOnce,
                    ),
                ],
            )
            .await?;

        let decision = match response.outcome {
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome { option_id, .. }) => {
                match option_id.0.as_ref() {
                    "approved" => ReviewDecision::Approved,
                    _ => ReviewDecision::Abort,
                }
            }
            RequestPermissionOutcome::Cancelled | _ => ReviewDecision::Abort,
        };

        self.thread
            .submit(Op::PatchApproval {
                id: self.submission_id.clone(),
                decision,
            })
            .await
            .map_err(|e| Error::from(anyhow::anyhow!(e)))?;
        Ok(())
    }

    async fn start_patch_apply(&mut self, client: &SessionClient, event: PatchApplyBeginEvent) {
        let raw_input = serde_json::json!(&event);
        let PatchApplyBeginEvent {
            call_id,
            auto_approved: _,
            changes,
            turn_id: _,
        } = event;

        let (title, locations, content) = extract_tool_call_content_from_changes(changes);

        client
            .send_tool_call(
                ToolCall::new(call_id.clone(), title)
                    .kind(ToolKind::Edit)
                    .status(ToolCallStatus::InProgress)
                    .locations(locations)
                    .content(content.collect())
                    .raw_input(raw_input),
            )
            .await;
        self.mark_tool_call_started(client, &call_id, "patch_apply");
    }

    async fn end_patch_apply(&mut self, client: &SessionClient, event: PatchApplyEndEvent) {
        let raw_output = cap_json_payload_for_ui(serde_json::json!(&event), "patch_apply_end");
        let PatchApplyEndEvent {
            call_id,
            stdout: _,
            stderr: _,
            success,
            changes,
            turn_id: _,
        } = event;

        let (title, locations, content) = if !changes.is_empty() {
            let (title, locations, content) = extract_tool_call_content_from_changes(changes);
            (Some(title), Some(locations), Some(content.collect()))
        } else {
            (None, None, None)
        };
        let status = if success {
            ToolCallStatus::Completed
        } else {
            ToolCallStatus::Failed
        };
        let kind = self
            .open_tool_calls
            .get(&call_id)
            .map(|open| open.kind)
            .unwrap_or("patch_apply");
        self.mark_tool_exec_finished(client, &call_id, kind, status, None, None);

        client
            .send_tool_call_update(ToolCallUpdate::new(
                call_id.clone(),
                ToolCallUpdateFields::new()
                    .status(status)
                    .raw_output(raw_output)
                    .title(title)
                    .locations(locations)
                    .content(content),
            ))
            .await;
        self.open_tool_calls.remove(&call_id);
        self.mark_tool_result_sent(client, &call_id, status, None);
    }

    async fn start_mcp_tool_call(
        &mut self,
        client: &SessionClient,
        call_id: String,
        invocation: McpInvocation,
    ) {
        let title = format!("Tool: {}/{}", invocation.server, invocation.tool);
        client
            .send_tool_call(
                ToolCall::new(call_id.clone(), title)
                    .status(ToolCallStatus::InProgress)
                    .raw_input(serde_json::json!(&invocation)),
            )
            .await;
        self.mark_tool_call_started(client, &call_id, "mcp_tool_call");
    }

    async fn end_mcp_tool_call(
        &mut self,
        client: &SessionClient,
        call_id: String,
        result: Result<CallToolResult, String>,
    ) {
        let is_error = match result.as_ref() {
            Ok(result) => result.is_error.unwrap_or_default(),
            Err(_) => true,
        };
        let raw_output = match result.as_ref() {
            Ok(result) => serde_json::json!(result),
            Err(err) => serde_json::json!(err),
        };
        let raw_output = cap_json_payload_for_ui(raw_output, "mcp_tool_call_end");
        let status = if is_error {
            ToolCallStatus::Failed
        } else {
            ToolCallStatus::Completed
        };
        let kind = self
            .open_tool_calls
            .get(&call_id)
            .map(|open| open.kind)
            .unwrap_or("mcp_tool_call");
        self.mark_tool_exec_finished(client, &call_id, kind, status, None, None);

        client
            .send_tool_call_update(ToolCallUpdate::new(
                call_id.clone(),
                ToolCallUpdateFields::new()
                    .status(status)
                    .raw_output(raw_output)
                    .content(result.ok().filter(|result| !result.content.is_empty()).map(
                        |result| {
                            result
                                .content
                                .into_iter()
                                .map(codex_content_to_acp_content)
                                .collect()
                        },
                    )),
            ))
            .await;
        self.open_tool_calls.remove(&call_id);
        self.mark_tool_result_sent(client, &call_id, status, None);
    }

    async fn exec_approval(
        &mut self,
        client: &SessionClient,
        event: ExecApprovalRequestEvent,
    ) -> Result<(), Error> {
        let raw_input = serde_json::json!(&event);
        let ExecApprovalRequestEvent {
            call_id,
            command: _,
            turn_id: _,
            cwd,
            reason,
            parsed_cmd,
            proposed_execpolicy_amendment,
        } = event;

        // Create a new tool call for the command execution
        let tool_call_id = ToolCallId::new(call_id.clone());
        let ParseCommandToolCall {
            title,
            locations,
            kind,
            ..
        } = parse_command_tool_call(parsed_cmd, &cwd);

        let mut content = vec![];

        if let Some(reason) = reason {
            content.push(reason);
        }
        if let Some(amendment) = proposed_execpolicy_amendment {
            content.push(format!(
                "Proposed Amendment: {}",
                amendment.command().join("\n")
            ));
        }

        let content = if content.is_empty() {
            None
        } else {
            Some(vec![content.join("\n").into()])
        };

        let response = client
            .request_permission(
                ToolCallUpdate::new(
                    tool_call_id,
                    ToolCallUpdateFields::new()
                        .kind(kind)
                        .status(ToolCallStatus::Pending)
                        .title(title)
                        .raw_input(raw_input)
                        .content(content)
                        .locations(if locations.is_empty() {
                            None
                        } else {
                            Some(locations)
                        }),
                ),
                vec![
                    PermissionOption::new(
                        "approved-for-session",
                        "Always",
                        PermissionOptionKind::AllowAlways,
                    ),
                    PermissionOption::new("approved", "Yes", PermissionOptionKind::AllowOnce),
                    PermissionOption::new(
                        "abort",
                        "No, provide feedback",
                        PermissionOptionKind::RejectOnce,
                    ),
                ],
            )
            .await?;

        let decision = match response.outcome {
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome { option_id, .. }) => {
                match option_id.0.as_ref() {
                    "approved-for-session" => ReviewDecision::ApprovedForSession,
                    "approved" => ReviewDecision::Approved,
                    _ => ReviewDecision::Abort,
                }
            }
            RequestPermissionOutcome::Cancelled | _ => ReviewDecision::Abort,
        };

        self.thread
            .submit(Op::ExecApproval {
                id: self.submission_id.clone(),
                decision,
            })
            .await
            .map_err(|e| Error::from(anyhow::anyhow!(e)))?;

        Ok(())
    }

    async fn exec_command_begin(&mut self, client: &SessionClient, event: ExecCommandBeginEvent) {
        let raw_input = serde_json::json!(&event);
        let ExecCommandBeginEvent {
            turn_id: _,
            source: _,
            interaction_input: _,
            call_id,
            command: _,
            cwd,
            parsed_cmd,
            process_id: _,
            terminal_id,
        } = event;
        if let Some(previous_call_id) = self
            .active_command
            .as_ref()
            .map(|active| active.call_id.clone())
        {
            self.fail_tool_call_with_reason(
                client,
                &previous_call_id,
                "superseded_by_new_exec_command",
            )
            .await;
        }

        // Create a new tool call for the command execution
        let tool_call_id = ToolCallId::new(call_id.clone());
        let ParseCommandToolCall {
            title,
            file_extension,
            locations,
            terminal_output,
            kind,
        } = parse_command_tool_call(parsed_cmd, &cwd);

        let active_command = ActiveCommand {
            call_id: call_id.clone(),
            tool_call_id: tool_call_id.clone(),
            terminal_id,
            output: String::new(),
            visible_output_bytes: 0,
            output_limit_bytes: Self::exec_output_limit_bytes_from_env(),
            output_truncated: false,
            truncation_notice_sent: false,
            pending_utf8_bytes: Vec::new(),
            file_extension,
            terminal_output,
        };
        let (content, meta) = if client.supports_embedded_terminal_output(&active_command) {
            let content = vec![ToolCallContent::Terminal(Terminal::new(
                active_command.display_terminal_id().to_string(),
            ))];
            let meta = Some(Meta::from_iter([(
                "terminal_info".to_owned(),
                serde_json::json!({
                    "terminal_id": active_command.display_terminal_id(),
                    "cwd": cwd
                }),
            )]));
            (content, meta)
        } else if client.supports_standard_terminal() {
            // Standard terminal clients only get terminal content when the exec layer already
            // delegated to ACP terminal/* and returned a real terminal id for this command.
            let content = active_command
                .terminal_id
                .clone()
                .map(Terminal::new)
                .map(ToolCallContent::Terminal)
                .into_iter()
                .collect();
            (content, None)
        } else {
            (vec![], None)
        };

        self.active_command = Some(active_command);

        client
            .send_tool_call(
                ToolCall::new(tool_call_id, title)
                    .kind(kind)
                    .status(ToolCallStatus::InProgress)
                    .locations(locations)
                    .raw_input(raw_input)
                    .content(content)
                    .meta(meta),
            )
            .await;
        self.mark_tool_call_started(client, &call_id, "exec_command");
    }

    async fn exec_command_output_delta(
        &mut self,
        client: &SessionClient,
        event: ExecCommandOutputDeltaEvent,
    ) {
        let ExecCommandOutputDeltaEvent {
            call_id,
            chunk,
            stream: _,
        } = event;
        // Stream output bytes to the display-only terminal via ToolCallUpdate meta.
        if let Some(active_command) = &mut self.active_command
            && *active_command.call_id == call_id
        {
            let data_str =
                decode_utf8_streaming(&mut active_command.pending_utf8_bytes, &chunk, false);
            Self::emit_exec_output_update(client, active_command, &call_id, data_str).await;
        }
    }

    async fn exec_command_end(&mut self, client: &SessionClient, event: ExecCommandEndEvent) {
        let raw_output = cap_json_payload_for_ui(serde_json::json!(&event), "exec_command_end");
        let ExecCommandEndEvent {
            turn_id: _,
            command: _,
            cwd: _,
            parsed_cmd: _,
            source: _,
            interaction_input: _,
            call_id,
            exit_code,
            stdout: _,
            stderr: _,
            aggregated_output: _,
            duration: _,
            formatted_output: _,
            process_id: _,
            terminal_id,
        } = event;
        let status = if exit_code == 0 {
            ToolCallStatus::Completed
        } else {
            ToolCallStatus::Failed
        };

        if let Some(active_command) = self.active_command.take() {
            if active_command.call_id == call_id {
                let mut active_command = active_command;
                let trailing =
                    decode_utf8_streaming(&mut active_command.pending_utf8_bytes, &[], true);
                Self::emit_exec_output_update(client, &mut active_command, &call_id, trailing)
                    .await;
                let kind = self
                    .open_tool_calls
                    .get(&call_id)
                    .map(|open| open.kind)
                    .unwrap_or("exec_command");
                self.mark_tool_exec_finished(client, &call_id, kind, status, Some(exit_code), None);

                client
                    .send_tool_call_update(
                        ToolCallUpdate::new(
                            active_command.tool_call_id.clone(),
                            ToolCallUpdateFields::new()
                                .status(status)
                                .raw_output(raw_output),
                        )
                        .meta(
                            client
                                .supports_embedded_terminal_output(&active_command)
                                .then(|| {
                                    Meta::from_iter([(
                                        "terminal_exit".into(),
                                        serde_json::json!({
                                            "terminal_id": terminal_id
                                                .as_deref()
                                                .or(active_command.terminal_id.as_deref())
                                                .unwrap_or(&call_id),
                                            "exit_code": exit_code,
                                            "signal": null
                                        }),
                                    )])
                                }),
                        ),
                    )
                    .await;
                self.open_tool_calls.remove(&call_id);
                self.mark_tool_result_sent(client, &call_id, status, None);
            } else {
                warn!(
                    "ExecCommandEnd call_id mismatch: active={} incoming={}",
                    active_command.call_id, call_id
                );
                self.active_command = Some(active_command);
                if self.open_tool_calls.contains_key(&call_id) {
                    let kind = self
                        .open_tool_calls
                        .get(&call_id)
                        .map(|open| open.kind)
                        .unwrap_or("exec_command");
                    self.mark_tool_exec_finished(
                        client,
                        &call_id,
                        kind,
                        status,
                        Some(exit_code),
                        Some("call_id_mismatch_fallback"),
                    );
                    client
                        .send_tool_call_update(ToolCallUpdate::new(
                            call_id.clone(),
                            ToolCallUpdateFields::new()
                                .status(status)
                                .raw_output(raw_output),
                        ))
                        .await;
                    self.open_tool_calls.remove(&call_id);
                    self.mark_tool_result_sent(
                        client,
                        &call_id,
                        status,
                        Some("call_id_mismatch_fallback"),
                    );
                }
            }
        } else if self.open_tool_calls.contains_key(&call_id) {
            let kind = self
                .open_tool_calls
                .get(&call_id)
                .map(|open| open.kind)
                .unwrap_or("exec_command");
            self.mark_tool_exec_finished(
                client,
                &call_id,
                kind,
                status,
                Some(exit_code),
                Some("missing_active_command_fallback"),
            );
            client
                .send_tool_call_update(ToolCallUpdate::new(
                    call_id.clone(),
                    ToolCallUpdateFields::new()
                        .status(status)
                        .raw_output(raw_output),
                ))
                .await;
            self.open_tool_calls.remove(&call_id);
            self.mark_tool_result_sent(
                client,
                &call_id,
                status,
                Some("missing_active_command_fallback"),
            );
        }
    }

    async fn terminal_interaction(
        &mut self,
        client: &SessionClient,
        event: TerminalInteractionEvent,
    ) {
        let TerminalInteractionEvent {
            call_id,
            process_id: _,
            stdin,
        } = event;

        let stdin = format!("\n{stdin}\n");
        // Stream output bytes to the display-only terminal via ToolCallUpdate meta.
        if let Some(active_command) = &mut self.active_command
            && *active_command.call_id == call_id
        {
            Self::emit_exec_output_update(client, active_command, &call_id, stdin).await;
        }
    }

    async fn start_web_search(&mut self, client: &SessionClient, call_id: String) {
        self.active_web_search = Some(call_id.clone());
        client
            .send_tool_call(
                ToolCall::new(call_id.clone(), "Searching the Web").kind(ToolKind::Fetch),
            )
            .await;
        self.mark_tool_call_started(client, &call_id, "web_search");
    }

    async fn update_web_search_query(
        &self,
        client: &SessionClient,
        call_id: String,
        query: String,
    ) {
        client
            .send_tool_call_update(ToolCallUpdate::new(
                call_id,
                ToolCallUpdateFields::new()
                    .status(ToolCallStatus::InProgress)
                    .title(format!("Searching for: {query}"))
                    .raw_input(serde_json::json!({
                        "query": query
                    })),
            ))
            .await;
    }

    async fn complete_web_search(&mut self, client: &SessionClient) {
        if let Some(call_id) = self.active_web_search.take() {
            let kind = self
                .open_tool_calls
                .get(&call_id)
                .map(|open| open.kind)
                .unwrap_or("web_search");
            self.mark_tool_exec_finished(
                client,
                &call_id,
                kind,
                ToolCallStatus::Completed,
                None,
                None,
            );
            client
                .send_tool_call_update(ToolCallUpdate::new(
                    call_id.clone(),
                    ToolCallUpdateFields::new().status(ToolCallStatus::Completed),
                ))
                .await;
            self.open_tool_calls.remove(&call_id);
            self.mark_tool_result_sent(client, &call_id, ToolCallStatus::Completed, None);
        }
    }
}

struct ParseCommandToolCall {
    title: String,
    file_extension: Option<String>,
    terminal_output: bool,
    locations: Vec<ToolCallLocation>,
    kind: ToolKind,
}

fn parse_command_tool_call(parsed_cmd: Vec<ParsedCommand>, cwd: &Path) -> ParseCommandToolCall {
    let mut titles = Vec::new();
    let mut locations = Vec::new();
    let mut file_extension = None;
    let mut terminal_output = false;
    let mut kind = ToolKind::Execute;

    for cmd in parsed_cmd {
        let mut cmd_path = None;
        match cmd {
            ParsedCommand::Read { cmd: _, name, path } => {
                titles.push(format!("Read {name}"));
                file_extension = path
                    .extension()
                    .map(|ext| ext.to_string_lossy().to_string());
                cmd_path = Some(path);
                kind = ToolKind::Read;
            }
            ParsedCommand::ListFiles { cmd: _, path } => {
                let dir = if let Some(path) = path.as_ref() {
                    &cwd.join(path)
                } else {
                    cwd
                };
                titles.push(format!("List {}", dir.display()));
                cmd_path = path.map(PathBuf::from);
                kind = ToolKind::Search;
            }
            ParsedCommand::Search { cmd, query, path } => {
                titles.push(match (query, path.as_ref()) {
                    (Some(query), Some(path)) => format!("Search {query} in {path}"),
                    (Some(query), None) => format!("Search {query}"),
                    _ => format!("Search {cmd}"),
                });
                kind = ToolKind::Search;
            }
            ParsedCommand::Unknown { cmd } => {
                titles.push(format!("Run {cmd}"));
                terminal_output = true;
            }
        }

        if let Some(path) = cmd_path {
            locations.push(ToolCallLocation::new(if path.is_relative() {
                cwd.join(&path)
            } else {
                path
            }));
        }
    }

    ParseCommandToolCall {
        title: titles.join(", "),
        file_extension,
        terminal_output,
        locations,
        kind,
    }
}

struct TaskState {
    prompt: PromptState,
}

impl TaskState {
    fn new(
        thread: Arc<dyn CodexThreadImpl>,
        response_tx: oneshot::Sender<Result<StopReason, Error>>,
        submission_id: String,
    ) -> Self {
        Self {
            prompt: PromptState::new(thread, response_tx, submission_id),
        }
    }

    fn new_background(thread: Arc<dyn CodexThreadImpl>, submission_id: String) -> Self {
        Self {
            prompt: PromptState::new_background(thread, submission_id),
        }
    }

    fn is_active(&self) -> bool {
        self.prompt.is_active()
    }

    fn longest_open_tool_call_runtime(&self) -> Option<OpenToolCallRuntime> {
        self.prompt.longest_open_tool_call_runtime()
    }

    async fn handle_event(&mut self, client: &SessionClient, event: EventMsg) {
        self.prompt.handle_event(client, event).await;
    }

    async fn handle_watchdog_tick(&mut self, client: &SessionClient) {
        self.prompt.handle_watchdog_tick(client).await;
    }

    async fn fail_open_tool_calls(&mut self, client: &SessionClient, reason: &str) {
        self.prompt.fail_open_tool_calls(client, reason).await;
    }

    async fn force_cancelled(&mut self, client: &SessionClient, reason: &str) {
        self.prompt.force_cancelled(client, reason).await;
    }
}

#[derive(Clone)]
struct SessionClient {
    session_id: SessionId,
    client: Arc<dyn Client>,
    client_capabilities: Arc<Mutex<ClientCapabilities>>,
    session_store: Option<SessionStore>,
    ui_visibility_mode: UiVisibilityMode,
    diagnostics: Arc<RuntimeDiagnosticsState>,
}

impl SessionClient {
    fn new(
        session_id: SessionId,
        client_capabilities: Arc<Mutex<ClientCapabilities>>,
        session_store: Option<SessionStore>,
    ) -> Self {
        Self {
            session_id,
            client: ACP_CLIENT.get().expect("Client should be set").clone(),
            client_capabilities,
            session_store,
            ui_visibility_mode: UiVisibilityMode::from_env(),
            diagnostics: Arc::new(RuntimeDiagnosticsState::new()),
        }
    }

    #[cfg(test)]
    fn with_client(
        session_id: SessionId,
        client: Arc<dyn Client>,
        client_capabilities: Arc<Mutex<ClientCapabilities>>,
        session_store: Option<SessionStore>,
    ) -> Self {
        Self::with_client_and_visibility(
            session_id,
            client,
            client_capabilities,
            session_store,
            UiVisibilityMode::from_env(),
        )
    }

    #[cfg(test)]
    fn with_client_and_visibility(
        session_id: SessionId,
        client: Arc<dyn Client>,
        client_capabilities: Arc<Mutex<ClientCapabilities>>,
        session_store: Option<SessionStore>,
        ui_visibility_mode: UiVisibilityMode,
    ) -> Self {
        Self {
            session_id,
            client,
            client_capabilities,
            session_store,
            ui_visibility_mode,
            diagnostics: Arc::new(RuntimeDiagnosticsState::new()),
        }
    }

    fn log_canonical(&self, kind: &str, data: serde_json::Value) {
        if let Some(store) = self.session_store.as_ref() {
            store.log(kind, data);
        }
    }

    fn ui_visibility_mode(&self) -> UiVisibilityMode {
        self.ui_visibility_mode
    }

    fn hides_internal_updates(&self) -> bool {
        self.ui_visibility_mode.hides_internal_updates()
    }

    fn is_zed_client(&self) -> bool {
        current_client_info()
            .as_deref()
            .is_some_and(|info| info.to_ascii_lowercase().contains("zed"))
    }

    fn plan_status_counts(plan: &[PlanItemArg]) -> (usize, usize, usize, usize) {
        let pending = plan
            .iter()
            .filter(|item| matches!(item.status, StepStatus::Pending))
            .count();
        let in_progress = plan
            .iter()
            .filter(|item| matches!(item.status, StepStatus::InProgress))
            .count();
        let completed = plan
            .iter()
            .filter(|item| matches!(item.status, StepStatus::Completed))
            .count();
        (completed, in_progress, pending, plan.len())
    }

    fn zed_plan_progress_entry(&self, plan: &[PlanItemArg]) -> Option<PlanEntry> {
        if !self.is_zed_client() || plan.is_empty() {
            return None;
        }

        let (completed, in_progress, pending, total) = Self::plan_status_counts(plan);
        let percent = if total == 0 {
            0
        } else {
            completed.saturating_mul(100) / total
        };
        let progress_bar = FlowVectorState::render_progress_bar(
            completed,
            total,
            monitor_progress_bar_width(monitor_panel_width()),
        );
        let status = if completed == total {
            PlanEntryStatus::Completed
        } else if completed > 0 || in_progress > 0 {
            PlanEntryStatus::InProgress
        } else {
            PlanEntryStatus::Pending
        };

        Some(PlanEntry::new(
            format!(
                "Progress: {progress_bar} {percent}% ({completed}/{total} completed, {in_progress} in_progress, {pending} pending)"
            ),
            PlanEntryPriority::Medium,
            status,
        ))
    }

    fn non_zed_plan_progress_text(
        &self,
        plan: &[PlanItemArg],
        explanation: Option<&str>,
    ) -> Option<String> {
        if self.is_zed_client() || plan.is_empty() {
            return None;
        }

        let (completed, in_progress, pending, total) = Self::plan_status_counts(plan);
        let mut lines = vec![format!(
            "Plan update: {completed}/{total} completed, {in_progress} in progress, {pending} pending."
        )];

        if let Some(current_step) = plan.iter().find_map(|item| {
            matches!(item.status, StepStatus::InProgress).then_some(item.step.as_str())
        }) {
            lines.push(format!("Current: {current_step}"));
        }

        if let Some(explanation) = explanation
            .map(str::trim)
            .filter(|explanation| !explanation.is_empty())
        {
            lines.push(format!("Note: {explanation}"));
        }

        Some(lines.join("\n"))
    }

    fn runtime_diagnostics_snapshot(
        &self,
        reason: impl Into<String>,
        active_tasks: usize,
    ) -> RuntimeDiagnosticsSnapshot {
        RuntimeDiagnosticsSnapshot {
            reason: reason.into(),
            client_info: current_client_info(),
            ui_visibility_mode: self.ui_visibility_mode.as_config_value().to_string(),
            uptime_secs: self.diagnostics.started_at.elapsed().as_secs(),
            active_tasks,
            notifications_sent: self.diagnostics.notifications_sent.load(Ordering::Relaxed),
            notification_errors: self.diagnostics.notification_errors.load(Ordering::Relaxed),
            current_rss_bytes: current_process_rss_bytes(),
            user_message_chunks: self.diagnostics.user_message_chunks.load(Ordering::Relaxed),
            user_message_chars: self.diagnostics.user_message_chars.load(Ordering::Relaxed),
            agent_message_chunks: self
                .diagnostics
                .agent_message_chunks
                .load(Ordering::Relaxed),
            agent_message_chars: self.diagnostics.agent_message_chars.load(Ordering::Relaxed),
            agent_thought_chunks: self
                .diagnostics
                .agent_thought_chunks
                .load(Ordering::Relaxed),
            agent_thought_chars: self.diagnostics.agent_thought_chars.load(Ordering::Relaxed),
            tool_calls: self.diagnostics.tool_calls.load(Ordering::Relaxed),
            tool_call_payload_chars: self
                .diagnostics
                .tool_call_payload_chars
                .load(Ordering::Relaxed),
            tool_call_updates: self.diagnostics.tool_call_updates.load(Ordering::Relaxed),
            tool_call_update_payload_chars: self
                .diagnostics
                .tool_call_update_payload_chars
                .load(Ordering::Relaxed),
            plan_updates: self.diagnostics.plan_updates.load(Ordering::Relaxed),
            permission_requests: self.diagnostics.permission_requests.load(Ordering::Relaxed),
        }
    }

    fn log_runtime_diagnostics(&self, reason: impl Into<String>, active_tasks: usize) {
        let snapshot = self.runtime_diagnostics_snapshot(reason, active_tasks);
        let value = serde_json::to_value(snapshot).unwrap_or_else(|_| {
            json!({
                "reason": "serialize_failed"
            })
        });
        self.log_canonical("acp.runtime_diagnostics", value);
    }

    fn maybe_log_runtime_diagnostics(&self, trigger: &str) {
        if !diagnostics_auto_log_enabled_from_env() {
            return;
        }

        let every = diagnostics_log_every_from_env();
        let notifications = self.diagnostics.notifications_sent.load(Ordering::Relaxed);
        if notifications < every {
            return;
        }

        let watermark = (notifications / every) * every;
        let previous = self
            .diagnostics
            .auto_log_notification_watermark
            .load(Ordering::Relaxed);
        if watermark <= previous {
            return;
        }

        if self
            .diagnostics
            .auto_log_notification_watermark
            .compare_exchange(previous, watermark, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            self.log_runtime_diagnostics(
                format!("auto_notification_threshold:{trigger}:{watermark}"),
                0,
            );
        }
    }

    fn supports_standard_terminal(&self) -> bool {
        self.client_capabilities.lock().unwrap().terminal
    }

    fn supports_legacy_terminal_output_extension(&self) -> bool {
        self.client_capabilities
            .lock()
            .unwrap()
            .meta
            .as_ref()
            .is_some_and(|v| {
                v.get("terminal_output")
                    .is_some_and(|v| v.as_bool().unwrap_or_default())
            })
    }

    fn supports_embedded_terminal_output(&self, active_command: &ActiveCommand) -> bool {
        active_command.terminal_output && self.supports_legacy_terminal_output_extension()
    }

    async fn send_notification(&self, update: SessionUpdate) {
        self.diagnostics
            .notifications_sent
            .fetch_add(1, Ordering::Relaxed);
        if let Err(e) = self
            .client
            .session_notification(SessionNotification::new(self.session_id.clone(), update))
            .await
        {
            self.diagnostics
                .notification_errors
                .fetch_add(1, Ordering::Relaxed);
            error!("Failed to send session notification: {:?}", e);
        }
        self.maybe_log_runtime_diagnostics("session_update");
    }

    async fn send_user_message(&self, text: impl Into<String>) {
        let text = text.into();
        self.diagnostics
            .user_message_chunks
            .fetch_add(1, Ordering::Relaxed);
        self.diagnostics
            .user_message_chars
            .fetch_add(text.chars().count() as u64, Ordering::Relaxed);
        self.log_canonical("acp.user_message_chunk", json!({ "text": text }));
        self.send_notification(SessionUpdate::UserMessageChunk(ContentChunk::new(
            text.into(),
        )))
        .await;
    }

    async fn send_agent_text(&self, text: impl Into<String>) {
        let text = normalize_outgoing_local_markdown_links(&text.into());
        let max_chars = ui_text_chunk_max_chars_from_env();
        for chunk in split_text_for_ui_chunks(&text, max_chars) {
            self.diagnostics
                .agent_message_chunks
                .fetch_add(1, Ordering::Relaxed);
            self.diagnostics
                .agent_message_chars
                .fetch_add(chunk.chars().count() as u64, Ordering::Relaxed);
            self.log_canonical("acp.agent_message_chunk", json!({ "text": &chunk }));
            self.send_notification(SessionUpdate::AgentMessageChunk(ContentChunk::new(
                chunk.into(),
            )))
            .await;
        }
    }

    async fn send_agent_thought(&self, text: impl Into<String>) {
        let text = text.into();
        self.diagnostics
            .agent_thought_chunks
            .fetch_add(1, Ordering::Relaxed);
        self.diagnostics
            .agent_thought_chars
            .fetch_add(text.chars().count() as u64, Ordering::Relaxed);
        self.log_canonical("acp.agent_thought_chunk", json!({ "text": text }));
        if self.hides_internal_updates() {
            return;
        }
        self.send_notification(SessionUpdate::AgentThoughtChunk(ContentChunk::new(
            text.into(),
        )))
        .await;
    }

    async fn send_tool_call(&self, tool_call: ToolCall) {
        let tool_call = cap_tool_call_payloads_for_ui(tool_call);
        let value = serde_json::to_value(&tool_call).unwrap_or_else(|_| {
            json!({
                "debug": format!("{tool_call:?}")
            })
        });
        self.diagnostics.tool_calls.fetch_add(1, Ordering::Relaxed);
        self.diagnostics
            .tool_call_payload_chars
            .fetch_add(json_payload_chars(&value), Ordering::Relaxed);
        self.log_canonical("acp.tool_call", value);
        if self.hides_internal_updates() {
            return;
        }
        self.send_notification(SessionUpdate::ToolCall(tool_call))
            .await;
    }

    async fn send_tool_call_update(&self, update: ToolCallUpdate) {
        let update = cap_tool_call_update_payloads_for_ui(update);
        let value = serde_json::to_value(&update).unwrap_or_else(|_| {
            json!({
                "debug": format!("{update:?}")
            })
        });
        self.diagnostics
            .tool_call_updates
            .fetch_add(1, Ordering::Relaxed);
        self.diagnostics
            .tool_call_update_payload_chars
            .fetch_add(json_payload_chars(&value), Ordering::Relaxed);
        self.log_canonical("acp.tool_call_update", value);
        if self.hides_internal_updates() {
            return;
        }
        self.send_notification(SessionUpdate::ToolCallUpdate(update))
            .await;
    }

    /// Send a completed tool call (used for replay and simple cases)
    async fn send_completed_tool_call(
        &self,
        call_id: impl Into<ToolCallId>,
        title: impl Into<String>,
        kind: ToolKind,
        raw_input: Option<serde_json::Value>,
    ) {
        let mut tool_call = ToolCall::new(call_id, title)
            .kind(kind)
            .status(ToolCallStatus::Completed);
        if let Some(input) = raw_input {
            tool_call = tool_call.raw_input(input);
        }
        self.send_tool_call(tool_call).await;
    }

    /// Send a tool call completion update (used for replay)
    async fn send_tool_call_completed(
        &self,
        call_id: impl Into<ToolCallId>,
        raw_output: Option<serde_json::Value>,
    ) {
        let mut fields = ToolCallUpdateFields::new().status(ToolCallStatus::Completed);
        if let Some(output) = raw_output {
            fields = fields.raw_output(output);
        }
        self.send_tool_call_update(ToolCallUpdate::new(call_id, fields))
            .await;
    }

    async fn update_plan(&self, plan: Vec<PlanItemArg>, explanation: Option<String>) {
        let progress_text = self.non_zed_plan_progress_text(&plan, explanation.as_deref());
        let mut data = json!({
            "items": plan.iter().map(|p| {
                json!({
                    "step": p.step,
                    "status": match p.status {
                        StepStatus::Pending => "pending",
                        StepStatus::InProgress => "in_progress",
                        StepStatus::Completed => "completed",
                    }
                })
            }).collect::<Vec<_>>(),
        });
        if let Some(explanation) = explanation {
            data["explanation"] = serde_json::Value::String(explanation);
        }
        self.diagnostics
            .plan_updates
            .fetch_add(1, Ordering::Relaxed);
        self.log_canonical("acp.plan", data);
        if self.hides_internal_updates() {
            return;
        }

        let mut entries = Vec::new();
        if let Some(summary) = self.zed_plan_progress_entry(&plan) {
            entries.push(summary);
        }
        entries.extend(plan.into_iter().map(|entry| {
            PlanEntry::new(
                entry.step,
                PlanEntryPriority::Medium,
                match entry.status {
                    StepStatus::Pending => PlanEntryStatus::Pending,
                    StepStatus::InProgress => PlanEntryStatus::InProgress,
                    StepStatus::Completed => PlanEntryStatus::Completed,
                },
            )
        }));

        self.send_notification(SessionUpdate::Plan(Plan::new(entries)))
            .await;

        if let Some(progress_text) = progress_text {
            self.send_agent_text(progress_text).await;
        }
    }

    async fn request_permission(
        &self,
        tool_call: ToolCallUpdate,
        options: Vec<PermissionOption>,
    ) -> Result<RequestPermissionResponse, Error> {
        self.diagnostics
            .permission_requests
            .fetch_add(1, Ordering::Relaxed);
        self.log_canonical(
            "acp.request_permission",
            json!({
                "tool_call": serde_json::to_value(&tool_call).unwrap_or_else(|_| json!({"debug": format!("{tool_call:?}")})),
                "options": serde_json::to_value(&options).unwrap_or_else(|_| json!({"debug": format!("{options:?}")})),
            }),
        );

        let resp = self
            .client
            .request_permission(RequestPermissionRequest::new(
                self.session_id.clone(),
                tool_call,
                options,
            ))
            .await?;

        self.log_canonical(
            "acp.request_permission_response",
            json!({
                "outcome": serde_json::to_value(&resp.outcome).unwrap_or_else(|_| json!({"debug": format!("{:?}", resp.outcome)})),
            }),
        );

        Ok(resp)
    }
}

struct ThreadActor<A> {
    /// Allows for logging out from slash commands
    auth: A,
    /// Used for sending messages back to the client.
    client: SessionClient,
    /// The thread associated with this task.
    thread: Arc<dyn CodexThreadImpl>,
    /// The configuration for the thread.
    config: Config,
    /// The custom prompts loaded for this workspace.
    custom_prompts: Rc<RefCell<Vec<CustomPrompt>>>,
    /// The models available for this thread.
    models_manager: Arc<dyn ModelsManagerImpl>,
    /// A sender for each interested `Op` submission that needs events routed.
    submissions: HashMap<String, SubmissionState>,
    /// A receiver for incoming thread messages.
    message_rx: mpsc::UnboundedReceiver<ThreadMessage>,
    /// Last config options state we emitted to the client, used for deduping updates.
    last_sent_config_options: Option<Vec<SessionConfigOption>>,
    /// Cached session list for /load lookups.
    last_session_list: Vec<SessionListEntry>,
    /// Context window optimization and auto-compaction controls.
    context_optimization: ContextOptimizationState,
    /// Task-level monitoring defaults and orchestration behavior.
    task_monitoring: TaskMonitoringState,
    /// Selected advanced config panel for narrow client widths.
    advanced_options_panel: AdvancedOptionsPanel,
    /// Setup wizard becomes "live" after `/setup` is first invoked.
    setup_wizard_active: bool,
    /// Tracks setup verification checks to keep Plan progress accurate.
    setup_wizard_progress: SetupWizardProgressState,
    /// Thread monitoring state for plan/progress and flow-direction UX.
    flow_vector: FlowVectorState,
}

impl<A: Auth> ThreadActor<A> {
    fn codex_work_orchestration_profile(&self) -> WorkOrchestrationProfile {
        BackendKind::Codex.work_orchestration_profile()
    }

    fn new(
        auth: A,
        client: SessionClient,
        thread: Arc<dyn CodexThreadImpl>,
        models_manager: Arc<dyn ModelsManagerImpl>,
        config: Config,
        message_rx: mpsc::UnboundedReceiver<ThreadMessage>,
    ) -> Self {
        Self {
            auth,
            client,
            thread,
            config,
            custom_prompts: Rc::default(),
            models_manager,
            submissions: HashMap::new(),
            message_rx,
            last_sent_config_options: None,
            last_session_list: Vec::new(),
            context_optimization: ContextOptimizationState::default(),
            task_monitoring: TaskMonitoringState::default(),
            advanced_options_panel: AdvancedOptionsPanel::Context,
            setup_wizard_active: false,
            setup_wizard_progress: SetupWizardProgressState::default(),
            flow_vector: FlowVectorState::default(),
        }
    }

    async fn spawn(mut self) {
        let mut watchdog_tick =
            tokio::time::interval(Duration::from_secs(TOOL_CALL_WATCHDOG_TICK_SECONDS));
        watchdog_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                message = self.message_rx.recv() => match message {
                    Some(message) => self.handle_message(message).await,
                    None => break,
                },
                event = self.thread.next_event() => match event {
                    Ok(event) => self.handle_event(event).await,
                    Err(e) => {
                        error!("Error getting next event: {:?}", e);
                        break;
                    }
                },
                _ = watchdog_tick.tick() => self.handle_watchdog_tick().await,
            }
            // Litter collection of senders with no receivers
            self.submissions
                .retain(|_, submission| submission.is_active());
        }
    }

    async fn handle_watchdog_tick(&mut self) {
        for submission in self.submissions.values_mut() {
            submission.handle_watchdog_tick(&self.client).await;
        }
    }

    async fn fail_open_tool_calls(&mut self, reason: &str) {
        for submission in self.submissions.values_mut() {
            submission.fail_open_tool_calls(&self.client, reason).await;
        }
    }

    async fn preempt_active_runs_before_prompt(&mut self) -> Result<(), Error> {
        if !self.task_monitoring.preempt_on_new_prompt {
            return Ok(());
        }

        let active_submission_ids = self
            .submissions
            .iter()
            .filter_map(|(submission_id, state)| {
                (state.monitor_label().is_some() && state.is_active())
                    .then_some(submission_id.clone())
            })
            .collect::<Vec<_>>();
        if active_submission_ids.is_empty() {
            return Ok(());
        }

        self.client.log_canonical(
            "acp.bridge.preempt_before_prompt",
            json!({
                "active_submission_ids": active_submission_ids.clone(),
            }),
        );

        self.handle_cancel().await?;
        self.fail_open_tool_calls("preempted_by_new_prompt").await;

        for submission_id in &active_submission_ids {
            if let Some(mut state) = self.submissions.remove(submission_id) {
                state
                    .force_cancelled(&self.client, "preempted_by_new_prompt")
                    .await;
            }
        }

        if self
            .context_optimization
            .auto_compact_submission_id
            .as_ref()
            .is_some_and(|id| {
                active_submission_ids
                    .iter()
                    .any(|active_id| active_id == id)
            })
        {
            self.context_optimization.auto_compact_submission_id = None;
        }

        Ok(())
    }

    async fn handle_message(&mut self, message: ThreadMessage) {
        match message {
            ThreadMessage::Load { response_tx } => {
                let result = self.handle_load().await;
                drop(response_tx.send(result));
                let client = self.client.clone();
                let mut available_commands = Self::builtin_commands();
                let load_custom_prompts = self.load_custom_prompts().await;
                let custom_prompts = self.custom_prompts.clone();

                // Have this happen after the session is loaded by putting it
                // in a separate task
                tokio::task::spawn_local(async move {
                    let mut new_custom_prompts = load_custom_prompts
                        .await
                        .map_err(|_| Error::internal_error())
                        .flatten()
                        .inspect_err(|e| error!("Failed to load custom prompts {e:?}"))
                        .unwrap_or_default();

                    for prompt in &new_custom_prompts {
                        available_commands.push(
                            AvailableCommand::new(
                                prompt.name.clone(),
                                prompt.description.clone().unwrap_or_default(),
                            )
                            .input(prompt.argument_hint.as_ref().map(
                                |hint| {
                                    AvailableCommandInput::Unstructured(
                                        UnstructuredCommandInput::new(hint.clone()),
                                    )
                                },
                            )),
                        );
                    }
                    std::mem::swap(
                        custom_prompts.borrow_mut().deref_mut(),
                        &mut new_custom_prompts,
                    );

                    client
                        .send_notification(SessionUpdate::AvailableCommandsUpdate(
                            AvailableCommandsUpdate::new(available_commands),
                        ))
                        .await;
                });
                if let Err(err) = self.handle_sessions_command().await {
                    error!("Failed to list sessions: {err:?}");
                }
            }
            ThreadMessage::GetConfigOptions { response_tx } => {
                let result = self.config_options().await;
                drop(response_tx.send(result));
            }
            ThreadMessage::Prompt {
                request,
                response_tx,
            } => {
                let result = self.handle_prompt(request).await;
                drop(response_tx.send(result));
            }
            ThreadMessage::SetMode { mode, response_tx } => {
                let result = self.handle_set_mode(mode).await;
                let is_ok = result.is_ok();
                drop(response_tx.send(result));
                if is_ok {
                    self.maybe_emit_config_options_update().await;
                    self.maybe_emit_setup_wizard_plan_update(Some(
                        "Setup progress updated from configuration change",
                    ))
                    .await;
                }
            }
            ThreadMessage::SetModel { model, response_tx } => {
                let result = self.handle_set_model(model).await;
                let is_ok = result.is_ok();
                drop(response_tx.send(result));
                if is_ok {
                    self.maybe_emit_config_options_update().await;
                    self.maybe_emit_setup_wizard_plan_update(Some(
                        "Setup progress updated from configuration change",
                    ))
                    .await;
                }
            }
            ThreadMessage::SetConfigOption {
                config_id,
                value,
                response_tx,
            } => {
                let result = self.handle_set_config_option(config_id, value).await;
                let is_ok = result.is_ok();
                drop(response_tx.send(result));
                if is_ok {
                    self.maybe_emit_config_options_update().await;
                    self.maybe_emit_setup_wizard_plan_update(Some(
                        "Setup progress updated from configuration change",
                    ))
                    .await;
                }
            }
            ThreadMessage::Cancel { response_tx } => {
                let result = self.handle_cancel().await;
                self.fail_open_tool_calls("cancel_requested").await;
                drop(response_tx.send(result));
            }
            ThreadMessage::ReplayHistory {
                history,
                response_tx,
            } => {
                let result = self.handle_replay_history(history).await;
                drop(response_tx.send(result));
            }
        }
    }

    fn builtin_commands() -> Vec<AvailableCommand> {
        vec![
            // CLI parity: expose common Codex CLI slash commands in ACP clients.
            // Commands that depend on interactive TUI menus are implemented as
            // "show config options" / "print instructions" in this adapter.
            AvailableCommand::new(
                "setup",
                "show a setup wizard for authentication and recommended session settings",
            ),
            AvailableCommand::new("model", "choose what model and reasoning effort to use"),
            AvailableCommand::new("personality", "choose a communication style for responses"),
            AvailableCommand::new("approvals", "choose what Codex can do without approval"),
            AvailableCommand::new("permissions", "choose what Codex is allowed to do"),
            AvailableCommand::new("experimental", "toggle beta features"),
            AvailableCommand::new(
                "skills",
                "use skills to improve how Codex performs specific tasks",
            )
            .input(AvailableCommandInput::Unstructured(
                UnstructuredCommandInput::new(
                    "optional: --enabled | --disabled | --scope <scope> | --reload | <keyword>",
                ),
            )),
            AvailableCommand::new("mcp", "list configured MCP tools"),
            AvailableCommand::new("status", "show current session configuration"),
            AvailableCommand::new(
                "new-window",
                "show instructions for opening a fresh thread window in the client",
            ),
            AvailableCommand::new(
                "monitor",
                "monitor plan progress, execution trace, and context optimization state",
            )
            .input(AvailableCommandInput::Unstructured(
                UnstructuredCommandInput::new("optional: detail | retro"),
            )),
            AvailableCommand::new(
                "vector",
                "show workflow direction minimap and semantic compass",
            ),
            AvailableCommand::new("new", "start a new chat during a conversation"),
            AvailableCommand::new("resume", "resume a saved chat"),
            AvailableCommand::new("fork", "fork the current chat"),
            AvailableCommand::new("diff", "show git diff"),
            AvailableCommand::new("mention", "mention a file"),
            AvailableCommand::new("feedback", "send logs to maintainers"),
            AvailableCommand::new("review", "Review my current changes and find issues").input(
                AvailableCommandInput::Unstructured(UnstructuredCommandInput::new(
                    "optional custom review instructions",
                )),
            ),
            AvailableCommand::new(
                "review-branch",
                "Review the code changes against a specific branch",
            )
            .input(AvailableCommandInput::Unstructured(
                UnstructuredCommandInput::new("branch name"),
            )),
            AvailableCommand::new(
                "review-commit",
                "Review the code changes introduced by a commit",
            )
            .input(AvailableCommandInput::Unstructured(
                UnstructuredCommandInput::new("commit sha"),
            )),
            AvailableCommand::new(
                "init",
                "create an AGENTS.md file with instructions for Codex",
            ),
            AvailableCommand::new(
                "compact",
                "summarize conversation to prevent hitting the context limit",
            ),
            AvailableCommand::new("undo", "undo Codex’s most recent turn"),
            AvailableCommand::new("sessions", "list recent sessions for the current workspace"),
            AvailableCommand::new("load", "show instructions to open a previous session").input(
                AvailableCommandInput::Unstructured(UnstructuredCommandInput::new(
                    "session id or list number",
                )),
            ),
            AvailableCommand::new("logout", "logout of Codex"),
        ]
    }

    async fn load_custom_prompts(&mut self) -> oneshot::Receiver<Result<Vec<CustomPrompt>, Error>> {
        let (response_tx, response_rx) = oneshot::channel();
        let submission_id = match self.thread.submit(Op::ListCustomPrompts).await {
            Ok(id) => id,
            Err(e) => {
                drop(response_tx.send(Err(Error::internal_error().data(e.to_string()))));
                return response_rx;
            }
        };

        self.submissions.insert(
            submission_id,
            SubmissionState::CustomPrompts(CustomPromptsState::new(response_tx)),
        );

        response_rx
    }

    fn modes(&self) -> Option<SessionModeState> {
        let current_mode_id = APPROVAL_PRESETS
            .iter()
            .find(|preset| {
                &preset.approval == self.config.approval_policy.get()
                    && &preset.sandbox == self.config.sandbox_policy.get()
            })
            .map(|preset| SessionModeId::new(preset.id))?;

        Some(SessionModeState::new(
            current_mode_id,
            APPROVAL_PRESETS
                .iter()
                .map(|preset| {
                    SessionMode::new(preset.id, preset.label).description(preset.description)
                })
                .collect(),
        ))
    }

    async fn find_current_model(&self) -> Option<ModelId> {
        let model_presets = self.models_manager.list_models(&self.config).await;
        let config_model = self.get_current_model().await;
        let preset = model_presets
            .iter()
            .find(|preset| preset.model == config_model)?;

        let effort = self
            .config
            .model_reasoning_effort
            .and_then(|effort| {
                preset
                    .supported_reasoning_efforts
                    .iter()
                    .find_map(|e| (e.effort == effort).then_some(effort))
            })
            .unwrap_or(preset.default_reasoning_effort);

        Some(Self::model_id(&preset.id, effort))
    }

    fn model_id(id: &str, effort: ReasoningEffort) -> ModelId {
        ModelId::new(format!("{id}/{effort}"))
    }

    fn parse_model_id(id: &ModelId) -> Option<(String, ReasoningEffort)> {
        let (model, reasoning) = id.0.split_once('/')?;
        let reasoning = serde_json::from_value(reasoning.into()).ok()?;
        Some((model.to_owned(), reasoning))
    }

    async fn config_options(&self) -> Result<Vec<SessionConfigOption>, Error> {
        let mut options = Vec::new();
        let show_advanced_options =
            matches!(ConfigOptionsDensity::from_env(), ConfigOptionsDensity::Full);
        let inline_all_advanced = std::env::var("COLUMNS")
            .ok()
            .and_then(|raw| raw.parse::<usize>().ok())
            .unwrap_or(MONITOR_PANEL_WIDTH_DEFAULT)
            >= CONFIG_OPTIONS_INLINE_FULL_MIN_COLUMNS;

        if let Some(modes) = self.modes() {
            let select_options = modes
                .available_modes
                .into_iter()
                .map(|m| SessionConfigSelectOption::new(m.id.0, m.name).description(m.description))
                .collect::<Vec<_>>();

            options.push(
                SessionConfigOption::select(
                    "mode",
                    "Approval Preset",
                    modes.current_mode_id.0,
                    select_options,
                )
                .category(SessionConfigOptionCategory::Mode)
                .description("Choose an approval and sandboxing preset for your session"),
            );
        }

        let presets = self.models_manager.list_models(&self.config).await;

        let current_model = self.get_current_model().await;
        let current_preset = presets.iter().find(|p| p.model == current_model).cloned();

        let mut model_select_options = Vec::new();

        if current_preset.is_none() {
            // If no preset found, return the current model string as-is
            model_select_options.push(SessionConfigSelectOption::new(
                current_model.clone(),
                current_model.clone(),
            ));
        };

        model_select_options.extend(
            presets
                .into_iter()
                .filter(|model| model.show_in_picker || model.model == current_model)
                .map(|preset| {
                    SessionConfigSelectOption::new(preset.id, preset.display_name)
                        .description(preset.description)
                }),
        );

        options.push(
            SessionConfigOption::select("model", "Model", current_model, model_select_options)
                .category(SessionConfigOptionCategory::Model)
                .description("Choose which model Codex should use"),
        );

        // Reasoning effort selector (only if the current preset exists and has >1 supported effort)
        if let Some(preset) = current_preset
            && preset.supported_reasoning_efforts.len() > 1
        {
            let supported = &preset.supported_reasoning_efforts;

            let current_effort = self
                .config
                .model_reasoning_effort
                .and_then(|effort| {
                    supported
                        .iter()
                        .find_map(|e| (e.effort == effort).then_some(effort))
                })
                .unwrap_or(preset.default_reasoning_effort);

            let effort_select_options = supported
                .iter()
                .map(|e| {
                    SessionConfigSelectOption::new(
                        e.effort.to_string(),
                        e.effort.to_string().to_title_case(),
                    )
                    .description(e.description.clone())
                })
                .collect::<Vec<_>>();

            options.push(
                SessionConfigOption::select(
                    "reasoning_effort",
                    "Reasoning Effort",
                    current_effort.to_string(),
                    effort_select_options,
                )
                .category(SessionConfigOptionCategory::ThoughtLevel)
                .description("Choose how much reasoning effort the model should use"),
            );
        }

        let current_personality = self
            .config
            .model_personality
            .map(|p| p.to_string())
            .unwrap_or_else(|| "auto".to_string());
        let personality_select_options = vec![
            SessionConfigSelectOption::new("auto", "Auto")
                .description("Use the default personality (no override)"),
            SessionConfigSelectOption::new(Personality::Friendly.to_string(), "Friendly"),
            SessionConfigSelectOption::new(Personality::Pragmatic.to_string(), "Pragmatic"),
        ];
        options.push(
            SessionConfigOption::select(
                "personality",
                "Personality",
                current_personality,
                personality_select_options,
            )
            .category(SessionConfigOptionCategory::Other)
            .description("Choose a communication style for responses"),
        );

        if show_advanced_options {
            if !inline_all_advanced {
                options.push(
                    SessionConfigOption::select(
                        "advanced_options_panel",
                        "Advanced Panel",
                        self.advanced_options_panel.as_config_value(),
                        vec![
                            SessionConfigSelectOption::new("context", "Context"),
                            SessionConfigSelectOption::new("tasks", "Tasks"),
                            SessionConfigSelectOption::new("beta", "Beta"),
                        ],
                    )
                    .category(SessionConfigOptionCategory::Other)
                    .description(
                        "Switch which advanced option group is shown in compact-width layouts",
                    ),
                );
            }

            let show_context = inline_all_advanced
                || matches!(self.advanced_options_panel, AdvancedOptionsPanel::Context);
            let show_tasks = inline_all_advanced
                || matches!(self.advanced_options_panel, AdvancedOptionsPanel::Tasks);
            let show_beta = inline_all_advanced
                || matches!(self.advanced_options_panel, AdvancedOptionsPanel::Beta);

            if show_context {
                let context_mode = self.context_optimization.mode.as_config_value();
                options.push(
                    SessionConfigOption::select(
                        "context_optimization_mode",
                        "Context Optimization",
                        context_mode,
                        vec![
                            SessionConfigSelectOption::new("off", "Off")
                                .description("Disable proactive context optimization telemetry/actions"),
                            SessionConfigSelectOption::new("monitor", "Monitor")
                                .description("Collect context telemetry only; no auto-compaction"),
                            SessionConfigSelectOption::new("auto", "Auto").description(
                                "Automatically compact history when context usage crosses threshold",
                            ),
                        ],
                    )
                    .category(SessionConfigOptionCategory::Other)
                    .description("Controls context-window monitoring and automatic optimization behavior"),
                );

                let trigger_options = CONTEXT_OPT_TRIGGER_PERCENT_OPTIONS
                    .iter()
                    .map(|percent| {
                        SessionConfigSelectOption::new(percent.to_string(), format!("{percent}%"))
                            .description(format!(
                                "Trigger automatic context compaction at or above {percent}% usage"
                            ))
                    })
                    .collect::<Vec<_>>();
                options.push(
                    SessionConfigOption::select(
                        "context_optimization_trigger_percent",
                        "Context Trigger Threshold",
                        self.context_optimization.trigger_percent.to_string(),
                        trigger_options,
                    )
                    .category(SessionConfigOptionCategory::Other)
                    .description("Threshold used when Context Optimization is set to Auto"),
                );
            }

            if show_tasks {
                options.push(
                    SessionConfigOption::select(
                        "task_orchestration_mode",
                        "Task Orchestration",
                        self.task_monitoring.orchestration_mode.as_config_value(),
                        vec![
                            SessionConfigSelectOption::new("parallel", "Parallel").description(
                                "Run independent prompt/task flows concurrently (recommended default)",
                            ),
                            SessionConfigSelectOption::new("sequential", "Sequential")
                                .description("Queue prompt/task flows one-by-one"),
                        ],
                    )
                    .category(SessionConfigOptionCategory::Other)
                    .description(
                        "Controls prompt/task orchestration strategy. Codex / ChatGPT evidence-backed default is `parallel` because ACP bridges live plan and tool updates.",
                    ),
                );

                options.push(
                    SessionConfigOption::select(
                        "task_monitoring_enabled",
                        "Task Monitoring",
                        self.task_monitoring.monitor_mode.as_config_value(),
                        vec![
                            SessionConfigSelectOption::new("on", "On").description(
                                "Track active tasks and expose per-task progress in /monitor",
                            ),
                            SessionConfigSelectOption::new("auto", "Auto").description(
                                "Monitor task queue automatically while active tasks exist",
                            ),
                            SessionConfigSelectOption::new("off", "Off")
                                .description("Disable task-level monitor reporting"),
                        ],
                    )
                    .category(SessionConfigOptionCategory::Other)
                    .description(
                        "Enable or disable task-level monitoring. Codex / ChatGPT evidence-backed default is `auto` to keep ACP progress visible while work is active.",
                    ),
                );

                options.push(
                    SessionConfigOption::select(
                        "task_vector_check_enabled",
                        "Progress Vector Checks",
                        if self.task_monitoring.vector_check_enabled {
                            "on"
                        } else {
                            "off"
                        },
                        vec![
                            SessionConfigSelectOption::new("on", "On")
                                .description("Show workflow vector/minimap checks in /monitor"),
                            SessionConfigSelectOption::new("off", "Off")
                                .description("Hide workflow vector checks from monitor output"),
                        ],
                    )
                    .category(SessionConfigOptionCategory::Other)
                    .description(
                        "Enable or disable progress vector checks. Codex / ChatGPT evidence-backed default is `on`.",
                    ),
                );

                options.push(
                    SessionConfigOption::select(
                        "preempt_on_new_prompt",
                        "New Prompt Preemption",
                        if self.task_monitoring.preempt_on_new_prompt {
                            "on"
                        } else {
                            "off"
                        },
                        vec![
                            SessionConfigSelectOption::new("on", "On").description(
                                "Cancel in-flight runs before starting a new prompt (recommended for bridge stability)",
                            ),
                            SessionConfigSelectOption::new("off", "Off").description(
                                "Allow existing runs to continue when a new prompt is submitted",
                            ),
                        ],
                    )
                    .category(SessionConfigOptionCategory::Other)
                    .description(
                        "Controls whether new prompts preempt currently running submissions. Codex / ChatGPT evidence-backed default is `on` for ACP bridge stability.",
                    ),
                );
            }

            if show_beta {
                for spec in experimental_feature_specs() {
                    let current = if self.config.features.enabled(spec.id) {
                        "on"
                    } else {
                        "off"
                    };
                    options.push(
                        SessionConfigOption::select(
                            experimental_feature_config_id(spec.key),
                            format!("Beta: {}", spec.name),
                            current,
                            vec![
                                SessionConfigSelectOption::new("on", "On")
                                    .description("Enable this beta capability"),
                                SessionConfigSelectOption::new("off", "Off")
                                    .description("Disable this beta capability"),
                            ],
                        )
                        .category(SessionConfigOptionCategory::Other)
                        .description(spec.description),
                    );
                }
            }
        }

        Ok(options)
    }

    async fn list_sessions_for_cwd(&self) -> Result<Vec<SessionListEntry>, Error> {
        let page = RolloutRecorder::list_threads(
            &self.config.codex_home,
            SESSION_LIST_PAGE_SIZE,
            None,
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
                let session_meta_line = item.head.first().and_then(|first| {
                    serde_json::from_value::<SessionMetaLine>(first.clone()).ok()
                })?;

                if session_meta_line.meta.cwd != self.config.cwd {
                    return None;
                }

                let mut title = None;
                for value in item.head {
                    if let Ok(response_item) = serde_json::from_value::<ResponseItem>(value)
                        && let Some(turn_item) = parse_turn_item(&response_item)
                        && let TurnItem::UserMessage(user) = turn_item
                    {
                        if let Some(formatted) = format_session_title(&user.message()) {
                            title = Some(formatted);
                        }
                        break;
                    }
                }

                let updated_at = item.updated_at.clone().or(item.created_at.clone());

                Some(SessionListEntry {
                    id: SessionId::new(session_meta_line.meta.id.to_string()),
                    title,
                    updated_at,
                })
            })
            .collect::<Vec<_>>();

        Ok(sessions)
    }

    async fn maybe_emit_config_options_update(&mut self) {
        let config_options = self.config_options().await.unwrap_or_default();

        if self
            .last_sent_config_options
            .as_ref()
            .is_some_and(|prev| prev == &config_options)
        {
            return;
        }

        self.last_sent_config_options = Some(config_options.clone());

        self.client
            .send_notification(SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(
                config_options,
            )))
            .await;
    }

    async fn maybe_emit_setup_wizard_plan_update(&mut self, explanation: Option<&str>) {
        if !self.setup_wizard_active {
            return;
        }
        let plan = self.setup_wizard_plan_items();
        let explanation = explanation.map(ToOwned::to_owned);
        self.flow_vector
            .record_plan_update(explanation.clone(), plan.as_slice());
        self.client.update_plan(plan, explanation).await;
    }

    async fn handle_set_config_option(
        &mut self,
        config_id: SessionConfigId,
        value: SessionConfigValueId,
    ) -> Result<(), Error> {
        let raw_config_id = config_id.0.to_string();
        match raw_config_id.as_str() {
            "mode" => self.handle_set_mode(SessionModeId::new(value.0)).await,
            "model" => self.handle_set_config_model(value).await,
            "reasoning_effort" => self.handle_set_config_reasoning_effort(value).await,
            "personality" => self.handle_set_config_personality(value).await,
            "advanced_options_panel" => self.handle_set_advanced_options_panel(value).await,
            "context_optimization_mode" => self.handle_set_context_optimization_mode(value).await,
            "context_optimization_trigger_percent" => {
                self.handle_set_context_optimization_trigger(value).await
            }
            "task_orchestration_mode" => self.handle_set_task_orchestration_mode(value).await,
            "task_monitoring_enabled" => self.handle_set_task_monitoring_mode(value).await,
            "task_vector_check_enabled" => self.handle_set_task_vector_check_enabled(value).await,
            "preempt_on_new_prompt" => self.handle_set_preempt_on_new_prompt(value).await,
            _ if parse_experimental_feature_config_id(&raw_config_id).is_some() => {
                let spec = parse_experimental_feature_config_id(&raw_config_id)
                    .ok_or_else(|| Error::invalid_params().data("Unsupported beta feature"))?;
                self.handle_set_beta_feature(spec, value).await
            }
            _ => Err(Error::invalid_params().data("Unsupported config option")),
        }
    }

    async fn handle_set_config_model(&mut self, value: SessionConfigValueId) -> Result<(), Error> {
        let model_id = value.0;

        let presets = self.models_manager.list_models(&self.config).await;
        let preset = presets.iter().find(|p| p.id.as_str() == &*model_id);

        let model_to_use = preset
            .map(|p| p.model.clone())
            .unwrap_or_else(|| model_id.to_string());

        if model_to_use.is_empty() {
            return Err(Error::invalid_params().data("No model selected"));
        }

        let effort_to_use = if let Some(preset) = preset {
            if let Some(effort) = self.config.model_reasoning_effort
                && preset
                    .supported_reasoning_efforts
                    .iter()
                    .any(|e| e.effort == effort)
            {
                Some(effort)
            } else {
                Some(preset.default_reasoning_effort)
            }
        } else {
            // If the user selected a raw model string (not a known preset), don't invent a default.
            // Keep whatever was previously configured (or leave unset) so Codex can decide.
            self.config.model_reasoning_effort
        };

        self.thread
            .submit(Op::OverrideTurnContext {
                cwd: None,
                approval_policy: None,
                sandbox_policy: None,
                model: Some(model_to_use.clone()),
                effort: Some(effort_to_use),
                summary: None,
                collaboration_mode: None,
                personality: None,
            })
            .await
            .map_err(|e| Error::from(anyhow::anyhow!(e)))?;

        self.config.model = Some(model_to_use);
        self.config.model_reasoning_effort = effort_to_use;

        Ok(())
    }

    async fn handle_set_config_reasoning_effort(
        &mut self,
        value: SessionConfigValueId,
    ) -> Result<(), Error> {
        let effort: ReasoningEffort =
            serde_json::from_value(value.0.as_ref().into()).map_err(|_| Error::invalid_params())?;

        let current_model = self.get_current_model().await;
        let presets = self.models_manager.list_models(&self.config).await;
        let Some(preset) = presets.iter().find(|p| p.model == current_model) else {
            return Err(Error::invalid_params()
                .data("Reasoning effort can only be set for known model presets"));
        };

        if !preset
            .supported_reasoning_efforts
            .iter()
            .any(|e| e.effort == effort)
        {
            return Err(
                Error::invalid_params().data("Unsupported reasoning effort for selected model")
            );
        }

        self.thread
            .submit(Op::OverrideTurnContext {
                cwd: None,
                approval_policy: None,
                sandbox_policy: None,
                model: None,
                effort: Some(Some(effort)),
                summary: None,
                collaboration_mode: None,
                personality: None,
            })
            .await
            .map_err(|e| Error::from(anyhow::anyhow!(e)))?;

        self.config.model_reasoning_effort = Some(effort);

        Ok(())
    }

    async fn handle_set_config_personality(
        &mut self,
        value: SessionConfigValueId,
    ) -> Result<(), Error> {
        let raw = value.0;
        if raw.as_ref() == "auto" {
            // Best-effort: protocol doesn't support clearing personality overrides via
            // OverrideTurnContext, so we only clear our local config state.
            self.config.model_personality = None;
            return Ok(());
        }

        let personality: Personality =
            serde_json::from_value(raw.as_ref().into()).map_err(|_| Error::invalid_params())?;

        self.thread
            .submit(Op::OverrideTurnContext {
                cwd: None,
                approval_policy: None,
                sandbox_policy: None,
                model: None,
                effort: None,
                summary: None,
                collaboration_mode: None,
                personality: Some(personality),
            })
            .await
            .map_err(|e| Error::from(anyhow::anyhow!(e)))?;

        self.config.model_personality = Some(personality);

        Ok(())
    }

    async fn handle_set_advanced_options_panel(
        &mut self,
        value: SessionConfigValueId,
    ) -> Result<(), Error> {
        let panel = AdvancedOptionsPanel::from_config_value(value.0.as_ref())?;
        self.advanced_options_panel = panel;
        Ok(())
    }

    async fn handle_set_context_optimization_mode(
        &mut self,
        value: SessionConfigValueId,
    ) -> Result<(), Error> {
        let mode = ContextOptimizationMode::from_config_value(value.0.as_ref())?;
        self.context_optimization.mode = mode;

        self.client.log_canonical(
            "acp.context_opt.mode",
            json!({
                "mode": mode.as_config_value(),
            }),
        );

        Ok(())
    }

    async fn handle_set_context_optimization_trigger(
        &mut self,
        value: SessionConfigValueId,
    ) -> Result<(), Error> {
        let parsed = value
            .0
            .as_ref()
            .parse::<i64>()
            .map_err(|_| Error::invalid_params().data("Trigger threshold must be a number"))?;

        if !CONTEXT_OPT_TRIGGER_PERCENT_OPTIONS.contains(&parsed) {
            return Err(Error::invalid_params().data(format!(
                "Unsupported threshold. Allowed values: {:?}",
                CONTEXT_OPT_TRIGGER_PERCENT_OPTIONS
            )));
        }

        self.context_optimization.trigger_percent = parsed;
        self.client.log_canonical(
            "acp.context_opt.trigger_percent",
            json!({
                "trigger_percent": parsed,
            }),
        );
        Ok(())
    }

    async fn handle_set_task_orchestration_mode(
        &mut self,
        value: SessionConfigValueId,
    ) -> Result<(), Error> {
        let mode = TaskOrchestrationMode::from_config_value(value.0.as_ref())?;
        self.task_monitoring.orchestration_mode = mode;
        self.client.log_canonical(
            "acp.task_monitoring.orchestration_mode",
            json!({
                "mode": mode.as_config_value(),
            }),
        );
        Ok(())
    }

    async fn handle_set_task_monitoring_mode(
        &mut self,
        value: SessionConfigValueId,
    ) -> Result<(), Error> {
        let monitor_mode = TaskMonitoringMode::from_config_value(value.0.as_ref())?;
        self.task_monitoring.monitor_mode = monitor_mode;
        self.client.log_canonical(
            "acp.task_monitoring.mode",
            json!({
                "mode": monitor_mode.as_config_value(),
            }),
        );
        Ok(())
    }

    async fn handle_set_task_vector_check_enabled(
        &mut self,
        value: SessionConfigValueId,
    ) -> Result<(), Error> {
        let enabled = parse_on_off_toggle(value.0.as_ref(), "Progress Vector Checks")?;
        self.task_monitoring.vector_check_enabled = enabled;
        self.client.log_canonical(
            "acp.task_monitoring.vector_checks",
            json!({
                "enabled": enabled,
            }),
        );
        Ok(())
    }

    async fn handle_set_preempt_on_new_prompt(
        &mut self,
        value: SessionConfigValueId,
    ) -> Result<(), Error> {
        let enabled = parse_on_off_toggle(value.0.as_ref(), "New Prompt Preemption")?;
        self.task_monitoring.preempt_on_new_prompt = enabled;
        self.client.log_canonical(
            "acp.task_monitoring.preempt_on_new_prompt",
            json!({
                "enabled": enabled,
            }),
        );
        Ok(())
    }

    async fn handle_set_beta_feature(
        &mut self,
        spec: ExperimentalFeatureSpec,
        value: SessionConfigValueId,
    ) -> Result<(), Error> {
        let enabled = match value.0.as_ref() {
            "on" => true,
            "off" => false,
            _ => {
                return Err(
                    Error::invalid_params().data("Beta feature values must be one of: on, off")
                );
            }
        };

        let mut builder = ConfigEditsBuilder::new(&self.config.codex_home)
            .with_profile(self.config.active_profile.as_deref());

        if enabled {
            self.config.features.enable(spec.id);
            builder = builder.set_feature_enabled(spec.key, true);
        } else {
            self.config.features.disable(spec.id);
            if spec.default_enabled {
                builder = builder.set_feature_enabled(spec.key, false);
            } else {
                builder = builder.with_edits(vec![ConfigEdit::ClearPath {
                    segments: vec!["features".to_string(), spec.key.to_string()],
                }]);
            }
        }

        if let Err(err) = builder.apply().await {
            return Err(Error::internal_error().data(format!(
                "failed to persist beta feature '{}': {err}",
                spec.key
            )));
        }

        self.client.log_canonical(
            "acp.beta_feature",
            json!({
                "feature": spec.key,
                "enabled": enabled,
            }),
        );

        Ok(())
    }

    fn render_experimental_features_message(&self) -> String {
        let specs = experimental_feature_specs();
        if specs.is_empty() {
            return "No experimental features are currently exposed for ACP.".to_string();
        }

        let mut lines = Vec::new();
        lines.push("Experimental features (ACP)".to_string());
        lines.push("Toggle these in Config Options (`Beta: ...`).".to_string());
        lines.push(String::new());
        for spec in specs {
            let state = if self.config.features.enabled(spec.id) {
                "on"
            } else {
                "off"
            };
            lines.push(format!("- {}: {} ({})", spec.name, state, spec.description));
        }
        lines.join("\n")
    }

    fn render_setup_wizard_message(&self) -> String {
        let profile = self.codex_work_orchestration_profile();
        let mut lines = Vec::new();
        lines.push("xsfire-camp setup wizard".to_string());
        lines.push("(See the Plan panel for the step-by-step checklist.)".to_string());
        lines.push(String::new());
        lines.push("0) Default execution protocol (all use cases)".to_string());
        lines.push("- Lock Goal (one sentence with verifiable done criteria).".to_string());
        lines.push("- Define Rubric: Must (with evidence) / Should (quality).".to_string());
        lines.push(
            "- Work-orchestration sequence: R->P->M->W->A (Root -> Phase -> Milestone -> Work package -> Action).".to_string(),
        );
        lines.push(
            "- Execution loop: Research -> Rubric -> Plan -> Implement -> Verify -> Score."
                .to_string(),
        );
        lines.push(
            "- Keep iterating until Must reaches 100%, keep Plan UI updated each iteration, and leave one ACP-visible action artifact per pass."
                .to_string(),
        );
        lines.push(String::new());
        lines.push("1) Authentication".to_string());
        lines.push(
            "- In most ACP clients (e.g., Zed), authentication happens via the client UI when the agent reports `auth_required`."
                .to_string(),
        );
        lines.push(
            "- Available methods: ChatGPT login (browser), or API key via environment variables."
                .to_string(),
        );
        lines.push(String::new());
        lines.push("2) Recommended Config Options (in your client UI)".to_string());
        lines.push("- Model + Reasoning Effort".to_string());
        lines.push("- Approval Preset (approvals/permissions)".to_string());
        lines.push("- Personality (optional)".to_string());
        lines.push(String::new());
        lines.push("3) Beta features".to_string());
        lines.push("- Toggle in Config Options: `Beta: ...`".to_string());
        lines.push(String::new());
        lines.push("4) Context window optimization".to_string());
        lines.push(format!(
            "- Current mode: {} (trigger {}%)",
            self.context_optimization.mode.as_config_value(),
            self.context_optimization.trigger_percent
        ));
        lines
            .push("- Use `/monitor` to observe token usage estimates + plan progress.".to_string());
        lines.push(
            "- Context Optimization defaults to `Monitor` (telemetry only). Set it to `Auto` if you want auto-compaction."
                .to_string(),
        );
        lines.push(String::new());
        lines.push("5) UX helpers".to_string());
        lines.push(format!(
            "- Codex / ChatGPT evidence: {}",
            profile.evidence_summary
        ));
        lines.push(format!(
            "- Task orchestration default: `{}`",
            self.task_monitoring.orchestration_mode.as_config_value()
        ));
        lines.push(format!(
            "- Task monitoring default: `{}`",
            self.task_monitoring.monitor_mode.as_config_value()
        ));
        lines.push(format!(
            "- Progress vector checks default: `{}`",
            if self.task_monitoring.vector_check_enabled {
                "on"
            } else {
                "off"
            }
        ));
        lines.push(format!(
            "- New prompt preemption default: `{}`",
            if self.task_monitoring.preempt_on_new_prompt {
                "on"
            } else {
                "off"
            }
        ));
        lines.push("- `/monitor detail`: progress + trace + context telemetry".to_string());
        lines.push("- `/monitor retro`: 회고형 상태 보고서(레인/진행률/리스크/다음)".to_string());
        lines.push("- `/vector`: workflow minimap + semantic compass".to_string());
        lines.push("- `/new-window`: how to open a fresh thread in your client".to_string());
        lines.push(String::new());
        lines.push("Next actions".to_string());
        lines.push(format!(
            "- Verification progress: {}/{}, {}%",
            self.setup_wizard_progress.completed_count(),
            SetupWizardProgressState::TOTAL_VERIFICATION_STEPS,
            self.setup_wizard_progress.progress_percent()
        ));
        lines.push("- Run: `/status`".to_string());
        lines.push("- Run: `/monitor`".to_string());
        lines.push("- Run: `/vector`".to_string());
        lines.push("- If needed: open Config Options and adjust settings above.".to_string());
        lines.join("\n")
    }

    fn setup_wizard_plan_items(&self) -> Vec<PlanItemArg> {
        let mut items = Vec::new();
        let profile = self.codex_work_orchestration_profile();

        items.push(PlanItemArg {
            step: "Protocol: Goal -> Rubric(Must/Should+Evidence) locked".to_string(),
            status: StepStatus::Completed,
        });

        items.push(PlanItemArg {
            step: format!(
                "Loop gate: iterate Research -> Rubric -> Plan -> Implement -> Verify -> Score until Must=100% ({}/{}, {}%)",
                self.setup_wizard_progress.completed_count(),
                SetupWizardProgressState::TOTAL_VERIFICATION_STEPS,
                self.setup_wizard_progress.progress_percent()
            ),
            status: self.setup_wizard_progress.verification_status(),
        });

        items.push(PlanItemArg {
            step: format!(
                "Protocol: {} sequence active ({})",
                WorkOrchestrationProfile::SEQUENCE,
                profile.display_name
            ),
            status: StepStatus::Completed,
        });

        // Authentication: if the session exists, the driver already passed `check_auth()` for
        // providers that require it. Mark this as completed to keep the wizard actionable.
        items.push(PlanItemArg {
            step: "Setup: authentication".to_string(),
            status: StepStatus::Completed,
        });

        let model_status = if self
            .config
            .model
            .as_ref()
            .is_some_and(|m| !m.trim().is_empty())
        {
            StepStatus::Completed
        } else {
            StepStatus::Pending
        };
        items.push(PlanItemArg {
            step: "Setup: choose model".to_string(),
            status: model_status,
        });

        let effort_status = if self.config.model_reasoning_effort.is_some() {
            StepStatus::Completed
        } else {
            StepStatus::Pending
        };
        items.push(PlanItemArg {
            step: "Setup: choose reasoning effort".to_string(),
            status: effort_status,
        });

        // Approval preset always has a value (defaults apply), but we want users to explicitly
        // choose one so they understand the approvals/sandbox tradeoffs.
        let approval_status = if self
            .config
            .did_user_set_custom_approval_policy_or_sandbox_mode
        {
            StepStatus::Completed
        } else {
            StepStatus::Pending
        };
        items.push(PlanItemArg {
            step: "Setup: choose approval preset".to_string(),
            status: approval_status,
        });

        let context_status =
            if matches!(self.context_optimization.mode, ContextOptimizationMode::Off) {
                StepStatus::Pending
            } else {
                StepStatus::Completed
            };
        items.push(PlanItemArg {
            step: "Setup: enable context optimization telemetry".to_string(),
            status: context_status,
        });

        let orchestration_status = if matches!(
            self.task_monitoring.orchestration_mode,
            TaskOrchestrationMode::Parallel
        ) {
            StepStatus::Completed
        } else {
            StepStatus::Pending
        };
        items.push(PlanItemArg {
            step: "Defaults: parallel task orchestration".to_string(),
            status: orchestration_status,
        });

        let task_monitoring_status = if self.task_monitoring.monitor_mode.is_enabled() {
            StepStatus::Completed
        } else {
            StepStatus::Pending
        };
        items.push(PlanItemArg {
            step: "Defaults: task monitoring mode enabled".to_string(),
            status: task_monitoring_status,
        });

        let vector_check_status = if self.task_monitoring.vector_check_enabled {
            StepStatus::Completed
        } else {
            StepStatus::Pending
        };
        items.push(PlanItemArg {
            step: "Defaults: progress vector checks enabled".to_string(),
            status: vector_check_status,
        });

        let verification_progress = self.setup_wizard_progress.completed_count();
        items.push(PlanItemArg {
            step: format!(
                "Verify: run /status, /monitor, and /vector ({}/{}, {}%)",
                verification_progress,
                SetupWizardProgressState::TOTAL_VERIFICATION_STEPS,
                self.setup_wizard_progress.progress_percent()
            ),
            status: self.setup_wizard_progress.verification_status(),
        });

        items
    }

    fn render_task_monitoring_snapshot(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!(
            "Task monitoring: orchestration={}, monitor={}, vector_checks={}, preempt_on_new_prompt={}",
            self.task_monitoring.orchestration_mode.as_config_value(),
            self.task_monitoring.monitor_mode.as_config_value(),
            if self.task_monitoring.vector_check_enabled {
                "on"
            } else {
                "off"
            },
            if self.task_monitoring.preempt_on_new_prompt {
                "on"
            } else {
                "off"
            },
        ));

        if !self.task_monitoring.monitor_mode.is_enabled() {
            lines.push("Task queue: monitoring disabled".to_string());
            return lines.join("\n");
        }

        let active_tasks = self
            .submissions
            .iter()
            .filter_map(|(submission_id, state)| {
                state
                    .monitor_label()
                    .and_then(|label| state.is_active().then_some((submission_id.clone(), label)))
            })
            .collect::<Vec<_>>();

        if matches!(self.task_monitoring.monitor_mode, TaskMonitoringMode::Auto)
            && active_tasks.is_empty()
        {
            return lines.join("\n");
        }

        let mut active_tasks = active_tasks;
        active_tasks.sort_by(|a, b| a.0.cmp(&b.0));

        if active_tasks.is_empty() {
            lines.push("Task queue: idle".to_string());
        } else {
            lines.push(format!("Task queue: {} active", active_tasks.len()));
            for (submission_id, label) in active_tasks {
                lines.push(format!("- [in_progress] {label}: {submission_id}"));
            }
        }
        lines.join("\n")
    }

    fn render_progress_signals_snapshot(&self, active_task_count: usize) -> String {
        let mut lines = Vec::new();
        lines.push("Progress signals".to_string());

        let bottleneck = self
            .submissions
            .iter()
            .filter_map(|(submission_id, state)| {
                let label = state.monitor_label()?;
                if !state.is_active() {
                    return None;
                }
                state
                    .longest_open_tool_call_runtime()
                    .map(|runtime| (submission_id, label, runtime))
            })
            .max_by_key(|(_, _, runtime)| runtime.elapsed);

        let bottleneck_line = match bottleneck {
            Some((submission_id, label, runtime))
                if runtime.elapsed.as_secs() >= MONITOR_BOTTLENECK_SLOW_SECS =>
            {
                format!(
                    "Bottleneck: `{}` in {label}:{submission_id} has been running for {}",
                    runtime.kind,
                    format_monitor_duration(runtime.elapsed)
                )
            }
            Some((submission_id, label, runtime)) => {
                format!(
                    "Bottleneck: longest open tool call is `{}` in {label}:{submission_id} ({} elapsed)",
                    runtime.kind,
                    format_monitor_duration(runtime.elapsed)
                )
            }
            None if active_task_count > 0 => {
                "Bottleneck: active tasks exist, but no open tool calls are currently running"
                    .to_string()
            }
            None => "Bottleneck: no active bottleneck signal".to_string(),
        };
        lines.push(bottleneck_line);
        lines.push(self.flow_vector.render_repeat_signal());
        lines.push(self.flow_vector.render_stall_signal(active_task_count));
        lines.join("\n")
    }

    fn render_monitor_retrospective(&self) -> String {
        let progress_bar = |latest_percent: u8| -> String {
            let width: usize = 10;
            let safe_percent = latest_percent.min(100) as usize;
            let filled = std::cmp::min(width, safe_percent * width / 100);
            let empty = width.saturating_sub(filled);
            format!("[{}{}]", "#".repeat(filled), "-".repeat(empty))
        };

        let render_lane = |label: char, checkpoints: &[u8]| -> String {
            let mut entries = checkpoints
                .iter()
                .map(|value| format!("{value}%"))
                .collect::<Vec<_>>();
            let latest = checkpoints.last().copied().unwrap_or(0);
            if entries.is_empty() {
                entries.push("0%".to_string());
            }
            format!(
                "Lane {label} 진행률: {} {}",
                entries.join(" → "),
                progress_bar(latest)
            )
        };

        let lines = vec![
            "회고형 상태 보고서 (2026-02-14)".to_string(),
            "병렬 오케스트레이션으로 각 순위를 분해해 동시 진행 중이며, 순위 순서대로 정리합니다."
                .to_string(),
            String::new(),
            "1. brain job payload templates by type: 스펙 확정 + 예제 입력/출력 정의".to_string(),
            "병렬 레인: A 스펙 확정 | B 예제 입력/출력 정의".to_string(),
            render_lane('A', &[41, 56, 69]),
            render_lane('B', &[28, 44, 61]),
            "회고: 타입 경계 정의에서 합의 시간이 길어졌고, 예제는 경계값 중심으로 정리되면서 속도가 붙었습니다.".to_string(),
            "배운 점: 스펙 합의가 먼저 고정되면 예제 정의가 안정적으로 따라옵니다.".to_string(),
            "다음: 스펙 승인 1회, 예제 3세트 확정, 템플릿 버전 태깅.".to_string(),
            "리스크/블로커: 타입별 예외 케이스 정의가 미완이면 예제 변경이 재발할 수 있음.".to_string(),
            String::new(),
            "2. runner multi-worker locking/duplicate prevention: 락 정책 정의 + 최소 통합 테스트 추가"
                .to_string(),
            "병렬 레인: A 락 정책 정의 | B 최소 통합 테스트 추가".to_string(),
            render_lane('A', &[33, 47, 62]),
            render_lane('B', &[22, 38, 55]),
            "회고: 락 범위/만료 전략 합의가 길어졌으나, 중복 방지 기준을 명확히 하며 테스트 범위가 좁혀졌습니다."
                .to_string(),
            "배운 점: 락 정책을 문서화할 때 재시도/타임아웃을 함께 정의해야 테스트 기준이 흔들리지 않습니다."
                .to_string(),
            "다음: 정책 문서 1차 확정, 통합 테스트 1건 최소 패스 기준 설정.".to_string(),
            "리스크/블로커: 재시도 정책이 확정되지 않으면 테스트 플랜이 무한 대기 시나리오를 놓칠 수 있음."
                .to_string(),
            String::new(),
            "3. E2E validation에 동시성 시나리오 1건 추가".to_string(),
            "병렬 레인: A 시나리오 설계 | B E2E 추가".to_string(),
            render_lane('A', &[18, 34, 52]),
            render_lane('B', &[12, 29, 46]),
            "회고: 시나리오 조건이 확정되며 추가 구현의 불확실성이 줄어들고 있습니다.".to_string(),
            "배운 점: 동시성 시나리오는 “성공 기준”과 “실패 허용 기준”을 동시에 적어야 검증이 단단해집니다."
                .to_string(),
            "다음: 시나리오 승인, E2E 1건 추가 후 안정성 확인.".to_string(),
            "리스크/블로커: 러너 락 정책이 확정되기 전에는 E2E 기준이 임시로 남아 있음.".to_string(),
            String::new(),
            "원하면 각 레인의 다음 업데이트 시 진행률 숫자를 더 촘촘히 틱업 형태로 표기하겠습니다."
                .to_string(),
        ];
        lines.join("\n")
    }

    fn render_monitor_message(&self, detail: bool) -> String {
        let panel_width = monitor_panel_width();
        let active_task_count = self
            .submissions
            .values()
            .filter(|state| state.monitor_label().is_some() && state.is_active())
            .count();
        let status_strip = format!(
            "Status strip: orchestration={} | monitor={} | vector_checks={} | preempt={} | active_tasks={} | context={}@{}% | detail={}",
            self.task_monitoring.orchestration_mode.as_config_value(),
            self.task_monitoring.monitor_mode.as_config_value(),
            if self.task_monitoring.vector_check_enabled {
                "on"
            } else {
                "off"
            },
            if self.task_monitoring.preempt_on_new_prompt {
                "on"
            } else {
                "off"
            },
            active_task_count,
            self.context_optimization.mode.as_config_value(),
            self.context_optimization.trigger_percent,
            if detail { "on" } else { "off" }
        );

        let mut lines = Vec::new();
        lines.push("Thread monitor".to_string());
        lines.push(monitor_fit_line(&status_strip, panel_width));
        lines.push(format!(
            "Viewport width: {panel_width} cols (auto-fit enabled)"
        ));
        lines.push(String::new());
        lines.push("Work thread (execution lane)".to_string());
        lines.extend(monitor_fit_block(
            &self.flow_vector.render_plan_progress(),
            panel_width,
        ));
        lines.push(String::new());
        lines.extend(monitor_fit_block(
            &self.render_task_monitoring_snapshot(),
            panel_width,
        ));
        lines.push(String::new());
        lines.extend(monitor_fit_block(
            &self.render_progress_signals_snapshot(active_task_count),
            panel_width,
        ));
        lines.push(String::new());
        lines.extend(monitor_fit_block(
            &self.flow_vector.render_recent_actions(detail),
            panel_width,
        ));
        lines.push(String::new());
        lines.push("Monitor thread (meta lane, pinned)".to_string());
        lines.push(
            "Pinned panels: context telemetry | flow telemetry | runtime diagnostics".to_string(),
        );
        lines.push("Panel: Context telemetry".to_string());
        lines.push(String::new());
        lines.push(monitor_fit_line(
            &format!(
                "Context optimization: mode={}, trigger={}%, auto_compact_runs={}",
                self.context_optimization.mode.as_config_value(),
                self.context_optimization.trigger_percent,
                self.context_optimization.auto_compact_count
            ),
            panel_width,
        ));

        if let Some(info) = &self.context_optimization.last_token_info {
            let total = info.total_token_usage.tokens_in_context_window();
            let context_window = info
                .model_context_window
                .or(self.config.model_context_window);
            if let Some(window) = context_window {
                let used = if window > 0 {
                    ((total as f64 / window as f64) * 100.0).round() as i64
                } else {
                    0
                };
                lines.push(monitor_fit_line(
                    &format!("Latest context usage: {total}/{window} tokens (~{used}% used)"),
                    panel_width,
                ));
            } else {
                lines.push(monitor_fit_line(
                    &format!("Latest context usage: {total} tokens (window unknown)"),
                    panel_width,
                ));
            }
        } else {
            lines.push(monitor_fit_line(
                "Latest context usage: not available yet",
                panel_width,
            ));
        }

        if let Some(estimate) = &self.context_optimization.last_prompt_estimate {
            lines.push(monitor_fit_line(
                &format!(
                    "Latest prompt estimate: {} tokens (text={}, embedded={}, links={}, image_assumed={}, audio_assumed={})",
                estimate.total_tokens,
                estimate.text_tokens,
                estimate.embedded_context_tokens,
                estimate.resource_link_tokens,
                estimate.image_tokens,
                estimate.audio_tokens
            ),
                panel_width,
            ));
        } else {
            lines.push(monitor_fit_line(
                "Latest prompt estimate: not available yet",
                panel_width,
            ));
        }

        lines.push(String::new());
        lines.push("Panel: Flow telemetry".to_string());
        if self.task_monitoring.vector_check_enabled {
            lines.extend(monitor_fit_block(
                &self.flow_vector.render_compass(),
                panel_width,
            ));
        } else {
            lines.push("Flow compass: vector checks are disabled.".to_string());
        }
        lines.push(monitor_fit_line(
            &format!("Flow minimap: {}", self.flow_vector.path_string()),
            panel_width,
        ));

        if detail {
            let diagnostics = self
                .client
                .runtime_diagnostics_snapshot("monitor_detail", active_task_count);
            self.client
                .log_runtime_diagnostics("monitor_detail", active_task_count);
            lines.push(String::new());
            lines.push("Panel: Runtime diagnostics".to_string());
            lines.push(String::new());
            lines.extend(monitor_fit_block(&diagnostics.render(), panel_width));
        }

        lines.join("\n")
    }

    fn render_vector_message(&self) -> String {
        let mut lines = Vec::new();
        lines.push("Workflow minimap + semantic compass".to_string());
        lines.push(self.flow_vector.render_compass());
        lines.push(format!("Flow path: {}", self.flow_vector.path_string()));
        lines.join("\n")
    }

    async fn models(&self) -> Result<SessionModelState, Error> {
        let mut available_models = Vec::new();
        let config_model = self.get_current_model().await;

        let current_model_id = if let Some(model_id) = self.find_current_model().await {
            model_id
        } else {
            // If no preset found, return the current model string as-is
            let model_id = ModelId::new(self.get_current_model().await);
            available_models.push(ModelInfo::new(model_id.clone(), model_id.to_string()));
            model_id
        };

        available_models.extend(
            self.models_manager
                .list_models(&self.config)
                .await
                .iter()
                .filter(|model| model.show_in_picker || model.model == config_model)
                .flat_map(|preset| {
                    preset.supported_reasoning_efforts.iter().map(|effort| {
                        ModelInfo::new(
                            Self::model_id(&preset.id, effort.effort),
                            format!("{} ({})", preset.display_name, effort.effort),
                        )
                        .description(format!("{} {}", preset.description, effort.description))
                    })
                }),
        );

        Ok(SessionModelState::new(current_model_id, available_models))
    }

    async fn handle_load(&mut self) -> Result<LoadSessionResponse, Error> {
        Ok(LoadSessionResponse::new()
            .models(self.models().await?)
            .modes(self.modes())
            .config_options(self.config_options().await?))
    }

    async fn handle_sessions_command(&mut self) -> Result<(), Error> {
        let sessions = self.list_sessions_for_cwd().await?;
        self.last_session_list = sessions.clone();
        let message = format_session_list_message(&self.config.cwd, &sessions);
        self.client.send_agent_text(message).await;
        Ok(())
    }

    async fn handle_load_command(&mut self, rest: &str) -> Result<(), Error> {
        let selection = rest.trim();
        if selection.is_empty() {
            self.client
                .send_agent_text("Usage: /load <session id or list number>")
                .await;
            return Ok(());
        }

        let target_id = if let Ok(index) = selection.parse::<usize>() {
            if index == 0 || index > self.last_session_list.len() {
                None
            } else {
                Some(self.last_session_list[index - 1].id.clone())
            }
        } else {
            Some(SessionId::new(selection.to_string()))
        };

        let Some(session_id) = target_id else {
            self.client
                .send_agent_text(
                    "Unknown session selection. Run /sessions and pick a valid number.",
                )
                .await;
            return Ok(());
        };

        self.client
            .send_agent_text(format!(
                "Session switching must be initiated by the ACP client. In Zed, open the thread list and select session id: {}",
                session_id
            ))
            .await;
        Ok(())
    }

    async fn handle_prompt(
        &mut self,
        request: PromptRequest,
    ) -> Result<oneshot::Receiver<Result<StopReason, Error>>, Error> {
        let (response_tx, response_rx) = oneshot::channel();

        self.client
            .log_canonical("acp.prompt", summarize_prompt_for_log(&request.prompt));
        let prompt_estimate = estimate_prompt_tokens(&request.prompt);
        self.context_optimization.last_prompt_estimate = Some(prompt_estimate.clone());
        self.client
            .log_canonical("acp.context_opt.prompt_estimate", prompt_estimate.to_json());

        let items = build_prompt_items(request.prompt);
        let op;
        let mut skills_options = SkillsCommandOptions::default();
        if let Some((name, rest)) = extract_slash_command(&items) {
            self.flow_vector
                .record_phase('C', format!("slash command: /{name}"));
            match name {
                // CLI parity commands that map to ACP config options / instructions.
                "model" => {
                    self.maybe_emit_config_options_update().await;
                    let current_model = self.get_current_model().await;
                    self.client
                        .send_agent_text(format!(
                            "Current model: {current_model}\n\nUse your client configuration UI (Config Options) to change `Model` and `Reasoning Effort`."
                        ))
                        .await;
                    drop(response_tx.send(Ok(StopReason::EndTurn)));
                    return Ok(response_rx);
                }
                "personality" => {
                    self.maybe_emit_config_options_update().await;
                    let current = self
                        .config
                        .model_personality
                        .map(|p| p.to_string())
                        .unwrap_or_else(|| "auto".to_string());
                    self.client
                        .send_agent_text(format!(
                            "Current personality: {current}\n\nUse your client configuration UI (Config Options) to change `Personality`."
                        ))
                        .await;
                    drop(response_tx.send(Ok(StopReason::EndTurn)));
                    return Ok(response_rx);
                }
                "approvals" | "permissions" => {
                    self.maybe_emit_config_options_update().await;
                    self.client
                        .send_agent_text(
                            "Use your client configuration UI (Config Options) to change `Approval Preset`.\n\nNote: this adapter currently models approvals and permissions together via `Approval Preset`."
                                .to_string(),
                        )
                        .await;
                    drop(response_tx.send(Ok(StopReason::EndTurn)));
                    return Ok(response_rx);
                }
                "experimental" => {
                    self.maybe_emit_config_options_update().await;
                    self.client
                        .send_agent_text(self.render_experimental_features_message())
                        .await;
                    drop(response_tx.send(Ok(StopReason::EndTurn)));
                    return Ok(response_rx);
                }
                "setup" => {
                    self.setup_wizard_active = true;
                    self.maybe_emit_config_options_update().await;
                    self.maybe_emit_setup_wizard_plan_update(None).await;
                    self.client
                        .send_agent_text(self.render_setup_wizard_message())
                        .await;
                    drop(response_tx.send(Ok(StopReason::EndTurn)));
                    return Ok(response_rx);
                }
                "monitor" => {
                    self.setup_wizard_progress.monitor_checked = true;
                    let mode = MonitorMode::from_rest(rest.trim());
                    match mode {
                        MonitorMode::Retrospective => {
                            self.client
                                .send_agent_text(self.render_monitor_retrospective())
                                .await;
                        }
                        _ => {
                            self.client
                                .send_agent_text(self.render_monitor_message(mode.is_detail()))
                                .await;
                        }
                    }
                    self.maybe_emit_setup_wizard_plan_update(Some(
                        "Setup verification progress updated",
                    ))
                    .await;
                    drop(response_tx.send(Ok(StopReason::EndTurn)));
                    return Ok(response_rx);
                }
                "vector" => {
                    self.setup_wizard_progress.vector_checked = true;
                    self.client
                        .send_agent_text(self.render_vector_message())
                        .await;
                    self.maybe_emit_setup_wizard_plan_update(Some(
                        "Setup verification progress updated",
                    ))
                    .await;
                    drop(response_tx.send(Ok(StopReason::EndTurn)));
                    return Ok(response_rx);
                }
                "status" => {
                    self.setup_wizard_progress.status_checked = true;
                    self.maybe_emit_config_options_update().await;
                    let profile = self.codex_work_orchestration_profile();
                    let current_model = self.get_current_model().await;
                    let approval_preset = self
                        .modes()
                        .map(|m| m.current_mode_id.0.to_string())
                        .unwrap_or_else(|| "unknown".to_string());
                    let effort = self
                        .config
                        .model_reasoning_effort
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "auto".to_string());
                    let personality = self
                        .config
                        .model_personality
                        .map(|p| p.to_string())
                        .unwrap_or_else(|| "auto".to_string());
                    let (x, y, magnitude, heading, semantic) = self.flow_vector.resultant_vector();
                    self.client
                        .send_agent_text(format!(
                            "Session status:\n- model: {current_model}\n- reasoning_effort: {effort}\n- personality: {personality}\n- approval_preset: {approval_preset}\n- work_orchestration_profile: {} ({})\n- acp_bridge: {}\n- context_optimization: {} (trigger {}%)\n- task_orchestration: {}\n- task_monitoring: {}\n- progress_vector_checks: {}\n- preempt_on_new_prompt: {}\n- workflow_vector: ({x}, {y}), |v|={magnitude:.2}, heading={heading}\n- workflow_semantic: {semantic}",
                            profile.display_name,
                            WorkOrchestrationProfile::SEQUENCE,
                            profile.bridge_summary(),
                            self.context_optimization.mode.as_config_value(),
                            self.context_optimization.trigger_percent,
                            self.task_monitoring.orchestration_mode.as_config_value(),
                            self.task_monitoring.monitor_mode.as_config_value(),
                            if self.task_monitoring.vector_check_enabled { "on" } else { "off" },
                            if self.task_monitoring.preempt_on_new_prompt { "on" } else { "off" },
                        ))
                        .await;
                    self.maybe_emit_setup_wizard_plan_update(Some(
                        "Setup verification progress updated",
                    ))
                    .await;
                    drop(response_tx.send(Ok(StopReason::EndTurn)));
                    return Ok(response_rx);
                }
                "new" | "new-window" | "resume" | "fork" | "agent" => {
                    self.client
                        .send_agent_text(
                            "Session/thread switching must be initiated by the ACP client.\nIn Zed, use the Agent Panel thread list (or the + button) to start a new thread, or pick a previous session."
                                .to_string(),
                        )
                        .await;
                    drop(response_tx.send(Ok(StopReason::EndTurn)));
                    return Ok(response_rx);
                }
                "mention" => {
                    self.client
                        .send_agent_text(
                            "Use `@file` mentions (or attach files) from your ACP client. This adapter already supports embedded context."
                                .to_string(),
                        )
                        .await;
                    drop(response_tx.send(Ok(StopReason::EndTurn)));
                    return Ok(response_rx);
                }
                "feedback" => {
                    self.client
                        .send_agent_text(
                            "In ACP, there is no built-in log upload flow yet. If you want, paste the relevant snippet from `logs/codex_chats/...` (redact secrets) and we can triage."
                                .to_string(),
                        )
                        .await;
                    drop(response_tx.send(Ok(StopReason::EndTurn)));
                    return Ok(response_rx);
                }
                "mcp" => op = Op::ListMcpTools,
                "skills" => {
                    let parsed = match parse_skills_command_options(rest) {
                        Ok(parsed) => parsed,
                        Err(error_message) => {
                            self.client
                                .send_agent_text(skills_command_usage_message(Some(&error_message)))
                                .await;
                            drop(response_tx.send(Ok(StopReason::EndTurn)));
                            return Ok(response_rx);
                        }
                    };
                    let force_reload = parsed.force_reload;
                    skills_options = parsed;
                    op = Op::ListSkills {
                        cwds: vec![],
                        force_reload,
                    }
                }
                "diff" => {
                    // Best-effort: run git diff in the configured cwd. Output will stream through
                    // ExecCommand events like other command executions.
                    op = Op::RunUserShellCommand {
                        command: "git diff --no-color --".to_string(),
                    }
                }
                "compact" => op = Op::Compact,
                "undo" => op = Op::Undo,
                "sessions" => {
                    self.handle_sessions_command().await?;
                    drop(response_tx.send(Ok(StopReason::EndTurn)));
                    return Ok(response_rx);
                }
                "load" => {
                    self.handle_load_command(rest).await?;
                    drop(response_tx.send(Ok(StopReason::EndTurn)));
                    return Ok(response_rx);
                }
                "init" => {
                    op = Op::UserInput {
                        items: vec![UserInput::Text {
                            text: INIT_COMMAND_PROMPT.into(),
                            text_elements: vec![],
                        }],
                        final_output_json_schema: None,
                    }
                }
                "review" => {
                    let instructions = rest.trim();
                    let target = if instructions.is_empty() {
                        ReviewTarget::UncommittedChanges
                    } else {
                        ReviewTarget::Custom {
                            instructions: instructions.to_owned(),
                        }
                    };

                    op = Op::Review {
                        review_request: ReviewRequest {
                            user_facing_hint: Some(user_facing_hint(&target)),
                            target,
                        },
                    }
                }
                "review-branch" if !rest.is_empty() => {
                    let target = ReviewTarget::BaseBranch {
                        branch: rest.trim().to_owned(),
                    };
                    op = Op::Review {
                        review_request: ReviewRequest {
                            user_facing_hint: Some(user_facing_hint(&target)),
                            target,
                        },
                    }
                }
                "review-commit" if !rest.is_empty() => {
                    let target = ReviewTarget::Commit {
                        sha: rest.trim().to_owned(),
                        title: None,
                    };
                    op = Op::Review {
                        review_request: ReviewRequest {
                            user_facing_hint: Some(user_facing_hint(&target)),
                            target,
                        },
                    }
                }
                "logout" => {
                    self.auth.logout()?;
                    return Err(Error::auth_required());
                }
                _ => {
                    if let Some(prompt) =
                        expand_custom_prompt(name, rest, self.custom_prompts.borrow().as_ref())
                            .map_err(|e| Error::invalid_params().data(e.user_message()))?
                    {
                        op = Op::UserInput {
                            items: vec![UserInput::Text {
                                text: prompt,
                                text_elements: vec![],
                            }],
                            final_output_json_schema: None,
                        }
                    } else {
                        op = Op::UserInput {
                            items,
                            final_output_json_schema: None,
                        }
                    }
                }
            }
        } else {
            self.flow_vector
                .record_phase('A', "free-form prompt submitted");
            op = Op::UserInput {
                items,
                final_output_json_schema: None,
            }
        }

        self.preempt_active_runs_before_prompt().await?;

        if matches!(
            self.task_monitoring.orchestration_mode,
            TaskOrchestrationMode::Sequential
        ) {
            let active_tasks = self
                .submissions
                .values()
                .filter(|state| state.monitor_label().is_some() && state.is_active())
                .count();
            if active_tasks > 0 {
                self.client
                    .send_agent_text(
                        "Task orchestration is set to `sequential`. Wait for the current task to finish, or switch `Task Orchestration` to `Parallel`."
                            .to_string(),
                    )
                    .await;
                drop(response_tx.send(Ok(StopReason::EndTurn)));
                return Ok(response_rx);
            }
        }

        let submission_id = self
            .thread
            .submit(op.clone())
            .await
            .map_err(|e| Error::internal_error().data(e.to_string()))?;

        info!("Submitted prompt with submission_id: {submission_id}");
        info!("Starting to wait for conversation events for submission_id: {submission_id}");

        self.client.log_canonical(
            "backend.codex.submit",
            json!({
                "submission_id": submission_id,
                "op_kind": op_kind_for_log(&op),
            }),
        );

        let state = match op {
            Op::Compact | Op::Undo => SubmissionState::Task(TaskState::new(
                self.thread.clone(),
                response_tx,
                submission_id.clone(),
            )),
            Op::ListMcpTools => SubmissionState::OneShot(OneShotCommandState::new(
                OneShotKind::McpTools,
                response_tx,
            )),
            Op::ListSkills { .. } => SubmissionState::OneShot(OneShotCommandState::new_skills(
                response_tx,
                skills_options,
            )),
            _ => SubmissionState::Prompt(PromptState::new(
                self.thread.clone(),
                response_tx,
                submission_id.clone(),
            )),
        };

        self.submissions.insert(submission_id, state);

        Ok(response_rx)
    }

    async fn handle_set_mode(&mut self, mode: SessionModeId) -> Result<(), Error> {
        let preset = APPROVAL_PRESETS
            .iter()
            .find(|preset| mode.0.as_ref() == preset.id)
            .ok_or_else(Error::invalid_params)?;

        self.thread
            .submit(Op::OverrideTurnContext {
                cwd: None,
                approval_policy: Some(preset.approval),
                sandbox_policy: Some(preset.sandbox.clone()),
                model: None,
                effort: None,
                summary: None,
                collaboration_mode: None,
                personality: None,
            })
            .await
            .map_err(|e| Error::from(anyhow::anyhow!(e)))?;

        self.config
            .approval_policy
            .set(preset.approval)
            .map_err(|e| Error::from(anyhow::anyhow!(e)))?;
        self.config
            .sandbox_policy
            .set(preset.sandbox.clone())
            .map_err(|e| Error::from(anyhow::anyhow!(e)))?;
        self.config
            .did_user_set_custom_approval_policy_or_sandbox_mode = true;

        match preset.sandbox {
            // Treat this user action as a trusted dir
            SandboxPolicy::DangerFullAccess
            | SandboxPolicy::WorkspaceWrite { .. }
            | SandboxPolicy::ExternalSandbox { .. } => {
                set_project_trust_level(
                    &self.config.codex_home,
                    &self.config.cwd,
                    TrustLevel::Trusted,
                )?;
            }
            SandboxPolicy::ReadOnly => {}
        }

        Ok(())
    }

    async fn get_current_model(&self) -> String {
        self.models_manager
            .get_model(&self.config.model, &self.config)
            .await
    }

    async fn handle_set_model(&mut self, model: ModelId) -> Result<(), Error> {
        // Try parsing as preset format, otherwise use as-is, fallback to config
        let (model_to_use, effort_to_use) = if let Some((m, e)) = Self::parse_model_id(&model) {
            (m, Some(e))
        } else {
            let model_str = model.0.to_string();
            let fallback = if !model_str.is_empty() {
                model_str
            } else {
                self.get_current_model().await
            };
            (fallback, self.config.model_reasoning_effort)
        };

        if model_to_use.is_empty() {
            return Err(Error::invalid_params().data("No model parsed or configured"));
        }

        self.thread
            .submit(Op::OverrideTurnContext {
                cwd: None,
                approval_policy: None,
                sandbox_policy: None,
                model: Some(model_to_use.clone()),
                effort: Some(effort_to_use),
                summary: None,
                collaboration_mode: None,
                personality: None,
            })
            .await
            .map_err(|e| Error::from(anyhow::anyhow!(e)))?;

        self.config.model = Some(model_to_use);
        self.config.model_reasoning_effort = effort_to_use;

        Ok(())
    }

    async fn handle_cancel(&mut self) -> Result<(), Error> {
        self.thread
            .submit(Op::Interrupt)
            .await
            .map_err(|e| Error::from(anyhow::anyhow!(e)))?;
        Ok(())
    }

    /// Replay conversation history to the client via session/update notifications.
    /// This is called when loading a session to stream all prior messages.
    ///
    /// We process both `EventMsg` and `ResponseItem`:
    /// - `EventMsg` for user/agent messages and reasoning (like the TUI does)
    /// - `ResponseItem` for tool calls only (not persisted as EventMsg)
    async fn handle_replay_history(&mut self, history: Vec<RolloutItem>) -> Result<(), Error> {
        for item in history {
            match item {
                RolloutItem::EventMsg(event_msg) => {
                    self.replay_event_msg(&event_msg).await;
                }
                RolloutItem::ResponseItem(response_item) => {
                    self.replay_response_item(&response_item).await;
                }
                // Skip SessionMeta, TurnContext, Compacted
                _ => {}
            }
        }
        Ok(())
    }

    /// Convert and send an EventMsg as ACP notification(s) during replay.
    /// Handles messages and reasoning - mirrors the live event handling in PromptState.
    async fn replay_event_msg(&self, msg: &EventMsg) {
        match msg {
            EventMsg::UserMessage(UserMessageEvent { message, .. }) => {
                self.client.send_user_message(message.clone()).await;
            }
            EventMsg::AgentMessage(AgentMessageEvent { message }) => {
                self.client.send_agent_text(message.clone()).await;
            }
            EventMsg::AgentReasoning(AgentReasoningEvent { text }) => {
                self.client.send_agent_thought(text.clone()).await;
            }
            EventMsg::AgentReasoningRawContent(AgentReasoningRawContentEvent { text }) => {
                self.client.send_agent_thought(text.clone()).await;
            }
            // Skip other event types during replay - they either:
            // - Are transient (deltas, turn lifecycle)
            // - Don't have direct ACP equivalents
            // - Are handled via ResponseItem instead
            _ => {}
        }
    }

    /// Parse apply_patch call input to extract patch content for display.
    /// Returns (title, locations, content) if successful.
    /// For CustomToolCall, the input is the patch string directly.
    fn parse_apply_patch_call(
        &self,
        input: &str,
    ) -> Option<(String, Vec<ToolCallLocation>, Vec<ToolCallContent>)> {
        // Try to parse the patch using codex-apply-patch parser
        let parsed = parse_patch(input).ok()?;

        let mut locations = Vec::new();
        let mut file_names = Vec::new();
        let mut content = Vec::new();

        for hunk in &parsed.hunks {
            match hunk {
                codex_apply_patch::Hunk::AddFile { path, contents } => {
                    let full_path = self.config.cwd.join(path);
                    file_names.push(path.display().to_string());
                    locations.push(ToolCallLocation::new(full_path.clone()));
                    // New file: no old_text, new_text is the contents
                    content.push(ToolCallContent::Diff(Diff::new(
                        full_path,
                        contents.clone(),
                    )));
                }
                codex_apply_patch::Hunk::DeleteFile { path } => {
                    let full_path = self.config.cwd.join(path);
                    file_names.push(path.display().to_string());
                    locations.push(ToolCallLocation::new(full_path.clone()));
                    // Delete file: old_text would be original content, new_text is empty
                    content.push(ToolCallContent::Diff(
                        Diff::new(full_path, "").old_text("[file deleted]"),
                    ));
                }
                codex_apply_patch::Hunk::UpdateFile {
                    path,
                    move_path,
                    chunks,
                } => {
                    let full_path = self.config.cwd.join(path);
                    let dest_path = move_path
                        .as_ref()
                        .map(|p| self.config.cwd.join(p))
                        .unwrap_or_else(|| full_path.clone());
                    file_names.push(path.display().to_string());
                    locations.push(ToolCallLocation::new(dest_path.clone()));

                    // Build old and new text from chunks
                    let old_lines: Vec<String> = chunks
                        .iter()
                        .flat_map(|c| c.old_lines.iter().cloned())
                        .collect();
                    let new_lines: Vec<String> = chunks
                        .iter()
                        .flat_map(|c| c.new_lines.iter().cloned())
                        .collect();

                    content.push(ToolCallContent::Diff(
                        Diff::new(dest_path, new_lines.join("\n")).old_text(old_lines.join("\n")),
                    ));
                }
            }
        }

        let title = if file_names.is_empty() {
            "Apply patch".to_string()
        } else {
            format!("Edit {}", file_names.join(", "))
        };

        Some((title, locations, content))
    }

    /// Parse shell function call arguments to extract command info for rich display.
    /// Returns (title, kind, locations) if successful.
    ///
    /// Handles both:
    /// - `shell` / `container.exec`: `command` is `Vec<String>`
    /// - `shell_command`: `command` is a `String` (shell script)
    fn parse_shell_function_call(
        &self,
        name: &str,
        arguments: &str,
    ) -> Option<(String, ToolKind, Vec<ToolCallLocation>)> {
        // Extract command and workdir based on tool type
        let (command_vec, workdir): (Vec<String>, Option<String>) = if name == "shell_command" {
            // shell_command: command is a string (shell script)
            #[derive(serde::Deserialize)]
            struct ShellCommandArgs {
                command: String,
                #[serde(default)]
                workdir: Option<String>,
            }
            let args: ShellCommandArgs = serde_json::from_str(arguments).ok()?;
            // Wrap in bash -lc for parsing
            (
                vec!["bash".to_string(), "-lc".to_string(), args.command],
                args.workdir,
            )
        } else {
            // shell / container.exec: command is Vec<String>
            #[derive(serde::Deserialize)]
            struct ShellArgs {
                command: Vec<String>,
                #[serde(default)]
                workdir: Option<String>,
            }
            let args: ShellArgs = serde_json::from_str(arguments).ok()?;
            (args.command, args.workdir)
        };

        let cwd = workdir
            .map(PathBuf::from)
            .unwrap_or_else(|| self.config.cwd.clone());

        let parsed_cmd = parse_command(&command_vec);
        let ParseCommandToolCall {
            title,
            file_extension: _,
            terminal_output: _,
            locations,
            kind,
        } = parse_command_tool_call(parsed_cmd, &cwd);

        Some((title, kind, locations))
    }

    /// Normalizes tool names that may arrive with provider prefixes.
    fn normalize_tool_name(name: &str) -> String {
        if let Some(without_prefix) = name.strip_prefix("functions.") {
            without_prefix.to_string()
        } else {
            name.to_string()
        }
    }

    /// Convert and send a single ResponseItem as ACP notification(s) during replay.
    /// Only handles tool calls - messages/reasoning are handled via EventMsg.
    async fn replay_response_item(&self, item: &ResponseItem) {
        match item {
            // Skip Message and Reasoning - these are handled via EventMsg
            ResponseItem::Message { .. } | ResponseItem::Reasoning { .. } => {}
            ResponseItem::FunctionCall {
                name,
                arguments,
                call_id,
                ..
            } => {
                let normalized_name = Self::normalize_tool_name(name.as_str());
                // Check if this is a shell command - parse it like we do for LocalShellCall
                if matches!(
                    normalized_name.as_str(),
                    "shell" | "container.exec" | "shell_command"
                ) && let Some((title, kind, locations)) =
                    self.parse_shell_function_call(&normalized_name, arguments)
                {
                    self.client
                        .send_tool_call(
                            ToolCall::new(call_id.clone(), title)
                                .kind(kind)
                                .status(ToolCallStatus::Completed)
                                .locations(locations)
                                .raw_input(
                                    serde_json::from_str::<serde_json::Value>(arguments).ok(),
                                ),
                        )
                        .await;
                    return;
                }

                // Fall through to generic function call handling
                self.client
                    .send_completed_tool_call(
                        call_id.clone(),
                        normalized_name,
                        ToolKind::Other,
                        serde_json::from_str(arguments).ok(),
                    )
                    .await;
            }
            ResponseItem::FunctionCallOutput { call_id, output } => {
                self.client
                    .send_tool_call_completed(
                        call_id.clone(),
                        serde_json::to_value(&output.content).ok(),
                    )
                    .await;
            }
            ResponseItem::LocalShellCall {
                call_id: Some(call_id),
                action,
                status,
                ..
            } => {
                let codex_protocol::models::LocalShellAction::Exec(exec) = action;
                let cwd = exec
                    .working_directory
                    .as_ref()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| self.config.cwd.clone());

                // Parse the command to get rich info like the live event handler does
                let parsed_cmd = parse_command(&exec.command);
                let ParseCommandToolCall {
                    title,
                    file_extension: _,
                    terminal_output: _,
                    locations,
                    kind,
                } = parse_command_tool_call(parsed_cmd, &cwd);

                let tool_status = match status {
                    codex_protocol::models::LocalShellStatus::Completed => {
                        ToolCallStatus::Completed
                    }
                    codex_protocol::models::LocalShellStatus::InProgress
                    | codex_protocol::models::LocalShellStatus::Incomplete => {
                        ToolCallStatus::Failed
                    }
                };
                self.client
                    .send_tool_call(
                        ToolCall::new(call_id.clone(), title)
                            .kind(kind)
                            .status(tool_status)
                            .locations(locations),
                    )
                    .await;
            }
            ResponseItem::CustomToolCall {
                name,
                input,
                call_id,
                ..
            } => {
                let normalized_name = Self::normalize_tool_name(name.as_str());
                // Check if this is an apply_patch call - show the patch content
                if normalized_name == "apply_patch"
                    && let Some((title, locations, content)) = self.parse_apply_patch_call(input)
                {
                    self.client
                        .send_tool_call(
                            ToolCall::new(call_id.clone(), title)
                                .kind(ToolKind::Edit)
                                .status(ToolCallStatus::Completed)
                                .locations(locations)
                                .content(content)
                                .raw_input(serde_json::from_str::<serde_json::Value>(input).ok()),
                        )
                        .await;
                    return;
                }

                // Fall through to generic custom tool call handling
                self.client
                    .send_completed_tool_call(
                        call_id.clone(),
                        normalized_name,
                        ToolKind::Other,
                        serde_json::from_str(input).ok(),
                    )
                    .await;
            }
            ResponseItem::CustomToolCallOutput { call_id, output } => {
                self.client
                    .send_tool_call_completed(
                        call_id.clone(),
                        Some(serde_json::Value::String(output.clone())),
                    )
                    .await;
            }
            ResponseItem::WebSearchCall { id, action, .. } => {
                let (title, call_id) = web_search_action_to_title_and_id(id, action);
                let Some(call_id) = call_id else {
                    return; // Skip unknown web search actions
                };
                self.client
                    .send_tool_call(
                        ToolCall::new(call_id, title)
                            .kind(ToolKind::Search)
                            .status(ToolCallStatus::Completed),
                    )
                    .await;
            }
            // Skip GhostSnapshot, Compaction, Other, LocalShellCall without call_id
            _ => {}
        }
    }

    fn observe_token_count(&mut self, submission_id: &str, token_count: &TokenCountEvent) {
        let Some(info) = token_count.info.clone() else {
            return;
        };

        self.context_optimization.last_token_info = Some(info.clone());
        let total_tokens = info.total_token_usage.tokens_in_context_window();
        let context_window = info
            .model_context_window
            .or(self.config.model_context_window);
        let used_percent = context_window.map(|window| {
            if window <= 0 {
                0
            } else {
                ((total_tokens as f64 / window as f64) * 100.0).round() as i64
            }
        });

        self.client.log_canonical(
            "acp.context_opt.token_usage",
            json!({
                "submission_id": submission_id,
                "total_tokens": total_tokens,
                "context_window": context_window,
                "used_percent": used_percent,
                "mode": self.context_optimization.mode.as_config_value(),
                "trigger_percent": self.context_optimization.trigger_percent,
            }),
        );

        let should_stage_auto_compact = matches!(
            self.context_optimization.mode,
            ContextOptimizationMode::Auto
        ) && self
            .context_optimization
            .auto_compact_submission_id
            .as_deref()
            .is_none_or(|id| id != submission_id)
            && used_percent.is_some_and(|used| used >= self.context_optimization.trigger_percent);

        if should_stage_auto_compact {
            self.context_optimization.pending_auto_compact = Some(PendingAutoCompact {
                submission_id: submission_id.to_string(),
                total_tokens,
                context_window,
                used_percent,
            });
        }
    }

    async fn maybe_trigger_auto_compact_after_turn(&mut self, submission_id: &str) {
        if self
            .context_optimization
            .auto_compact_submission_id
            .as_deref()
            == Some(submission_id)
        {
            self.context_optimization.auto_compact_submission_id = None;
            self.client.log_canonical(
                "acp.context_opt.auto_compact_completed",
                json!({ "submission_id": submission_id }),
            );
            self.flow_vector
                .record_phase('C', "auto compaction completed");
            return;
        }

        let Some(pending) = self.context_optimization.pending_auto_compact.clone() else {
            return;
        };
        if pending.submission_id != submission_id {
            return;
        }
        if !matches!(
            self.context_optimization.mode,
            ContextOptimizationMode::Auto
        ) {
            return;
        }
        if self
            .context_optimization
            .auto_compact_submission_id
            .is_some()
        {
            return;
        }

        self.context_optimization.pending_auto_compact = None;

        match self.thread.submit(Op::Compact).await {
            Ok(auto_submission_id) => {
                self.context_optimization.auto_compact_submission_id =
                    Some(auto_submission_id.clone());
                self.context_optimization.auto_compact_count += 1;
                self.flow_vector.record_phase(
                    'C',
                    format!(
                        "auto compact triggered at {}% usage",
                        pending.used_percent.unwrap_or_default()
                    ),
                );
                self.client.log_canonical(
                    "acp.context_opt.auto_compact_triggered",
                    json!({
                        "source_submission_id": submission_id,
                        "compact_submission_id": auto_submission_id,
                        "total_tokens": pending.total_tokens,
                        "context_window": pending.context_window,
                        "used_percent": pending.used_percent,
                    }),
                );

                let auto_submission_id = self
                    .context_optimization
                    .auto_compact_submission_id
                    .clone()
                    .unwrap_or_default();
                self.submissions.insert(
                    auto_submission_id.clone(),
                    SubmissionState::Task(TaskState::new_background(
                        self.thread.clone(),
                        auto_submission_id,
                    )),
                );
            }
            Err(err) => {
                warn!("failed to trigger automatic compact: {err}");
                self.client.log_canonical(
                    "acp.context_opt.auto_compact_error",
                    json!({
                        "source_submission_id": submission_id,
                        "error": err.to_string(),
                    }),
                );
            }
        }
    }

    fn observe_flow_event(&mut self, msg: &EventMsg) {
        match msg {
            EventMsg::PlanUpdate(UpdatePlanArgs { explanation, plan }) => {
                self.flow_vector
                    .record_plan_update(explanation.clone(), plan.as_slice());
            }
            EventMsg::ExecCommandBegin(event) => self
                .flow_vector
                .record_phase('E', format!("exec begin: {:?}", event.command)),
            EventMsg::McpToolCallBegin(McpToolCallBeginEvent { invocation, .. }) => {
                self.flow_vector.record_phase(
                    'E',
                    format!("mcp tool: {}.{}", invocation.server, invocation.tool),
                )
            }
            EventMsg::PatchApplyBegin(_) => self.flow_vector.record_phase('E', "apply_patch"),
            EventMsg::WebSearchBegin(_) => self.flow_vector.record_phase('E', "web search"),
            EventMsg::AgentReasoning(_) | EventMsg::AgentReasoningRawContent(_) => {
                self.flow_vector.record_phase('A', "agent reasoning")
            }
            EventMsg::EnteredReviewMode(_) | EventMsg::ExitedReviewMode(_) => {
                self.flow_vector.record_phase('V', "review")
            }
            EventMsg::ContextCompacted(_) => {
                self.flow_vector.record_phase('C', "context compacted")
            }
            _ => {}
        }
    }

    async fn handle_event(&mut self, Event { id, msg }: Event) {
        self.observe_flow_event(&msg);
        if let EventMsg::TokenCount(token_count) = &msg {
            self.observe_token_count(&id, token_count);
        }

        if let Some(submission) = self.submissions.get_mut(&id) {
            submission.handle_event(&self.client, msg.clone()).await;
        } else {
            warn!("Received event for unknown submission ID: {id} {msg:?}");
        }

        match &msg {
            EventMsg::TurnComplete(_) => {
                self.maybe_trigger_auto_compact_after_turn(&id).await;
            }
            EventMsg::TurnAborted(_) | EventMsg::Error(_) | EventMsg::StreamError(_) => {
                if self
                    .context_optimization
                    .auto_compact_submission_id
                    .as_deref()
                    == Some(id.as_str())
                {
                    self.context_optimization.auto_compact_submission_id = None;
                }
            }
            _ => {}
        }
    }
}

fn build_prompt_items(prompt: Vec<ContentBlock>) -> Vec<UserInput> {
    prompt
        .into_iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text_block) => Some(UserInput::Text {
                text: text_block.text,
                text_elements: vec![],
            }),
            ContentBlock::Image(image_block) => Some(UserInput::Image {
                image_url: format!("data:{};base64,{}", image_block.mime_type, image_block.data),
            }),
            ContentBlock::ResourceLink(ResourceLink { name, uri, .. }) => Some(UserInput::Text {
                text: format_uri_as_link(Some(name), uri),
                text_elements: vec![],
            }),
            ContentBlock::Resource(EmbeddedResource {
                resource:
                    EmbeddedResourceResource::TextResourceContents(TextResourceContents {
                        text,
                        uri,
                        ..
                    }),
                ..
            }) => Some(UserInput::Text {
                text: format!(
                    "{}\n<context ref=\"{uri}\">\n{text}\n</context>",
                    format_uri_as_link(None, uri.clone())
                ),
                text_elements: vec![],
            }),
            // Skip other content types for now
            ContentBlock::Audio(..) | ContentBlock::Resource(..) | _ => None,
        })
        .collect()
}

fn estimate_prompt_tokens(prompt: &[ContentBlock]) -> PromptTokenEstimate {
    fn estimate_text_tokens(text: &str) -> i64 {
        // Coarse heuristic for monitoring only: ~4 chars/token.
        ((text.chars().count() as f64) / 4.0).ceil() as i64
    }

    let mut estimate = PromptTokenEstimate::default();
    for block in prompt {
        match block {
            ContentBlock::Text(text_block) => {
                estimate.text_tokens += estimate_text_tokens(&text_block.text);
            }
            ContentBlock::ResourceLink(ResourceLink { name, uri, .. }) => {
                let mut token_guess = 12_i64;
                token_guess += estimate_text_tokens(uri);
                token_guess += estimate_text_tokens(name);
                estimate.resource_link_tokens += token_guess;
            }
            ContentBlock::Resource(EmbeddedResource {
                resource:
                    EmbeddedResourceResource::TextResourceContents(TextResourceContents {
                        text,
                        uri,
                        ..
                    }),
                ..
            }) => {
                estimate.embedded_context_tokens += estimate_text_tokens(text);
                estimate.resource_link_tokens += estimate_text_tokens(uri);
            }
            // Model-dependent assumptions for visibility only.
            ContentBlock::Image(_) => estimate.image_tokens += 1024,
            ContentBlock::Audio(_) => estimate.audio_tokens += 2048,
            _ => {}
        }
    }

    estimate.total_tokens = estimate.text_tokens
        + estimate.embedded_context_tokens
        + estimate.resource_link_tokens
        + estimate.image_tokens
        + estimate.audio_tokens;
    estimate
}

fn summarize_prompt_for_log(prompt: &[ContentBlock]) -> serde_json::Value {
    let log_embedded_context = std::env::var("ACP_LOG_EMBEDDED_CONTEXT")
        .ok()
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    let max_text_chars: usize = std::env::var("ACP_LOG_MAX_TEXT_CHARS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(16_384);

    let mut text_blocks: Vec<String> = Vec::new();
    let mut resource_links: Vec<serde_json::Value> = Vec::new();
    let mut embedded_text_resources: Vec<serde_json::Value> = Vec::new();

    let mut image_count = 0usize;
    let mut audio_count = 0usize;
    let mut other_count = 0usize;

    for block in prompt {
        match block {
            ContentBlock::Text(t) => text_blocks.push(t.text.clone()),
            ContentBlock::ResourceLink(ResourceLink { name, uri, .. }) => {
                resource_links.push(json!({ "name": name, "uri": uri }));
            }
            ContentBlock::Resource(EmbeddedResource {
                resource:
                    EmbeddedResourceResource::TextResourceContents(TextResourceContents {
                        text,
                        uri,
                        ..
                    }),
                ..
            }) => {
                // Avoid duplicating large / sensitive context by default.
                let mut item = json!({
                    "uri": uri,
                    "text_len": text.len(),
                    "included": log_embedded_context,
                });

                if log_embedded_context {
                    let mut s = text.clone();
                    if s.chars().count() > max_text_chars {
                        s = s.chars().take(max_text_chars).collect::<String>() + "\n...[truncated]";
                    }
                    item["text"] = serde_json::Value::String(s);
                }
                embedded_text_resources.push(item);
            }
            ContentBlock::Image(_) => image_count += 1,
            ContentBlock::Audio(_) => audio_count += 1,
            _ => other_count += 1,
        }
    }

    let mut text = text_blocks.join("\n");
    if text.chars().count() > max_text_chars {
        text = text.chars().take(max_text_chars).collect::<String>() + "\n...[truncated]";
    }

    json!({
        "block_count": prompt.len(),
        "text": text,
        "resource_links": resource_links,
        "embedded_text_resources": embedded_text_resources,
        "image_count": image_count,
        "audio_count": audio_count,
        "other_count": other_count,
    })
}

fn op_kind_for_log(op: &Op) -> &'static str {
    match op {
        Op::UserInput { .. } => "user_input",
        Op::Review { .. } => "review",
        Op::Compact => "compact",
        Op::Undo => "undo",
        Op::ListMcpTools => "list_mcp_tools",
        Op::ListSkills { .. } => "list_skills",
        Op::RunUserShellCommand { .. } => "run_shell_command",
        Op::ListCustomPrompts => "list_custom_prompts",
        _ => "other",
    }
}

fn format_uri_as_link(name: Option<String>, uri: String) -> String {
    if let Some(name) = name
        && !name.is_empty()
    {
        format!("[@{name}]({uri})")
    } else if let Some(path) = uri.strip_prefix("file://") {
        let name = path.split('/').next_back().unwrap_or(path);
        format!("[@{name}]({uri})")
    } else if uri.starts_with("zed://") {
        let name = uri.split('/').next_back().unwrap_or(&uri);
        format!("[@{name}]({uri})")
    } else {
        uri
    }
}

fn codex_content_to_acp_content(content: mcp_types::ContentBlock) -> ToolCallContent {
    ToolCallContent::Content(Content::new(match content {
        mcp_types::ContentBlock::TextContent(mcp_types::TextContent {
            annotations, text, ..
        }) => ContentBlock::Text(
            TextContent::new(text).annotations(annotations.map(convert_annotations)),
        ),
        mcp_types::ContentBlock::ImageContent(mcp_types::ImageContent {
            annotations,
            data,
            mime_type,
            ..
        }) => ContentBlock::Image(
            ImageContent::new(data, mime_type).annotations(annotations.map(convert_annotations)),
        ),
        mcp_types::ContentBlock::AudioContent(mcp_types::AudioContent {
            annotations,
            data,
            mime_type,
            ..
        }) => ContentBlock::Audio(
            AudioContent::new(data, mime_type).annotations(annotations.map(convert_annotations)),
        ),
        mcp_types::ContentBlock::ResourceLink(mcp_types::ResourceLink {
            annotations,
            description,
            mime_type,
            name,
            size,
            title,
            uri,
            ..
        }) => ContentBlock::ResourceLink(
            ResourceLink::new(name, uri)
                .annotations(annotations.map(convert_annotations))
                .description(description)
                .mime_type(mime_type)
                .size(size)
                .title(title),
        ),
        mcp_types::ContentBlock::EmbeddedResource(mcp_types::EmbeddedResource {
            annotations,
            resource,
            ..
        }) => {
            let resource = match resource {
                mcp_types::EmbeddedResourceResource::TextResourceContents(
                    mcp_types::TextResourceContents {
                        mime_type,
                        text,
                        uri,
                    },
                ) => EmbeddedResourceResource::TextResourceContents(
                    TextResourceContents::new(text, uri).mime_type(mime_type),
                ),
                mcp_types::EmbeddedResourceResource::BlobResourceContents(
                    mcp_types::BlobResourceContents {
                        blob,
                        mime_type,
                        uri,
                    },
                ) => EmbeddedResourceResource::BlobResourceContents(
                    BlobResourceContents::new(blob, uri).mime_type(mime_type),
                ),
            };
            ContentBlock::Resource(
                EmbeddedResource::new(resource).annotations(annotations.map(convert_annotations)),
            )
        }
    }))
}

fn convert_annotations(
    mcp_types::Annotations {
        audience,
        last_modified,
        priority,
    }: mcp_types::Annotations,
) -> Annotations {
    Annotations::new()
        .audience(audience.map(|a| {
            a.into_iter()
                .map(|audience| match audience {
                    mcp_types::Role::Assistant => agent_client_protocol::Role::Assistant,
                    mcp_types::Role::User => agent_client_protocol::Role::User,
                })
                .collect::<Vec<_>>()
        }))
        .last_modified(last_modified)
        .priority(priority)
}

fn extract_tool_call_content_from_changes(
    changes: HashMap<PathBuf, FileChange>,
) -> (
    String,
    Vec<ToolCallLocation>,
    impl Iterator<Item = ToolCallContent>,
) {
    (
        format!(
            "Edit {}",
            changes.keys().map(|p| p.display().to_string()).join(", ")
        ),
        changes.keys().map(ToolCallLocation::new).collect(),
        changes.into_iter().map(|(path, change)| {
            ToolCallContent::Diff(match change {
                codex_core::protocol::FileChange::Add { content } => Diff::new(path, content),
                codex_core::protocol::FileChange::Delete { content } => {
                    Diff::new(path, String::new()).old_text(content)
                }
                codex_core::protocol::FileChange::Update {
                    unified_diff: _,
                    move_path,
                    old_content,
                    new_content,
                } => Diff::new(move_path.unwrap_or(path), new_content).old_text(old_content),
            })
        }),
    )
}

/// Extract title and call_id from a WebSearchAction (used for replay)
fn web_search_action_to_title_and_id(
    id: &Option<String>,
    action: &codex_protocol::models::WebSearchAction,
) -> (String, Option<String>) {
    match action {
        codex_protocol::models::WebSearchAction::Search { query } => {
            let title = query.clone().unwrap_or_else(|| "Web search".to_string());
            let call_id = id
                .clone()
                .or_else(|| Some(generate_fallback_id("web_search")));
            (title, call_id)
        }
        codex_protocol::models::WebSearchAction::OpenPage { url } => {
            let title = url.clone().unwrap_or_else(|| "Open page".to_string());
            let call_id = id
                .clone()
                .or_else(|| Some(generate_fallback_id("web_open")));
            (title, call_id)
        }
        codex_protocol::models::WebSearchAction::FindInPage { pattern, .. } => {
            let title = pattern
                .clone()
                .unwrap_or_else(|| "Find in page".to_string());
            let call_id = id
                .clone()
                .or_else(|| Some(generate_fallback_id("web_find")));
            (title, call_id)
        }
        codex_protocol::models::WebSearchAction::Other => ("Unknown".to_string(), None),
    }
}

/// Generate a fallback ID using UUID (used when id is missing)
fn generate_fallback_id(prefix: &str) -> String {
    format!("{}_{}", prefix, Uuid::new_v4())
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

fn monitor_panel_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
        .map(|columns| columns.saturating_sub(2))
        .unwrap_or(MONITOR_PANEL_WIDTH_DEFAULT)
        .clamp(MONITOR_PANEL_WIDTH_MIN, MONITOR_PANEL_WIDTH_MAX)
}

fn monitor_progress_bar_width(panel_width: usize) -> usize {
    (panel_width / 3).clamp(
        MONITOR_PROGRESS_BAR_MIN_WIDTH,
        MONITOR_PROGRESS_BAR_MAX_WIDTH,
    )
}

fn monitor_fit_line(text: &str, max_graphemes: usize) -> String {
    if text.is_empty() {
        String::new()
    } else {
        truncate_graphemes(text, max_graphemes.max(8))
    }
}

fn monitor_fit_block(block: &str, max_graphemes: usize) -> Vec<String> {
    block
        .lines()
        .map(|line| monitor_fit_line(line, max_graphemes))
        .collect()
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

fn format_session_list_message(cwd: &Path, sessions: &[SessionListEntry]) -> String {
    if sessions.is_empty() {
        return format!(
            "No previous sessions found for {}.\nStart chatting to create one.",
            cwd.display()
        );
    }

    let mut lines = Vec::with_capacity(sessions.len() + 2);
    lines.push(format!("Sessions for {}:", cwd.display()));
    for (index, entry) in sessions.iter().enumerate() {
        let title = entry.title.as_deref().unwrap_or("(untitled)");
        let updated = entry.updated_at.as_deref().unwrap_or("unknown");
        lines.push(format!(
            "{}) {} [id: {}] (updated: {})",
            index + 1,
            title,
            entry.id,
            updated
        ));
    }
    lines.push("Use /load <id or number> to show how to open a previous session.".to_string());
    lines.join("\n")
}

fn format_mcp_tools_message(event: &codex_core::protocol::McpListToolsResponseEvent) -> String {
    let mut tool_names = event.tools.keys().cloned().collect::<Vec<_>>();
    tool_names.sort();

    let mut lines = Vec::new();
    if tool_names.is_empty() {
        lines.push("No MCP tools configured.".to_string());
    } else {
        lines.push(format!("MCP tools ({}):", tool_names.len()));
        lines.extend(tool_names.into_iter().map(|name| format!("- {name}")));
    }

    if !event.auth_statuses.is_empty() {
        let mut statuses = event
            .auth_statuses
            .iter()
            .map(|(server, status)| format!("{server}: {status}"))
            .collect::<Vec<_>>();
        statuses.sort();
        lines.push(String::new());
        lines.push("MCP auth:".to_string());
        lines.extend(statuses.into_iter().map(|s| format!("- {s}")));
    }

    lines.join("\n")
}

fn skills_command_usage_message(error: Option<&str>) -> String {
    let mut lines = Vec::new();

    if let Some(error) = error {
        lines.push(format!("Invalid /skills option: {error}"));
        lines.push(String::new());
    }

    lines.push("Usage:".to_string());
    lines.push("- /skills".to_string());
    lines.push("- /skills --enabled".to_string());
    lines.push("- /skills --disabled".to_string());
    lines.push("- /skills --scope <scope>".to_string());
    lines.push("- /skills --reload".to_string());
    lines.push("- /skills <keyword>".to_string());
    lines.push(String::new());
    lines.push("Examples:".to_string());
    lines.push("- /skills --scope repo".to_string());
    lines.push("- /skills --enabled review".to_string());

    lines.join("\n")
}

fn parse_skills_command_options(rest: &str) -> Result<SkillsCommandOptions, String> {
    let mut options = SkillsCommandOptions::default();
    let mut tokens = rest.split_whitespace().peekable();
    let mut query_parts = Vec::new();

    while let Some(token) = tokens.next() {
        match token {
            "--reload" | "reload" => {
                options.force_reload = true;
            }
            "--enabled" | "enabled" => {
                if options.enabled == Some(false) {
                    return Err("cannot combine --enabled and --disabled".to_string());
                }
                options.enabled = Some(true);
            }
            "--disabled" | "disabled" => {
                if options.enabled == Some(true) {
                    return Err("cannot combine --enabled and --disabled".to_string());
                }
                options.enabled = Some(false);
            }
            "--scope" => {
                let Some(scope) = tokens.next() else {
                    return Err("expected a value after --scope".to_string());
                };
                if scope.starts_with("--") {
                    return Err("expected a scope value after --scope".to_string());
                }
                options.scope = Some(scope.to_ascii_lowercase());
            }
            _ if token.starts_with("--scope=") => {
                let scope = token.trim_start_matches("--scope=").trim();
                if scope.is_empty() {
                    return Err("expected a value in --scope=<value>".to_string());
                }
                options.scope = Some(scope.to_ascii_lowercase());
            }
            _ if token.starts_with("--") => {
                return Err(format!("unknown option `{token}`"));
            }
            _ => {
                query_parts.push(token.to_string());
                query_parts.extend(tokens.map(ToString::to_string));
                break;
            }
        }
    }

    if !query_parts.is_empty() {
        options.query = Some(query_parts.join(" ").to_ascii_lowercase());
    }

    Ok(options)
}

fn format_skills_message(
    event: &codex_core::protocol::ListSkillsResponseEvent,
    options: &SkillsCommandOptions,
) -> String {
    let mut lines = Vec::new();
    if event.skills.is_empty() {
        lines.push("No skills found.".to_string());
        lines.push(String::new());
        lines.push(skills_command_usage_message(None));
        return lines.join("\n");
    }

    if options.force_reload || options.has_filters() {
        let enabled = match options.enabled {
            Some(true) => "enabled only",
            Some(false) => "disabled only",
            None => "all",
        };
        let scope = options.scope.as_deref().unwrap_or("all");
        let query = options.query.as_deref().unwrap_or("(none)");
        lines.push(format!(
            "Applied filters: enabled={enabled}, scope={scope}, query={query}, reload={}",
            options.force_reload
        ));
        lines.push(String::new());
    }

    let mut matched_any = false;

    for entry in &event.skills {
        lines.push(format!("Skills for {}:", entry.cwd.display()));
        let mut skills = entry.skills.clone();
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        skills.retain(|skill| {
            if let Some(enabled) = options.enabled
                && skill.enabled != enabled
            {
                return false;
            }

            let scope_name = format!("{:?}", skill.scope).to_ascii_lowercase();
            if let Some(scope) = options.scope.as_deref()
                && scope_name != scope
            {
                return false;
            }

            if let Some(query) = options.query.as_deref() {
                let name = skill.name.to_ascii_lowercase();
                let description = skill.description.to_ascii_lowercase();
                if !name.contains(query)
                    && !description.contains(query)
                    && !scope_name.contains(query)
                {
                    return false;
                }
            }

            true
        });

        if skills.is_empty() {
            lines.push("- (none)".to_string());
        } else {
            matched_any = true;
            for skill in skills {
                let enabled = if skill.enabled { "enabled" } else { "disabled" };
                lines.push(format!(
                    "- {} ({:?}, {enabled}): {}",
                    skill.name, skill.scope, skill.description
                ));
            }
        }

        if !entry.errors.is_empty() {
            lines.push("Skill errors:".to_string());
            for err in &entry.errors {
                lines.push(format!("- {}: {}", err.path.display(), err.message));
            }
        }

        lines.push(String::new());
    }

    if options.has_filters() && !matched_any {
        lines.push("No skills matched the current filters.".to_string());
        lines.push(String::new());
    }

    lines.push("Available /skills options:".to_string());
    lines.push("- --enabled | --disabled".to_string());
    lines.push("- --scope <scope>".to_string());
    lines.push("- --reload".to_string());
    lines.push("- <keyword>".to_string());

    // Remove trailing blank line for cleaner display
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }

    lines.join("\n")
}

/// Checks if a prompt is slash command
fn extract_slash_command(content: &[UserInput]) -> Option<(&str, &str)> {
    let line = content.first().and_then(|block| match block {
        UserInput::Text { text, .. } => Some(text),
        _ => None,
    })?;

    parse_slash_name(line)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use codex_core::{
        config::ConfigOverrides, models_manager::model_presets::all_model_presets,
        protocol::AgentMessageEvent,
    };
    use tokio::{
        sync::{Mutex, mpsc::UnboundedSender},
        task::LocalSet,
    };

    use super::*;

    struct EnvVarRestore {
        key: String,
        old: Option<String>,
    }

    impl EnvVarRestore {
        fn set(key: &str, value: Option<&str>) -> Self {
            let old = std::env::var(key).ok();
            // Safe in tests when serialized by ENV_LOCK.
            unsafe {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
            Self {
                key: key.to_string(),
                old,
            }
        }
    }

    impl Drop for EnvVarRestore {
        fn drop(&mut self) {
            // Safe in tests when serialized by ENV_LOCK.
            unsafe {
                match &self.old {
                    Some(value) => std::env::set_var(&self.key, value),
                    None => std::env::remove_var(&self.key),
                }
            }
        }
    }

    async fn test_actor_for_config_options() -> anyhow::Result<ThreadActor<StubAuth>> {
        let session_id = SessionId::new("test-config-options");
        let client = Arc::new(StubClient::new());
        let thread = Arc::new(StubCodexThread::new());
        let session_client =
            SessionClient::with_client(session_id.clone(), client, Arc::default(), None);
        let models_manager = Arc::new(StubModelsManager);
        let config = Config::load_with_cli_overrides_and_harness_overrides(
            vec![],
            ConfigOverrides::default(),
        )
        .await?;
        let (_message_tx, message_rx) = tokio::sync::mpsc::unbounded_channel();

        Ok(ThreadActor::new(
            StubAuth,
            session_client,
            thread,
            models_manager,
            config,
            message_rx,
        ))
    }

    fn has_option(options: &[SessionConfigOption], id: &str) -> bool {
        options.iter().any(|option| option.id.0.as_ref() == id)
    }

    #[test]
    fn test_normalize_tool_name() {
        struct TestAuth;
        impl Auth for TestAuth {
            fn logout(&self) -> Result<bool, Error> {
                Ok(true)
            }
        }

        assert_eq!(
            ThreadActor::<TestAuth>::normalize_tool_name("functions.apply_patch"),
            "apply_patch"
        );
        assert_eq!(
            ThreadActor::<TestAuth>::normalize_tool_name("apply_patch"),
            "apply_patch"
        );
        assert_eq!(
            ThreadActor::<TestAuth>::normalize_tool_name("functions.shell"),
            "shell"
        );
        assert_eq!(
            ThreadActor::<TestAuth>::normalize_tool_name("prefix.shell"),
            "prefix.shell"
        );
    }

    #[test]
    fn test_decode_utf8_streaming_preserves_split_multibyte_sequence() {
        let mut pending = Vec::new();

        let first = decode_utf8_streaming(&mut pending, &[0xE2, 0x82], false);
        assert_eq!(first, "");
        assert_eq!(pending, vec![0xE2, 0x82]);

        let second = decode_utf8_streaming(&mut pending, &[0xAC], false);
        assert_eq!(second, "€");
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn test_replay_history_normalizes_namespaced_custom_tool_name() -> anyhow::Result<()> {
        let (_session_id, client, _thread, message_tx, local_set) = setup(vec![]).await?;

        tokio::try_join!(
            async {
                let (response_tx, response_rx) = tokio::sync::oneshot::channel();
                let history = vec![RolloutItem::ResponseItem(ResponseItem::CustomToolCall {
                    id: None,
                    status: None,
                    call_id: "tc-1".to_string(),
                    name: "functions.apply_patch".to_string(),
                    input: "{}".to_string(),
                })];
                message_tx.send(ThreadMessage::ReplayHistory {
                    history,
                    response_tx,
                })?;
                response_rx.await??;
                drop(message_tx);
                Ok::<(), anyhow::Error>(())
            },
            async {
                local_set.await;
                Ok::<(), anyhow::Error>(())
            }
        )?;

        let notifications = client.notifications.lock().unwrap();
        let mut found = false;
        for notification in notifications.iter() {
            if let SessionUpdate::ToolCall(tool_call) = &notification.update
                && tool_call.title == "apply_patch"
            {
                found = true;
            }
        }
        assert!(
            found,
            "expected replay to emit normalized tool-call title. notifications: {notifications:?}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_replay_history_normalizes_namespaced_function_tool_name() -> anyhow::Result<()> {
        let (_session_id, client, _thread, message_tx, local_set) = setup(vec![]).await?;

        tokio::try_join!(
            async {
                let (response_tx, response_rx) = tokio::sync::oneshot::channel();
                let history = vec![RolloutItem::ResponseItem(ResponseItem::FunctionCall {
                    id: None,
                    name: "functions.shell_command".to_string(),
                    arguments: serde_json::json!({
                        "command": "echo hello"
                    })
                    .to_string(),
                    call_id: "tc-func-1".to_string(),
                })];
                message_tx.send(ThreadMessage::ReplayHistory {
                    history,
                    response_tx,
                })?;
                response_rx.await??;
                drop(message_tx);
                Ok::<(), anyhow::Error>(())
            },
            async {
                local_set.await;
                Ok::<(), anyhow::Error>(())
            }
        )?;

        let notifications = client.notifications.lock().unwrap();
        let mut found = false;
        for notification in notifications.iter() {
            if let SessionUpdate::ToolCall(tool_call) = &notification.update
                && tool_call.tool_call_id.0.as_ref() == "tc-func-1"
            {
                found = true;
                assert_ne!(
                    tool_call.title, "functions.shell",
                    "expected normalized function title. notifications: {notifications:?}"
                );
                assert_ne!(
                    tool_call.title, "functions.shell_command",
                    "expected normalized function title. notifications: {notifications:?}"
                );
            }
        }
        assert!(
            found,
            "expected replay to emit function tool call. notifications: {notifications:?}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_prompt() -> anyhow::Result<()> {
        let (session_id, client, _, message_tx, local_set) = setup(vec![]).await?;
        let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();

        message_tx.send(ThreadMessage::Prompt {
            request: PromptRequest::new(session_id.clone(), vec!["Hi".into()]),
            response_tx: prompt_response_tx,
        })?;

        tokio::try_join!(
            async {
                let stop_reason = prompt_response_rx.await??.await??;
                assert_eq!(stop_reason, StopReason::EndTurn);
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        let notifications = client.notifications.lock().unwrap();
        assert_eq!(notifications.len(), 1);
        assert!(matches!(
            &notifications[0].update,
            SessionUpdate::AgentMessageChunk(ContentChunk {
                content: ContentBlock::Text(TextContent { text, .. }),
                ..
            }) if text == "Hi"
        ));

        Ok(())
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_slash_command_smoke_flow() -> anyhow::Result<()> {
        let _guard = crate::session_store::ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();
        let _visibility_restore = EnvVarRestore::set("ACP_UI_VISIBILITY_MODE", Some("full"));
        let _chunk_restore = EnvVarRestore::set("ACP_UI_TEXT_CHUNK_MAX_CHARS", Some("12000"));

        let (session_id, client, thread, message_tx, local_set) = setup(vec![]).await?;

        async fn send_prompt(
            message_tx: &UnboundedSender<ThreadMessage>,
            session_id: &SessionId,
            prompt: &str,
        ) -> anyhow::Result<StopReason> {
            let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();
            message_tx.send(ThreadMessage::Prompt {
                request: PromptRequest::new(session_id.clone(), vec![prompt.into()]),
                response_tx: prompt_response_tx,
            })?;
            Ok(prompt_response_rx.await??.await??)
        }

        // The thread actor runs on a tokio LocalSet. Drive the LocalSet concurrently with
        // sending prompts, otherwise the test will deadlock waiting for responses.
        tokio::try_join!(
            async {
                // Basic end-to-end smoke: CLI parity slash commands work and can be chained.
                assert_eq!(
                    send_prompt(&message_tx, &session_id, "/init").await?,
                    StopReason::EndTurn
                );
                assert_eq!(
                    send_prompt(&message_tx, &session_id, "Hello").await?,
                    StopReason::EndTurn
                );
                assert_eq!(
                    send_prompt(&message_tx, &session_id, "/review").await?,
                    StopReason::EndTurn
                );
                assert_eq!(
                    send_prompt(&message_tx, &session_id, "/compact").await?,
                    StopReason::EndTurn
                );

                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        let notifications = client.notifications.lock().unwrap();
        let texts: Vec<String> = notifications
            .iter()
            .map(|n| match &n.update {
                SessionUpdate::AgentMessageChunk(ContentChunk {
                    content: ContentBlock::Text(TextContent { text, .. }),
                    ..
                }) => text.clone(),
                other => panic!("Unexpected notification update: {other:?}"),
            })
            .collect();

        assert_eq!(
            texts.as_slice(),
            &[
                INIT_COMMAND_PROMPT.to_string(),
                "Hello".to_string(),
                "current changes".to_string(),
                "Compact task completed".to_string(),
            ]
        );

        let ops = thread.ops.lock().unwrap();
        assert_eq!(
            ops.as_slice(),
            &[
                Op::UserInput {
                    items: vec![UserInput::Text {
                        text: INIT_COMMAND_PROMPT.to_string(),
                        text_elements: vec![]
                    }],
                    final_output_json_schema: None,
                },
                Op::UserInput {
                    items: vec![UserInput::Text {
                        text: "Hello".to_string(),
                        text_elements: vec![]
                    }],
                    final_output_json_schema: None,
                },
                Op::Review {
                    review_request: ReviewRequest {
                        user_facing_hint: Some(user_facing_hint(&ReviewTarget::UncommittedChanges)),
                        target: ReviewTarget::UncommittedChanges,
                    }
                },
                Op::Compact,
            ],
            "ops don't match {ops:?}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_compact() -> anyhow::Result<()> {
        let (session_id, client, thread, message_tx, local_set) = setup(vec![]).await?;
        let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();

        message_tx.send(ThreadMessage::Prompt {
            request: PromptRequest::new(session_id.clone(), vec!["/compact".into()]),
            response_tx: prompt_response_tx,
        })?;

        tokio::try_join!(
            async {
                let stop_reason = prompt_response_rx.await??.await??;
                assert_eq!(stop_reason, StopReason::EndTurn);
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        let notifications = client.notifications.lock().unwrap();
        assert_eq!(notifications.len(), 1);
        assert!(matches!(
            &notifications[0].update,
            SessionUpdate::AgentMessageChunk(ContentChunk {
                content: ContentBlock::Text(TextContent { text, .. }),
                ..
            }) if text == "Compact task completed"
        ));
        let ops = thread.ops.lock().unwrap();
        assert_eq!(ops.as_slice(), &[Op::Compact]);

        Ok(())
    }

    #[tokio::test]
    async fn test_undo() -> anyhow::Result<()> {
        let (session_id, client, thread, message_tx, local_set) = setup(vec![]).await?;
        let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();

        message_tx.send(ThreadMessage::Prompt {
            request: PromptRequest::new(session_id.clone(), vec!["/undo".into()]),
            response_tx: prompt_response_tx,
        })?;

        tokio::try_join!(
            async {
                let stop_reason = prompt_response_rx.await??.await??;
                assert_eq!(stop_reason, StopReason::EndTurn);
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        let notifications = client.notifications.lock().unwrap();
        assert_eq!(
            notifications.len(),
            2,
            "notifications don't match {notifications:?}"
        );
        assert!(matches!(
            &notifications[0].update,
            SessionUpdate::AgentMessageChunk(ContentChunk {
                content: ContentBlock::Text(TextContent { text, .. }),
                ..
            }) if text == "Undo in progress..."
        ));
        assert!(matches!(
            &notifications[1].update,
            SessionUpdate::AgentMessageChunk(ContentChunk {
                content: ContentBlock::Text(TextContent { text, .. }),
                ..
            }) if text == "Undo completed."
        ));

        let ops = thread.ops.lock().unwrap();
        assert_eq!(ops.as_slice(), &[Op::Undo]);

        Ok(())
    }

    #[tokio::test]
    async fn test_mcp() -> anyhow::Result<()> {
        let (session_id, client, thread, message_tx, local_set) = setup(vec![]).await?;
        let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();

        message_tx.send(ThreadMessage::Prompt {
            request: PromptRequest::new(session_id.clone(), vec!["/mcp".into()]),
            response_tx: prompt_response_tx,
        })?;

        tokio::try_join!(
            async {
                let stop_reason = prompt_response_rx.await??.await??;
                assert_eq!(stop_reason, StopReason::EndTurn);
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        let notifications = client.notifications.lock().unwrap();
        assert_eq!(notifications.len(), 1);
        assert!(
            matches!(
                &notifications[0].update,
                SessionUpdate::AgentMessageChunk(ContentChunk {
                    content: ContentBlock::Text(TextContent { text, .. }),
                    ..
                }) if text.contains("MCP") // "No MCP tools..." or listing
            ),
            "notifications don't match {notifications:?}"
        );

        let ops = thread.ops.lock().unwrap();
        assert_eq!(ops.as_slice(), &[Op::ListMcpTools]);

        Ok(())
    }

    #[tokio::test]
    async fn test_monitor_command() -> anyhow::Result<()> {
        let (session_id, client, thread, message_tx, local_set) = setup(vec![]).await?;
        let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();

        message_tx.send(ThreadMessage::Prompt {
            request: PromptRequest::new(session_id.clone(), vec!["/monitor".into()]),
            response_tx: prompt_response_tx,
        })?;

        tokio::try_join!(
            async {
                let stop_reason = prompt_response_rx.await??.await??;
                assert_eq!(stop_reason, StopReason::EndTurn);
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        let notifications = client.notifications.lock().unwrap();
        let text_chunks = notifications
            .iter()
            .filter_map(|n| match &n.update {
                SessionUpdate::AgentMessageChunk(ContentChunk {
                    content: ContentBlock::Text(TextContent { text, .. }),
                    ..
                }) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            text_chunks
                .iter()
                .any(|text| text.contains("Thread monitor")),
            "notifications don't match {notifications:?}"
        );
        assert!(
            text_chunks
                .iter()
                .any(|text| text.contains("Work thread (execution lane)")),
            "monitor output should include work-thread section. notifications={notifications:?}"
        );
        assert!(
            text_chunks
                .iter()
                .any(|text| text.contains("Monitor thread (meta lane, pinned)")),
            "monitor output should include monitor-thread section. notifications={notifications:?}"
        );
        assert!(
            text_chunks.iter().any(|text| text.contains("Progress: [")),
            "monitor output should include default plan progress bar. notifications={notifications:?}"
        );
        assert!(
            text_chunks.iter().any(|text| text.contains(
                "Task monitoring: orchestration=parallel, monitor=auto, vector_checks=on"
            )),
            "monitor output should include task monitoring defaults. notifications={notifications:?}"
        );
        assert!(
            text_chunks
                .iter()
                .any(|text| text.contains("Progress signals")),
            "monitor output should include progress signals panel. notifications={notifications:?}"
        );
        assert!(
            text_chunks.iter().any(|text| text.contains("Bottleneck:")),
            "monitor output should include bottleneck signal. notifications={notifications:?}"
        );
        assert!(
            text_chunks.iter().any(|text| text.contains("Repeat loop:")),
            "monitor output should include repeat-loop signal. notifications={notifications:?}"
        );
        assert!(
            text_chunks
                .iter()
                .any(|text| text.contains("Progress stall:")),
            "monitor output should include stall signal. notifications={notifications:?}"
        );

        let ops = thread.ops.lock().unwrap();
        assert!(
            ops.is_empty(),
            "monitor command should not submit backend op"
        );

        Ok(())
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn test_monitor_detail_command_includes_runtime_diagnostics() -> anyhow::Result<()> {
        let _guard = crate::session_store::ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();

        crate::record_client_info(Some("zed@diagnostics-test".to_string()));
        let (session_id, client, _thread, message_tx, local_set) = setup(vec![]).await?;
        let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();

        message_tx.send(ThreadMessage::Prompt {
            request: PromptRequest::new(session_id.clone(), vec!["/monitor detail".into()]),
            response_tx: prompt_response_tx,
        })?;

        tokio::try_join!(
            async {
                let stop_reason = prompt_response_rx.await??.await??;
                assert_eq!(stop_reason, StopReason::EndTurn);
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        let notifications = client.notifications.lock().unwrap();
        let monitor_text = notifications
            .iter()
            .filter_map(|n| match &n.update {
                SessionUpdate::AgentMessageChunk(ContentChunk {
                    content: ContentBlock::Text(TextContent { text, .. }),
                    ..
                }) if text.contains("Thread monitor") => Some(text.as_str()),
                _ => None,
            })
            .next_back()
            .expect("expected /monitor detail output");

        assert!(
            monitor_text.contains("Panel: Runtime diagnostics"),
            "monitor detail should include runtime diagnostics panel. notifications={notifications:?}"
        );
        assert!(
            monitor_text.contains("Process RSS:"),
            "monitor detail should include process RSS line. notifications={notifications:?}"
        );
        assert!(
            monitor_text.contains("ACP client: zed@diagnostics-test"),
            "monitor detail should include captured client info. notifications={notifications:?}"
        );

        crate::record_client_info(None);
        Ok(())
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_monitor_detail_logs_runtime_diagnostics_to_canonical_log() -> anyhow::Result<()> {
        let _guard = crate::session_store::ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();

        let root = std::env::temp_dir().join(format!("acp-monitor-detail-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;

        unsafe {
            std::env::set_var("ACP_HOME", &root);
        }
        crate::record_client_info(Some("zed@canonical-log-test".to_string()));

        let mut idx = crate::session_store::GlobalSessionIndex::load()
            .expect("ACP_HOME should be resolvable");
        let global_id = idx.get_or_create("codex:test-monitor-detail").unwrap();
        let store = crate::session_store::SessionStore::init(
            global_id.clone(),
            "codex",
            "acp-session-id",
            "backend-session-id",
            Some(Path::new("/tmp/repo")),
        )
        .expect("SessionStore should init");

        let session_id = SessionId::new("test-monitor-detail");
        let client = Arc::new(StubClient::new());
        let session_client =
            SessionClient::with_client(session_id.clone(), client, Arc::default(), Some(store));
        let conversation = Arc::new(StubCodexThread::new());
        let models_manager = Arc::new(StubModelsManager);
        let config = Config::load_with_cli_overrides_and_harness_overrides(
            vec![],
            ConfigOverrides::default(),
        )
        .await?;
        let (message_tx, message_rx) = tokio::sync::mpsc::unbounded_channel();

        let actor = ThreadActor::new(
            StubAuth,
            session_client,
            conversation,
            models_manager,
            config,
            message_rx,
        );

        let local_set = LocalSet::new();
        local_set.spawn_local(actor.spawn());

        let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();
        message_tx.send(ThreadMessage::Prompt {
            request: PromptRequest::new(session_id.clone(), vec!["/monitor detail".into()]),
            response_tx: prompt_response_tx,
        })?;

        tokio::try_join!(
            async {
                let stop_reason = prompt_response_rx.await??.await??;
                assert_eq!(stop_reason, StopReason::EndTurn);
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        let canonical_path = root
            .join("sessions")
            .join(&global_id)
            .join("canonical.jsonl");
        let lines = std::fs::read_to_string(&canonical_path)?;
        let diagnostic_event = lines
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find(|value| {
                value.get("kind").and_then(|kind| kind.as_str()) == Some("acp.runtime_diagnostics")
            })
            .expect("expected acp.runtime_diagnostics canonical event");

        assert_eq!(
            diagnostic_event
                .pointer("/data/reason")
                .and_then(|v| v.as_str()),
            Some("monitor_detail")
        );
        assert_eq!(
            diagnostic_event
                .pointer("/data/client_info")
                .and_then(|v| v.as_str()),
            Some("zed@canonical-log-test")
        );
        assert_eq!(
            diagnostic_event
                .pointer("/data/ui_visibility_mode")
                .and_then(|v| v.as_str()),
            Some("full")
        );
        assert!(
            diagnostic_event
                .pointer("/data/current_rss_bytes")
                .is_some(),
            "runtime diagnostics event should include rss field: {diagnostic_event:?}"
        );

        crate::record_client_info(None);
        drop(std::fs::remove_dir_all(&root));
        unsafe {
            std::env::remove_var("ACP_HOME");
        }

        Ok(())
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_auto_runtime_diagnostics_logging_at_notification_threshold() -> anyhow::Result<()>
    {
        let _guard = crate::session_store::ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();

        let root =
            std::env::temp_dir().join(format!("acp-auto-diagnostics-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;

        let _auto_restore = EnvVarRestore::set(DIAGNOSTICS_AUTO_LOG_ENV, Some("on"));
        let _every_restore = EnvVarRestore::set(DIAGNOSTICS_LOG_EVERY_ENV, Some("5"));
        unsafe {
            std::env::set_var("ACP_HOME", &root);
        }
        crate::record_client_info(Some("zed@auto-log-test".to_string()));

        let mut idx = crate::session_store::GlobalSessionIndex::load()
            .expect("ACP_HOME should be resolvable");
        let global_id = idx
            .get_or_create("codex:test-auto-runtime-diagnostics")
            .unwrap();
        let store = crate::session_store::SessionStore::init(
            global_id.clone(),
            "codex",
            "acp-session-id",
            "backend-session-id",
            Some(Path::new("/tmp/repo")),
        )
        .expect("SessionStore should init");

        let session_client = SessionClient::with_client(
            SessionId::new("auto-runtime-diagnostics"),
            Arc::new(StubClient::new()),
            Arc::default(),
            Some(store),
        );

        assert!(diagnostics_auto_log_enabled_from_env());
        assert_eq!(diagnostics_log_every_from_env(), 5);
        session_client
            .diagnostics
            .notifications_sent
            .store(5, Ordering::Relaxed);
        session_client.maybe_log_runtime_diagnostics("unit_test");
        drop(session_client);

        let canonical_path = root
            .join("sessions")
            .join(&global_id)
            .join("canonical.jsonl");
        let lines = std::fs::read_to_string(&canonical_path)?;
        let diagnostic_event = lines
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find(|value| {
                value.get("kind").and_then(|kind| kind.as_str()) == Some("acp.runtime_diagnostics")
            });
        assert!(
            diagnostic_event.is_some(),
            "expected auto acp.runtime_diagnostics canonical event. canonical={lines}"
        );
        let diagnostic_event = diagnostic_event.unwrap();

        let reason = diagnostic_event
            .pointer("/data/reason")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        assert!(
            reason.starts_with("auto_notification_threshold:unit_test:5"),
            "unexpected auto diagnostics reason: {reason}"
        );

        crate::record_client_info(None);
        drop(std::fs::remove_dir_all(&root));
        unsafe {
            std::env::remove_var("ACP_HOME");
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_task_monitoring_auto_mode_updates_status() -> anyhow::Result<()> {
        let (session_id, client, _thread, message_tx, local_set) = setup(vec![]).await?;
        tokio::try_join!(
            async {
                let (set_config_tx, set_config_rx) = tokio::sync::oneshot::channel();
                message_tx.send(ThreadMessage::SetConfigOption {
                    config_id: SessionConfigId::new("task_monitoring_enabled"),
                    value: SessionConfigValueId::new("auto"),
                    response_tx: set_config_tx,
                })?;
                set_config_rx.await??;

                let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();
                message_tx.send(ThreadMessage::Prompt {
                    request: PromptRequest::new(session_id.clone(), vec!["/status".into()]),
                    response_tx: prompt_response_tx,
                })?;
                let stop_reason = prompt_response_rx.await??.await??;
                assert_eq!(stop_reason, StopReason::EndTurn);

                let notifications = client.notifications.lock().unwrap();
                assert!(
                    notifications.iter().any(|notification| {
                        matches!(
                            &notification.update,
                            SessionUpdate::AgentMessageChunk(ContentChunk {
                                content: ContentBlock::Text(TextContent { text, .. }),
                                ..
                            }) if text.contains("- task_monitoring: auto")
                                && text.contains("- work_orchestration_profile: Codex / ChatGPT (R->P->M->W->A)")
                                && text.contains("- acp_bridge: live ACP plan/tool updates available")
                        )
                    }),
                    "status should reflect the codex work-orchestration profile after setting config. notifications={notifications:?}"
                );
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;
        Ok(())
    }

    #[tokio::test]
    async fn test_monitoring_auto_mode_hides_task_queue_without_active_tasks() -> anyhow::Result<()>
    {
        let session_id = SessionId::new("test-monitor-auto-idle");
        let client = Arc::new(StubClient::new());
        let thread = Arc::new(StubCodexThread::new());
        let session_client =
            SessionClient::with_client(session_id.clone(), client, Arc::default(), None);
        let models_manager = Arc::new(StubModelsManager);
        let config = Config::load_with_cli_overrides_and_harness_overrides(
            vec![],
            ConfigOverrides::default(),
        )
        .await?;
        let (_message_tx, message_rx) = tokio::sync::mpsc::unbounded_channel();

        let mut actor = ThreadActor::new(
            StubAuth,
            session_client,
            thread,
            models_manager,
            config,
            message_rx,
        );
        actor.task_monitoring.monitor_mode = TaskMonitoringMode::Auto;
        actor.submissions = HashMap::new();

        assert!(
            !actor
                .render_task_monitoring_snapshot()
                .contains("Task queue:"),
            "auto mode should suppress task queue output when no active tasks exist. snapshot={:?}",
            actor.render_task_monitoring_snapshot()
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_new_prompt_preempts_active_submission_when_enabled() -> anyhow::Result<()> {
        let session_id = SessionId::new("preempt-test");
        let client = Arc::new(StubClient::new());
        let thread = Arc::new(StubCodexThread::new());
        let session_client =
            SessionClient::with_client(session_id.clone(), client, Arc::default(), None);
        let models_manager = Arc::new(StubModelsManager);
        let config = Config::load_with_cli_overrides_and_harness_overrides(
            vec![],
            ConfigOverrides::default(),
        )
        .await?;
        let (_message_tx, message_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut actor = ThreadActor::new(
            StubAuth,
            session_client,
            thread.clone(),
            models_manager,
            config,
            message_rx,
        );
        actor.task_monitoring.preempt_on_new_prompt = true;

        let (old_response_tx, old_response_rx) = tokio::sync::oneshot::channel();
        actor.submissions.insert(
            "old-submission".to_string(),
            SubmissionState::Prompt(PromptState::new(
                thread.clone(),
                old_response_tx,
                "old-submission".to_string(),
            )),
        );

        let _new_response_rx = actor
            .handle_prompt(PromptRequest::new(session_id, vec!["new request".into()]))
            .await?;

        let old_stop_reason = old_response_rx.await??;
        assert_eq!(old_stop_reason, StopReason::Cancelled);
        assert!(
            !actor.submissions.contains_key("old-submission"),
            "expected previous submission to be removed after preemption"
        );

        let ops = thread.ops.lock().unwrap();
        assert!(
            matches!(ops.first(), Some(Op::Interrupt)),
            "expected preemption to issue Op::Interrupt before new prompt. ops={ops:?}"
        );
        assert!(
            ops.iter().any(|op| matches!(op, Op::UserInput { .. })),
            "expected new prompt submission after preemption. ops={ops:?}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_monitoring_auto_mode_shows_task_queue_with_active_task() -> anyhow::Result<()> {
        let session_id = SessionId::new("test-monitor-auto-active");
        let client = Arc::new(StubClient::new());
        let thread = Arc::new(StubCodexThread::new());
        let session_client =
            SessionClient::with_client(session_id.clone(), client, Arc::default(), None);
        let models_manager = Arc::new(StubModelsManager);
        let config = Config::load_with_cli_overrides_and_harness_overrides(
            vec![],
            ConfigOverrides::default(),
        )
        .await?;
        let (_message_tx, message_rx) = tokio::sync::mpsc::unbounded_channel();

        let mut actor = ThreadActor::new(
            StubAuth,
            session_client,
            thread,
            models_manager,
            config,
            message_rx,
        );
        actor.task_monitoring.monitor_mode = TaskMonitoringMode::Auto;
        actor.submissions = HashMap::new();
        actor.submissions.insert(
            "submission-1".to_string(),
            SubmissionState::Task(TaskState::new_background(
                actor.thread.clone(),
                "submission-1".to_string(),
            )),
        );

        let snapshot = actor.render_task_monitoring_snapshot();
        assert!(
            snapshot.contains("Task queue: 1 active"),
            "auto mode should show queue when active tasks exist. snapshot={snapshot}"
        );
        assert!(
            snapshot.contains("[in_progress] task: submission-1"),
            "active task monitoring entry should include label and id. snapshot={snapshot}"
        );

        Ok(())
    }

    #[test]
    fn test_flow_vector_repeat_and_stall_signals_show_stagnation() {
        let mut flow = FlowVectorState::default();
        for _ in 0..MONITOR_REPEAT_STREAK_WARN {
            flow.record_phase('E', "exec begin: cargo test");
        }

        let now = Instant::now();
        flow.last_plan_update_at = Some(now.checked_sub(Duration::from_secs(90)).unwrap_or(now));
        flow.last_progress_at = Some(now.checked_sub(Duration::from_secs(90)).unwrap_or(now));
        flow.stalled_plan_updates = MONITOR_STALL_PLAN_UPDATE_WARN;

        let repeat_signal = flow.render_repeat_signal();
        assert!(
            repeat_signal.contains("repeated"),
            "repeat signal should flag same-action streaks. signal={repeat_signal}"
        );

        let stall_signal = flow.render_stall_signal(1);
        assert!(
            stall_signal.contains("consecutive plan updates"),
            "stall signal should flag repeated plan updates without progress. signal={stall_signal}"
        );
    }

    #[tokio::test]
    async fn test_progress_signals_snapshot_reports_long_running_open_tool_call()
    -> anyhow::Result<()> {
        let session_id = SessionId::new("test-progress-signals-bottleneck");
        let client = Arc::new(StubClient::new());
        let thread = Arc::new(StubCodexThread::new());
        let session_client =
            SessionClient::with_client(session_id.clone(), client, Arc::default(), None);
        let models_manager = Arc::new(StubModelsManager);
        let config = Config::load_with_cli_overrides_and_harness_overrides(
            vec![],
            ConfigOverrides::default(),
        )
        .await?;
        let (_message_tx, message_rx) = tokio::sync::mpsc::unbounded_channel();

        let mut actor = ThreadActor::new(
            StubAuth,
            session_client,
            thread,
            models_manager,
            config,
            message_rx,
        );

        let mut prompt = PromptState::new_background(actor.thread.clone(), "submission-1".into());
        let started_at = Instant::now()
            .checked_sub(Duration::from_secs(MONITOR_BOTTLENECK_SLOW_SECS + 5))
            .unwrap_or_else(Instant::now);
        prompt.open_tool_calls.insert(
            "exec-call-1".to_string(),
            OpenToolCall {
                kind: "exec",
                started_at,
            },
        );
        actor
            .submissions
            .insert("submission-1".to_string(), SubmissionState::Prompt(prompt));

        let snapshot = actor.render_progress_signals_snapshot(1);
        assert!(
            snapshot.contains("Bottleneck: `exec`"),
            "progress signals should include bottleneck tool kind. snapshot={snapshot}"
        );
        assert!(
            snapshot.contains("submission-1"),
            "progress signals should include submission id for bottleneck attribution. snapshot={snapshot}"
        );
        assert!(
            snapshot.contains("running for"),
            "progress signals should include bottleneck runtime duration. snapshot={snapshot}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_monitor_retrospective_command() -> anyhow::Result<()> {
        let (session_id, client, thread, message_tx, local_set) = setup(vec![]).await?;
        let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();

        message_tx.send(ThreadMessage::Prompt {
            request: PromptRequest::new(session_id.clone(), vec!["/monitor retro".into()]),
            response_tx: prompt_response_tx,
        })?;

        tokio::try_join!(
            async {
                let stop_reason = prompt_response_rx.await??.await??;
                assert_eq!(stop_reason, StopReason::EndTurn);
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        let notifications = client.notifications.lock().unwrap();
        let text_chunks = notifications
            .iter()
            .filter_map(|n| match &n.update {
                SessionUpdate::AgentMessageChunk(ContentChunk {
                    content: ContentBlock::Text(TextContent { text, .. }),
                    ..
                }) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            text_chunks
                .iter()
                .any(|text| text.contains("회고형 상태 보고서 (2026-02-14)")),
            "monitor retrospective output should include report header. notifications={notifications:?}"
        );
        assert!(
            text_chunks.iter().any(|text| text.contains(
                "1. brain job payload templates by type: 스펙 확정 + 예제 입력/출력 정의"
            )),
            "retrospective output should include item 1 title. notifications={notifications:?}"
        );

        let ops = thread.ops.lock().unwrap();
        assert!(
            ops.is_empty(),
            "monitor retro command should not submit backend op"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_vector_command() -> anyhow::Result<()> {
        let (session_id, client, thread, message_tx, local_set) = setup(vec![]).await?;
        let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();

        message_tx.send(ThreadMessage::Prompt {
            request: PromptRequest::new(session_id.clone(), vec!["/vector".into()]),
            response_tx: prompt_response_tx,
        })?;

        tokio::try_join!(
            async {
                let stop_reason = prompt_response_rx.await??.await??;
                assert_eq!(stop_reason, StopReason::EndTurn);
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        let notifications = client.notifications.lock().unwrap();
        let text_chunks = notifications
            .iter()
            .filter_map(|n| match &n.update {
                SessionUpdate::AgentMessageChunk(ContentChunk {
                    content: ContentBlock::Text(TextContent { text, .. }),
                    ..
                }) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            text_chunks
                .iter()
                .any(|text| text.contains("Workflow minimap + semantic compass")),
            "notifications don't match {notifications:?}"
        );

        let ops = thread.ops.lock().unwrap();
        assert!(
            ops.is_empty(),
            "vector command should not submit backend op"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_new_window_command() -> anyhow::Result<()> {
        let (session_id, client, thread, message_tx, local_set) = setup(vec![]).await?;
        let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();

        message_tx.send(ThreadMessage::Prompt {
            request: PromptRequest::new(session_id.clone(), vec!["/new-window".into()]),
            response_tx: prompt_response_tx,
        })?;

        tokio::try_join!(
            async {
                let stop_reason = prompt_response_rx.await??.await??;
                assert_eq!(stop_reason, StopReason::EndTurn);
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        let notifications = client.notifications.lock().unwrap();
        let text_chunks = notifications
            .iter()
            .filter_map(|n| match &n.update {
                SessionUpdate::AgentMessageChunk(ContentChunk {
                    content: ContentBlock::Text(TextContent { text, .. }),
                    ..
                }) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            text_chunks.iter().any(|text| text.contains("thread list")),
            "notifications don't match {notifications:?}"
        );

        let ops = thread.ops.lock().unwrap();
        assert!(
            ops.is_empty(),
            "new-window command should not submit backend op"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_experimental_command() -> anyhow::Result<()> {
        let (session_id, client, thread, message_tx, local_set) = setup(vec![]).await?;
        let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();

        message_tx.send(ThreadMessage::Prompt {
            request: PromptRequest::new(session_id.clone(), vec!["/experimental".into()]),
            response_tx: prompt_response_tx,
        })?;

        tokio::try_join!(
            async {
                let stop_reason = prompt_response_rx.await??.await??;
                assert_eq!(stop_reason, StopReason::EndTurn);
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        let notifications = client.notifications.lock().unwrap();
        let text_chunks = notifications
            .iter()
            .filter_map(|n| match &n.update {
                SessionUpdate::AgentMessageChunk(ContentChunk {
                    content: ContentBlock::Text(TextContent { text, .. }),
                    ..
                }) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            text_chunks
                .iter()
                .any(|text| text.contains("Experimental features")),
            "notifications don't match {notifications:?}"
        );

        let ops = thread.ops.lock().unwrap();
        assert!(
            ops.is_empty(),
            "experimental command should not submit backend op"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_setup_command() -> anyhow::Result<()> {
        let (session_id, client, thread, message_tx, local_set) = setup(vec![]).await?;
        let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();

        message_tx.send(ThreadMessage::Prompt {
            request: PromptRequest::new(session_id.clone(), vec!["/setup".into()]),
            response_tx: prompt_response_tx,
        })?;

        tokio::try_join!(
            async {
                let stop_reason = prompt_response_rx.await??.await??;
                assert_eq!(stop_reason, StopReason::EndTurn);
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        let notifications = client.notifications.lock().unwrap();
        let text_chunks = notifications
            .iter()
            .filter_map(|n| match &n.update {
                SessionUpdate::AgentMessageChunk(ContentChunk {
                    content: ContentBlock::Text(TextContent { text, .. }),
                    ..
                }) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            text_chunks.iter().any(|text| text.contains("setup wizard")),
            "notifications don't match {notifications:?}"
        );
        assert!(
            text_chunks
                .iter()
                .any(|text| text.contains("R->P->M->W->A")
                    && text.contains("Codex / ChatGPT evidence")),
            "setup wizard should expose the codex work-orchestration baseline. notifications={notifications:?}"
        );

        let plan = notifications.iter().find_map(|n| match &n.update {
            SessionUpdate::Plan(plan) => Some(plan),
            _ => None,
        });
        let Some(plan) = plan else {
            panic!("expected /setup to emit SessionUpdate::Plan. notifications={notifications:?}");
        };
        let steps = plan
            .entries
            .iter()
            .map(|entry| entry.content.as_str())
            .collect::<Vec<_>>();
        for expected in [
            "Protocol: Goal -> Rubric(Must/Should+Evidence) locked",
            "Protocol: R->P->M->W->A sequence active (Codex / ChatGPT)",
            "Setup: authentication",
            "Setup: choose model",
            "Setup: choose reasoning effort",
            "Setup: choose approval preset",
            "Setup: enable context optimization telemetry",
            "Defaults: parallel task orchestration",
            "Defaults: task monitoring mode enabled",
            "Defaults: progress vector checks enabled",
        ] {
            assert!(
                steps.contains(&expected),
                "expected plan to include step {expected:?}. steps={steps:?}"
            );
        }
        assert!(
            steps.iter().any(|entry| {
                entry.starts_with(
                    "Loop gate: iterate Research -> Rubric -> Plan -> Implement -> Verify -> Score until Must=100% (",
                )
            }),
            "expected plan to include rubric loop gate step. steps={steps:?}"
        );
        assert!(
            steps
                .iter()
                .any(|entry| entry.starts_with("Verify: run /status, /monitor, and /vector (")),
            "expected plan to include verify progress step. steps={steps:?}"
        );

        let ops = thread.ops.lock().unwrap();
        assert!(ops.is_empty(), "setup command should not submit backend op");

        Ok(())
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn test_setup_emits_zed_plan_progress_summary() -> anyhow::Result<()> {
        let _guard = crate::session_store::ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();

        crate::record_client_info(Some("zed@plan-progress-test".to_string()));
        let result = async {
            let (session_id, client, thread, message_tx, local_set) = setup(vec![]).await?;
            let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();

            message_tx.send(ThreadMessage::Prompt {
                request: PromptRequest::new(session_id.clone(), vec!["/setup".into()]),
                response_tx: prompt_response_tx,
            })?;

            tokio::try_join!(
                async {
                    let stop_reason = prompt_response_rx.await??.await??;
                    assert_eq!(stop_reason, StopReason::EndTurn);
                    drop(message_tx);
                    anyhow::Ok(())
                },
                async {
                    local_set.await;
                    anyhow::Ok(())
                }
            )?;

            let notifications = client.notifications.lock().unwrap();
            let plan = notifications.iter().find_map(|n| match &n.update {
                SessionUpdate::Plan(plan) => Some(plan),
                _ => None,
            });
            let Some(plan) = plan else {
                panic!(
                    "expected /setup to emit SessionUpdate::Plan. notifications={notifications:?}"
                );
            };

            let summary = plan
                .entries
                .first()
                .expect("expected progress summary entry");
            assert!(
                summary.content.starts_with("Progress: ["),
                "expected zed progress summary first. plan={plan:?}"
            );
            assert!(
                summary.content.contains('%'),
                "expected progress summary to include percent. plan={plan:?}"
            );
            assert!(
                plan.entries
                    .iter()
                    .any(|entry| entry.content == "Setup: authentication"),
                "expected original setup step to remain. plan={plan:?}"
            );

            let ops = thread.ops.lock().unwrap();
            assert!(ops.is_empty(), "setup command should not submit backend op");

            Ok(())
        }
        .await;
        crate::record_client_info(None);
        result
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn test_setup_does_not_emit_plan_progress_summary_for_non_zed_client()
    -> anyhow::Result<()> {
        let _guard = crate::session_store::ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();

        crate::record_client_info(Some("other-client@plan-progress-test".to_string()));
        let result = async {
            let (session_id, client, thread, message_tx, local_set) = setup(vec![]).await?;
            let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();

            message_tx.send(ThreadMessage::Prompt {
                request: PromptRequest::new(session_id.clone(), vec!["/setup".into()]),
                response_tx: prompt_response_tx,
            })?;

            tokio::try_join!(
                async {
                    let stop_reason = prompt_response_rx.await??.await??;
                    assert_eq!(stop_reason, StopReason::EndTurn);
                    drop(message_tx);
                    anyhow::Ok(())
                },
                async {
                    local_set.await;
                    anyhow::Ok(())
                }
            )?;

            let notifications = client.notifications.lock().unwrap();
            let plan = notifications.iter().find_map(|n| match &n.update {
                SessionUpdate::Plan(plan) => Some(plan),
                _ => None,
            });
            let Some(plan) = plan else {
                panic!(
                    "expected /setup to emit SessionUpdate::Plan. notifications={notifications:?}"
                );
            };

            let first_entry = plan
                .entries
                .first()
                .expect("expected setup plan to have entries");
            assert_eq!(
                first_entry.content, "Protocol: Goal -> Rubric(Must/Should+Evidence) locked",
                "expected non-zed clients to keep the first setup step unchanged. plan={plan:?}"
            );
            assert!(
                plan.entries
                    .iter()
                    .all(|entry| !entry.content.starts_with("Progress: [")),
                "expected non-zed clients to omit progress summary row. plan={plan:?}"
            );

            let ops = thread.ops.lock().unwrap();
            assert!(ops.is_empty(), "setup command should not submit backend op");

            Ok(())
        }
        .await;
        crate::record_client_info(None);
        result
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_update_plan_emits_visible_progress_text_for_non_zed_client() -> anyhow::Result<()>
    {
        let _guard = crate::session_store::ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();

        crate::record_client_info(Some("other-client@plan-text-test".to_string()));
        let result = async {
            let session_id = SessionId::new("plan-text-test");
            let client = Arc::new(StubClient::new());
            let session_client =
                SessionClient::with_client(session_id, client.clone(), Arc::default(), None);

            session_client
                .update_plan(
                    vec![
                        PlanItemArg {
                            step: "Research current ACP behavior".to_string(),
                            status: StepStatus::Completed,
                        },
                        PlanItemArg {
                            step: "Patch ACP runtime behavior".to_string(),
                            status: StepStatus::InProgress,
                        },
                        PlanItemArg {
                            step: "Verify targeted regressions".to_string(),
                            status: StepStatus::Pending,
                        },
                    ],
                    Some("Keep ACP users aware of live plan progress.".to_string()),
                )
                .await;

            let notifications = client.notifications.lock().unwrap();
            let plan = notifications.iter().find_map(|notification| match &notification.update {
                SessionUpdate::Plan(plan) => Some(plan),
                _ => None,
            });
            let Some(plan) = plan else {
                panic!("expected plan notification. notifications={notifications:?}");
            };

            assert!(
                plan.entries.iter().all(|entry| {
                    !entry.content.starts_with("Progress: [") && !entry.content.starts_with("Plan update:")
                }),
                "expected plan entries to stay canonical for non-zed clients. plan={plan:?}"
            );

            let text_chunks = notifications
                .iter()
                .filter_map(|notification| match &notification.update {
                    SessionUpdate::AgentMessageChunk(ContentChunk {
                        content: ContentBlock::Text(TextContent { text, .. }),
                        ..
                    }) => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>();

            assert!(
                text_chunks.iter().any(|text| text.contains(
                    "Plan update: 1/3 completed, 1 in progress, 1 pending."
                )),
                "expected visible plan progress text for non-zed client. notifications={notifications:?}"
            );
            assert!(
                text_chunks
                    .iter()
                    .any(|text| text.contains("Current: Patch ACP runtime behavior")),
                "expected plan progress text to include current step. notifications={notifications:?}"
            );
            assert!(
                text_chunks.iter().any(|text| {
                    text.contains("Note: Keep ACP users aware of live plan progress.")
                }),
                "expected plan progress text to include explanation. notifications={notifications:?}"
            );

            Ok(())
        }
        .await;
        crate::record_client_info(None);
        result
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_update_plan_avoids_duplicate_progress_text_for_zed_client() -> anyhow::Result<()>
    {
        let _guard = crate::session_store::ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();

        crate::record_client_info(Some("zed@plan-text-test".to_string()));
        let result = async {
            let session_id = SessionId::new("plan-text-zed-test");
            let client = Arc::new(StubClient::new());
            let session_client =
                SessionClient::with_client(session_id, client.clone(), Arc::default(), None);

            session_client
                .update_plan(
                    vec![PlanItemArg {
                        step: "Patch ACP runtime behavior".to_string(),
                        status: StepStatus::InProgress,
                    }],
                    Some("Zed already renders plan updates.".to_string()),
                )
                .await;

            let notifications = client.notifications.lock().unwrap();
            let text_chunks = notifications
                .iter()
                .filter_map(|notification| match &notification.update {
                    SessionUpdate::AgentMessageChunk(ContentChunk {
                        content: ContentBlock::Text(TextContent { text, .. }),
                        ..
                    }) => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>();

            assert!(
                text_chunks
                    .iter()
                    .all(|text| !text.contains("Plan update:")),
                "expected zed client to rely on plan rows instead of duplicate text. notifications={notifications:?}"
            );

            Ok(())
        }
        .await;
        crate::record_client_info(None);
        result
    }

    #[tokio::test]
    async fn test_setup_plan_visible_in_monitor_output() -> anyhow::Result<()> {
        let (session_id, client, _, message_tx, local_set) = setup(vec![]).await?;

        async fn send_prompt(
            message_tx: &UnboundedSender<ThreadMessage>,
            session_id: &SessionId,
            prompt: &str,
        ) -> anyhow::Result<StopReason> {
            let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();
            message_tx.send(ThreadMessage::Prompt {
                request: PromptRequest::new(session_id.clone(), vec![prompt.into()]),
                response_tx: prompt_response_tx,
            })?;
            Ok(prompt_response_rx.await??.await??)
        }

        tokio::try_join!(
            async {
                assert_eq!(
                    send_prompt(&message_tx, &session_id, "/setup").await?,
                    StopReason::EndTurn
                );
                assert_eq!(
                    send_prompt(&message_tx, &session_id, "/monitor").await?,
                    StopReason::EndTurn
                );
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        let notifications = client.notifications.lock().unwrap();
        let monitor_text = notifications
            .iter()
            .filter_map(|n| match &n.update {
                SessionUpdate::AgentMessageChunk(ContentChunk {
                    content: ContentBlock::Text(TextContent { text, .. }),
                    ..
                }) if text.contains("Thread monitor") => Some(text.as_str()),
                _ => None,
            })
            .next_back()
            .expect("expected /monitor output");

        assert!(
            monitor_text.contains("Verify: run /status, /monitor, and /vector ("),
            "monitor output should include current setup plan steps. notifications={notifications:?}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_setup_plan_verification_progress_updates() -> anyhow::Result<()> {
        let (session_id, client, _, message_tx, local_set) = setup(vec![]).await?;

        async fn send_prompt(
            message_tx: &UnboundedSender<ThreadMessage>,
            session_id: &SessionId,
            prompt: &str,
        ) -> anyhow::Result<StopReason> {
            let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();
            message_tx.send(ThreadMessage::Prompt {
                request: PromptRequest::new(session_id.clone(), vec![prompt.into()]),
                response_tx: prompt_response_tx,
            })?;
            Ok(prompt_response_rx.await??.await??)
        }

        tokio::try_join!(
            async {
                assert_eq!(
                    send_prompt(&message_tx, &session_id, "/setup").await?,
                    StopReason::EndTurn
                );
                assert_eq!(
                    send_prompt(&message_tx, &session_id, "/status").await?,
                    StopReason::EndTurn
                );
                assert_eq!(
                    send_prompt(&message_tx, &session_id, "/monitor").await?,
                    StopReason::EndTurn
                );
                assert_eq!(
                    send_prompt(&message_tx, &session_id, "/vector").await?,
                    StopReason::EndTurn
                );
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        let notifications = client.notifications.lock().unwrap();
        let plans = notifications
            .iter()
            .filter_map(|n| match &n.update {
                SessionUpdate::Plan(plan) => Some(plan),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert!(
            plans.len() >= 4,
            "expected multiple plan updates as setup verification progresses. notifications={notifications:?}"
        );

        let verify_step = plans
            .last()
            .and_then(|plan| {
                plan.entries.iter().find(|entry| {
                    entry
                        .content
                        .starts_with("Verify: run /status, /monitor, and /vector (")
                })
            })
            .expect("expected verify step in latest setup plan");

        assert_eq!(
            verify_step.status,
            PlanEntryStatus::Completed,
            "verify step should be completed after running /status, /monitor, /vector"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_monitoring_auto_mode_clears_completed_prompt_tasks() -> anyhow::Result<()> {
        let (session_id, client, _, message_tx, local_set) = setup(vec![]).await?;

        async fn send_prompt(
            message_tx: &UnboundedSender<ThreadMessage>,
            session_id: &SessionId,
            prompt: &str,
        ) -> anyhow::Result<StopReason> {
            let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();
            message_tx.send(ThreadMessage::Prompt {
                request: PromptRequest::new(session_id.clone(), vec![prompt.into()]),
                response_tx: prompt_response_tx,
            })?;
            Ok(prompt_response_rx.await??.await??)
        }

        tokio::try_join!(
            async {
                assert_eq!(
                    send_prompt(&message_tx, &session_id, "hello").await?,
                    StopReason::EndTurn
                );
                assert_eq!(
                    send_prompt(&message_tx, &session_id, "/monitor").await?,
                    StopReason::EndTurn
                );
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        let notifications = client.notifications.lock().unwrap();
        let monitor_text = notifications
            .iter()
            .filter_map(|n| match &n.update {
                SessionUpdate::AgentMessageChunk(ContentChunk {
                    content: ContentBlock::Text(TextContent { text, .. }),
                    ..
                }) if text.contains("Thread monitor") => Some(text.as_str()),
                _ => None,
            })
            .next_back()
            .expect("expected /monitor output");

        assert!(
            !monitor_text.contains("Task queue:"),
            "auto mode should hide task queue after completed prompts. notifications={notifications:?}"
        );

        Ok(())
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_config_options_compact_density_hides_advanced_groups() -> anyhow::Result<()> {
        let _guard = crate::session_store::ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();
        let _density_restore = EnvVarRestore::set(CONFIG_OPTIONS_DENSITY_ENV, Some("compact"));
        let _columns_restore = EnvVarRestore::set("COLUMNS", Some("200"));
        let actor = test_actor_for_config_options().await?;
        let options = actor.config_options().await?;

        assert!(!has_option(&options, "advanced_options_panel"));
        assert!(!has_option(&options, "context_optimization_mode"));
        assert!(!has_option(
            &options,
            "context_optimization_trigger_percent"
        ));
        assert!(!has_option(&options, "task_orchestration_mode"));
        assert!(!has_option(&options, "task_monitoring_enabled"));
        assert!(!has_option(&options, "task_vector_check_enabled"));
        assert!(!has_option(&options, "preempt_on_new_prompt"));
        assert!(
            !options.iter().any(|option| option
                .id
                .0
                .as_ref()
                .starts_with(FEATURE_CONFIG_ID_PREFIX))
        );

        Ok(())
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_config_options_full_density_narrow_width_shows_panel_selector()
    -> anyhow::Result<()> {
        let _guard = crate::session_store::ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();
        let _density_restore = EnvVarRestore::set(CONFIG_OPTIONS_DENSITY_ENV, Some("full"));
        let _columns_restore = EnvVarRestore::set("COLUMNS", Some("120"));
        let actor = test_actor_for_config_options().await?;
        let options = actor.config_options().await?;

        assert!(has_option(&options, "advanced_options_panel"));
        assert!(has_option(&options, "context_optimization_mode"));
        assert!(has_option(&options, "context_optimization_trigger_percent"));
        assert!(!has_option(&options, "task_orchestration_mode"));
        assert!(!has_option(&options, "task_monitoring_enabled"));
        assert!(!has_option(&options, "task_vector_check_enabled"));
        assert!(!has_option(&options, "preempt_on_new_prompt"));
        assert!(
            !options.iter().any(|option| option
                .id
                .0
                .as_ref()
                .starts_with(FEATURE_CONFIG_ID_PREFIX))
        );

        Ok(())
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_config_options_panel_switch_changes_visible_group() -> anyhow::Result<()> {
        let _guard = crate::session_store::ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();
        let _density_restore = EnvVarRestore::set(CONFIG_OPTIONS_DENSITY_ENV, Some("full"));
        let _columns_restore = EnvVarRestore::set("COLUMNS", Some("120"));
        let mut actor = test_actor_for_config_options().await?;
        actor.advanced_options_panel = AdvancedOptionsPanel::Tasks;
        let options = actor.config_options().await?;

        assert!(has_option(&options, "advanced_options_panel"));
        assert!(!has_option(&options, "context_optimization_mode"));
        assert!(!has_option(
            &options,
            "context_optimization_trigger_percent"
        ));
        assert!(has_option(&options, "task_orchestration_mode"));
        assert!(has_option(&options, "task_monitoring_enabled"));
        assert!(has_option(&options, "task_vector_check_enabled"));
        assert!(has_option(&options, "preempt_on_new_prompt"));
        assert!(
            !options.iter().any(|option| option
                .id
                .0
                .as_ref()
                .starts_with(FEATURE_CONFIG_ID_PREFIX))
        );

        Ok(())
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_config_options_full_density_wide_width_inlines_all_advanced_options()
    -> anyhow::Result<()> {
        let _guard = crate::session_store::ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();
        let _density_restore = EnvVarRestore::set(CONFIG_OPTIONS_DENSITY_ENV, Some("full"));
        let _columns_restore = EnvVarRestore::set("COLUMNS", Some("160"));
        let actor = test_actor_for_config_options().await?;
        let options = actor.config_options().await?;

        assert!(!has_option(&options, "advanced_options_panel"));
        assert!(has_option(&options, "context_optimization_mode"));
        assert!(has_option(&options, "context_optimization_trigger_percent"));
        assert!(has_option(&options, "task_orchestration_mode"));
        assert!(has_option(&options, "task_monitoring_enabled"));
        assert!(has_option(&options, "task_vector_check_enabled"));
        assert!(has_option(&options, "preempt_on_new_prompt"));
        assert!(
            options
                .iter()
                .any(|option| option.id.0.as_ref().starts_with(FEATURE_CONFIG_ID_PREFIX))
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_set_config_rejects_invalid_advanced_panel_value() -> anyhow::Result<()> {
        let mut actor = test_actor_for_config_options().await?;
        let err = actor
            .handle_set_config_option(
                SessionConfigId::new("advanced_options_panel"),
                SessionConfigValueId::new("invalid"),
            )
            .await
            .expect_err("invalid panel value should fail");

        assert!(
            format!("{err:?}")
                .contains("Advanced Panel values must be one of: context, tasks, beta")
        );

        Ok(())
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_canonical_log_correlation_path() -> anyhow::Result<()> {
        let _guard = crate::session_store::ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();

        let root =
            std::env::temp_dir().join(format!("acp-thread-correlation-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;

        // Safe within this test due to ENV_LOCK serialization.
        unsafe {
            std::env::set_var("ACP_HOME", &root);
        }

        let mut idx = crate::session_store::GlobalSessionIndex::load()
            .expect("ACP_HOME should be resolvable");
        let global_id = idx.get_or_create("codex:test-thread-correlation").unwrap();

        let store = crate::session_store::SessionStore::init(
            global_id.clone(),
            "codex",
            "acp-session-id",
            "backend-session-id",
            Some(Path::new("/tmp/repo")),
        )
        .expect("SessionStore should init");

        let session_id = SessionId::new("test");
        let client = Arc::new(StubClient::new());
        let session_client = SessionClient::with_client(
            session_id.clone(),
            client.clone(),
            Arc::default(),
            Some(store),
        );
        let conversation = Arc::new(StubCodexThread::new());
        let models_manager = Arc::new(StubModelsManager);
        let config = Config::load_with_cli_overrides_and_harness_overrides(
            vec![],
            ConfigOverrides::default(),
        )
        .await?;
        let (message_tx, message_rx) = tokio::sync::mpsc::unbounded_channel();

        let actor = ThreadActor::new(
            StubAuth,
            session_client,
            conversation,
            models_manager,
            config,
            message_rx,
        );

        let local_set = LocalSet::new();
        local_set.spawn_local(actor.spawn());

        let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();
        message_tx.send(ThreadMessage::Prompt {
            request: PromptRequest::new(session_id.clone(), vec!["/diff".into()]),
            response_tx: prompt_response_tx,
        })?;

        tokio::try_join!(
            async {
                let stop_reason = prompt_response_rx.await??.await??;
                assert_eq!(stop_reason, StopReason::EndTurn);
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        let canonical_path = root
            .join("sessions")
            .join(&global_id)
            .join("canonical.jsonl");
        let s = std::fs::read_to_string(&canonical_path)?;

        let mut prompt_idx: Option<usize> = None;
        let mut plan_idx: Option<usize> = None;
        let mut request_idx: Option<usize> = None;
        let mut response_idx: Option<usize> = None;
        let mut tool_call_idx: Option<usize> = None;

        let mut plan_explanation: Option<String> = None;
        let mut permission_tool_call_id: Option<String> = None;
        let mut tool_call_id: Option<String> = None;

        for (i, line) in s.lines().enumerate() {
            let v: serde_json::Value = serde_json::from_str(line)?;
            let kind = v.get("kind").and_then(|k| k.as_str()).unwrap_or_default();
            match kind {
                "acp.prompt" if prompt_idx.is_none() => prompt_idx = Some(i),
                "acp.plan" if plan_idx.is_none() => {
                    plan_idx = Some(i);
                    plan_explanation = v
                        .pointer("/data/explanation")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }
                "acp.request_permission" if request_idx.is_none() => {
                    request_idx = Some(i);
                    permission_tool_call_id = v
                        .pointer("/data/tool_call/toolCallId")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }
                "acp.request_permission_response" if response_idx.is_none() => {
                    response_idx = Some(i)
                }
                "acp.tool_call" if tool_call_idx.is_none() => {
                    tool_call_idx = Some(i);
                    tool_call_id = v
                        .pointer("/data/toolCallId")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }
                _ => {}
            }
        }

        let prompt_idx = prompt_idx.expect("expected acp.prompt event");
        let plan_idx = plan_idx.expect("expected acp.plan event");
        let request_idx = request_idx.expect("expected acp.request_permission event");
        let response_idx = response_idx.expect("expected acp.request_permission_response event");
        let tool_call_idx = tool_call_idx.expect("expected acp.tool_call event");

        assert!(
            prompt_idx < plan_idx,
            "expected acp.prompt before acp.plan. prompt={prompt_idx} plan={plan_idx}"
        );
        assert!(
            plan_idx < request_idx,
            "expected acp.plan before acp.request_permission. plan={plan_idx} request={request_idx}"
        );
        assert!(
            request_idx < response_idx,
            "expected acp.request_permission before response. request={request_idx} response={response_idx}"
        );
        assert!(
            response_idx < tool_call_idx,
            "expected permission response before tool call. response={response_idx} tool_call={tool_call_idx}"
        );

        assert_eq!(
            plan_explanation.as_deref(),
            Some("Test plan explanation"),
            "expected acp.plan to include explanation"
        );
        assert_eq!(
            permission_tool_call_id, tool_call_id,
            "expected permission toolCallId to match tool call id"
        );

        drop(std::fs::remove_dir_all(&root));
        // Safe within this test due to ENV_LOCK serialization.
        unsafe {
            std::env::remove_var("ACP_HOME");
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_skills() -> anyhow::Result<()> {
        let (session_id, client, thread, message_tx, local_set) = setup(vec![]).await?;
        let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();

        message_tx.send(ThreadMessage::Prompt {
            request: PromptRequest::new(session_id.clone(), vec!["/skills".into()]),
            response_tx: prompt_response_tx,
        })?;

        tokio::try_join!(
            async {
                let stop_reason = prompt_response_rx.await??.await??;
                assert_eq!(stop_reason, StopReason::EndTurn);
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        let notifications = client.notifications.lock().unwrap();
        assert_eq!(notifications.len(), 1);
        assert!(
            matches!(
                &notifications[0].update,
                SessionUpdate::AgentMessageChunk(ContentChunk {
                    content: ContentBlock::Text(TextContent { text, .. }),
                    ..
                }) if text.contains("demo-skill") && text.contains("--enabled")
            ),
            "notifications don't match {notifications:?}"
        );

        let ops = thread.ops.lock().unwrap();
        assert_eq!(
            ops.as_slice(),
            &[Op::ListSkills {
                cwds: vec![],
                force_reload: false,
            }]
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_skills_with_reload_option() -> anyhow::Result<()> {
        let (session_id, client, thread, message_tx, local_set) = setup(vec![]).await?;
        let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();

        message_tx.send(ThreadMessage::Prompt {
            request: PromptRequest::new(session_id.clone(), vec!["/skills --reload".into()]),
            response_tx: prompt_response_tx,
        })?;

        tokio::try_join!(
            async {
                let stop_reason = prompt_response_rx.await??.await??;
                assert_eq!(stop_reason, StopReason::EndTurn);
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        let notifications = client.notifications.lock().unwrap();
        assert_eq!(notifications.len(), 1);
        assert!(
            matches!(
                &notifications[0].update,
                SessionUpdate::AgentMessageChunk(ContentChunk {
                    content: ContentBlock::Text(TextContent { text, .. }),
                    ..
                }) if text.contains("reload=true")
            ),
            "notifications don't match {notifications:?}"
        );

        let ops = thread.ops.lock().unwrap();
        assert_eq!(
            ops.as_slice(),
            &[Op::ListSkills {
                cwds: vec![],
                force_reload: true,
            }]
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_skills_with_enabled_filter_option() -> anyhow::Result<()> {
        let (session_id, client, thread, message_tx, local_set) = setup(vec![]).await?;
        let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();

        message_tx.send(ThreadMessage::Prompt {
            request: PromptRequest::new(session_id.clone(), vec!["/skills --enabled".into()]),
            response_tx: prompt_response_tx,
        })?;

        tokio::try_join!(
            async {
                let stop_reason = prompt_response_rx.await??.await??;
                assert_eq!(stop_reason, StopReason::EndTurn);
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        let notifications = client.notifications.lock().unwrap();
        assert_eq!(notifications.len(), 1);
        assert!(
            matches!(
                &notifications[0].update,
                SessionUpdate::AgentMessageChunk(ContentChunk {
                    content: ContentBlock::Text(TextContent { text, .. }),
                    ..
                }) if text.contains("enabled=enabled only")
            ),
            "notifications don't match {notifications:?}"
        );

        let ops = thread.ops.lock().unwrap();
        assert_eq!(
            ops.as_slice(),
            &[Op::ListSkills {
                cwds: vec![],
                force_reload: false,
            }]
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_skills_with_invalid_option_returns_usage_without_submit() -> anyhow::Result<()> {
        let (session_id, client, thread, message_tx, local_set) = setup(vec![]).await?;
        let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();

        message_tx.send(ThreadMessage::Prompt {
            request: PromptRequest::new(session_id.clone(), vec!["/skills --invalid".into()]),
            response_tx: prompt_response_tx,
        })?;

        tokio::try_join!(
            async {
                let stop_reason = prompt_response_rx.await??.await??;
                assert_eq!(stop_reason, StopReason::EndTurn);
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        let notifications = client.notifications.lock().unwrap();
        assert_eq!(notifications.len(), 1);
        assert!(
            matches!(
                &notifications[0].update,
                SessionUpdate::AgentMessageChunk(ContentChunk {
                    content: ContentBlock::Text(TextContent { text, .. }),
                    ..
                }) if text.contains("Invalid /skills option") && text.contains("Usage:")
            ),
            "notifications don't match {notifications:?}"
        );

        let ops = thread.ops.lock().unwrap();
        assert!(
            ops.is_empty(),
            "no op should be submitted for invalid /skills options, got {ops:?}"
        );

        Ok(())
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_init() -> anyhow::Result<()> {
        let _guard = crate::session_store::ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();
        let _visibility_restore = EnvVarRestore::set("ACP_UI_VISIBILITY_MODE", Some("full"));
        let _chunk_restore = EnvVarRestore::set("ACP_UI_TEXT_CHUNK_MAX_CHARS", Some("12000"));

        let (session_id, client, thread, message_tx, local_set) = setup(vec![]).await?;
        let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();

        message_tx.send(ThreadMessage::Prompt {
            request: PromptRequest::new(session_id.clone(), vec!["/init".into()]),
            response_tx: prompt_response_tx,
        })?;

        tokio::try_join!(
            async {
                let stop_reason = prompt_response_rx.await??.await??;
                assert_eq!(stop_reason, StopReason::EndTurn);
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        let notifications = client.notifications.lock().unwrap();
        assert_eq!(notifications.len(), 1);
        assert!(
            matches!(
                &notifications[0].update,
                SessionUpdate::AgentMessageChunk(ContentChunk {
                    content: ContentBlock::Text(TextContent { text, .. }), ..
                }) if text == INIT_COMMAND_PROMPT // we echo the prompt
            ),
            "notifications don't match {notifications:?}"
        );
        let ops = thread.ops.lock().unwrap();
        assert_eq!(
            ops.as_slice(),
            &[Op::UserInput {
                items: vec![UserInput::Text {
                    text: INIT_COMMAND_PROMPT.to_string(),
                    text_elements: vec![]
                }],
                final_output_json_schema: None,
            }],
            "ops don't match {ops:?}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_review() -> anyhow::Result<()> {
        let (session_id, client, thread, message_tx, local_set) = setup(vec![]).await?;
        let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();

        message_tx.send(ThreadMessage::Prompt {
            request: PromptRequest::new(session_id.clone(), vec!["/review".into()]),
            response_tx: prompt_response_tx,
        })?;

        tokio::try_join!(
            async {
                let stop_reason = prompt_response_rx.await??.await??;
                assert_eq!(stop_reason, StopReason::EndTurn);
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        let notifications = client.notifications.lock().unwrap();
        assert_eq!(notifications.len(), 1);
        assert!(
            matches!(
                &notifications[0].update,
                SessionUpdate::AgentMessageChunk(ContentChunk {
                    content: ContentBlock::Text(TextContent { text, .. }),
                    ..
                }) if text == "current changes" // we echo the prompt
            ),
            "notifications don't match {notifications:?}"
        );

        let ops = thread.ops.lock().unwrap();
        assert_eq!(
            ops.as_slice(),
            &[Op::Review {
                review_request: ReviewRequest {
                    user_facing_hint: Some(user_facing_hint(&ReviewTarget::UncommittedChanges)),
                    target: ReviewTarget::UncommittedChanges,
                }
            }],
            "ops don't match {ops:?}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_custom_review() -> anyhow::Result<()> {
        let (session_id, client, thread, message_tx, local_set) = setup(vec![]).await?;
        let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();
        let instructions = "Review what we did in agents.md";

        message_tx.send(ThreadMessage::Prompt {
            request: PromptRequest::new(
                session_id.clone(),
                vec![format!("/review {instructions}").into()],
            ),
            response_tx: prompt_response_tx,
        })?;

        tokio::try_join!(
            async {
                let stop_reason = prompt_response_rx.await??.await??;
                assert_eq!(stop_reason, StopReason::EndTurn);
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        let notifications = client.notifications.lock().unwrap();
        assert_eq!(notifications.len(), 1);
        assert!(
            matches!(
                &notifications[0].update,
                SessionUpdate::AgentMessageChunk(ContentChunk {
                    content: ContentBlock::Text(TextContent { text, .. }),
                    ..
                }) if text == "Review what we did in agents.md" // we echo the prompt
            ),
            "notifications don't match {notifications:?}"
        );

        let ops = thread.ops.lock().unwrap();
        assert_eq!(
            ops.as_slice(),
            &[Op::Review {
                review_request: ReviewRequest {
                    user_facing_hint: Some(user_facing_hint(&ReviewTarget::Custom {
                        instructions: instructions.to_owned()
                    })),
                    target: ReviewTarget::Custom {
                        instructions: instructions.to_owned()
                    },
                }
            }],
            "ops don't match {ops:?}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_commit_review() -> anyhow::Result<()> {
        let (session_id, client, thread, message_tx, local_set) = setup(vec![]).await?;
        let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();

        message_tx.send(ThreadMessage::Prompt {
            request: PromptRequest::new(session_id.clone(), vec!["/review-commit 123456".into()]),
            response_tx: prompt_response_tx,
        })?;

        tokio::try_join!(
            async {
                let stop_reason = prompt_response_rx.await??.await??;
                assert_eq!(stop_reason, StopReason::EndTurn);
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        let notifications = client.notifications.lock().unwrap();
        assert_eq!(notifications.len(), 1);
        assert!(
            matches!(
                &notifications[0].update,
                SessionUpdate::AgentMessageChunk(ContentChunk {
                    content: ContentBlock::Text(TextContent { text, .. }),
                    ..
                }) if text == "commit 123456" // we echo the prompt
            ),
            "notifications don't match {notifications:?}"
        );

        let ops = thread.ops.lock().unwrap();
        assert_eq!(
            ops.as_slice(),
            &[Op::Review {
                review_request: ReviewRequest {
                    user_facing_hint: Some(user_facing_hint(&ReviewTarget::Commit {
                        sha: "123456".to_owned(),
                        title: None
                    })),
                    target: ReviewTarget::Commit {
                        sha: "123456".to_owned(),
                        title: None
                    },
                }
            }],
            "ops don't match {ops:?}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_branch_review() -> anyhow::Result<()> {
        let (session_id, client, thread, message_tx, local_set) = setup(vec![]).await?;
        let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();

        message_tx.send(ThreadMessage::Prompt {
            request: PromptRequest::new(session_id.clone(), vec!["/review-branch feature".into()]),
            response_tx: prompt_response_tx,
        })?;

        tokio::try_join!(
            async {
                let stop_reason = prompt_response_rx.await??.await??;
                assert_eq!(stop_reason, StopReason::EndTurn);
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        let notifications = client.notifications.lock().unwrap();
        assert_eq!(notifications.len(), 1);
        assert!(
            matches!(
                &notifications[0].update,
                SessionUpdate::AgentMessageChunk(ContentChunk {
                    content: ContentBlock::Text(TextContent { text, .. }),
                    ..
                }) if text == "changes against 'feature'" // we echo the prompt
            ),
            "notifications don't match {notifications:?}"
        );

        let ops = thread.ops.lock().unwrap();
        assert_eq!(
            ops.as_slice(),
            &[Op::Review {
                review_request: ReviewRequest {
                    user_facing_hint: Some(user_facing_hint(&ReviewTarget::BaseBranch {
                        branch: "feature".to_owned()
                    })),
                    target: ReviewTarget::BaseBranch {
                        branch: "feature".to_owned()
                    },
                }
            }],
            "ops don't match {ops:?}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_custom_prompts() -> anyhow::Result<()> {
        let custom_prompts = vec![CustomPrompt {
            name: "custom".to_string(),
            path: "/tmp/custom.md".into(),
            content: "Custom prompt with $1 arg.".into(),
            description: None,
            argument_hint: None,
        }];
        let (session_id, client, thread, message_tx, local_set) = setup(custom_prompts).await?;
        let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();

        message_tx.send(ThreadMessage::Prompt {
            request: PromptRequest::new(session_id.clone(), vec!["/custom foo".into()]),
            response_tx: prompt_response_tx,
        })?;

        tokio::try_join!(
            async {
                let stop_reason = prompt_response_rx.await??.await??;
                assert_eq!(stop_reason, StopReason::EndTurn);
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        let notifications = client.notifications.lock().unwrap();
        assert_eq!(notifications.len(), 1);
        assert!(
            matches!(
                &notifications[0].update,
                SessionUpdate::AgentMessageChunk(ContentChunk {
                    content: ContentBlock::Text(TextContent { text, .. }),
                    ..
                }) if text == "Custom prompt with foo arg."
            ),
            "notifications don't match {notifications:?}"
        );

        let ops = thread.ops.lock().unwrap();
        assert_eq!(
            ops.as_slice(),
            &[Op::UserInput {
                items: vec![UserInput::Text {
                    text: "Custom prompt with foo arg.".into(),
                    text_elements: vec![]
                }],
                final_output_json_schema: None,
            }],
            "ops don't match {ops:?}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_delta_deduplication() -> anyhow::Result<()> {
        let (session_id, client, _, message_tx, local_set) = setup(vec![]).await?;
        let (prompt_response_tx, prompt_response_rx) = tokio::sync::oneshot::channel();

        message_tx.send(ThreadMessage::Prompt {
            request: PromptRequest::new(session_id.clone(), vec!["test delta".into()]),
            response_tx: prompt_response_tx,
        })?;

        tokio::try_join!(
            async {
                let stop_reason = prompt_response_rx.await??.await??;
                assert_eq!(stop_reason, StopReason::EndTurn);
                drop(message_tx);
                anyhow::Ok(())
            },
            async {
                local_set.await;
                anyhow::Ok(())
            }
        )?;

        // We should only get ONE notification, not duplicates from both delta and non-delta
        let notifications = client.notifications.lock().unwrap();
        assert_eq!(
            notifications.len(),
            1,
            "Should only receive delta event, not duplicate non-delta. Got: {notifications:?}"
        );
        assert!(matches!(
            &notifications[0].update,
            SessionUpdate::AgentMessageChunk(ContentChunk {
                content: ContentBlock::Text(TextContent { text, .. }),
                ..
            }) if text == "test delta"
        ));

        Ok(())
    }

    fn test_exec_begin_event(call_id: &str) -> ExecCommandBeginEvent {
        test_exec_begin_event_with_terminal(call_id, None)
    }

    fn test_exec_begin_event_with_terminal(
        call_id: &str,
        terminal_id: Option<&str>,
    ) -> ExecCommandBeginEvent {
        ExecCommandBeginEvent {
            call_id: call_id.to_string(),
            process_id: None,
            terminal_id: terminal_id.map(str::to_string),
            turn_id: "turn-1".to_string(),
            command: vec!["sleep".to_string(), "300".to_string()],
            cwd: PathBuf::from("/tmp/repo"),
            parsed_cmd: vec![ParsedCommand::Unknown {
                cmd: "sleep 300".to_string(),
            }],
            source: codex_core::protocol::ExecCommandSource::UserShell,
            interaction_input: None,
        }
    }

    fn test_exec_end_event(call_id: &str, exit_code: i32) -> ExecCommandEndEvent {
        test_exec_end_event_with_terminal(call_id, None, exit_code)
    }

    fn test_exec_end_event_with_terminal(
        call_id: &str,
        terminal_id: Option<&str>,
        exit_code: i32,
    ) -> ExecCommandEndEvent {
        ExecCommandEndEvent {
            call_id: call_id.to_string(),
            process_id: None,
            terminal_id: terminal_id.map(str::to_string),
            turn_id: "turn-1".to_string(),
            command: vec!["sleep".to_string(), "300".to_string()],
            cwd: PathBuf::from("/tmp/repo"),
            parsed_cmd: vec![ParsedCommand::Unknown {
                cmd: "sleep 300".to_string(),
            }],
            source: codex_core::protocol::ExecCommandSource::UserShell,
            interaction_input: None,
            stdout: "ok".to_string(),
            stderr: String::new(),
            aggregated_output: "ok".to_string(),
            exit_code,
            duration: std::time::Duration::from_millis(1),
            formatted_output: "ok".to_string(),
        }
    }

    fn test_exec_output_delta_event(call_id: &str, data: &str) -> ExecCommandOutputDeltaEvent {
        ExecCommandOutputDeltaEvent {
            call_id: call_id.to_string(),
            stream: codex_core::protocol::ExecOutputStream::Stdout,
            chunk: data.as_bytes().to_vec(),
        }
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_final_only_visibility_hides_internal_updates() -> anyhow::Result<()> {
        let _guard = crate::session_store::ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();
        let _visibility_restore = EnvVarRestore::set("ACP_UI_VISIBILITY_MODE", Some("final_only"));

        let session_id = SessionId::new("final-only-visibility");
        let client = Arc::new(StubClient::new());
        let session_client =
            SessionClient::with_client(session_id, client.clone(), Arc::default(), None);
        let thread = Arc::new(StubCodexThread::new());
        let mut state = PromptState::new_background(thread, "submission-final-only".to_string());

        state
            .handle_event(
                &session_client,
                EventMsg::ReasoningContentDelta(ReasoningContentDeltaEvent {
                    thread_id: "thread-1".to_string(),
                    turn_id: "turn-1".to_string(),
                    item_id: "item-1".to_string(),
                    summary_index: 0,
                    delta: "Thinking hidden".to_string(),
                }),
            )
            .await;
        state
            .handle_event(
                &session_client,
                EventMsg::ExecCommandBegin(test_exec_begin_event("exec-final-only")),
            )
            .await;
        state
            .handle_event(
                &session_client,
                EventMsg::ExecCommandEnd(test_exec_end_event("exec-final-only", 0)),
            )
            .await;
        state
            .handle_event(
                &session_client,
                EventMsg::AgentMessageContentDelta(AgentMessageContentDeltaEvent {
                    thread_id: "thread-1".to_string(),
                    turn_id: "turn-1".to_string(),
                    item_id: "item-1".to_string(),
                    delta: "final answer".to_string(),
                }),
            )
            .await;
        state
            .handle_event(
                &session_client,
                EventMsg::TurnComplete(TurnCompleteEvent {
                    last_agent_message: None,
                }),
            )
            .await;

        let notifications = client.notifications.lock().unwrap();
        let text_chunks = notifications
            .iter()
            .filter_map(|notification| match &notification.update {
                SessionUpdate::AgentMessageChunk(ContentChunk {
                    content: ContentBlock::Text(TextContent { text, .. }),
                    ..
                }) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            text_chunks,
            vec!["final answer"],
            "expected only final answer text when final_only visibility is enabled. notifications={notifications:?}"
        );
        assert!(
            !notifications.iter().any(|notification| matches!(
                notification.update,
                SessionUpdate::AgentThoughtChunk(_)
            )),
            "final_only visibility should hide thought chunks. notifications={notifications:?}"
        );
        assert!(
            !notifications
                .iter()
                .any(|notification| matches!(notification.update, SessionUpdate::ToolCall(_))),
            "final_only visibility should hide tool calls. notifications={notifications:?}"
        );
        assert!(
            !notifications.iter().any(|notification| matches!(
                notification.update,
                SessionUpdate::ToolCallUpdate(_)
            )),
            "final_only visibility should hide tool-call updates. notifications={notifications:?}"
        );

        Ok(())
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_send_agent_text_respects_ui_text_chunk_limit_env() -> anyhow::Result<()> {
        let _guard = crate::session_store::ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();
        let _chunk_restore = EnvVarRestore::set("ACP_UI_TEXT_CHUNK_MAX_CHARS", Some("512"));

        let session_id = SessionId::new("chunk-limit-test");
        let client = Arc::new(StubClient::new());
        let session_client =
            SessionClient::with_client(session_id, client.clone(), Arc::default(), None);

        let text = format!("{}{}{}", "a".repeat(512), "b".repeat(512), "c");
        session_client.send_agent_text(text).await;

        let notifications = client.notifications.lock().unwrap();
        let text_chunks = notifications
            .iter()
            .filter_map(|notification| match &notification.update {
                SessionUpdate::AgentMessageChunk(ContentChunk {
                    content: ContentBlock::Text(TextContent { text, .. }),
                    ..
                }) => Some(text.to_string()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            text_chunks,
            vec!["a".repeat(512), "b".repeat(512), "c".to_string()],
            "expected ACP_UI_TEXT_CHUNK_MAX_CHARS to chunk outgoing agent text. notifications={notifications:?}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_send_agent_text_preserves_local_markdown_file_links() -> anyhow::Result<()> {
        let session_id = SessionId::new("local-link-normalization-test");
        let client = Arc::new(StubClient::new());
        let session_client =
            SessionClient::with_client(session_id, client.clone(), Arc::default(), None);

        session_client
            .send_agent_text("[open](/Volumes/Extend/Projects/Writer/_open/test.md)")
            .await;

        let notifications = client.notifications.lock().unwrap();
        let text_chunks = notifications
            .iter()
            .filter_map(|notification| match &notification.update {
                SessionUpdate::AgentMessageChunk(ContentChunk {
                    content: ContentBlock::Text(TextContent { text, .. }),
                    ..
                }) => Some(text.to_string()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            text_chunks,
            vec!["[open](/Volumes/Extend/Projects/Writer/_open/test.md)".to_string()],
            "expected outgoing ACP agent text to preserve bare local markdown links. notifications={notifications:?}"
        );

        Ok(())
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_send_tool_call_update_caps_raw_output_for_ui() -> anyhow::Result<()> {
        let _guard = crate::session_store::ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();
        let _raw_limit_restore = EnvVarRestore::set("ACP_TOOL_RAW_OUTPUT_MAX_CHARS", Some("2048"));

        let session_id = SessionId::new("raw-output-cap-test");
        let client = Arc::new(StubClient::new());
        let session_client =
            SessionClient::with_client(session_id, client.clone(), Arc::default(), None);

        let oversized_output = json!({
            "blob": "x".repeat(4096)
        });
        session_client
            .send_tool_call_update(ToolCallUpdate::new(
                "tool-cap-1",
                ToolCallUpdateFields::new().raw_output(oversized_output),
            ))
            .await;

        let notifications = client.notifications.lock().unwrap();
        let Some(SessionNotification {
            update: SessionUpdate::ToolCallUpdate(update),
            ..
        }) = notifications.last()
        else {
            panic!("expected tool call update notification. notifications={notifications:?}");
        };

        let raw_output = update
            .fields
            .raw_output
            .as_ref()
            .expect("expected raw_output to be present");
        assert_eq!(
            raw_output
                .get("truncated")
                .and_then(serde_json::Value::as_bool),
            Some(true),
            "expected capped payload metadata. raw_output={raw_output:?}"
        );
        assert_eq!(
            raw_output
                .get("payload")
                .and_then(serde_json::Value::as_str),
            Some("tool_call_update.raw_output"),
            "expected payload marker for capped raw output. raw_output={raw_output:?}"
        );
        assert_eq!(
            raw_output
                .get("limit_chars")
                .and_then(serde_json::Value::as_u64),
            Some(2048),
            "expected clamp-aware limit marker for capped raw output. raw_output={raw_output:?}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_prompt_state_closes_exec_tool_call_on_turn_aborted() -> anyhow::Result<()> {
        let session_id = SessionId::new("turn-aborted-test");
        let client = Arc::new(StubClient::new());
        let session_client =
            SessionClient::with_client(session_id, client.clone(), Arc::default(), None);
        let thread = Arc::new(StubCodexThread::new());
        let mut state = PromptState::new_background(thread, "submission-1".to_string());

        state
            .handle_event(
                &session_client,
                EventMsg::ExecCommandBegin(test_exec_begin_event("exec-call-1")),
            )
            .await;
        state
            .handle_event(
                &session_client,
                EventMsg::TurnAborted(TurnAbortedEvent {
                    reason: codex_core::protocol::TurnAbortReason::Interrupted,
                }),
            )
            .await;

        let notifications = client.notifications.lock().unwrap();
        let saw_tool_call = notifications.iter().any(|notification| {
            matches!(
                &notification.update,
                SessionUpdate::ToolCall(tool_call)
                    if tool_call.tool_call_id.0.as_ref() == "exec-call-1"
            )
        });
        let saw_failed_update = notifications.iter().any(|notification| {
            matches!(
                &notification.update,
                SessionUpdate::ToolCallUpdate(update)
                    if update.tool_call_id.0.as_ref() == "exec-call-1"
                        && update.fields.status == Some(ToolCallStatus::Failed)
            )
        });

        assert!(
            saw_tool_call,
            "expected an in-progress tool call notification"
        );
        assert!(
            saw_failed_update,
            "expected failed tool-call update on turn aborted. notifications={notifications:?}"
        );
        assert!(
            state.open_tool_calls.is_empty(),
            "expected no dangling open tool calls"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_prompt_state_watchdog_times_out_open_exec_tool_call() -> anyhow::Result<()> {
        let session_id = SessionId::new("watchdog-test");
        let client = Arc::new(StubClient::new());
        let session_client =
            SessionClient::with_client(session_id, client.clone(), Arc::default(), None);
        let thread = Arc::new(StubCodexThread::new());
        let mut state = PromptState::new_background(thread, "submission-2".to_string());
        state.tool_watchdog_timeout = Duration::from_millis(1);

        state
            .handle_event(
                &session_client,
                EventMsg::ExecCommandBegin(test_exec_begin_event("exec-call-timeout")),
            )
            .await;

        tokio::time::sleep(Duration::from_millis(10)).await;
        state.handle_watchdog_tick(&session_client).await;

        let notifications = client.notifications.lock().unwrap();
        let saw_failed_update = notifications.iter().any(|notification| {
            matches!(
                &notification.update,
                SessionUpdate::ToolCallUpdate(update)
                    if update.tool_call_id.0.as_ref() == "exec-call-timeout"
                        && update.fields.status == Some(ToolCallStatus::Failed)
            )
        });

        assert!(
            saw_failed_update,
            "expected failed tool-call update after watchdog timeout. notifications={notifications:?}"
        );
        assert!(
            state.open_tool_calls.is_empty(),
            "expected watchdog to clear open tool calls"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_prompt_state_closes_exec_tool_call_when_end_arrives_without_active_command()
    -> anyhow::Result<()> {
        let session_id = SessionId::new("missing-active-command-fallback-test");
        let client = Arc::new(StubClient::new());
        let session_client =
            SessionClient::with_client(session_id, client.clone(), Arc::default(), None);
        let thread = Arc::new(StubCodexThread::new());
        let mut state = PromptState::new_background(thread, "submission-3".to_string());

        state
            .handle_event(
                &session_client,
                EventMsg::ExecCommandBegin(test_exec_begin_event("exec-call-fallback")),
            )
            .await;
        state.active_command = None;

        state
            .handle_event(
                &session_client,
                EventMsg::ExecCommandEnd(test_exec_end_event("exec-call-fallback", 0)),
            )
            .await;

        let notifications = client.notifications.lock().unwrap();
        let saw_completed_update = notifications.iter().any(|notification| {
            matches!(
                &notification.update,
                SessionUpdate::ToolCallUpdate(update)
                    if update.tool_call_id.0.as_ref() == "exec-call-fallback"
                        && update.fields.status == Some(ToolCallStatus::Completed)
            )
        });

        assert!(
            saw_completed_update,
            "expected completed tool-call update when exec end arrives without active command. notifications={notifications:?}"
        );
        assert!(
            state.open_tool_calls.is_empty(),
            "expected missing-active-command fallback to clear open tool calls"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_exec_command_uses_legacy_terminal_extension_when_opted_in() -> anyhow::Result<()>
    {
        let session_id = SessionId::new("legacy-terminal-extension-test");
        let client = Arc::new(StubClient::new());
        let capabilities = Arc::new(std::sync::Mutex::new(ClientCapabilities::new().meta(
            Meta::from_iter([("terminal_output".to_string(), json!(true))]),
        )));
        let session_client =
            SessionClient::with_client(session_id, client.clone(), capabilities, None);
        let thread = Arc::new(StubCodexThread::new());
        let mut state = PromptState::new_background(thread, "submission-terminal-legacy".into());

        state
            .handle_event(
                &session_client,
                EventMsg::ExecCommandBegin(test_exec_begin_event_with_terminal(
                    "exec-terminal-legacy",
                    Some("terminal-legacy-123"),
                )),
            )
            .await;
        state
            .handle_event(
                &session_client,
                EventMsg::ExecCommandOutputDelta(test_exec_output_delta_event(
                    "exec-terminal-legacy",
                    "hello from terminal\n",
                )),
            )
            .await;
        state
            .handle_event(
                &session_client,
                EventMsg::ExecCommandEnd(test_exec_end_event_with_terminal(
                    "exec-terminal-legacy",
                    Some("terminal-legacy-123"),
                    0,
                )),
            )
            .await;

        let notifications = client.notifications.lock().unwrap();
        let Some(SessionNotification {
            update: SessionUpdate::ToolCall(tool_call),
            ..
        }) = notifications.iter().find(|notification| {
            matches!(
                &notification.update,
                SessionUpdate::ToolCall(tool_call)
                    if tool_call.tool_call_id.0.as_ref() == "exec-terminal-legacy"
            )
        })
        else {
            panic!("expected tool call notification. notifications={notifications:?}");
        };
        assert!(
            matches!(
                tool_call.content.as_slice(),
                [ToolCallContent::Terminal(terminal)] if terminal.terminal_id.0.as_ref() == "terminal-legacy-123"
            ),
            "expected terminal content for legacy terminal extension. tool_call={tool_call:?}"
        );
        let terminal_info = tool_call
            .meta
            .as_ref()
            .and_then(|meta| meta.get("terminal_info"))
            .and_then(serde_json::Value::as_object)
            .expect("expected terminal_info meta");
        assert_eq!(
            terminal_info
                .get("terminal_id")
                .and_then(serde_json::Value::as_str),
            Some("terminal-legacy-123")
        );

        let terminal_output =
            notifications
                .iter()
                .find_map(|notification| match &notification.update {
                    SessionUpdate::ToolCallUpdate(update)
                        if update.tool_call_id.0.as_ref() == "exec-terminal-legacy" =>
                    {
                        update
                            .meta
                            .as_ref()
                            .and_then(|meta| meta.get("terminal_output"))
                    }
                    _ => None,
                });
        let terminal_output = terminal_output.expect("expected terminal_output meta");
        assert_eq!(
            terminal_output
                .get("terminal_id")
                .and_then(serde_json::Value::as_str),
            Some("terminal-legacy-123")
        );
        assert_eq!(
            terminal_output
                .get("data")
                .and_then(serde_json::Value::as_str),
            Some("hello from terminal\n")
        );

        let terminal_exit =
            notifications
                .iter()
                .find_map(|notification| match &notification.update {
                    SessionUpdate::ToolCallUpdate(update)
                        if update.tool_call_id.0.as_ref() == "exec-terminal-legacy" =>
                    {
                        update
                            .meta
                            .as_ref()
                            .and_then(|meta| meta.get("terminal_exit"))
                    }
                    _ => None,
                });
        let terminal_exit = terminal_exit.expect("expected terminal_exit meta");
        assert_eq!(
            terminal_exit
                .get("terminal_id")
                .and_then(serde_json::Value::as_str),
            Some("terminal-legacy-123")
        );
        assert_eq!(
            terminal_exit
                .get("exit_code")
                .and_then(serde_json::Value::as_i64),
            Some(0)
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_exec_command_standard_terminal_clients_use_real_terminal_id_when_available()
    -> anyhow::Result<()> {
        let session_id = SessionId::new("standard-terminal-real-id-test");
        let client = Arc::new(StubClient::new());
        let capabilities = Arc::new(std::sync::Mutex::new(
            ClientCapabilities::new().terminal(true),
        ));
        let session_client =
            SessionClient::with_client(session_id, client.clone(), capabilities, None);
        let thread = Arc::new(StubCodexThread::new());
        let mut state = PromptState::new_background(thread, "submission-terminal-standard".into());

        state
            .handle_event(
                &session_client,
                EventMsg::ExecCommandBegin(test_exec_begin_event_with_terminal(
                    "exec-terminal-standard",
                    Some("terminal-standard-123"),
                )),
            )
            .await;
        state
            .handle_event(
                &session_client,
                EventMsg::ExecCommandOutputDelta(test_exec_output_delta_event(
                    "exec-terminal-standard",
                    "hello from real terminal\n",
                )),
            )
            .await;
        state
            .handle_event(
                &session_client,
                EventMsg::ExecCommandEnd(test_exec_end_event_with_terminal(
                    "exec-terminal-standard",
                    Some("terminal-standard-123"),
                    0,
                )),
            )
            .await;

        let notifications = client.notifications.lock().unwrap();
        let Some(SessionNotification {
            update: SessionUpdate::ToolCall(tool_call),
            ..
        }) = notifications.iter().find(|notification| {
            matches!(
                &notification.update,
                SessionUpdate::ToolCall(tool_call)
                    if tool_call.tool_call_id.0.as_ref() == "exec-terminal-standard"
            )
        })
        else {
            panic!("expected tool call notification. notifications={notifications:?}");
        };
        assert!(
            matches!(
                tool_call.content.as_slice(),
                [ToolCallContent::Terminal(terminal)] if terminal.terminal_id.0.as_ref() == "terminal-standard-123"
            ),
            "expected real terminal content for standard terminal client. tool_call={tool_call:?}"
        );
        assert!(
            tool_call.meta.is_none(),
            "standard terminal content should not carry legacy terminal meta. tool_call={tool_call:?}"
        );
        assert!(
            notifications
                .iter()
                .all(|notification| match &notification.update {
                    SessionUpdate::ToolCallUpdate(update)
                        if update.tool_call_id.0.as_ref() == "exec-terminal-standard" =>
                    {
                        update.fields.content.is_none()
                            && update.meta.as_ref().is_none_or(|meta| {
                                !meta.contains_key("terminal_output")
                                    && !meta.contains_key("terminal_exit")
                            })
                    }
                    _ => true,
                }),
            "standard terminal clients with real terminal ids should not receive legacy or text fallback terminal updates. notifications={notifications:?}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_exec_command_standard_terminal_clients_fall_back_to_text_updates_without_terminal_id()
    -> anyhow::Result<()> {
        let session_id = SessionId::new("standard-terminal-fallback-test");
        let client = Arc::new(StubClient::new());
        let capabilities = Arc::new(std::sync::Mutex::new(
            ClientCapabilities::new().terminal(true),
        ));
        let session_client =
            SessionClient::with_client(session_id, client.clone(), capabilities, None);
        let thread = Arc::new(StubCodexThread::new());
        let mut state = PromptState::new_background(thread, "submission-terminal-standard".into());

        state
            .handle_event(
                &session_client,
                EventMsg::ExecCommandBegin(test_exec_begin_event("exec-terminal-standard")),
            )
            .await;
        state
            .handle_event(
                &session_client,
                EventMsg::ExecCommandOutputDelta(test_exec_output_delta_event(
                    "exec-terminal-standard",
                    "hello from fallback\n",
                )),
            )
            .await;
        state
            .handle_event(
                &session_client,
                EventMsg::ExecCommandEnd(test_exec_end_event("exec-terminal-standard", 0)),
            )
            .await;

        let notifications = client.notifications.lock().unwrap();
        let Some(SessionNotification {
            update: SessionUpdate::ToolCall(tool_call),
            ..
        }) = notifications.iter().find(|notification| {
            matches!(
                &notification.update,
                SessionUpdate::ToolCall(tool_call)
                    if tool_call.tool_call_id.0.as_ref() == "exec-terminal-standard"
            )
        })
        else {
            panic!("expected tool call notification. notifications={notifications:?}");
        };
        assert!(
            tool_call.content.is_empty(),
            "standard terminal support without legacy extension must not emit synthetic terminal content. tool_call={tool_call:?}"
        );
        assert!(
            tool_call.meta.is_none(),
            "standard terminal fallback should not attach legacy terminal meta. tool_call={tool_call:?}"
        );

        let Some(SessionNotification {
            update: SessionUpdate::ToolCallUpdate(update),
            ..
        }) = notifications.iter().find(|notification| {
            matches!(
                &notification.update,
                SessionUpdate::ToolCallUpdate(update)
                    if update.tool_call_id.0.as_ref() == "exec-terminal-standard"
                        && update.fields.content.is_some()
            )
        })
        else {
            panic!("expected textual tool-call update. notifications={notifications:?}");
        };
        let content = update
            .fields
            .content
            .as_ref()
            .expect("expected content in text fallback update");
        assert!(
            matches!(
                content.as_slice(),
                [ToolCallContent::Content(block)]
                    if matches!(
                        &block.content,
                        ContentBlock::Text(TextContent { text, .. })
                            if text == "```sh\nhello from fallback\n```\n"
                    )
            ),
            "expected plain text fallback output for standard terminal clients. update={update:?}"
        );
        assert!(
            update.meta.is_none(),
            "text fallback should not include legacy terminal_output meta. update={update:?}"
        );
        assert!(
            notifications
                .iter()
                .all(|notification| match &notification.update {
                    SessionUpdate::ToolCallUpdate(update)
                        if update.tool_call_id.0.as_ref() == "exec-terminal-standard" =>
                    {
                        update
                            .meta
                            .as_ref()
                            .is_none_or(|meta| !meta.contains_key("terminal_exit"))
                    }
                    _ => true,
                }),
            "standard terminal fallback should not emit terminal_exit meta. notifications={notifications:?}"
        );

        Ok(())
    }

    async fn setup(
        custom_prompts: Vec<CustomPrompt>,
    ) -> anyhow::Result<(
        SessionId,
        Arc<StubClient>,
        Arc<StubCodexThread>,
        UnboundedSender<ThreadMessage>,
        LocalSet,
    )> {
        let session_id = SessionId::new("test");
        let client = Arc::new(StubClient::new());
        let session_client = SessionClient::with_client_and_visibility(
            session_id.clone(),
            client.clone(),
            Arc::default(),
            None,
            UiVisibilityMode::Full,
        );
        let conversation = Arc::new(StubCodexThread::new());
        let models_manager = Arc::new(StubModelsManager);
        let config = Config::load_with_cli_overrides_and_harness_overrides(
            vec![],
            ConfigOverrides::default(),
        )
        .await?;
        let (message_tx, message_rx) = tokio::sync::mpsc::unbounded_channel();

        let mut actor = ThreadActor::new(
            StubAuth,
            session_client,
            conversation.clone(),
            models_manager,
            config,
            message_rx,
        );
        actor.custom_prompts = Rc::new(RefCell::new(custom_prompts));

        let local_set = LocalSet::new();
        local_set.spawn_local(actor.spawn());
        Ok((session_id, client, conversation, message_tx, local_set))
    }

    struct StubAuth;

    impl Auth for StubAuth {
        fn logout(&self) -> Result<bool, Error> {
            Ok(true)
        }
    }

    struct StubModelsManager;

    #[async_trait::async_trait]
    impl ModelsManagerImpl for StubModelsManager {
        async fn get_model(&self, _model_id: &Option<String>, _config: &Config) -> String {
            all_model_presets()[0].to_owned().id
        }

        async fn list_models(&self, _config: &Config) -> Vec<ModelPreset> {
            all_model_presets().to_owned()
        }
    }

    #[derive(Clone)]
    struct PendingExec {
        call_id: String,
        command: Vec<String>,
        cwd: PathBuf,
        parsed_cmd: Vec<ParsedCommand>,
    }

    struct StubCodexThread {
        current_id: AtomicUsize,
        ops: std::sync::Mutex<Vec<Op>>,
        op_tx: mpsc::UnboundedSender<Event>,
        op_rx: Mutex<mpsc::UnboundedReceiver<Event>>,
        pending_exec: std::sync::Mutex<std::collections::HashMap<String, PendingExec>>,
    }

    impl StubCodexThread {
        fn new() -> Self {
            let (op_tx, op_rx) = mpsc::unbounded_channel();
            StubCodexThread {
                current_id: AtomicUsize::new(0),
                ops: std::sync::Mutex::default(),
                op_tx,
                op_rx: Mutex::new(op_rx),
                pending_exec: std::sync::Mutex::default(),
            }
        }
    }

    #[async_trait::async_trait]
    impl CodexThreadImpl for StubCodexThread {
        async fn submit(&self, op: Op) -> Result<String, CodexErr> {
            let id = self
                .current_id
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

            self.ops.lock().unwrap().push(op.clone());

            match op {
                Op::ListMcpTools => {
                    self.op_tx
                        .send(Event {
                            id: id.to_string(),
                            msg: EventMsg::McpListToolsResponse(
                                codex_core::protocol::McpListToolsResponseEvent {
                                    tools: std::collections::HashMap::new(),
                                    resources: std::collections::HashMap::new(),
                                    resource_templates: std::collections::HashMap::new(),
                                    auth_statuses: std::collections::HashMap::new(),
                                },
                            ),
                        })
                        .unwrap();
                    self.op_tx
                        .send(Event {
                            id: id.to_string(),
                            msg: EventMsg::TurnComplete(TurnCompleteEvent {
                                last_agent_message: None,
                            }),
                        })
                        .unwrap();
                }
                Op::ListSkills { .. } => {
                    self.op_tx
                        .send(Event {
                            id: id.to_string(),
                            msg: EventMsg::ListSkillsResponse(
                                codex_core::protocol::ListSkillsResponseEvent {
                                    skills: vec![codex_core::protocol::SkillsListEntry {
                                        cwd: PathBuf::from("/tmp/repo"),
                                        skills: vec![codex_core::protocol::SkillMetadata {
                                            name: "demo-skill".to_string(),
                                            description: "Demo skill".to_string(),
                                            short_description: None,
                                            interface: None,
                                            path: PathBuf::from("/tmp/repo/SKILL.md"),
                                            scope: codex_core::protocol::SkillScope::Repo,
                                            enabled: true,
                                        }],
                                        errors: vec![],
                                    }],
                                },
                            ),
                        })
                        .unwrap();
                    self.op_tx
                        .send(Event {
                            id: id.to_string(),
                            msg: EventMsg::TurnComplete(TurnCompleteEvent {
                                last_agent_message: None,
                            }),
                        })
                        .unwrap();
                }
                Op::UserInput { items, .. } => {
                    let prompt = items
                        .into_iter()
                        .map(|i| match i {
                            UserInput::Text { text, .. } => text,
                            _ => unimplemented!(),
                        })
                        .join("\n");

                    self.op_tx
                        .send(Event {
                            id: id.to_string(),
                            msg: EventMsg::AgentMessageContentDelta(
                                AgentMessageContentDeltaEvent {
                                    thread_id: id.to_string(),
                                    turn_id: id.to_string(),
                                    item_id: id.to_string(),
                                    delta: prompt.clone(),
                                },
                            ),
                        })
                        .unwrap();
                    // Send non-delta event (should be deduplicated, but handled by deduplication)
                    self.op_tx
                        .send(Event {
                            id: id.to_string(),
                            msg: EventMsg::AgentMessage(AgentMessageEvent { message: prompt }),
                        })
                        .unwrap();
                    self.op_tx
                        .send(Event {
                            id: id.to_string(),
                            msg: EventMsg::TurnComplete(TurnCompleteEvent {
                                last_agent_message: None,
                            }),
                        })
                        .unwrap();
                }
                Op::Compact => {
                    self.op_tx
                        .send(Event {
                            id: id.to_string(),
                            msg: EventMsg::TurnStarted(TurnStartedEvent {
                                model_context_window: None,
                            }),
                        })
                        .unwrap();
                    self.op_tx
                        .send(Event {
                            id: id.to_string(),
                            msg: EventMsg::AgentMessage(AgentMessageEvent {
                                message: "Compact task completed".to_string(),
                            }),
                        })
                        .unwrap();
                    self.op_tx
                        .send(Event {
                            id: id.to_string(),
                            msg: EventMsg::TurnComplete(TurnCompleteEvent {
                                last_agent_message: None,
                            }),
                        })
                        .unwrap();
                }
                Op::Undo => {
                    self.op_tx
                        .send(Event {
                            id: id.to_string(),
                            msg: EventMsg::UndoStarted(codex_core::protocol::UndoStartedEvent {
                                message: Some("Undo in progress...".to_string()),
                            }),
                        })
                        .unwrap();
                    self.op_tx
                        .send(Event {
                            id: id.to_string(),
                            msg: EventMsg::UndoCompleted(
                                codex_core::protocol::UndoCompletedEvent {
                                    success: true,
                                    message: Some("Undo completed.".to_string()),
                                },
                            ),
                        })
                        .unwrap();
                    self.op_tx
                        .send(Event {
                            id: id.to_string(),
                            msg: EventMsg::TurnComplete(TurnCompleteEvent {
                                last_agent_message: None,
                            }),
                        })
                        .unwrap();
                }
                Op::Review { review_request } => {
                    self.op_tx
                        .send(Event {
                            id: id.to_string(),
                            msg: EventMsg::EnteredReviewMode(review_request.clone()),
                        })
                        .unwrap();
                    self.op_tx
                        .send(Event {
                            id: id.to_string(),
                            msg: EventMsg::ExitedReviewMode(ExitedReviewModeEvent {
                                review_output: Some(ReviewOutputEvent {
                                    findings: vec![],
                                    overall_correctness: String::new(),
                                    overall_explanation: review_request
                                        .user_facing_hint
                                        .clone()
                                        .unwrap_or_default(),
                                    overall_confidence_score: 1.,
                                }),
                            }),
                        })
                        .unwrap();
                    self.op_tx
                        .send(Event {
                            id: id.to_string(),
                            msg: EventMsg::TurnComplete(TurnCompleteEvent {
                                last_agent_message: None,
                            }),
                        })
                        .unwrap();
                }
                Op::RunUserShellCommand { command } => {
                    let submission_id = id.to_string();
                    let call_id = format!("exec-{submission_id}");
                    let cwd = PathBuf::from("/tmp/repo");
                    let tokens = shlex::split(&command).unwrap_or_else(|| vec![command.clone()]);
                    let parsed_cmd = vec![ParsedCommand::Unknown { cmd: command }];

                    let pending = PendingExec {
                        call_id: call_id.clone(),
                        command: tokens.clone(),
                        cwd: cwd.clone(),
                        parsed_cmd: parsed_cmd.clone(),
                    };
                    self.pending_exec
                        .lock()
                        .unwrap()
                        .insert(submission_id.clone(), pending);

                    self.op_tx
                        .send(Event {
                            id: submission_id.clone(),
                            msg: EventMsg::PlanUpdate(UpdatePlanArgs {
                                explanation: Some("Test plan explanation".to_string()),
                                plan: vec![
                                    PlanItemArg {
                                        step: "Test: plan step".to_string(),
                                        status: StepStatus::InProgress,
                                    },
                                    PlanItemArg {
                                        step: "Test: execute step".to_string(),
                                        status: StepStatus::Pending,
                                    },
                                ],
                            }),
                        })
                        .unwrap();

                    self.op_tx
                        .send(Event {
                            id: submission_id,
                            msg: EventMsg::ExecApprovalRequest(ExecApprovalRequestEvent {
                                call_id,
                                turn_id: id.to_string(),
                                command: tokens,
                                cwd,
                                reason: Some("Test: permission required".to_string()),
                                proposed_execpolicy_amendment: None,
                                parsed_cmd,
                            }),
                        })
                        .unwrap();
                }
                Op::ExecApproval {
                    id: exec_id,
                    decision: _,
                } => {
                    let pending = self
                        .pending_exec
                        .lock()
                        .unwrap()
                        .remove(&exec_id)
                        .unwrap_or_else(|| {
                            panic!("missing pending exec request for submission id {exec_id}")
                        });

                    let stdout = "stub exec output\n".to_string();
                    self.op_tx
                        .send(Event {
                            id: exec_id.clone(),
                            msg: EventMsg::ExecCommandBegin(ExecCommandBeginEvent {
                                call_id: pending.call_id.clone(),
                                process_id: None,
                                terminal_id: None,
                                turn_id: exec_id.clone(),
                                command: pending.command.clone(),
                                cwd: pending.cwd.clone(),
                                parsed_cmd: pending.parsed_cmd.clone(),
                                source: codex_core::protocol::ExecCommandSource::UserShell,
                                interaction_input: None,
                            }),
                        })
                        .unwrap();
                    self.op_tx
                        .send(Event {
                            id: exec_id.clone(),
                            msg: EventMsg::ExecCommandOutputDelta(ExecCommandOutputDeltaEvent {
                                call_id: pending.call_id.clone(),
                                stream: codex_core::protocol::ExecOutputStream::Stdout,
                                chunk: stdout.as_bytes().to_vec(),
                            }),
                        })
                        .unwrap();
                    self.op_tx
                        .send(Event {
                            id: exec_id.clone(),
                            msg: EventMsg::ExecCommandEnd(ExecCommandEndEvent {
                                call_id: pending.call_id,
                                process_id: None,
                                terminal_id: None,
                                turn_id: exec_id.clone(),
                                command: pending.command,
                                cwd: pending.cwd,
                                parsed_cmd: pending.parsed_cmd,
                                source: codex_core::protocol::ExecCommandSource::UserShell,
                                interaction_input: None,
                                stdout: stdout.clone(),
                                stderr: String::new(),
                                aggregated_output: stdout.clone(),
                                exit_code: 0,
                                duration: std::time::Duration::from_millis(1),
                                formatted_output: stdout,
                            }),
                        })
                        .unwrap();
                    self.op_tx
                        .send(Event {
                            id: exec_id,
                            msg: EventMsg::TurnComplete(TurnCompleteEvent {
                                last_agent_message: None,
                            }),
                        })
                        .unwrap();
                }
                Op::Interrupt => {
                    let pending_ids = self
                        .pending_exec
                        .lock()
                        .unwrap()
                        .drain()
                        .map(|(submission_id, _)| submission_id)
                        .collect::<Vec<_>>();
                    for submission_id in pending_ids {
                        self.op_tx
                            .send(Event {
                                id: submission_id,
                                msg: EventMsg::TurnAborted(TurnAbortedEvent {
                                    reason: codex_core::protocol::TurnAbortReason::Interrupted,
                                }),
                            })
                            .unwrap();
                    }
                }
                _ => {
                    unimplemented!()
                }
            }
            Ok(id.to_string())
        }

        async fn next_event(&self) -> Result<Event, CodexErr> {
            let Some(event) = self.op_rx.lock().await.recv().await else {
                return Err(CodexErr::InternalAgentDied);
            };
            Ok(event)
        }
    }

    struct StubClient {
        notifications: std::sync::Mutex<Vec<SessionNotification>>,
    }

    impl StubClient {
        fn new() -> Self {
            StubClient {
                notifications: std::sync::Mutex::default(),
            }
        }
    }

    #[async_trait::async_trait(?Send)]
    impl Client for StubClient {
        async fn request_permission(
            &self,
            _args: RequestPermissionRequest,
        ) -> Result<RequestPermissionResponse, Error> {
            Ok(RequestPermissionResponse::new(
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new("approved")),
            ))
        }

        async fn session_notification(&self, args: SessionNotification) -> Result<(), Error> {
            self.notifications.lock().unwrap().push(args);
            Ok(())
        }
    }
}
