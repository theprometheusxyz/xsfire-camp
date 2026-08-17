# xsfire-camp

ACP(Agent Client Protocol) 클라이언트에서 Codex, Claude Code, Gemini를 실행형 에이전트로 라우팅하는 통합 브리지입니다.

`xsfire-camp` lets ACP-compatible clients (for example Zed) run Codex CLI, Claude Code CLI, and Gemini CLI as execution-first agent sessions.

- ACP: https://agentclientprotocol.com/
- Codex: https://github.com/openai/codex
- Project purpose: keep session continuity across IDE and CLI with structured logs and approval flow

## Quick Start (60 sec)

1. Build
```bash
cargo build --release
```

2. Run
```bash
OPENAI_API_KEY=... CODEX_HOME="$HOME/.codex" target/release/xsfire-camp
```

3. Optional: multi-backend mode
```bash
target/release/xsfire-camp --backend=multi
```

4. Verify
```bash
cargo test
```

## Who This Is For

- ACP client users who want stable Codex behavior independent of client adapter changes
- Teams that need traceable tool/plan/approval logs for review and safety
- Operators who want to preserve context across terminal and IDE sessions

## Prerequisites

| Item | Required | Notes |
| --- | --- | --- |
| Rust toolchain | Yes (build from source) | For `cargo build --release` |
| ACP client (example: Zed) | Yes | Must support stdio ACP agent |
| Auth (`OPENAI_API_KEY` or `CODEX_API_KEY`) | Yes (Codex backend) | Depends on backend/auth route |
| `CODEX_HOME` | Recommended | Session/thread continuity root |
| `ACP_HOME` | Optional | Canonical ACP log root (default `~/.acp`) |

## Run Modes

```bash
target/release/xsfire-camp --backend=codex
target/release/xsfire-camp --backend=claude-code
target/release/xsfire-camp --backend=gemini
target/release/xsfire-camp --backend=multi
```

Notes:
- `claude-code` and `gemini` backends require their CLIs to be installed and authenticated.
- In `multi` mode, switch backend in-thread: `/backend codex|claude-code|gemini`.
- Backend-specific overrides:
  - `XSFIRE_CODEX_OPEN_BROWSER=1` to let ACP-triggered ChatGPT login try opening your browser automatically
  - `XSFIRE_CLAUDE_BIN`, `XSFIRE_CLAUDE_ARGS`
  - `XSFIRE_GEMINI_BIN`, `XSFIRE_GEMINI_ARGS`, `XSFIRE_GEMINI_APPROVAL_MODE`

## Common Commands Snapshot

| Category | Commands |
| --- | --- |
| Core | `/setup`, `/review`, `/review-branch`, `/review-commit`, `/compact`, `/undo`, `/init`, `/status` |
| Session | `/sessions`, `/load` |
| Integrations | `/mcp`, `/skills` |
| Monitoring | `/monitor`, `/monitor retro`, `/vector`, `/experimental` |
| UX | `/new-window` |

## 현재 보유 기능 목록

### 핵심 런타임
- ACP(Agent Client Protocol) V1 호환 어댑터로 ACP 클라이언트와 stdio 기반으로 동작한다.
- 멀티 백엔드 모드(`codex`, `claude-code`, `gemini`, `multi`)를 지원한다.
- 멀티 모드에서는 세션 단위 라우팅과 `/backend <backend>` 동적 전환이 가능하다.

### 세션 연속성 및 운영
- 새 세션 생성, 이어가기, 포크(`fork`), 동기화/요약(`compact`), 되돌리기(`undo`)를 지원한다.
- 세션/이벤트 로그를 Canonical 스토어에 영속화하고, 백엔드별 식별자 매핑을 통해 재로드를 지원한다.
- 세션 진행은 `/status`, `/monitor`, `/monitor retro`, `/vector`, `/new-window`로 점검한다.

### 승인/도구 실행 제어
- 툴 호출 전 사용자 승인 흐름(`approvals`)과 이벤트(`permission`/`tool`/`plan`/`thought`) 로깅을 제공한다.
- 위험 동작/명령에 대한 사전 확인이 가능한 실행 우선 정책을 유지한다.

