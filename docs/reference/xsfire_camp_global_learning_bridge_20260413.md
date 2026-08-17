# xsfire-camp Global Learning Bridge (2026-04-13)

## Goal
- xsfire-camp의 최근 코드/세션 회고와 추가 리서치를 전역 지식 정본에 저장하고, 이 저장소에서는 그 정본을 다시 찾을 수 있는 링크 계층만 유지한다.

## Canonical Global SoT
- Global learning log:
  - `/Volumes/Extend/.codex/knowledge/learning/logs/xsfire-camp/20260413_retrospective_learning_project_bridge.md`
- Global research note:
  - `/Volumes/Extend/.codex/knowledge/research/20260413_xsfire_camp_retrospective_learning_bridge.md`

## What This Bridge Covers
- 최근 xsfire-camp 세션에서 반복된 ACP UX 불신 신호:
  - 잘못된 로컬 파일 링크
  - macOS `-50` 실행 오류
  - 완료 후에도 계속 처리 중으로 보이는 상태
  - Plan UI 업데이트 불확실성
- 현재 코드가 추가한 대응 방향:
  - backend별 `WorkOrchestrationProfile` 명시
  - Claude/Gemini help/status에서 순차 실행 제약 노출
  - raw executable path를 비클릭 텍스트로 유지하는 링크 정규화
- 남아 있는 검증 갭:
  - fresh ACP client session에서 `/setup -> /status -> /monitor -> /vector`를 다시 실행한 실제 transcript

## Local Supporting Evidence
- `docs/quality/iteration_fit_v0.9.24_acp_readiness.md`
- `docs/quality/qa_checklist.md`
- `src/backend.rs`
- `src/claude_code_agent.rs`
- `src/gemini_agent.rs`
- `src/link_paths.rs`
- `src/thread.rs`

## Usage Rule
- 분석/학습 정본은 전역 `.codex/knowledge`에만 둔다.
- 이 저장소에는 정본 요약을 복제하지 않고, 경로와 적용 지점만 유지한다.
- 동일 주제 후속 회고가 생기면 새 전역 로그/리서치를 추가하고 이 브리지 문서 또는 `docs/README.md` 인덱스만 갱신한다.

## Next Action
- live ACP client를 재시작한 뒤 fresh transcript 1건을 확보하면, 그 결과를 `docs/quality/iteration_fit_v0.9.24_acp_readiness.md`에 붙여 manual verification gap을 닫는다.
