#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="$ROOT_DIR/logs/smoke"
TIMESTAMP="$(date +"%Y%m%d_%H%M%S")"
REPORT_PATH="$REPORT_DIR/acp_compat_smoke_${TIMESTAMP}.md"
LOG_DIR="$REPORT_DIR/logs"

usage() {
  cat <<'EOF'
Usage: scripts/acp_compat_smoke.sh [--skip-tests] [--strict]

Runs ACP compatibility smoke checks and writes a markdown report under logs/smoke/.

Options:
  --skip-tests   Run static checks only (skip cargo test commands)
  --strict       Enforce fixed critical ACP test set and record failure logs
  -h, --help     Show this help message
EOF
}

SKIP_TESTS="false"
STRICT_MODE="false"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-tests)
      SKIP_TESTS="true"
      shift
      ;;
    --strict)
      STRICT_MODE="true"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage
      exit 1
      ;;
  esac
done

mkdir -p "$REPORT_DIR" "$LOG_DIR"

export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

STATIC_FAILURES=0
TEST_FAILURES=0
declare -a STATIC_LINES
declare -a TEST_LINES
declare -a DETAIL_LINES

search_pattern() {
  local pattern="$1"
  local file="$2"
  if command -v rg >/dev/null 2>&1; then
    rg -n --no-messages -- "$pattern" "$file" >/dev/null 2>&1
  else
    grep -qE -- "$pattern" "$file" >/dev/null 2>&1
  fi
}

run_rg_present() {
  local label="$1"
  local pattern="$2"
  local file="$3"
  if (cd "$ROOT_DIR" && search_pattern "$pattern" "$file"); then
    STATIC_LINES+=("- ${label}: pass")
  else
    STATIC_LINES+=("- ${label}: fail")
    DETAIL_LINES+=("${label}: expected pattern '${pattern}' in ${file}")
    STATIC_FAILURES=1
  fi
}

run_rg_absent() {
  local label="$1"
  local pattern="$2"
  local file="$3"
  if (cd "$ROOT_DIR" && search_pattern "$pattern" "$file"); then
    STATIC_LINES+=("- ${label}: fail")
    DETAIL_LINES+=("${label}: unexpected pattern '${pattern}' found in ${file}")
    STATIC_FAILURES=1
  else
    STATIC_LINES+=("- ${label}: pass")
  fi
}

run_test_command() {
  local label="$1"
  local command="$2"
  local log_file="$3"
  if (cd "$ROOT_DIR" && bash -lc "$command" >"$log_file" 2>&1); then
    TEST_LINES+=("- ${label}: pass")
    rm -f "$log_file"
  else
    TEST_LINES+=("- ${label}: fail")
    DETAIL_LINES+=("${label}: command failed -> ${command}")
    DETAIL_LINES+=("${label}: log -> ${log_file}")
    TEST_FAILURES=1
  fi
}

# ACP initialize/capability contract checks.
run_rg_present "initialize forces protocol v1" \
  "ProtocolVersion::V1" \
  "src/acp_agent.rs"
run_rg_present "prompt capabilities advertise embedded context + image" \
  "PromptCapabilities::new\\(\\)\\.embedded_context\\(true\\)\\.image\\(true\\)" \
  "src/acp_agent.rs"
run_rg_present "MCP capabilities advertise HTTP transport" \
  "McpCapabilities::new\\(\\)\\.http\\(true\\)" \
  "src/acp_agent.rs"
run_rg_absent "MCP SSE is not advertised as enabled" \
  "\\.sse\\(true\\)" \
  "src/acp_agent.rs"
run_rg_present "session list capability is advertised" \
  "SessionCapabilities::new\\(\\)\\.list\\(SessionListCapabilities::new\\(\\)\\)" \
  "src/acp_agent.rs"
run_rg_present "load_session capability is delegated to backend driver" \
  "let load_session = self\\.driver\\.supports_load_session\\(\\);" \
  "src/acp_agent.rs"

# Event stream and progress/plan pathways.
run_rg_present "plan updates are emitted to ACP notifications" \
  "client\\.update_plan\\(plan, explanation\\)\\.await;" \
  "src/thread.rs"
run_rg_present "non-zed clients also receive visible plan progress text" \
  "self\\.send_agent_text\\(progress_text\\)\\.await;" \
  "src/thread.rs"
run_rg_present "tool call updates are emitted" \
  "SessionUpdate::ToolCallUpdate" \
  "src/thread.rs"
run_rg_present "permission request is canonically logged" \
  "\"acp.request_permission\"" \
  "src/thread.rs"