### 내장 명령어 커버리지
- 핵심: `/setup`, `/model`, `/personality`, `/approvals`, `/permissions`, `/status`
- 세션: `/new`, `/resume`, `/fork`, `/diff`, `/load`, `/sessions`, `/undo`, `/compact`
- 운영/검토: `/feedback`, `/review`, `/review-branch`, `/review-commit`, `/init`, `/logout`
- 통합: `/mcp`, `/skills`, `/mention`, `/vector`, `/monitor`, `/experimental`

### 백엔드/외부 통합
- `codex` 백엔드는 인증/세션/리스트/로드/모드·모델·옵션 제어가 포함된 완전 모드에 가깝다.
- `claude-code`, `gemini` 백엔드는 CLI 연동형으로 동작하며 백엔드별 제약이 있는 경량 모드이다.
- 커스텀 프롬프트(`prompts`) 로딩을 지원하고 `prompts:` 접두사 동적 명령을 사용할 수 있다.

### 배포·개발 지원
- `cargo build --release` 와 `cargo test` 가 기본 검증 라인이다.
- `build_and_install.sh`, `acp_compat_smoke.sh`, `tag_release.sh`로 빌드/릴리스 운영 루틴이 갖춰져 있다.
- `extension.toml` 기반 Zed 연동과 GitHub release binary 배포 채널을 유지한다.

## Current Feature Inventory

### Core runtime
- Operates as an ACP (Agent Client Protocol) V1-compatible adapter over stdio.
- Supports multi-backend execution (`codex`, `claude-code`, `gemini`, `multi`).
- Allows in-session backend switching via `/backend <backend>` in multi mode.

### Session continuity and operations
- Supports creating, resuming, forking, compacting, and undoing sessions.
- Persists session/event logs in canonical storage and remaps backend identifiers for reload.
- Uses `/status`, `/monitor`, `/monitor retro`, `/vector`, and `/new-window` for runtime visibility.

### Approval and tool execution control
- Supports pre-execution approval flow and logs events such as permission, tool, plan, and thought updates.
- Keeps risky actions gated by explicit user confirmation.

### Built-in command coverage
- Core: `/setup`, `/model`, `/personality`, `/approvals`, `/permissions`, `/status`
- Session: `/new`, `/resume`, `/fork`, `/diff`, `/load`, `/sessions`, `/undo`, `/compact`
- Review/ops: `/feedback`, `/review`, `/review-branch`, `/review-commit`, `/init`, `/logout`
- Integrations: `/mcp`, `/skills`, `/mention`, `/vector`, `/monitor`, `/experimental`

### Backend and external integration
- `codex` is the most complete backend with authentication, session, model, mode, and option controls.
- `claude-code` and `gemini` are CLI-bridged lightweight modes with different session capabilities.
- Supports dynamic custom prompts through prompt loading and `prompts:` command prefixes.

### Delivery and development support
- Core verification commands remain: `cargo build --release` and `cargo test`.
- Build/deploy scripts include `build_and_install.sh`, `acp_compat_smoke.sh`, and `tag_release.sh`.
- Keeps Zed and GitHub release binary distribution aligned through `extension.toml` and release metadata.

## Release Notes (3-line summary)

### 한글
1. ACP stdio 적응기 기반으로 멀티 백엔드 라우팅과 세션 연속성 로그를 제공합니다.
2. 승인/도구 실행 가드와 핵심 `/setup /review /compact /undo /monitor /status` 계열 명령을 강화했습니다.
3. Zed/ACP 배포 경로(확장 매니페스트·GitHub release binary)를 기반으로 릴리스/운영 루틴을 정비했습니다.

### English
1. Provides ACP stdio-first execution with multi-backend routing and persistent session continuity logs.
2. Tightens approval-first tool execution with core workflow commands across setup, review, session, and monitoring.
3. Aligns release and distribution flow with Zed extension manifests and GitHub release binaries.

## Client Integration

### Zed custom agent registration

`settings.json` example:

```json
{
  "agent_servers": {
    "xsfire-camp": {
      "type": "custom",
      "command": "/absolute/path/to/xsfire-camp",
      "env": {
        "CODEX_HOME": "/Users/you/.codex"
      }
    }
  }
}
```

### VS Code notes

This repository does not ship a VS Code ACP extension. Use a VS Code ACP client extension that can run a stdio custom agent.

Compatibility note:

```bash
xsfire-camp acp
```

If the client resolves agents from `PATH`, install from the release binary and expose it directly:

