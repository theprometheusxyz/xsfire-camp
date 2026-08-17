# xsfire-camp Goal Contract (2026-08-10)

Format: `docs/quality/cross_environment_execution_protocol.md`의 Goal/Rubric 규칙(1문장 목표 + 검증 가능한 완료 조건 + Must/Should + Evidence)을 적용한다.
목표 원문 출처: `docs/plans/roadmap.md`. 완료 증거 출처: `docs/quality/system_requirements_done_criteria.md`.
Scope: "이 앱이 지금 달성하려는 목표"를 조건 단위로 고정하고, 각 조건의 현재 상태(pass/partial/blocked)를 근거와 함께 기록한다. 이 문서는 스냅샷이며, `roadmap.md`나 `system_requirements_done_criteria.md`가 바뀌면 재발행한다.

## Goal (1문장 + 검증 가능한 완료 조건)

- 원문 (`docs/plans/roadmap.md:7`): "Keep ACP work context continuous across clients and backend choices while preserving backend-native execution behavior (tool calls, approvals, file edits) instead of degrading to chat-only behavior."
- 완료 조건: 아래 `Must` 조건이 전부 `pass`이고 `Non-Goal` 경계를 넘는 변경이 없을 때만 "현재 목표 달성"으로 판정한다. 하나라도 `partial`/`blocked`이면 목표는 진행 중(open)이다.

## Rubric

### Must — Roadmap Milestone 조건 (현재 상태 포함)

**GC-01. Driver Boundary Hardening** (roadmap Milestone 1)
- 조건: 드라이버 capability 계약이 ACP 오케스트레이션 로직과 분리되고, 이벤트 변환 경로가 드라이버 간 결정적이며, 기존 Codex slash-command 동작이 하위호환을 유지한다.
- 현재 상태: **Pass** — `BackendDriver` trait(`src/backend.rs`)와 순수 위임 어댑터 `AcpAgent`(`src/acp_agent.rs`)로 이미 구조적 분리 완료.
- 미확인: Should 항목(`backend_development_guide.md` 체크리스트, 전용 회귀 테스트 1건)은 이번 조사에서 재확인하지 않음.
- Evidence: `src/backend.rs`, `src/acp_agent.rs`

**GC-02. Non-Codex Backend Fidelity** (roadmap Milestone 2) — **현재 가장 큰 갭**
- 조건: 비-Codex 백엔드의 tool/approval/terminal 진행 신호가 의미 손실 최소로 ACP 이벤트 카테고리에 매핑되고, `/backend <name>` 전환이 세션 연속성을 깨지 않으며, 인증 라우팅이 method-id 기준으로 문서화된다.
- 현재 상태: **Partial** — 전환 안정성(2번째 조건)은 `multi_backend.rs` 테스트로 뒷받침되어 pass로 보임. 1번째 조건은 **미충족**: `WorkOrchestrationProfile`이 Claude/Gemini에 대해 스스로 "returns one ACP text stream; live tool/approval bridging is not wired yet"라고 선언한다.
- Evidence: `src/backend.rs:107` (Claude), `src/backend.rs:118` (Gemini), `src/backend.rs:51` (`bridge_summary` = "single ACP message chunk only"), `docs/plans/next_actions_20260401.md` Phase D1

**GC-03. Session Continuity & Canonical Log Quality** (roadmap Milestone 3)
- 조건: 로그 스키마 버전/필수 필드가 명시되고, prompt→tool→approval→file 타임라인 correlation ID가 유지되며, redaction 정책이 테스트로 커버된다.
- 현재 상태: **Partial** — correlation 유지·redaction은 `system_requirements_done_criteria.md` FR-03/FR-05로 pass 근거가 있음. 로그 스키마의 명시적 버저닝 여부는 이번 조사에서 별도 확인하지 못함(과단정하지 않고 gap으로 남김).
- Evidence: `system_requirements_done_criteria.md` FR-03, FR-05, `src/session_store.rs`

**GC-04. Client Readiness & Release Operations** (roadmap Milestone 4)
- 조건: KR/EN quick-start 경로 일관성, 재현 가능한 릴리스 프로세스, platform package detection 정상 동작.
- 현재 상태: **Partial-Pass** — 릴리스 프로세스는 v0.9.8~v0.9.24까지 12회 이상 반복 실행되어 재현성 확인됨. quick-start 일관성·platform detection 세부는 이번 조사에서 재확인하지 않음. 참고: README 자체가 "npm 배포 채널 제거"(`aff98ea`)를 명시하므로, roadmap.md의 `node npm/testing/test-platform-detection.js` 검증 커맨드가 현재도 유효한지는 별도 확인이 필요하다(잠재적 문서 드리프트, 이번 범위 밖).
- Evidence: git tag 이력, `docs/releases/`, `README.md` Troubleshooting #4

