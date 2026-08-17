# 로컬 `.codex` 학습 구조 리포트 (2026-04-03)

## Goal
- `/Volumes/Extend/.codex`에 현재 세팅된 학습 구조를 경로, 루프, 자동화, 최신 상태 지표까지 포함해 재현 가능하게 정리한다.

## Rubric (Must/Should)
### Must
- M1. 학습 구조의 System of Record(SoT) 경로와 우선순위를 명시한다.
  - Evidence: `/Volumes/Extend/.codex/GLOBAL_SKILL_PRIORITY_POLICY.md`
- M2. 세션 수집부터 학습 리포트 생성까지의 운영 루프를 설명한다.
  - Evidence: `/Volumes/Extend/.codex/scripts/daily_learning_ops.sh`
- M3. 현재 활성 상태를 수치와 함께 요약한다.
  - Evidence: `/Volumes/Extend/.codex/knowledge/learning/reports/20260402_system_health.md`
  - Evidence: `/Volumes/Extend/.codex/knowledge/learning/reports/20260402_memory_benchmark.md`
  - Evidence: `/Volumes/Extend/.codex/knowledge/learning/reports/20260402_skill_governance_audit.md`

### Should
- S1. 학습 구조의 병목과 운영 리스크를 함께 적는다.
  - Evidence: `/Volumes/Extend/.codex/knowledge/learning/reports/20260402_global_path_policy_gate.md`
- S2. 다음에 확인해야 할 핵심 파일/명령을 남긴다.
  - Evidence: `/Volumes/Extend/.codex/knowledge/learning/README.md`
  - Evidence: `/Volumes/Extend/.codex/knowledge/skill-governance-operating-model.md`

## Glossary
- SoT (System of Record): 실행과 판단의 정본 경로.
- MCP (Model Context Protocol): 도구와 서비스 연결 규약.
- RPD (Recognition-Primed Decision): 숙련자 의사결정 패턴 기반 운영 프레임.
- NDM (Naturalistic Decision Making): 실제 작업 맥락 기반 의사결정 프레임.

## Executive Summary
- 현재 로컬 `.codex`의 학습 구조는 `global-first` 모델이다. 실행 정본은 `/Volumes/Extend/.codex/skills`, `/Volumes/Extend/.codex/tools`, `/Volumes/Extend/.codex/knowledge`이고, 프로젝트 로컬 자원은 보완 계층으로 취급된다.
- 메인 학습 루프는 `/Volumes/Extend/.codex/scripts/daily_learning_ops.sh`가 담당한다. 이 스크립트는 세션 보존 정책, 세션→학습 로그 변환, 일일 패턴 추출, 스킬 적응, 메모리 벤치마크, 시스템 헬스, 자동화 거버넌스를 한 번에 실행한다.
- 2026-04-02 기준 최신 상태는 전반적으로 운영 중이지만 완전 무결 상태는 아니다. 시스템 헬스는 `0.89 / Warning`이며, 핵심 경고는 경로 정책 fail 1건과 세션 저장소 용량 증가, `codex_memory` 프로브 불가다.

## 구조 맵

```text
/Volumes/Extend/.codex
├─ AGENTS.md
├─ GLOBAL_SKILL_PRIORITY_POLICY.md
├─ skills -> /Volumes/Extend/.codex-relocated/skills
├─ tools/
├─ knowledge/
│  ├─ learning/
│  │  ├─ README.md
│  │  ├─ TEMPLATE.md
│  │  ├─ REGISTRY.md
│  │  ├─ logs/
│  │  ├─ reports/
│  │  ├─ skill_feedback/
│  │  ├─ sources/
│  │  └─ manifests/
│  ├─ research/
│  ├─ semantic-system/
│  ├─ skill-governance-registry.md
│  └─ skill-governance-operating-model.md
├─ sessions/
├─ archived_sessions/
├─ automations/
└─ scripts/
```

## 1. 거버넌스 레이어

### 1-1. 전역 우선 경로
- 전역 우선 정책은 `/Volumes/Extend/.codex/GLOBAL_SKILL_PRIORITY_POLICY.md`에 고정되어 있다.
- 정본 경로는 다음 세 가지다.
  - skills: `/Volumes/Extend/.codex/skills`
  - tools: `/Volumes/Extend/.codex/tools`
  - knowledge: `/Volumes/Extend/.codex/knowledge`
- 정책상 우선순위는 `global skill > local skill`이다. 로컬 오버라이드는 명시적 근거가 있을 때만 허용된다.

### 1-2. 스킬 거버넌스
- 스킬 레지스트리 `/Volumes/Extend/.codex/knowledge/skill-governance-registry.md`는 4개의 외부 실존 규약과 12개의 로컬 거버넌스 문서를 연결한다.
- 최신 감사 `/Volumes/Extend/.codex/knowledge/learning/reports/20260402_skill_governance_audit.md` 기준:
  - 전체 인벤토리: `202`
  - `global_active`: `100`
  - `project_active`: `6`
  - `archived`: `85`
  - `reference`: `9`
  - `system`: `2`
  - Rubric 누락: `0`
  - `agents/openai.yaml` 파싱 실패: `0`
  - 정렬 게이트 이슈: `0`
