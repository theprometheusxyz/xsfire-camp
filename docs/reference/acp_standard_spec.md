# ACP 표준 스펙 및 xsfire-camp 기능 매핑

이 문서는 ACP(Agent Client Protocol) 표준의 핵심 스펙을 `xsfire-camp` 기준으로 정리하고,
현재 구현 기능을 백엔드별로 매핑한 운영 문서입니다.

## 1) 기준 버전 및 범위

- ACP SDK: `agent-client-protocol` `0.9.3`
- ACP 스키마: `agent-client-protocol-schema` `0.10.6`
- 프로토콜 버전: `v1` (JSON-RPC 2.0)
- 본 문서 범위:
  - ACP 메서드/알림/capability 표준 요약
  - `xsfire-camp` 실제 지원 범위(특히 `codex`, `claude-code`, `gemini`)
  - 운영 시 주의할 제약과 확장 포인트

## 2) ACP 표준 핵심 요약

### 2-1. 전송 및 연결 모델

- ACP는 JSON-RPC 2.0 기반 프로토콜입니다.
- 일반적으로 클라이언트(IDE)가 에이전트를 subprocess로 실행하고 `stdio`로 통신합니다.
- 연결 수립 후 `initialize`에서 프로토콜 버전과 capability를 협상합니다.

### 2-2. 표준 흐름(세션 단위)

1. `initialize`
2. (필요 시) `authenticate`
3. `session/new` 또는 `session/load`
4. `session/prompt` 반복
   - 중간 상태는 `session/update` notification으로 스트리밍
   - 민감 작업은 `session/request_permission` 왕복으로 승인 처리
5. 필요 시 `session/cancel`

### 2-3. 핵심 메서드/알림

| 구분 | 이름 | 스펙 상태 | 설명 |
|---|---|---|---|
| agent request | `initialize` | stable | 버전/capability/auth method 협상 |
| agent request | `authenticate` | stable | 인증 방법 실행 |
| agent request | `session/new` | stable | 새 세션 생성 |
| agent request | `session/load` | stable (optional capability) | 기존 세션 로드 |
| agent request | `session/prompt` | stable | 사용자 프롬프트 처리 |
| agent notification | `session/cancel` | stable | 진행 중 turn 취소 |
| agent request | `session/set_mode` | stable | 세션 모드 전환 |
| agent request | `session/list` | unstable | 세션 목록 조회 |
| agent request | `session/set_model` | unstable | 세션 모델 전환 |
| agent request | `session/set_config_option` | unstable | 세션 설정 옵션 변경 |
| agent request | `session/fork` | unstable | 세션 포크 |
| agent request | `session/resume` | unstable | 히스토리 replay 없이 재개 |
| client notification | `session/update` | stable | 에이전트의 스트리밍 업데이트 수신 |
| client request | `session/request_permission` | stable | 에이전트가 사용자 승인 요청 |
| client request | `fs/read_text_file`, `fs/write_text_file` | stable | 파일 읽기/쓰기 |
| client request | `terminal/*` | stable | 터미널 생성/출력/종료/해제 |

추가로 ACP는 `_prefix` 기반 ext method/notification을 허용합니다(확장성).

### 2-4. 콘텐츠/툴/플랜 스키마 핵심

- `ContentBlock` 기본 지원: `text`, `resource_link` (프롬프트 최소 기준)
- 선택 지원: `image`, `audio`, `resource`(embedded context)
- `ToolCall`/`ToolCallUpdate`는 `pending/in_progress/completed/failed` 상태를 가집니다.
- `Plan`은 엔트리 단위로 `pending/in_progress/completed`를 스트리밍합니다.
- `PromptResponse.stopReason` 주요 값:
  - `end_turn`, `max_tokens`, `max_turn_requests`, `refusal`, `cancelled`

## 3) xsfire-camp 구현 매핑

### 3-1. initialize 응답(capability 광고)

`xsfire-camp`는 initialize 시 다음과 같이 capability를 광고합니다.

