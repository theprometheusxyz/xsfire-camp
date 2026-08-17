#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="$ROOT_DIR/logs/manual_verification"
TIMESTAMP="$(date +"%Y%m%d_%H%M%S")"
REPORT_PATH="$REPORT_DIR/setup_monitor_${TIMESTAMP}.md"

usage() {
  cat <<'EOF'
Usage: scripts/manual_verification_setup_monitor.sh [--skip-gates]

Runs a deterministic preflight for external ACP-client verification and creates
a manual verification checklist report under logs/manual_verification/.

Options:
  --skip-gates   Skip automated gates (cargo fmt --check, cargo test, node test)
EOF
}

SKIP_GATES="false"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-gates)
      SKIP_GATES="true"
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

mkdir -p "$REPORT_DIR"

STATUS_FMT="not_run"
STATUS_TEST="not_run"
STATUS_NODE="not_run"
WARNINGS=()

append_warning_once() {
  local warning_line="$1"
  local existing
  for existing in "${WARNINGS[@]}"; do
    if [[ "$existing" == "$warning_line" ]]; then
      return
    fi
  done
  WARNINGS+=("$warning_line")
}

collect_warnings_from_output() {
  local output="$1"
  local line
  while IFS= read -r line; do
    if [[ "$line" == WARNING:* ]]; then
      append_warning_once "$line"
    fi
  done <<< "$output"
}

if [[ "$SKIP_GATES" != "true" ]]; then
  if FMT_OUTPUT="$(cd "$ROOT_DIR" && cargo fmt --check 2>&1)"; then
    STATUS_FMT="pass"
    [[ -n "$FMT_OUTPUT" ]] && printf '%s\n' "$FMT_OUTPUT"
  else
    STATUS_FMT="fail"
    [[ -n "$FMT_OUTPUT" ]] && printf '%s\n' "$FMT_OUTPUT" >&2
  fi
  collect_warnings_from_output "$FMT_OUTPUT"

  if TEST_OUTPUT="$(cd "$ROOT_DIR" && cargo test 2>&1)"; then
    STATUS_TEST="pass"
    [[ -n "$TEST_OUTPUT" ]] && printf '%s\n' "$TEST_OUTPUT"
  else
    STATUS_TEST="fail"
    [[ -n "$TEST_OUTPUT" ]] && printf '%s\n' "$TEST_OUTPUT" >&2
  fi
  collect_warnings_from_output "$TEST_OUTPUT"

  if NODE_OUTPUT="$(cd "$ROOT_DIR" && node npm/testing/test-platform-detection.js 2>&1)"; then
    STATUS_NODE="pass"
    [[ -n "$NODE_OUTPUT" ]] && printf '%s\n' "$NODE_OUTPUT"
  else
    STATUS_NODE="fail"
    [[ -n "$NODE_OUTPUT" ]] && printf '%s\n' "$NODE_OUTPUT" >&2
  fi
  collect_warnings_from_output "$NODE_OUTPUT"
fi

WARNINGS_SECTION="- none"
if [[ ${#WARNINGS[@]} -gt 0 ]]; then
  WARNINGS_SECTION=""
  for warning_line in "${WARNINGS[@]}"; do
    WARNINGS_SECTION+=$'\n'"- $warning_line"
  done
  WARNINGS_SECTION="${WARNINGS_SECTION#"$'\n'"}"
fi

cat > "$REPORT_PATH" <<EOF
# Setup/Monitor Manual Verification Report

- Generated at: $(date -u +"%Y-%m-%dT%H:%M:%SZ")
- Repository: $ROOT_DIR

## Automated preflight

- cargo fmt --check: $STATUS_FMT
- cargo test: $STATUS_TEST
- node npm/testing/test-platform-detection.js: $STATUS_NODE

## Manual scenario checklist

Automated core-runtime coverage already exists in:

- \`thread::tests::test_core_runtime_acceptance_setup_status_monitor_vector_and_config_updates\`

Use this checklist only for target ACP client / editor behavior that the repo
cannot prove by itself. Run these in an ACP client (for example Zed Agent
Panel) using the same workspace.

If \`xsfire-camp\` was already running before reinstalling the binary, restart the ACP client session or restart Zed first so the client respawns the updated command target.

1. Restart the ACP client session or restart Zed so the updated \`xsfire-camp\` binary is respawned.
2. Run \`/setup\` and confirm setup wizard text + Plan surface appears.
3. Ask ACP to output two local references in one reply:
   - a markdown link to a known source/doc file in this workspace; confirm ACP renders it as a \`file:///...\` link and clicking opens the intended file.
   - a markdown link to a known raw executable artifact (for example \`target/release/<binary>\`); confirm ACP renders it as non-clickable code text like \`name: \`/abs/path\`\` instead of a clickable file link, so no macOS \`-50\` dialog appears.
4. While the task is still running, confirm ACP shows live plan progress in at least one visible surface:
   - Zed: Plan panel rows update immediately.
   - Non-Zed ACP client: agent text includes \`Plan update: ...\`, \`Current: ...\`, and optional \`Note: ...\`.
5. Open Config Options and change one option among:
   - Model / Reasoning Effort / Approval Preset
   - Task Orchestration / Task Monitoring / Progress Vector Checks
   Confirm Plan progress updates immediately.
6. Finish a prompt that triggers at least one tool call or exec step, wait for the final \`completed\` message, and confirm ACP leaves processing state promptly instead of spinning indefinitely.
7. Set \`Task Orchestration\` to \`sequential\`, start one task, then send another prompt.
   Confirm sequential wait guidance appears instead of submitting a parallel task.
8. Inspect logs:
   - \`logs/codex_chats/.../*.md\` contains Plan/ToolCall/RequestPermission traces.
   - Optional: \`ACP_HOME/sessions/<id>/canonical.jsonl\` contains \`acp.plan\` updates.

## Non-fatal warnings (separated from result)

$WARNINGS_SECTION

## Result summary

- Automated preflight overall: $( [[ "$STATUS_FMT" == "pass" && "$STATUS_TEST" == "pass" && "$STATUS_NODE" == "pass" ]] && echo "pass" || { [[ "$SKIP_GATES" == "true" ]] && echo "skipped"; [[ "$SKIP_GATES" != "true" ]] && echo "check-failures"; } )
- Manual checks: pending (fill during execution)
EOF

echo "Generated report: $REPORT_PATH"
if [[ "$SKIP_GATES" != "true" ]]; then
  if [[ "$STATUS_FMT" != "pass" || "$STATUS_TEST" != "pass" || "$STATUS_NODE" != "pass" ]]; then
    echo "One or more preflight checks failed. See report: $REPORT_PATH" >&2
    exit 2
  fi
fi

exit 0