run_rg_present "permission response is canonically logged" \
  "\"acp.request_permission_response\"" \
  "src/thread.rs"

# Backend-specific compatibility expectations.
run_rg_present "claude load_session unsupported contract" \
  "load_session is not supported for --backend=claude-code yet" \
  "src/claude_code_agent.rs"
run_rg_present "gemini load_session unsupported contract" \
  "load_session is not supported for --backend=gemini yet" \
  "src/gemini_agent.rs"
run_rg_present "multi backend wraps codex pagination cursors" \
  "MULTI_CODEX_CURSOR_PREFIX" \
  "src/multi_backend.rs"
run_rg_present "multi backend exposes deferred routed cursor" \
  "MULTI_ROUTED_CURSOR" \
  "src/multi_backend.rs"
run_rg_present "ACP server advertises session fork support when backend does" \
  "if self.driver.supports_fork_session\\(\\)" \
  "src/acp_agent.rs"
run_rg_present "ACP server advertises session resume support when backend does" \
  "if self.driver.supports_resume_session\\(\\)" \
  "src/acp_agent.rs"
run_rg_present "codex backend implements session fork" \
  "async fn fork_session\\(" \
  "src/codex_agent.rs"
run_rg_present "codex backend implements session resume" \
  "async fn resume_session\\(" \
  "src/codex_agent.rs"
run_rg_present "multi backend routes session fork through codex" \
  "self\\.codex_backing_session_for\\(&request\\.session_id, \"fork\"\\)" \
  "src/multi_backend.rs"
run_rg_present "multi backend routes session resume through codex" \
  "self\\.codex_backing_session_for\\(&request\\.session_id, \"resume\"\\)" \
  "src/multi_backend.rs"
run_rg_present "codex ACP exec creates terminals through client RPC" \
  "let create_response = acp_create_terminal\\(create_request\\)" \
  "src/codex_agent.rs"
run_rg_present "codex ACP exec polls terminal output through client RPC" \
  "let output_response = acp_terminal_output\\(TerminalOutputRequest::new\\(" \
  "src/codex_agent.rs"
run_rg_present "codex ACP exec releases terminals through client RPC" \
  "acp_release_terminal\\(ReleaseTerminalRequest::new\\(" \
  "src/codex_agent.rs"
run_rg_present "codex ACP exec cancel path waits for terminal exit" \
  "acp_wait_for_terminal_exit\\(WaitForTerminalExitRequest::new\\(" \
  "src/codex_agent.rs"
run_rg_present "standard terminal clients receive real terminal ids when available" \
  "Standard terminal clients only get terminal content when the exec layer already" \
  "src/thread.rs"
run_rg_present "ACP reference documents real terminal lifecycle" \
  "terminal/create -> terminal/output -> terminal/release" \
  "docs/reference/acp_standard_spec.md"
run_rg_present "ACP reference documents codex fork resume support" \
  '`session/fork` \\(unstable\\) \\| 지원' \
  "docs/reference/acp_standard_spec.md"
run_rg_present "ACP reference documents multi codex-backed fork resume routing" \
  'session/fork`: `codex` 세션 id 또는 `codex`-backed routed session만 지원' \
  "docs/reference/acp_standard_spec.md"
run_rg_absent "ACP reference no longer claims terminal rpc is unimplemented" \
  '현재 구현은 ACP 표준 `terminal/\*` RPC 전체를 직접 구현하지 않고' \
  "docs/reference/acp_standard_spec.md"
run_rg_absent "ACP reference no longer claims fork resume are unsupported" \
  'session/fork`, `session/resume`은 아직 미지원입니다' \
  "docs/reference/acp_standard_spec.md"

if [[ "$SKIP_TESTS" == "true" ]]; then
  TEST_LINES+=("- cargo tests: skipped (--skip-tests)")
