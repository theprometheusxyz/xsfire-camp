# xsfire-camp 시스템 요구사항 및 완료 기준

Updated: 2026-04-13

## Goal

`xsfire-camp`의 전체 기능 요구사항, 완료 판정 조건, 현재 검증 증거를 하나의 정본으로 고정한다.

## Scope

- 이 문서는 `README.md`의 기능 인벤토리, `docs/reference/acp_standard_spec.md`의 ACP 매핑, `docs/quality/qa_checklist.md`의 출고 점검, 최신 자동 검증 산출물을 하나의 판단 표면으로 합친다.
- 목적은 `구현 완료`, `운영 수용`, `외부 릴리스/레지스트리`를 같은 체크리스트에 섞지 않고 분리하는 것이다.

## Completion Model

### Level 1. Implementation Complete

아래 `Repo-controlled Must`가 모두 통과하면 구현 완료로 본다.

- 코드/문서/테스트/스모크/빌드로 저장소 안에서 재현 가능해야 한다.
- 수동 에디터 조작이나 외부 메인테이너 승인에 의존하면 안 된다.

### Level 2. External Client Compatibility

아래 `External Client Gates`가 모두 통과하면 target ACP client 수용 완료로 본다.

- ACP 클라이언트 또는 Zed 같은 실제 런타임에서 다시 확인해야 한다.
- 구현 미완료와 구분하기 위해 별도 게이트로 관리한다.

### Level 3. Release / Registry Readiness

릴리스 태그, GitHub release, ACP registry PR 상태, maintainer blocker는 별도 운영 게이트다.

## Primary Evidence Sources

- 기능 범위: `README.md`, `docs/backend/backends.md`
- ACP 계약 정본: `docs/reference/acp_standard_spec.md`, `docs/reference/event_handling.md`
- 출고/운영 점검: `docs/quality/qa_checklist.md`, `docs/quality/iteration_fit_v0.9.24_acp_readiness.md`
- 자동 검증: `cargo fmt --check`, `cargo test`, `cargo build --release`, `scripts/acp_compat_smoke.sh --strict`
- 수동 검증 보조: `scripts/manual_verification_setup_monitor.sh`

## Repo-controlled Must

### FR-01. ACP 서버 계약과 capability 광고

Requirement

- 서버는 stdio 기반 ACP v1 어댑터로 동작해야 한다.
- `initialize`는 구현된 capability와 일치하는 광고를 반환해야 한다.

Done criteria

- `protocolVersion=v1`
- `promptCapabilities.embeddedContext=true`
- `promptCapabilities.image=true`
- `promptCapabilities.audio=false`
- `mcp.http=true`
- `mcp.sse=false`
- `session.list=true`

Evidence

- `docs/reference/acp_standard_spec.md`
- `logs/smoke/acp_compat_smoke_20260413_081510.md`

Current status

- Pass

### FR-02. 백엔드 표면과 전환 일관성

Requirement

- `codex`, `claude-code`, `gemini`, `multi` 백엔드가 선언된 계약대로 동작해야 한다.
- `multi` 모드의 `/backend <backend>`와 config selector 전환은 같은 검증 규칙과 같은 상태 갱신 규칙을 따라야 한다.

Done criteria

- `multi|all`은 selector 입력으로 허용되지 않는다.
- 유효한 `/backend <backend>` 전환은 세션 상태와 `ConfigOptionUpdate`를 함께 갱신한다.
- `claude-code`, `gemini`는 선언된 경량 계약만 제공하고 unsupported surface는 명시적으로 거부한다.

Evidence

- `src/backend.rs`
- `src/multi_backend.rs`
- `src/claude_code_agent.rs`
- `src/gemini_agent.rs`
- `logs/smoke/acp_compat_smoke_20260413_081510.md`

Current status

- Pass

### FR-03. 세션 연속성 및 canonical 저장

Requirement

- 새 세션, 목록, 이어가기, 포크, 재개, compact/undo 관련 연속성이 선언 범위 안에서 유지돼야 한다.
- canonical 로그는 세션/이벤트 상관관계를 보존하고 민감값을 그대로 남기지 않아야 한다.

Done criteria

- `codex`는 `session/load`, `session/fork`, `session/resume`를 지원한다.
- `multi`는 codex-backed 세션에 한해 wrapped cursor/fork/resume 규칙을 유지한다.
- canonical log는 correlation path를 유지하고 secret redaction이 적용된다.

Evidence

- `docs/reference/acp_standard_spec.md`
- `src/session_store.rs`
- `src/multi_backend.rs`
- `logs/smoke/acp_compat_smoke_20260413_081510.md`

Current status

- Pass

### FR-04. 진행 상태, plan, monitor 가시성

Requirement

- ACP 세션은 setup/status/monitor/vector 흐름에서 진행 상태를 추적 가능하게 보여야 한다.
- Zed 이외 클라이언트에서도 최소한의 visible progress text를 유지해야 한다.

Done criteria

- setup plan verification progress 테스트가 통과한다.
- 같은 runtime에서 `/setup -> /status -> /monitor -> /vector`를 순서대로 실행했을 때 verify step이 `completed`까지 진행된다.
- config option 변경 후 `/status`와 `ConfigOptionUpdate`가 같은 값(`task_orchestration=sequential` 등)을 보고한다.
- `/monitor` 가시성 관련 회귀 테스트가 통과한다.
- non-Zed client는 visible progress text를 받고, Zed client는 중복 텍스트를 피한다.
- canonical log가 `acp.prompt`, `acp.plan`, `acp.task_monitoring.*` 상관관계를 남긴다.

Evidence

- `src/thread.rs`
- `docs/quality/iteration_fit_v0.9.24_acp_readiness.md`
- `logs/smoke/acp_compat_smoke_20260413_081510.md`

