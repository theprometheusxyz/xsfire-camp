# 🔥 xsfire-camp — 내 컴퓨터의 모든 AI를 IDE와 연결하는 통합 ACP 브리지

> **"내 맥북에 이미 설치되어 있는 Gemini, Codex, Claude를 Zed 같은 IDE에서 바로 불러와 사용할 수는 없을까?"**
>
> **`xsfire-camp`는 복잡한 인증이나 복잡한 환경 설정 없이, 내 컴퓨터에 이미 세팅된 AI 도구들을 알아서 탐지하고 IDE와 하나로 묶어주는 초경량 ACP(Agent Client Protocol) 통합 엔진입니다.**

---

## 🌟 왜 xsfire-camp 인가요? (핵심 장점 3가지)

```
                       ┌─────────────────────────┐
                       │   Zed IDE / ACP Client  │
                       └────────────┬────────────┘
                                    │ (ACP Stdio)
                       ┌────────────▼────────────┐
                       │       xsfire-camp       │
                       │ (Local-First Discovery) │
                       └─────┬──────┬──────┬─────┘
                             │      │      │
           ┌─────────────────┘      │      └─────────────────┐
           ▼                        ▼                        ▼
┌──────────────────┐     ┌──────────────────┐     ┌──────────────────┐
│    Gemini CLI    │     │    Codex CLI     │     │ Claude Code CLI  │
│ (/opt/homebrew)  │     │  (ChatGPT Engine)│     │  (Anthropic CLI) │
└──────────────────┘     └──────────────────┘     └──────────────────┘
```

### 1. ⚡ 로컬 AI 무인증 자동 연결 (Zero-Setup Auth-Less Binding)
별도의 API 키 복사-붙여넣기나 팝업 창 승인 없이, **내 컴퓨터에 이미 세팅해 둔 AI 도구들(`/opt/homebrew/bin/gemini`, `codex`, `claude`)을 알아서 인식하여 즉시 연결**합니다.

### 2. 🔀 대화창에서 명령 한 줄로 AI 엔진 자유 교체
Zed나 에디터 대화창에서 `/backend gemini` 또는 `/backend codex`를 입력하는 것만으로, 맥락을 유지한 채 내가 원하는 AI 엔진으로 즉시 스위칭하여 답변을 비교하고 작업을 이어갈 수 있습니다.

### 3. 🛡️ 안전한 작업 기록과 언제든 가능한 타임트래블
모든 AI의 코드 수정 내역과 명령어 실행 로그를 내 컴퓨터(`~/.acp`)에 안전하게 저장합니다. 실수가 발생하면 언제든 `/undo`로 되돌리거나 `/compact`로 긴 대화를 요약해 문맥을 정리할 수 있습니다.

---

## 🚀 60초 초간단 시작하기 (Quick Start)

### 1단계: 설치하기 (원하는 방식을 고르세요)

#### 💡 [방법 A] 가장 추천: 명령어 한 줄로 자동 설치
터미널을 열고 아래 명령어를 붙여넣으면 즉시 설치됩니다.

```bash
curl -fsSL https://raw.githubusercontent.com/theprometheusxyz/xsfire-camp/main/scripts/build_and_install.sh | bash
```

#### 📦 [방법 B] NPM으로 설치하기
Node.js가 설치되어 있다면 NPM으로 편리하게 글로벌 설치할 수 있습니다.

```bash
npm install -g @theprometheusxyz/xsfire-camp@latest
```

---

### 2단계: Zed IDE에 등록하기

1. Zed IDE를 실행하고 **`Cmd + ,`** 키를 눌러 설정(`settings.json`)을 엽니다.
2. 아래 내용을 복사해서 `"agent_servers"` 항목에 추가하세요:

```json
{
  "agent_servers": {
    "xsfire-camp": {
      "type": "custom",
      "command": "xsfire-camp",
      "args": ["--backend=multi"]
    }
  }
}
```

---

### 3단계: 바로 사용해보기!

1. Zed 우측의 AI 패널을 열고 에이전트 목록에서 **`xsfire-camp`**를 선택합니다.
2. 대화창에 인사나 질문을 입력해 보세요.
3. 다른 AI 엔진으로 바꾸고 싶다면 대화창에 아래 슬래시 커맨드를 입력하세요:
   - `/backend gemini` ➔ **Gemini AI로 전환**
   - `/backend codex` ➔ **Codex / ChatGPT 엔진으로 전환**
   - `/backend claude-code` ➔ **Claude Code 엔진으로 전환**

---

## 🎮 자주 쓰는 유용한 슬래시 커맨드 모음

대화창에서 아래 명령어들을 입력해 AI 작업을 손쉽게 제어하세요:

| 카테고리 | 슬래시 커맨드 | 어떤 기능인가요? |
| :--- | :--- | :--- |
| **엔진 전환** | `/backend <engine>` | `gemini`, `codex`, `claude-code` 중 사용할 AI를 실시간 교체합니다. |
| **작업 검토** | `/review` | AI가 새로 작성하거나 수정한 코드 변경사항 전체를 정밀 리뷰합니다. |
| **브랜치 리뷰** | `/review-branch` | 현재 Git 브랜치와 메인 브랜치 간의 변경점을 비교 검토합니다. |
| **대화 정리** | `/compact` | 길어진 대화 내역을 간결하게 요약하여 메모리와 응답 속도를 최적화합니다. |
| **작업 취소** | `/undo` | AI가 실행한 이전 작업을 안전하게 취소하고 이전 상태로 복원합니다. |
| **상태 확인** | `/status` | 현재 연결된 AI 백엔드와 세션 상태, 처리 지표를 점검합니다. |
| **세션 관리** | `/sessions`, `/load` | 저장된 이전 대화 목록을 확인하고 원하는 세션을 다시 불러옵니다. |

---

## 🛠️ 자주 묻는 질문 (FAQ & Troubleshooting)

<details>
<summary><b>Q1. 로컬에 Gemini CLI가 설치되어 있는지 어떻게 확인하나요?</b></summary>
<br>
터미널에서 <code>gemini --version</code>을 실행해 보세요. 만약 설치되어 있지 않다면 Homebrew로 쉽게 설치할 수 있습니다:
<pre><code>brew install gemini-cli</code></pre>
</details>

<details>
<summary><b>Q2. API 키를 따로 입력하지 않아도 되나요?</b></summary>
<br>
네! 내 터미널에서 이미 <code>gemini</code>, <code>codex</code>, <code>claude</code> 명령어를 사용할 수 있는 상태라면, <code>xsfire-camp</code>가 로컬 자격증명을 자동으로 사용하므로 추가 인증 입력이 필요하지 않습니다.
</details>

<details>
<summary><b>Q3. 지원되는 OS 환경은 무엇인가요?</b></summary>
<br>
macOS (Apple Silicon 및 Intel), Linux (x86_64 및 ARM64), Windows (x64 및 ARM64) 등 거의 모든 6대 주요 플랫폼을 완벽하게 지원합니다.
</details>

---

## 📜 라이선스 및 기여

`xsfire-camp`는 오픈소스 생태계를 지지합니다. 버그 제보나 기능 제안은 언제든지 [GitHub Issues](https://github.com/theprometheusxyz/xsfire-camp/issues)를 통해 남겨주세요!
