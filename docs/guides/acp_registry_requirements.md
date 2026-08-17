# ACP Registry 등록 요구사항

> 소스: `agentclientprotocol/registry` 리포지토리 분석 (2026-04-01 기준)

---

## 1. 디렉토리 구조

레지스트리 루트에 에이전트 ID 이름의 디렉토리를 생성하고, 두 파일을 포함한다:

```
<agent-id>/
├── agent.json   # 필수 — 에이전트 매니페스트
└── icon.svg     # 필수 — 16×16 모노크롬 아이콘
```

- 에이전트 ID는 **소문자 + 숫자 + 하이픈**만 허용, 반드시 문자로 시작 (`^[a-z][a-z0-9-]*$`)
- 디렉토리 이름과 `agent.json` 내 `id` 필드가 정확히 일치해야 한다
- ID는 레지스트리 전체에서 유일해야 한다

---

## 2. agent.json 스키마

스키마 정의: `agent.schema.json`

### 필수 필드

| 필드 | 타입 | 제약 |
|------|------|------|
| `id` | string | `^[a-z][a-z0-9-]*$`, 디렉토리명과 일치 |
| `name` | string | minLength: 1, 표시 이름 |
| `version` | string | `^[0-9]+\.[0-9]+\.[0-9]+`, 시맨틱 버전 |
| `description` | string | minLength: 1, 간단한 설명 |
| `distribution` | object | 최소 1개 배포 방식 포함 |

### 선택 필드

| 필드 | 타입 | 설명 |
|------|------|------|
| `repository` | string (URI) | 소스코드 리포지토리 URL |
| `website` | string (URI) | 홈페이지/문서 URL |
| `authors` | string[] | 저자 목록 |
| `license` | string | SPDX 식별자 또는 `"proprietary"` |
| `icon` | — | **제출 시 포함 금지** — 빌드가 자동 설정 |

---

## 3. Distribution 타입

최소 하나의 배포 방식을 선택해야 한다. 혼합 가능.

### 3.1 Binary (`distribution.binary`)

플랫폼 키 (허용 목록):
- `darwin-aarch64`, `darwin-x86_64`
- `linux-aarch64`, `linux-x86_64`
- `windows-aarch64`, `windows-x86_64`

각 플랫폼 필수/선택 필드:

| 필드 | 필수 | 설명 |
|------|------|------|
| `archive` | O | 다운로드 URL (URI) |
| `cmd` | O | 추출 후 실행 커맨드 |
| `args` | X | CLI 인자 (string[]) |
| `env` | X | 환경 변수 (object, string values) |

### 3.2 npx (`distribution.npx`)

| 필드 | 필수 | 설명 |
|------|------|------|
| `package` | O | npm 패키지 (e.g. `@scope/pkg@1.0.0`) |
| `args` | X | CLI 인자 |
| `env` | X | 환경 변수 |

### 3.3 uvx (`distribution.uvx`)

| 필드 | 필수 | 설명 |
|------|------|------|
| `package` | O | PyPI 패키지 |
| `args` | X | CLI 인자 |
| `env` | X | 환경 변수 |

---

## 4. 아이콘 (icon.svg) 요구사항

| 규칙 | 상세 |
|------|------|
| 포맷 | 유효한 SVG/XML, 루트 요소 `<svg>` |
| 크기 | 정확히 **16×16 px** (width/height 또는 viewBox) |
| 비율 | 정사각형 (width == height) |
| 색상 | **모노크롬** — `currentColor`만 사용 |
| fill/stroke | `currentColor`, `none`, `inherit`만 허용 |
| currentColor | 최소 1회 사용 필수 |
| 하드코딩 색상 | 속성, 인라인 스타일, `<style>` 블록 내 모두 금지 |
| HTML 주석 | `<!-- -->` 금지 (MDX 임베딩 깨짐) |

---

## 5. 버전 검증 규칙

CI가 자동으로 배포 URL/패키지에서 버전을 추출하여 `agent.json`의 `version`과 비교한다.

| 검증 | 상세 |
|------|------|
| 바이너리 URL | URL 경로에서 버전 추출 (e.g. `/download/v0.9.24/`) → `version` 필드와 일치 |
| npm 패키지 | `@scope/pkg@1.0.0`에서 `1.0.0` 추출 → 일치 확인 |
| PyPI 패키지 | 동일 방식 |
| `/latest/` 금지 | URL에 `/latest/` 포함 불가 |
| `@latest` 금지 | npm/pypi 패키지에 `@latest` 사용 불가 |

---

## 6. URL 접근성 검증

CI에서 모든 배포 URL의 접근 가능 여부를 확인한다:

- 바이너리: `archive` URL에 HTTP HEAD/GET → **200 OK** 필수
- npm: `registry.npmjs.org`에서 패키지 존재 확인
- PyPI: `pypi.org`에서 패키지 존재 확인
- 아카이브 포맷 허용: `.zip`, `.tar.gz`, `.tgz`, `.tar.bz2`, `.tbz2`, raw 바이너리
- 아카이브 포맷 거부: `.dmg`, `.pkg`, `.deb`, `.rpm`, `.msi`, `.appimage`