- `protocolVersion`: 항상 `v1`
- `promptCapabilities`:
  - `embeddedContext=true`
  - `image=true`
  - `audio=false`
- `mcpCapabilities`:
  - `http=true`
  - `sse=false`
- `sessionCapabilities`:
  - `list` 지원 광고
- `loadSession`:
  - `--backend=codex`, `--backend=multi`일 때 `true`
  - `multi`는 `codex` 세션만 load 가능
  - 그 외 백엔드는 `false`

### 3-2. 백엔드별 ACP 메서드 지원 현황

| ACP 항목 | `codex` | `claude-code` | `gemini` |
|---|---|---|---|
| `authenticate` | 지원 (ChatGPT/API key 방식) | 지원 (`claude auth status` 검증, interactive login 시작은 미지원) | 부분 지원 (CLI 가용성 + auth 설정 준비 상태 검증, interactive login/status 실행은 미지원) |
| `session/new` | 지원 (지속형 세션) | 지원 (in-memory 세션) | 지원 (in-memory 세션) |
| `session/load` | 지원 | 미지원 (`invalid_params`) | 미지원 (`invalid_params`) |
| `session/list` | 지원 (`CODEX_HOME` 기반 목록) | 지원 (현재 프로세스 메모리 목록) | 지원 (현재 프로세스 메모리 목록) |
| `session/prompt` | 지원 (스트리밍/툴콜/승인/플랜 포함) | 지원 (원샷 텍스트 청크 중심) | 지원 (원샷 텍스트 청크 중심) |
| `session/cancel` | 지원 (실행 중 turn 취소) | 지원 (실행 중 CLI prompt 취소) | 지원 (실행 중 CLI prompt 취소) |
| `session/set_mode` | 지원 | 미지원 (`invalid_params`) | 미지원 (`invalid_params`) |
| `session/set_model` (unstable) | 지원 | 지원 | 지원 |
| `session/set_config_option` (unstable) | 지원 | 지원 (`model`만) | 지원 (`model`만) |
| `session/fork` (unstable) | 지원 | 미지원 | 미지원 |
| `session/resume` (unstable) | 지원 | 미지원 | 미지원 |
| ext method/notification | 기본 동작(no-op/null) | 기본 동작(no-op/null) | 기본 동작(no-op/null) |

`multi` 백엔드는 다음처럼 동작합니다.

- `authenticate`: method id에 따라 `codex` / `claude-code` / `gemini`로 위임
- `session/load`: `codex` 세션만 load 가능
- `session/list`: `codex`의 load 가능한 세션 목록과 현재 프로세스의 routed session 목록을 함께 노출
  - 첫 페이지부터 `codex` 목록을 우선 반환합니다.
  - `codex`에 다음 페이지가 있으면 `nextCursor`는 `multi:codex:<cursor>` 형태로 래핑됩니다.
  - `codex` 페이지가 끝난 뒤 routed session이 남아 있으면 마지막 커서는 `multi:routed`를 반환합니다.
  - routed session 목록에는 synthetic id(`multi:*`)만 포함되며, `load_session`으로 가져온 `codex` 세션은 `codex` 목록에서 계속 발견됩니다.
- `session/fork`: `codex` 세션 id 또는 `codex`-backed routed session만 지원하며, 결과는 새로운 synthetic routed session id(`multi:*`)로 감쌉니다.
- `session/resume`: `codex` 세션 id 또는 `codex`-backed routed session만 지원하며, 요청한 session id를 유지한 채 `codex` 자식 세션에 다시 연결합니다.
- `session/set_config_option`의 `backend`: 현재 thread의 활성 backend 전환

### 3-3. 세션 업데이트(`session/update`) 동작

`codex` 백엔드 기준으로 다음 업데이트가 적극적으로 사용됩니다.

- `AgentMessageChunk`, `AgentThoughtChunk`
- `ToolCall`, `ToolCallUpdate`
- `Plan`
- `AvailableCommandsUpdate`
- `ConfigOptionUpdate` (unstable session config option 사용 시)

