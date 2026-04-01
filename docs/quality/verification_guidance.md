# ACP 검증 실행 가이드

이 문서는 ACP 호환성 검증을 `실행 명령`과 `기대 결과` 중심으로 정리한 런북입니다.
정본 스펙/매핑 문서는 아래 두 파일을 기준으로 삼습니다.

- `docs/reference/acp_standard_spec.md`
- `docs/reference/event_handling.md`

## 0-b. 환경 독립 실행 규약(선행 단계)

이 워크플로우 이전에, 환경 독립형 구현 규약을 먼저 고정한다.
해당 규약은 모델/에이전트에 상관없이 재사용 가능하며,
항목은 `Goal -> Rubric -> Iteration` 형식으로 고정해야 한다.

참고 문서:
- `docs/quality/cross_environment_execution_protocol.md`
- 반복 기록 템플릿: `docs/quality/iteration_fit_template.md`

## 0. 사전 준비

### Step 0-1. 작업 디렉토리 확인

명령:

```bash
pwd
```

기대 결과:

- 출력 경로가 저장소 루트(예: `.../xsfire`)여야 합니다.

### Step 0-2. Rust 실행 가능 확인

명령:

```bash
cargo --version
```

기대 결과:

- 명령 종료 코드 `0`.
- 버전 문자열이 출력되고 `command not found`가 없어야 합니다.

## 1. 자동 기본 게이트

### Step 1-1. 포맷 검증

명령:

```bash
cargo fmt --check
```

기대 결과:

- 종료 코드 `0`.
- 포맷 오류가 없고 diff 출력이 없어야 합니다.

### Step 1-2. 단위 테스트

명령:

```bash
cargo test
```

기대 결과:

- 종료 코드 `0`.
- `thread`/`prompt_args`/`session_store` 관련 테스트가 모두 통과합니다.

### Step 1-3. release binary 빌드 확인

명령:

```bash
cargo build --release
```

기대 결과:

- 종료 코드 `0`.
- `target/release/xsfire-camp` 바이너리가 생성됩니다.

## 2. ACP 호환 Smoke(초안) 자동 점검

### Step 2-1. 스모크 스크립트 실행

명령:

```bash
scripts/acp_compat_smoke.sh
```

기대 결과:

- 종료 코드 `0`.
- `src/acp_agent.rs`의 핵심 메서드/초기 capability 선언 정적 점검이 `pass`.
- ACP 관련 타깃 테스트가 `pass`.
- 리포트 파일이 `logs/smoke/acp_compat_smoke_<timestamp>.md`에 생성됩니다.

### Step 2-2. 테스트 생략 모드(문서/코드 정적 체크만)

명령:

```bash
scripts/acp_compat_smoke.sh --skip-tests
```

기대 결과:

- 종료 코드 `0`.
- 정적 체크 결과만 포함한 리포트가 생성됩니다.

### Step 2-3. 엄격 모드(필수 ACP 회귀 테스트 고정 실행)

명령:

```bash
scripts/acp_compat_smoke.sh --strict
```

기대 결과:

- 종료 코드 `0`.
- 리포트에 `Strict mode: true`가 표시됩니다.
- 아래 필수 테스트가 모두 `pass`로 표시됩니다.
  - `thread::tests::test_setup_plan_verification_progress_updates`
  - `thread::tests::test_setup_plan_visible_in_monitor_output`
  - `thread::tests::test_monitoring_auto_mode_clears_completed_prompt_tasks`
  - `thread::tests::test_canonical_log_correlation_path`
  - `session_store::tests::writes_canonical_log_and_redacts_secrets`

### Step 2-4. 최신 스모크 리포트 확인

명령:

```bash
ls -1t logs/smoke/acp_compat_smoke_*.md | head -n 1
```

기대 결과:

- 최신 리포트 경로가 1개 출력됩니다.

## 3. Setup/Monitor 수동 시나리오

### Step 3-1. 수동 체크리스트 리포트 템플릿 생성

명령:

```bash
scripts/manual_verification_setup_monitor.sh
```

기대 결과:

- 종료 코드 `0`.
- `logs/manual_verification/setup_monitor_<timestamp>.md` 파일이 생성됩니다.

### Step 3-2. ACP 클라이언트에서 명령 순서 검증

명령(ACP 클라이언트 slash command):

```text
/setup
/status
/monitor
/vector
```

기대 결과:

- setup wizard Plan이 활성화됩니다.
- Plan 항목 `Verify: run /status, /monitor, and /vector`가 `pending -> in_progress -> completed`로 이동합니다.
- `/monitor` 출력에 `Task monitoring: orchestration=..., monitor=..., vector_checks=...`가 포함됩니다.

### Step 3-3. 모니터 회고 모드 검증

명령(ACP 클라이언트 slash command):

```text
/monitor retro
```

기대 결과:

- 레인 기반 회고형 상태 보고서(진행률/리스크/다음 작업)가 출력됩니다.

### Step 3-4. sequential 오케스트레이션 동작 검증

명령(ACP 클라이언트 설정 + 프롬프트 입력):

```text
Task Orchestration = sequential
```

기대 결과:

- 활성 task가 있는 상태에서 새 요청을 보내면 병렬 제출 대신 대기 안내 메시지가 나옵니다.

## 4. 로그/이벤트 매핑 확인

### Step 4-1. 대화 로그에 핵심 이벤트 기록 여부 확인

명령:

```bash
rg -n "Plan|ToolCall|RequestPermission" logs/codex_chats -g "*.md"
```

기대 결과:

- 최근 대화 로그에서 Plan/ToolCall/Permission 관련 라인이 검색됩니다.

### Step 4-2. 이벤트 매핑 문서와 구현 추적 대조

명령:

```bash
rg -n "PlanUpdate|ExecCommand|McpToolCall|RequestUserInput" docs/reference/event_handling.md
```

기대 결과:

- 표에 핵심 이벤트-ACP 출력 매핑이 존재해야 합니다.

## 5. 실패 시 처리 기준

- `cargo fmt --check` 실패: 포맷 정리 후 재실행.
- `cargo test` 실패: 실패 테스트 이름과 로그를 첨부해 원인 분석.
- smoke 스크립트 실패: 리포트(`logs/smoke/...`)에서 fail 항목 확인 후 문서/구현 싱크 재검증.
- strict 모드 테스트 실패: 로그(`logs/smoke/logs/*.log`)의 실패 케이스를 이슈에 첨부.
- 수동 시나리오 실패: `docs/reference/acp_standard_spec.md`와 실제 동작 차이를 이슈로 기록하고 릴리즈 노트에 반영.