else
  if [[ "$STRICT_MODE" == "true" ]]; then
    run_test_command \
      "cargo test -q thread::tests::test_setup_plan_verification_progress_updates" \
      "cargo test -q thread::tests::test_setup_plan_verification_progress_updates" \
      "$LOG_DIR/acp_smoke_setup_plan_${TIMESTAMP}.log"
    run_test_command \
      "cargo test -q thread::tests::test_update_plan_emits_visible_progress_text_for_non_zed_client" \
      "cargo test -q thread::tests::test_update_plan_emits_visible_progress_text_for_non_zed_client" \
      "$LOG_DIR/acp_smoke_plan_text_non_zed_${TIMESTAMP}.log"
    run_test_command \
      "cargo test -q thread::tests::test_update_plan_avoids_duplicate_progress_text_for_zed_client" \
      "cargo test -q thread::tests::test_update_plan_avoids_duplicate_progress_text_for_zed_client" \
      "$LOG_DIR/acp_smoke_plan_text_zed_${TIMESTAMP}.log"
    run_test_command \
      "cargo test -q thread::tests::test_setup_plan_visible_in_monitor_output" \
      "cargo test -q thread::tests::test_setup_plan_visible_in_monitor_output" \
      "$LOG_DIR/acp_smoke_setup_monitor_${TIMESTAMP}.log"
    run_test_command \
      "cargo test -q thread::tests::test_send_agent_text_preserves_local_markdown_file_links" \
      "cargo test -q thread::tests::test_send_agent_text_preserves_local_markdown_file_links" \
      "$LOG_DIR/acp_smoke_link_paths_${TIMESTAMP}.log"
    run_test_command \
      "cargo test -q thread::tests::test_monitoring_auto_mode_clears_completed_prompt_tasks" \
      "cargo test -q thread::tests::test_monitoring_auto_mode_clears_completed_prompt_tasks" \
      "$LOG_DIR/acp_smoke_task_cleanup_${TIMESTAMP}.log"
    run_test_command \
      "cargo test -q thread::tests::test_prompt_state_closes_exec_tool_call_when_end_arrives_without_active_command" \
      "cargo test -q thread::tests::test_prompt_state_closes_exec_tool_call_when_end_arrives_without_active_command" \
      "$LOG_DIR/acp_smoke_exec_completion_fallback_${TIMESTAMP}.log"
    run_test_command \
      "cargo test -q thread::tests::test_canonical_log_correlation_path" \
      "cargo test -q thread::tests::test_canonical_log_correlation_path" \
      "$LOG_DIR/acp_smoke_canonical_${TIMESTAMP}.log"
    run_test_command \
      "cargo test -q session_store::tests::writes_canonical_log_and_redacts_secrets" \
      "cargo test -q session_store::tests::writes_canonical_log_and_redacts_secrets" \
      "$LOG_DIR/acp_smoke_session_store_${TIMESTAMP}.log"
    run_test_command \
      "cargo test -q multi_backend::tests::list_sessions_defers_routed_sessions_until_codex_pages_finish" \
      "cargo test -q multi_backend::tests::list_sessions_defers_routed_sessions_until_codex_pages_finish" \
      "$LOG_DIR/acp_smoke_multi_session_list_${TIMESTAMP}.log"
    run_test_command \
      "cargo test -q multi_backend::tests::fork_session_wraps_codex_child_in_synthetic_multi_session" \
      "cargo test -q multi_backend::tests::fork_session_wraps_codex_child_in_synthetic_multi_session" \
      "$LOG_DIR/acp_smoke_multi_fork_${TIMESTAMP}.log"
    run_test_command \
      "cargo test -q multi_backend::tests::resume_session_registers_requested_session_id_as_codex_route" \
      "cargo test -q multi_backend::tests::resume_session_registers_requested_session_id_as_codex_route" \
      "$LOG_DIR/acp_smoke_multi_resume_${TIMESTAMP}.log"
    run_test_command \
      "cargo test -q claude_code_agent::tests::authenticate_checks_claude_status" \
      "cargo test -q claude_code_agent::tests::authenticate_checks_claude_status" \
      "$LOG_DIR/acp_smoke_claude_auth_${TIMESTAMP}.log"
    run_test_command \
      "cargo test -q claude_code_agent::tests::cancel_stops_running_prompt" \
      "cargo test -q claude_code_agent::tests::cancel_stops_running_prompt" \
      "$LOG_DIR/acp_smoke_claude_cancel_${TIMESTAMP}.log"
    run_test_command \
      "cargo test -q claude_code_agent::tests::set_session_model_updates_claude_session" \
      "cargo test -q claude_code_agent::tests::set_session_model_updates_claude_session" \
      "$LOG_DIR/acp_smoke_claude_model_${TIMESTAMP}.log"
    run_test_command \
      "cargo test -q claude_code_agent::tests::set_session_config_option_updates_model_and_rejects_other_options" \
      "cargo test -q claude_code_agent::tests::set_session_config_option_updates_model_and_rejects_other_options" \
      "$LOG_DIR/acp_smoke_claude_config_${TIMESTAMP}.log"
    run_test_command \
      "cargo test -q gemini_agent::tests::authenticate_accepts_gemini_api_key_env" \
      "cargo test -q gemini_agent::tests::authenticate_accepts_gemini_api_key_env" \
      "$LOG_DIR/acp_smoke_gemini_auth_${TIMESTAMP}.log"
    run_test_command \
      "cargo test -q gemini_agent::tests::cancel_stops_running_prompt" \
      "cargo test -q gemini_agent::tests::cancel_stops_running_prompt" \
      "$LOG_DIR/acp_smoke_gemini_cancel_${TIMESTAMP}.log"
    run_test_command \
      "cargo test -q gemini_agent::tests::set_session_model_updates_gemini_session" \
      "cargo test -q gemini_agent::tests::set_session_model_updates_gemini_session" \
      "$LOG_DIR/acp_smoke_gemini_model_${TIMESTAMP}.log"
    run_test_command \
      "cargo test -q gemini_agent::tests::set_session_config_option_updates_model_and_rejects_other_options" \
      "cargo test -q gemini_agent::tests::set_session_config_option_updates_model_and_rejects_other_options" \
      "$LOG_DIR/acp_smoke_gemini_config_${TIMESTAMP}.log"
    run_test_command \
      "cargo test -q thread::tests::test_exec_command_uses_legacy_terminal_extension_when_opted_in" \
      "cargo test -q thread::tests::test_exec_command_uses_legacy_terminal_extension_when_opted_in" \
      "$LOG_DIR/acp_smoke_terminal_legacy_${TIMESTAMP}.log"
    run_test_command \
      "cargo test -q thread::tests::test_exec_command_standard_terminal_clients_use_real_terminal_id_when_available" \
      "cargo test -q thread::tests::test_exec_command_standard_terminal_clients_use_real_terminal_id_when_available" \
      "$LOG_DIR/acp_smoke_terminal_standard_${TIMESTAMP}.log"
    run_test_command \
      "cargo test -q thread::tests::test_exec_command_standard_terminal_clients_fall_back_to_text_updates_without_terminal_id" \
      "cargo test -q thread::tests::test_exec_command_standard_terminal_clients_fall_back_to_text_updates_without_terminal_id" \
      "$LOG_DIR/acp_smoke_terminal_fallback_${TIMESTAMP}.log"
  else
    run_test_command \
      "cargo test -q thread::tests::" \
      "cargo test -q thread::tests::" \
      "$LOG_DIR/acp_smoke_thread_${TIMESTAMP}.log"
    run_test_command \
      "cargo test -q session_store::tests::writes_canonical_log_and_redacts_secrets" \
      "cargo test -q session_store::tests::writes_canonical_log_and_redacts_secrets" \
      "$LOG_DIR/acp_smoke_session_store_${TIMESTAMP}.log"
  fi