---

## 7. 인증 검증 (Authentication) — 핵심 게이트

CI에서 에이전트 바이너리를 다운로드/실행하여 ACP 핸드셰이크를 수행한다. **이 검증을 통과하지 못하면 머지 불가.**

### 7.1 검증 프로세스

1. 바이너리를 다운로드하고 추출
2. `cmd` + `args`로 프로세스 시작
3. stdin으로 JSON-RPC `initialize` 요청 전송
4. stdout에서 응답 수신 (120초 타임아웃)
5. 응답에 `authMethods` 포함 여부 확인

### 7.2 전송되는 initialize 요청

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {
    "protocolVersion": 1,
    "clientInfo": {
      "name": "ACP Registry Validator",
      "version": "1.0.0"
    },
    "clientCapabilities": {
      "terminal": true,
      "fs": { "readTextFile": true, "writeTextFile": true },
      "_meta": {
        "terminal_output": true,
        "terminal-auth": true
      }
    }
  }
}
```

### 7.3 요구되는 응답 형식

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "serverInfo": { "name": "...", "version": "..." },
    "authMethods": [
      {
        "id": "...",
        "name": "...",
        "description": "...",
        "type": "agent"
      }
    ]
  }
}
```

### 7.4 인증 타입

| 타입 | 설명 |
|------|------|
| `agent` | 에이전트가 OAuth 플로우 처리 (브라우저 오픈, 로컬 HTTP 콜백) |
| `terminal` | 터미널 기반 대화형 인증 (TUI), 추가 `args`/`env` 필요 |

타입 감지 우선순위:
1. `authMethod.type` 필드 직접 지정
2. `_meta` 키: `"terminal-auth"` → `"terminal"`, `"agent-auth"` → `"agent"`
3. 미지정 시 기본값: `"agent"`

### 7.5 통신 프로토콜

- **Newline-delimited JSON-RPC** over stdin/stdout
- stdout에 JSON이 아닌 출력 **금지** — 진단 출력은 반드시 **stderr**로
- 타임아웃: 120초

---

## 8. CI 파이프라인 (Build Registry)

### 워크플로 트리거

`*/agent.json`, `*/icon.svg`, `.github/workflows/**`, `*.schema.json` 변경 시 실행

### 실행 단계

| 단계 | 내용 |
|------|------|
| 1. Lint & Test | ruff check/format, pytest |
| 2. Build & Validate | `build_registry.py` (스키마 + URL + 아이콘 검증) |
| 3. Auth Verify | `verify_agents.py --auth-check` (15분 타임아웃) |
| 4. Upload to S3 | main 머지 시에만 |
| 5. Publish Release | main 머지 시에만 |

### 포크 PR 제한

포크에서 올린 PR의 워크플로는 `action_required` 상태로 시작되며, **레지스트리 메인테이너가 직접 워크플로를 승인/재실행**해야 한다.

---

## 9. 로컬 검증 커맨드

```bash
# 레지스트리 리포 클론 후 실행

# 전체 검증 (스키마 + URL + 아이콘)
uv run --with jsonschema .github/workflows/build_registry.py

# URL 검증 스킵 (릴리스 전 테스트)
SKIP_URL_VALIDATION=1 uv run --with jsonschema .github/workflows/build_registry.py

# 특정 에이전트 인증 검증
python3 .github/workflows/verify_agents.py --auth-check --agent xsfire-camp

# 드라이런 (dist/ 미생성)
uv run --with jsonschema .github/workflows/build_registry.py --dry-run
```

---

## 10. 기존 등록 에이전트 패턴 참고

| 에이전트 | 배포 방식 | 라이선스 | 플랫폼 | 비고 |
|----------|-----------|----------|--------|------|
| claude-acp | npx만 | proprietary | — | 바이너리 없음 |
| codex-acp | binary + npx | Apache-2.0 | 6개 전체 | 듀얼 배포 |
| goose | binary만 | Apache-2.0 | 5개 (win-arm 제외) | `args: ["acp"]` 사용 |
| cursor | binary만 | proprietary | 6개 전체 | 비-GitHub URL, calver |

---

## 11. xsfire-camp 현재 상태 대비 체크리스트

| 항목 | 상태 | 비고 |
|------|------|------|
| `agent.json` 스키마 준수 | ✅ | 6개 플랫폼 binary, 모든 필수 필드 |
| `icon.svg` 16×16 모노크롬 | ✅ | PR에 포함 |
| 버전 URL 일치 | ✅ | `v0.9.24` 일관 |
| 아카이브 URL 접근 가능 | ✅ | GitHub Releases 배포 완료 |
| ACP 인증 핸드셰이크 | ⚠️ 미검증 | CI가 아직 실행되지 않아 확인 불가 |
| 메인테이너 워크플로 승인 | ❌ 대기 | 포크 PR 특성상 외부 대기 |

**핵심 리스크**: 인증 검증(§7)은 CI가 실행되어야 확인 가능하며, 로컬에서 `verify_agents.py --auth-check --agent xsfire-camp`로 사전 검증 권장.
