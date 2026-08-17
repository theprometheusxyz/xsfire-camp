# 후속 작업 목록 (2026-04-01 기준, 근거 기반)

> 채택 방향: PR #93 머지 후 ACP 크레이트 0.10.4 업그레이드 → 새 릴리스(v0.9.25) → 레지스트리 자동 버전 감지 cron 반영

---

## Phase A: 즉시 실행 가능

### A1. 수동 검증 항목 실행 (6a~6g)

- **근거**: `qa_checklist.md:32-39` — 7개 항목 미체크, 레지스트리 등록과 기술적 의존 없음
- **의존**: 로컬 바이너리 + Zed ACP 클라이언트만 필요
- **항목**:
  - 6a: `/setup` 실행 (`CODEX_HOME` 지정)
  - 6b: `/status` → `/monitor` → `/vector` 흐름 → setup plan step `completed` 확인
  - 6c: config 변경 → Plan 업데이트 즉시 반영 확인
  - 6d: `/monitor` 태스크 스냅샷 표시 확인
  - 6e: `logs/codex_chats/` 에서 `Plan`, `ToolCall`, `RequestPermission` 항목 확인
  - 6f: (선택) `~/.acp/canonical.jsonl` 생성 확인
  - 6g: Zed 에이전트 패널 업데이트 확인

### A2. vendor/codex-rs 갱신 검토

- **근거**: `Cargo.toml` patch 섹션, `cargo clippy` 경고 5개가 vendor 코드에서 발생
- **현재**: 2025-02-18 커밋 기준
- **작업**: `zed-industries/codex` acp 브랜치와 동기화 필요 여부 판단

### A3. `acp_registry_requirements.md` 커밋

- **근거**: 현재 untracked 상태 (`git status`)
- **파일**: `docs/guides/acp_registry_requirements.md`

---

## Phase B: 외부 대기 (레지스트리)

| # | 작업 | 근거 | 트리거 | 주체 |
|---|------|------|--------|------|
| B1 | ACP Registry PR #93 머지 대기 | 로컬 검증 6/6 통과, 코멘트 완료 ([issuecomment-4170950756](https://github.com/agentclientprotocol/registry/pull/93#issuecomment-4170950756)) | 메인테이너 워크플로 승인 | 외부 |
| B2 | 리뷰 피드백 대응 (영어) | Zed #4811에서 메인테이너가 영어 전용 요구 (MrSubidubi, 2026-02-25) | 메인테이너 코멘트 시 | 나 |

---

## Phase C: PR #93 머지 직후

### C1. ACP 크레이트 업그레이드 → v0.9.25 릴리스

- **채택 근거**: 머지 후 업그레이드하면 PR 갱신 마찰 없음. 릴리스 후 레지스트리 hourly cron이 새 버전 자동 감지/반영
- **현재 → 목표**:

| 크레이트 | 현재 | 목표 | 변경 사항 |
|----------|------|------|-----------|
| `agent-client-protocol` | 0.9.3 (2026-01-09) | 0.10.4 (2026-03-31) | API 변경 대응 |
| `agent-client-protocol-schema` | 0.10.6 | 0.11.2 | AuthMethod struct→tagged enum 마이그레이션 |

- **리스크**: AuthMethod 타입 마이그레이션, 0.10.x API breaking changes
- **이점**: `type` 필드 명시적 직렬화, unstable 메서드 안정성 향상, 레지스트리 검증기와 완전 호환
- **파일**: `Cargo.toml`, `Cargo.lock`, `src/codex_agent.rs`, `src/claude_code_agent.rs`, `src/gemini_agent.rs`, `src/backend.rs`
- **검증**: `cargo test` → `cargo clippy` → `scripts/acp_compat_smoke.sh --strict` → 릴리스 → 레지스트리 자동 반영 확인

### C2. extension.toml 및 릴리스 문서 갱신

- v0.9.25 릴리스에 맞춰 `extension.toml` 아카이브 URL/SHA256 갱신
- `docs/releases/release_notes_v0.9.25.md` 작성

---

## Phase D: 기능 확장 (로드맵 마일스톤 순서)

### D1. Milestone 2: Non-Codex 백엔드 Fidelity

- **근거**: `docs/plans/roadmap.md`, `docs/backend/backend_development_guide.md:144-149`
- **의존**: C1 완료 권장 (새 AuthMethod enum으로 백엔드별 인증 타입 명시)

| 백엔드 | 현재 지원 | 미지원 (갭) |
|--------|-----------|-------------|
| claude-code | authenticate, new_session, prompt, cancel, set_model | load_session, fork/resume, tool/approval 스트리밍, terminal lifecycle |
| gemini | authenticate, new_session, prompt, cancel, set_model | load_session, fork/resume, tool/approval 스트리밍, terminal lifecycle |

**우선 작업 순서** (backend_development_guide 기준):
1. persistent session loading (`load_session`)
2. streaming bridge (단일 청크 → 스트리밍)
3. approval mediation (tool/plan 승인 흐름)
4. 크로스 백엔드 config 옵션 정규화

**파일**: `src/claude_code_agent.rs`, `src/gemini_agent.rs`, `src/multi_backend.rs`

### D2. Milestone 3: Session Continuity & Canonical Log Quality

- **근거**: `docs/plans/roadmap.md`
- canonical log 스키마 버전 관리 명시적 정의
- correlation ID 무결성 (prompt → tool → approval → file)
- redaction 정책 강화

### D3. Milestone 4: Client Readiness & Release Operations

- **근거**: `docs/plans/roadmap.md`
- quick-start 경로 일관성 (binary/npm)
- 릴리스 프로세스 재현성 (이미 대부분 완료)
- ACP 레지스트리 자동 반영 확인 (C1 이후)

---

## Phase E: 선택 / 모니터링

| # | 작업 | 근거 | 우선도 |
|---|------|------|--------|
| E1 | 디자인 시스템 통합 | `qa_checklist.md:55-74` — 9개 항목 전부 미체크 | Optional |
| E2 | ACP 호환성 체크 재검증 | `qa_checklist.md:43-50` — 5개 항목, 스모크 통과하나 스키마 드리프트 시 재확인 | 모니터링 |
| E3 | unstable ACP 메서드 안정화 추적 | `session/fork`, `session/resume` 등 5개 | 모니터링 |

---

## 의존 관계 다이어그램

```
즉시 실행 가능 ─── A1 수동 검증 (Zed + 로컬)
               ├── A2 vendor/codex-rs 갱신 검토
               └── A3 문서 커밋

외부 대기 ──────── B1 PR #93 머지 (메인테이너)
               └── B2 피드백 대응

B1 완료 후 ─────── C1 ACP 크레이트 업그레이드 → v0.9.25 릴리스
               │   (레지스트리 hourly cron 자동 반영)
               └── C2 릴리스 문서 갱신

C1 완료 후 ─────── D1 비-codex 백엔드 fidelity
               └── D2 세션/로그 품질
               └── D3 릴리스 ops 완료
```

---

## 정량 요약

| 카테고리 | 항목 수 | 상태 |
|----------|---------|------|
| 즉시 실행 가능 | 3 (수동 검증 7개 포함) | 바로 착수 |
| 외부 대기 | 2 | 메인테이너 의존 |
| 머지 직후 | 2 | B1 완료 트리거 |
| 기능 확장 | 3개 마일스톤 | 순서 의존적 |
| 선택/모니터링 | 3 | 필요 시 |
| **합계** | **~13개 작업 단위** | |
