# xsfire-camp Multi-AI Collaboration Rules

When executing complex development tasks, leverage the local multi-AI engine topology:

1. **Engine Selection**:
   - For fast reasoning, large context search, and Google SDK workflows, prefer `gemini`.
   - For focused code edits, refactorings, and ChatGPT tool integrations, prefer `codex`.
   - For deep architectural analysis and Unix CLI-centric debugging, prefer `claude-code`.
2. **Review & Verification**:
   - After completing edits, run `/review` or automated unit tests before concluding.
   - Use `/undo` immediately if an unintended code destruction occurs.