fi

if [[ "$SKIP_TESTS" == "true" ]]; then
  if [[ $STATIC_FAILURES -eq 0 ]]; then
    OVERALL_STATUS="pass (static-only)"
  else
    OVERALL_STATUS="check-failures"
  fi
else
  if [[ $STATIC_FAILURES -eq 0 && $TEST_FAILURES -eq 0 ]]; then
    OVERALL_STATUS="pass"
  else
    OVERALL_STATUS="check-failures"
  fi
fi

{
  echo "# ACP Compatibility Smoke Report"
  echo
  echo "- Generated at: $(date -u +"%Y-%m-%dT%H:%M:%SZ")"
  echo "- Repository: $ROOT_DIR"
  echo "- Script: scripts/acp_compat_smoke.sh"
  echo "- Skip tests: $SKIP_TESTS"
  echo "- Strict mode: $STRICT_MODE"
  echo
  echo "## Static checks"
  for line in "${STATIC_LINES[@]}"; do
    echo "$line"
  done
  echo
  echo "## Test checks"
  for line in "${TEST_LINES[@]}"; do
    echo "$line"
  done
  if [[ ${#DETAIL_LINES[@]} -gt 0 ]]; then
    echo
    echo "## Failure details"
    for detail in "${DETAIL_LINES[@]}"; do
      echo "- $detail"
    done
  fi
  echo
  echo "## Result summary"
  echo "- Overall: $OVERALL_STATUS"
} > "$REPORT_PATH"

echo "Generated report: $REPORT_PATH"

if [[ "$OVERALL_STATUS" != "pass" && "$OVERALL_STATUS" != "pass (static-only)" ]]; then
  echo "ACP smoke check failed. See report: $REPORT_PATH" >&2
  exit 2
fi

exit 0