Current status

- Pass

### FR-05. 승인 흐름과 안전한 실행 기록

Requirement

- 민감 동작은 permission round-trip으로 보호돼야 한다.
- plan/tool/permission 이벤트는 canonical 로그에서 추적 가능해야 한다.

Done criteria

- `session/request_permission` 요청/응답 쌍이 기록된다.
- canonical log correlation path가 보존된다.
- 위험 실행은 승인 정책에 묶여 동작한다.

Evidence

- `src/thread.rs`
- `docs/reference/event_handling.md`
- `docs/backend/session_store.md`
- `logs/smoke/acp_compat_smoke_20260413_081510.md`

Current status

- Pass

### FR-06. 파일시스템 경계와 링크 안전화

Requirement

- ACP FS capability가 있는 경우 파일 read/write는 session root 경계 밖으로 나가면 안 된다.
- ACP FS capability가 없을 때만 로컬 FS fallback을 사용해야 한다.
- source/doc local file links는 `file:///...` URI로 정규화되고, raw executable artifact path는 비클릭 텍스트로 유지돼야 한다.

Done criteria

- client FS capability enabled 시 out-of-root read/write가 `PermissionDenied`로 실패한다.
- client FS capability disabled 시 로컬 FS fallback read/write가 동작한다.
- outgoing agent text가 local markdown links를 `file:///...` URI로 정규화한다.
- raw executable path는 clickable local file link로 노출되지 않는다.

Evidence

- `src/local_spawner.rs`
- `src/link_paths.rs`
- `src/thread.rs`
- `docs/quality/iteration_fit_v0.9.24_acp_readiness.md`
- `logs/smoke/acp_compat_smoke_20260413_081510.md`

Current status

- Pass

### FR-07. terminal lifecycle 계약

Requirement

- `codex` exec는 ACP 표준 `terminal/*` lifecycle을 사용해야 한다.
- legacy embedded terminal 확장은 호환 범위 안에서만 유지돼야 한다.

Done criteria

- `terminal/create -> terminal/output -> terminal/release`
- cancel path: `terminal/kill -> terminal/wait_for_exit -> terminal/release`
- real `terminal_id`가 있으면 표준 terminal content를 사용하고, 없을 때만 text fallback을 사용한다.

Evidence

- `src/codex_agent.rs`
- `src/thread.rs`
- `docs/reference/acp_standard_spec.md`
- `logs/smoke/acp_compat_smoke_20260413_081510.md`

Current status

- Pass

### FR-08. 빌드, 스모크, 배포 가능성

Requirement

- 저장소는 release binary를 빌드할 수 있고, 핵심 ACP 회귀를 strict smoke로 재현 가능해야 한다.

Done criteria

- `cargo fmt --check` 통과
- `cargo test` 통과
- `cargo build --release` 통과
- `scripts/acp_compat_smoke.sh --strict` 통과

Evidence

- `cargo fmt --check` on 2026-04-13: pass
- `cargo test` on 2026-04-13: `118 passed, 0 failed`
- `cargo build --release` on 2026-04-13: pass
- `logs/smoke/acp_compat_smoke_20260413_081510.md`

Current status

- Pass

### FR-09. 운영 문서와 traceability

Requirement

- 기능 정본, ACP 매핑, 검증 가이드, release checklist가 서로 orphan 되지 않고 연결돼야 한다.

Done criteria

- 이 문서가 `docs/README.md`와 `README.md`에서 발견 가능하다.
- QA checklist와 readiness record가 이 문서를 참조한다.

Evidence

- `README.md`
- `docs/README.md`
- `docs/quality/qa_checklist.md`

Current status

- Pass

## External Client Gates

아래 항목은 구현 완료 판정과 분리한다. 코드/테스트만으로 닫히지 않고 target ACP client가 필요하다.

### EG-01. Live local-link rendering in ACP client

- source/doc file link는 클릭 시 정상 open.
- raw executable path는 non-clickable code text.
- macOS `-50` Launch Services dialog가 재발하지 않아야 한다.

Current status

- Pending target-client evidence

### EG-02. Live plan/progress rendering in ACP client

- `/setup` 후 plan surface가 실제 target client UI에 렌더링돼야 한다.
- `Model`, `Approval Preset`, 또는 monitoring option 변경 시 plan/progress UI가 즉시 갱신돼야 한다.
- tool call / progress surface가 spinning 없이 종료 상태로 닫혀야 한다.

Current status

- Pending target-client evidence

## Release / Registry Gates

- tag, GitHub release, registry PR comment/run evidence는 `docs/quality/qa_checklist.md`와 `docs/guides/github_registry_release_runbook.md`를 따른다.
- upstream maintainer approval, registry CI re-run approval은 저장소 밖 운영 게이트다.

## Sequence-dependent Completion Plan (R -> P -> M -> W -> A)

1. Research
   - 기능 인벤토리, ACP 매핑, 기존 QA/readiness 문서에서 현재 범위를 고정한다.
2. Plan
   - `Repo-controlled Must`, `Operator Acceptance Gates`, `Release / Registry Gates`를 분리한다.
3. Make
   - 문서 정본을 작성한다.
   - repo-controlled 증거가 비어 있는 항목은 테스트/스모크로 메운다.
4. Verify
   - `cargo fmt --check`
   - `cargo test`
   - `cargo build --release`
   - `scripts/acp_compat_smoke.sh --strict`
5. Accept
   - external-client-only gate는 live ACP client UI evidence로 닫는다.

## Current Decision

- `Implementation Complete`: pass
- `External Client Compatibility`: pending `EG-01..02`
- `Release / Registry Readiness`: separate operational gate