- 런타임 노출 경로는 심링크다.
  - `/Volumes/Extend/.codex/skills -> /Volumes/Extend/.codex-relocated/skills`

## 2. 세션/메모리 레이어

### 2-1. 세션 저장소
- 활성 세션 증거는 `/Volumes/Extend/.codex/sessions`에 쌓인다.
- 보존 세션은 `/Volumes/Extend/.codex/archived_sessions`에 월 단위로 분리된다.
- 현재 보이는 상위 세션 구조는 세 가지다.
  - 연/월 단위 롤아웃 저장: `sessions/2026/02`, `03`, `04`
  - 개별 UUID 세션 디렉터리
  - 대화 요약/기록 디렉터리: `sessions/codex_chats`
- 2026-04-02 시스템 헬스 기준 저장소 상태:
  - `sessions`: `3.8 GB`, 파일 `2503`
  - `archived_sessions`: `1.2 GB`, 파일 `248`

### 2-2. 상태/이력 파일
- 지속성 핵심 파일:
  - `/Volumes/Extend/.codex/history.jsonl`
  - `/Volumes/Extend/.codex/session_index.jsonl`
  - `/Volumes/Extend/.codex/.codex-global-state.json`
  - `/Volumes/Extend/.codex/state_5.sqlite` (심링크)
- 메모리 벤치마크 `/Volumes/Extend/.codex/knowledge/learning/reports/20260402_memory_benchmark.md` 기준:
  - `history.jsonl` 파싱 라인: `169`
  - 고유 세션 수: `25`
  - 저장된 워크스페이스 수: `1`
  - 최근 학습 로그 수(24h): `18`

## 3. 학습 저장소 레이어

### 3-1. Canonical learning base
- 정본 학습 저장소는 `/Volumes/Extend/.codex/knowledge/learning`이다.
- README 규칙 `/Volumes/Extend/.codex/knowledge/learning/README.md`는 다음을 강제한다.
  - 모든 Codex 세션당 최소 1개 엔트리
  - 비밀정보 저장 금지
  - 파일당 1개 엔트리
  - `YYYYMMDD_topic.md` 네이밍
- 템플릿 `/Volumes/Extend/.codex/knowledge/learning/TEMPLATE.md`는 다음 루프를 고정한다.
  - `Context -> Observe -> Hypothesize -> Act -> Review -> Next -> Memory`

### 3-2. 현재 저장 규모
- 조사 시점 기준 집계:
  - `logs/` 하위 프로젝트 디렉터리: `8`
  - `logs/` 파일 수: `49`
  - `reports/` 파일 수: `300`
  - `skill_feedback/` 파일 수: `7`
  - 자동화 정의 파일 수: `5`
- `REGISTRY.md`는 학습 로그와 일부 리포트를 인덱싱한다.
- `knowledge`는 learning 외에도 두 개의 보조 레이어를 가진다.
  - `research/`: 정책 리서치와 외부 자료 동기화 결과
  - `semantic-system/`: 개념 체계, 런타임 어댑터, 레지스트리

## 4. 학습 적응 레이어

### 4-1. 패턴 -> 스킬 적응
- 스킬 적응 정본은 `/Volumes/Extend/.codex/knowledge/learning/skill_feedback`이다.
- 주요 파일:
  - `feedback_and_adapt.jsonl`: append-only 액션 로그
  - `pattern_to_skills.json`: 키워드-스킬 매핑
  - `skill_backlog.md`: 열린 적응 과제
  - `skill_execution_concept_map.{json,md,html}`: 관계 시각화
- 최신 스킬 라이프사이클 리포트 `/Volumes/Extend/.codex/knowledge/learning/reports/20260402_skill_lifecycle.md` 기준:
  - 신호: `tool 22`, `path 4`, `config 4`, `token 4`, `state 3`
  - 제안 액션: `update 11`, `create 0`, `self-improve 1`
  - 우선 갱신 대상: `developer-agent`, `quality-gate`, `work-orchestration`, `knowledge-ops`, `ops-guardian`

### 4-2. 거버넌스 운영 모델
- 운영 모델 `/Volumes/Extend/.codex/knowledge/skill-governance-operating-model.md`은 다음 루프를 사용한다.
  - `Research -> Create/Update -> Manage -> Evaluate`
- 운영 명령은 네 가지로 정리되어 있다.
  - `normalize_skill_contracts.py`
  - `skill_governance_audit.py`
  - `check_skill_agents_alignment.sh --all`
  - `skill-creator quick_validate.py`

## 5. 자동화 레이어

