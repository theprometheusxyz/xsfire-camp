# Zed Marketplace Extension PR Template

Use this template when submitting the extension update to `zed-industries/extensions`.

```markdown
## Extension Submission: xsfire-camp

### Summary
- registers xsfire-camp as a Zed extension
- uses `xsfire-camp` binary (v0.9.24) and shares CODEX_HOME with CLI
- adds documentation links for installation, verification, and CODEX_HOME sharing
- includes darwin/linux/windows targets with release asset URLs and `sha256`

### Testing
- `cargo test`
- `xsfire-camp` run with `/review`, `/compact`, `/undo` (manual)
- Verified Plan/ToolCall/RequestPermission logging via `docs/reference/event_handling.md`

### Files added/updated
- `extensions/xsfire-camp/manifest.toml`
- (if updates) `extensions.toml` entry for the extension
```

Fill in any additional data required by the Zed repo's PR checklist (e.g., supported architectures, maintainers). Replace bullet list with real commit references if needed.
