---
name: xsfire-camp
description: >-
  Bridge and orchestrate local AI tools (Gemini CLI, Codex/ChatGPT, Claude Code) using xsfire-camp.
  Use when delegating tasks, switching backends, reviewing code changes, or rolling back sessions.
---

# xsfire-camp Orchestration Skill

`xsfire-camp` connects local AI CLIs (`gemini`, `codex`, `claude`) to your environment over a local-first ACP/MCP bridge.

## Key Capabilities

1. **Multi-Backend AI Routing**:
   - `/backend gemini`: Route prompts to local Gemini CLI.
   - `/backend codex`: Route prompts to local Codex / ChatGPT engine.
   - `/backend claude-code`: Route prompts to Anthropic Claude Code CLI.
2. **Review & Diffing**:
   - `/review`: Full review of modified files.
   - `/review-branch`: Diff and review against the main branch.
3. **Session Hygiene & Undo**:
   - `/undo`: Safely revert previous modifications.
   - `/compact`: Summarize context history for performance and token hygiene.
   - `/status`: Check connected engines and active session metrics.

## CLI Usage

```bash
# Run multi-backend bridge
xsfire-camp --backend=multi

# Run specific single-engine backend
xsfire-camp --backend=gemini
xsfire-camp --backend=codex
xsfire-camp --backend=claude-code
```