```bash
install -m 0755 target/release/xsfire-camp /usr/local/bin/xsfire-camp
```

## ACP Registry Notes

- The ACP registry entry installs `xsfire-camp` from GitHub release binaries, not from the npm package.
- `codex` is the most complete backend for ACP use. It carries the full `xsfire-camp` auth/session/tool-plan flow.
- `claude-code` and `gemini` are lightweight CLI bridges. Registry install only provides `xsfire-camp` itself; you still need the upstream CLI installed and authenticated on the local machine.
- If your ACP client expects one self-contained agent binary with no extra local setup, prefer the `codex` backend.

## Troubleshooting (Top 5)

1. Auth error on startup
- Check `OPENAI_API_KEY` or `CODEX_API_KEY` is set for Codex backend.
- If you already ran `codex login`, point both CLI and ACP to the same `CODEX_HOME`; `xsfire-camp` will reuse the saved credentials.
- ChatGPT login from ACP prints a localhost auth URL to stderr. Browser auto-open is disabled by default to avoid macOS app-open failures; set `XSFIRE_CODEX_OPEN_BROWSER=1` only if you want ACP to try launching the browser itself.
- For headless or SSH workflows, set `NO_BROWSER=1` and use an API key auth method instead.

2. Sessions not shared between CLI and ACP client
- Ensure both run with the same `CODEX_HOME`.

3. Backend switch fails
- Confirm target backend CLI (`claude`/`gemini`) is installed and authenticated.

4. Legacy npm instructions still appear
- `xsfire-camp` no longer ships through npm. Use the GitHub release binary or ACP registry entry instead.

5. ACP registry entry not visible yet
- Check ACP registry PR/check status first (`agentclientprotocol/registry`), especially `action_required` workflows that need maintainer intervention. Keep registry PR comments in English and post only evidence-backed updates.

## Docs Index

### Architecture and backend
- `docs/backend/backends.md`
- `docs/backend/session_store.md`
- `docs/backend/policies.md`
- `docs/reference/acp_standard_spec.md`
- `docs/reference/event_handling.md`
- `docs/reference/codex_home_overview.md`

### Planning and roadmap
- `docs/plans/roadmap.md`
- `docs/plans/next_cycle_execution_plan.md`
- `docs/plans/orchestration_plan_backend_driver_setup_context.md`

### Quality and release
- `docs/quality/system_requirements_done_criteria.md`
- `docs/quality/verification_guidance.md`
- `docs/quality/qa_checklist.md`
- `docs/guides/github_registry_release_runbook.md`
- `docs/releases/release_notes_v0.9.24.md`
- `docs/releases/release_notes_v0.9.23.md`
- `docs/releases/release_notes_v0.9.22.md`
- `docs/releases/release_notes_v0.9.21.md`
- `docs/releases/release_notes_v0.9.20.md`
- `docs/releases/release_notes_v0.9.19.md`
- `docs/releases/release_notes_v0.9.18.md`
- `docs/releases/release_notes_v0.9.17.md`
- `docs/releases/release_notes_v0.9.16.md`
- `docs/releases/release_notes_v0.9.15.md`
- `docs/releases/release_notes_v0.9.14.md`
- `docs/releases/release_notes_v0.9.13.md`
- `docs/releases/release_notes_v0.9.12.md`
- `docs/releases/release_notes_v0.9.11.md`
- `docs/releases/release_notes_v0.9.10.md`
- `docs/releases/release_notes_v0.9.8.md`

### ACP/Zed integration
- `docs/zed/zed_extension_pr_template.md`
- `docs/zed/extensions_toml_sample.md`
- `docs/zed/install_shared_settings.md`

## README Update Checklist (for each release)

1. Version strings and examples align with the current release.
2. Quick Start command flags still match binary behavior.
3. Verification commands are still valid.
4. `docs/` links are alive and accurate.
5. GitHub release binary and ACP registry status text is current.
6. New release notes link is added.

## English Summary

`xsfire-camp` is an ACP bridge around Codex CLI.
It focuses on:
- execution-first sessions instead of plain chat,
- continuity across IDE and terminal via shared `CODEX_HOME`,
- traceable tool/plan/approval updates,
- explicit control gates for risky operations.

If you are new, start from **Quick Start**, then jump to **Docs Index**.

## License

CC BY-NC-SA 4.0. See `LICENSE`.