### 5-1. 등록 자동화
- 현재 등록/활성 자동화는 모두 5개다.
- `/Volumes/Extend/.codex/knowledge/learning/reports/20260401_automation_governance.md` 기준 파싱 성공 `5/5`, 점수 `100.0`, 수동 조치 `0`이다.

| Automation ID | 역할 | 작업 디렉터리 |
| --- | --- | --- |
| `collaboration-compression-report` | 최근 24시간 협업 효율 리포트 생성 | `/Volumes/Extend/.codex` |
| `daily-codex-sync` | `codex-custom-scripts` 동기화 | `/Volumes/Extend/.codex/codex-custom-scripts` |
| `daily-learning-ops` | 학습 파이프라인 전체 실행 | `/Volumes/Extend/.codex` |
| `schema-org-global-sync` | schema.org 연구 자료 동기화 | `/Volumes/Extend/Projects/DevWorkspace/content-intelligence-pipeline` |
| `skill-web-sync` | 스킬 거버넌스 + 웹 export 번들 갱신 | `/Volumes/Extend/.codex` |

### 5-2. 메인 루프
- `daily-learning-ops`는 현재 학습 구조의 핵심 오케스트레이터다.
- `/Volumes/Extend/.codex/scripts/daily_learning_ops.sh` 기준 실행 순서는 다음과 같다.
  1. 경로 정책 게이트 실행
  2. 세션 retention 처리
  3. 세션을 학습 로그로 변환
  4. 일일 패턴 리포트 생성
  5. 스킬 적응 리포트와 컨셉맵 생성
  6. 메모리 벤치마크 생성
  7. 시스템 헬스 스냅샷 생성
  8. 자동화 거버넌스 검사

## 6. 현재 건강상태

### 6-1. 최신 스냅샷
- 최신 스냅샷은 `/Volumes/Extend/.codex/knowledge/learning/reports/20260402_system_health.md`다.
- 상태 요약:
  - 전체 점수: `0.89`
  - 전체 상태: `Warning`

### 6-2. 정상 영역
- `policies`: `Healthy`
- `automations`: `Healthy`
- `sessions learning`: `Healthy`

### 6-3. 경고/위험 영역
- `skills`: `Warning`
  - 근거: `status.md` 최신성 `14.48일`
- `resources and tools`: `Critical`
  - 근거: 세션 저장소 `3.8 GB`, 아카이브 `1.2 GB`
  - `pw`, `vision-shot` 자체는 `Healthy`
  - `codex_memory` 감사는 probe 불가 상태
- `path policy`: `FAIL`
  - 매칭 `2건`
  - 대상 파일은 모두 `chatgpt-web/export/validation-20260402/...` 하위 검증 산출물이다.

## 7. 운영 해석

### 결론
- 현재 `.codex` 학습 구조는 "세션 증거 -> 학습 로그 -> 패턴 리포트 -> 스킬 적응 -> 헬스/거버넌스"로 이어지는 폐루프를 이미 갖췄다.

### 근거
- 세션과 학습 로그, 자동화, 스킬 감사가 서로 분리된 폴더가 아니라 같은 `knowledge/learning`과 `scripts`를 중심으로 연결되어 있다.
- 최신 자동화/거버넌스 점수는 높다.
  - automation governance: `100.0`
  - memory benchmark suitability: `100.0/100`
  - skill governance Must: `7/7`
- 따라서 현재의 문제는 구조 부재가 아니라 유지보수성이다.
  - 경로 정책 검증 산출물 정리
  - 세션 저장소 부피 관리
  - `codex_memory` probe 복구

### 추천 확인 순서
1. `/Volumes/Extend/.codex/knowledge/learning/reports/20260402_system_health.md`
2. `/Volumes/Extend/.codex/knowledge/learning/reports/20260402_daily_patterns.md`
3. `/Volumes/Extend/.codex/knowledge/learning/reports/20260402_skill_lifecycle.md`
4. `/Volumes/Extend/.codex/knowledge/learning/reports/20260402_skill_governance_audit.md`
5. `/Volumes/Extend/.codex/scripts/daily_learning_ops.sh`

## Iteration 1 Result
- `.codex` 학습 구조를 전역 SoT, 세션/메모리, learning 저장소, skill feedback, 자동화, 건강상태의 6계층으로 재정리했다.
- 각 계층은 실제 경로와 최신 리포트 수치를 근거로 연결했다.

## Current Score / Remaining Gaps
- Must: `3/3` pass
- Should: `2/2` pass
- Remaining gaps:
  - 구조 설명은 완료됐지만 경로 정책 fail 자체를 수정한 것은 아니다.
  - 세션 저장소 용량과 `codex_memory` probe 실패는 운영 후속 과제로 남아 있다.

## Next Action or Done
- Done.
- 후속 운영 작업이 필요하면 `path-policy remediation`, `session retention threshold 조정`, `codex_memory probe 복구` 순으로 다루는 것이 효율적이다.