별도 mode 전용 업데이트 타입은 현재 `xsfire-camp`가 적극적으로 송신하지 않습니다.
현재 모드/설정 변화는 `ConfigOptionUpdate`, status text, plan progress update로 노출합니다.

이벤트 매핑 상세는 `docs/reference/event_handling.md`를 정본으로 사용합니다.

### 3-4. 승인/파일/터미널 연동

- 승인:
  - 민감 동작은 `session/request_permission`으로 사용자 선택을 받아 진행합니다.
  - canonical 로그(`ACP_HOME`)에 승인 요청/응답이 함께 기록됩니다.
- 파일:
  - 클라이언트가 `fs/*` capability를 광고하면 ACP 파일 API를 우선 사용합니다.
  - 그렇지 않으면 로컬 FS 경로를 사용합니다.
  - ACP 파일 API 경로에는 세션 루트(`cwd`) 바깥 접근 차단 검사가 적용됩니다.
  - 로컬 FS fallback 경로는 백엔드 sandbox/권한 정책에 의해 별도로 통제됩니다.
- 터미널:
  - `codex` backend의 exec는 ACP client RPC `terminal/create -> terminal/output -> terminal/release`로 구동됩니다.
  - 취소/정리 경로는 `terminal/kill -> terminal/wait_for_exit -> terminal/release`를 사용합니다.
  - `clientCapabilities.terminal=true`를 광고한 표준 클라이언트는 ACP client가 돌려준 실제 `terminal_id`를 `ToolCallContent::Terminal`로 받습니다.
  - `_meta.terminal_output=true`를 광고한 legacy 클라이언트는 embedded terminal 뷰 호환을 위해 `_meta.terminal_output`/`terminal_exit` 확장을 계속 받습니다.
  - plain-text `ToolCallUpdate` fallback은 exec 계층에서 실제 `terminal_id`를 확보하지 못한 경우에만 사용합니다.

### 3-5. 저장소/로그 연속성

- `codex` native 세션 저장소: `CODEX_HOME`
- 글로벌 canonical 로그(선택): `ACP_HOME` (기본 `~/.acp`)
  - `acp.prompt`
  - `acp.plan`
  - `acp.tool_call`, `acp.tool_call_update`
  - `acp.request_permission`, `acp.request_permission_response`
  - `acp.agent_message_chunk`, `acp.agent_thought_chunk`

상세 포맷/보안 정책:

- `docs/backend/session_store.md`
- `docs/backend/policies.md`

## 4) 구현 갭 및 운영 주의사항

1. `session/list`, `session/set_model`, `session/set_config_option`은 ACP unstable 영역이므로 스키마 변경에 대비해야 합니다.
2. `claude-code`, `gemini` 백엔드는 ACP 표면은 유지하지만, 현재는 최소 구현(in-memory + 원샷 프롬프트)입니다.
3. `claude-code`의 `authenticate`는 CLI auth status를 검증하고, `gemini`의 `authenticate`는 CLI 가용성과 auth 설정 준비 상태까지 검증합니다. 다만 둘 다 interactive login/status 실행 자체는 ACP에서 직접 수행하지 않습니다.
4. `codex` backend 터미널 연동은 ACP 표준 `terminal/*` lifecycle(`create/output/kill/wait_for_exit/release`)을 사용합니다. legacy `_meta.terminal_output` 확장은 embedded terminal UI 호환을 위해 유지되며, plain-text fallback은 실제 `terminal_id`가 없는 경우로 제한됩니다.
5. `session/fork`, `session/resume`은 `codex`와 `multi`(`codex`-backed session만)에서만 지원합니다.
6. ext method는 현재 기본 no-op이므로, 확장 기능이 필요하면 명시 구현이 필요합니다.

## 5) 관련 문서(정본 링크)

- 이벤트 매핑: `docs/reference/event_handling.md`
- 백엔드 개요: `docs/backend/backends.md`
- 백엔드 구현 가이드: `docs/backend/backend_development_guide.md`
- 세션 저장소: `docs/backend/session_store.md`
- 운영 정책: `docs/backend/policies.md`
