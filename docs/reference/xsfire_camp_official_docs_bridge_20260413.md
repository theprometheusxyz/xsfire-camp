# xsfire-camp Official Docs Bridge (2026-04-13)

## Goal
- xsfire-camp ACP, external agent, macOS link-open 이슈 판단에 필요한 공식 매뉴얼과 공식 명시의 전역 정본 경로를 이 저장소에서 다시 찾을 수 있게 연결한다.

## Canonical Global SoT
- Global coverage audit:
  - `/Volumes/Extend/.codex/knowledge/research/20260413_xsfire_camp_official_docs_coverage_audit.md`
- Official source snapshot root:
  - `/Volumes/Extend/.codex/knowledge/research/sources/20260413_xsfire_camp_official_docs`
- Snapshot manifest:
  - `/Volumes/Extend/.codex/knowledge/research/sources/20260413_xsfire_camp_official_docs/README.md`

## What This Bridge Covers
- Zed 공식 문서:
  - ACP
  - Agent Panel
  - External Agents
  - Claude Code via ACP 공식 블로그
- Anthropic 공식 문서:
  - Claude Code getting started
  - slash commands
  - MCP
- Google / Gemini CLI 공식 문서:
  - Gemini CLI overview
  - installation
  - commands reference
  - MCP server
  - IDE integration
- Apple 공식 문서:
  - Launch Services 개념/작업/키
  - `paramErr (-50)` 공식 header excerpt

## Operational Rule
- 기능 가능 여부는 먼저 공식 문서 커버리지에서 확인한다.
- 실제 UI 동작 여부는 공식 문서만으로 닫지 않고 fresh ACP session transcript로 다시 확인한다.
- `-50`은 Apple 공식 의미상 `invalid parameter class`까지는 좁힐 수 있지만, 구체 원인은 실제 링크 값 재현으로 닫는다.

## Relationship To Existing Bridge
- 회고/학습 정본 브리지는 `xsfire_camp_global_learning_bridge_20260413.md`가 담당한다.
- 이 문서는 그 후속 작업으로 확보한 `공식 매뉴얼/공식 명시` 정본만 가리킨다.

## Next Use
- ACP 회귀 판단 시 아래 순서를 유지한다:
  - official docs coverage
  - installed binary / live process
  - fresh panel transcript