### Must — Cross-cutting Safety/Boundary 조건 (roadmap 마일스톤에 명시적으로 없음)

**GC-05. 승인 게이트 없는 위험 실행 금지**
- 조건: 위험 동작은 실행 전 `session/request_permission` 왕복을 거쳐야 하고, 그 왕복이 canonical 로그에 기록돼야 한다.
- 현재 상태: **Pass** — `system_requirements_done_criteria.md` FR-05.
- Evidence: `src/thread.rs`, `docs/reference/event_handling.md`

**GC-06. 파일시스템 경계 및 링크 신뢰성**
- 조건: ACP FS capability 활성 시 세션 루트 밖 read/write는 거부되고, outgoing 텍스트의 로컬 링크는 `file:///` 정규화, raw 실행 경로는 비클릭 텍스트로 유지된다(macOS `-50` 재발 방지).
- 현재 상태: **Pass (repo-controlled) / Pending (live client)** — FR-06 pass, `EG-01` 실 클라이언트 증거는 아직 없음.
- Evidence: `src/link_paths.rs`, `src/local_spawner.rs`, `system_requirements_done_criteria.md` EG-01

**GC-07. 사설/개인 시스템과 결합하지 않음** *(신규, 2026-08-10 이번 세션에서 확정)*
- 조건: 어떤 슬래시 커맨드나 커넥터도 사용자 개인 워크스페이스(cogarch 등)의 비공개 자산·크리덴셜·로그를 노출하거나 그리로 라우팅하지 않는다. 선언된 백엔드 계약은 공개적인 Codex/Claude Code/Gemini로 한정한다.
- 현재 상태: **Pass** — `/cogarch` CA-ACP 게이트웨이(`src/ca_acp_gateway.rs`)와 관련 계약 문서(`docs/reference/ca_acp_packaged_system.md`) 삭제, README/문서 인덱스 6곳 정리 완료. `cargo test` 118 passed로 회귀 없음 재확인.
- Evidence: 이번 세션 삭제/편집 diff, `cargo check --all-targets` + `cargo test` 재실행 결과

**GC-08. 저장소 통제 품질 게이트**
- 조건: 릴리스 태깅 전 `cargo fmt --check`, `cargo test`, `cargo build --release`, `scripts/acp_compat_smoke.sh --strict`가 모두 통과해야 한다.
- 현재 상태: **Pass** (2026-08-10 재확인: `cargo fmt --check` clean, `cargo test` 118/0, `cargo check --all-targets` 통과). `cargo build --release`와 strict smoke는 이번 세션에서 재실행하지 않음.
- Evidence: FR-08, 이번 세션 커맨드 출력

### Must — External/Operational (저장소 통제 밖)

**GC-09. ACP 레지스트리 등록 경로**
- 조건: `agent.json`/`icon.svg`가 registry 스키마를 만족하고 CI auth-handshake 검증을 통과해야 한다.
- 현재 상태: **Blocked-external** — PR #93 메인테이너 워크플로 승인 대기 중.
- Evidence: `docs/guides/acp_registry_requirements.md`, `docs/plans/next_actions_20260401.md` Phase B

### Should
- 비-Codex 백엔드 feature matrix 문서화 (`docs/backend/backends.md`)
- Design system(Carbon) 통합 — `qa_checklist.md` 9항목 전부 미체크
- unstable ACP 메서드(`session/fork`, `session/resume` 등) 안정화 추적

## Non-Goals (경계 조건)
- 벤더별 세션 저장 포맷을 하나의 물리 포맷으로 강제 통합하지 않는다.
- 백엔드 고유 실행 시맨틱을 "chat-only 상호운용성"을 위해 희생하지 않는다.
- 사설 개인 시스템(cogarch/CognitiveArchtecture 등)과의 결합을 다시 도입하지 않는다 — GC-07 위반이면 즉시 회귀로 간주한다.

## Current Score
- Must: pass 4 (GC-01, GC-05, GC-07, GC-08) / partial 4 (GC-02, GC-03, GC-04, GC-06) / blocked-external 1 (GC-09) — 총 9
- Should: 0/3 확인됨

## Next Action
- GC-02(가장 큰 갭)를 닫으려면 Milestone 2 착수 — 비-Codex 백엔드 tool/approval 스트리밍 매핑 (`docs/plans/next_actions_20260401.md` Phase D1)
- GC-09는 외부 대기(PR #93 머지) 외 로컬에서 취할 액션 없음
- GC-03/GC-04의 "미확인" 항목은 다음 조사에서 로그 스키마 버저닝 문서와 npm 테스트 스크립트 현재 유효성을 확인해 이 문서를 갱신한다
