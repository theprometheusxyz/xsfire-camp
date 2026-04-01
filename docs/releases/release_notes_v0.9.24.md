# Release Notes - v0.9.24

## Summary

- Completed ACP standard terminal lifecycle support with client-driven `terminal/*` orchestration for `codex` exec.
- Added ACP unstable `session/fork` and `session/resume` support for `codex`, with wrapped support in `multi` for codex-backed sessions only.
- Hardened release reproducibility by vendoring the `codex-rs` workspace and removing the repo-local `.codex_tmp` patch dependency from release builds.

## Details

- `src/lib.rs`, `src/codex_agent.rs`, and `src/thread.rs` now drive terminal execution through ACP client RPCs:
  - `terminal/create`
  - `terminal/output`
  - `terminal/kill`
  - `terminal/wait_for_exit`
  - `terminal/release`
- Legacy embedded terminal updates remain supported through `_meta.terminal_output`, but plain-text fallback is now limited to cases where no real `terminal_id` is available.
- `src/acp_agent.rs`, `src/backend.rs`, `src/codex_agent.rs`, and `src/multi_backend.rs` now advertise and implement unstable `session/fork` and `session/resume`.
- `src/claude_code_agent.rs` and `src/gemini_agent.rs` keep the ACP contract aligned for cancel/auth/config smoke coverage added during this cycle.
- `scripts/acp_compat_smoke.sh`, `docs/reference/acp_standard_spec.md`, `docs/reference/event_handling.md`, and `docs/quality/qa_checklist.md` were updated to match the shipped ACP behavior.
- `Cargo.toml` now patches `https://github.com/zed-industries/codex` crates to committed `vendor/codex-rs/*` paths so release builds do not depend on a local `.codex_tmp` checkout.

## Verification

- `cargo test --quiet`
- `cargo build --release`
- `scripts/acp_compat_smoke.sh --strict`
- `node npm/testing/test-platform-detection.js`
- `git diff --check`

## Release

- Tag: `v0.9.24`
- GitHub Release: `https://github.com/theprometheusxyz/xsfire-camp/releases/tag/v0.9.24`

## Release Verification Snapshot

- Pre-release gates passed: `cargo test --quiet`, `cargo build --release`, `scripts/acp_compat_smoke.sh --strict`, `node npm/testing/test-platform-detection.js`, and `git diff --check`.
- `release.yml` published `v0.9.24` successfully in run `23708294063`:
  - `https://github.com/theprometheusxyz/xsfire-camp/actions/runs/23708294063`
- GitHub release `Release 0.9.24` was published at `2026-03-29T12:04:45Z`:
  - `https://github.com/theprometheusxyz/xsfire-camp/releases/tag/v0.9.24`
- ACP registry follow-up remains open: PR `agentclientprotocol/registry#93` is updated to `v0.9.24` on head `fed906a`, but its latest `Build Registry` run `23709093662` is still blocked with `action_required`.

## Fit / Context-Fit / Release Feedback

- fit_score:
  - `Must 3/3 pass`
  - `Should 2/2 pass`
- Context-Fit decision:
  - `retain`
  - Reason: the current ACP release line passed strict smoke without protocol-drift evidence, so the near-term need is release evidence hygiene and non-`codex` parity work, not an urgent ACP schema upgrade.
- Evidence anchors:
  - strict smoke report: [acp_compat_smoke_20260329_221344.md](../../logs/smoke/acp_compat_smoke_20260329_221344.md)
  - ACP mapping doc: [acp_standard_spec.md](../reference/acp_standard_spec.md)
  - registry/release state: [github_registry_release_runbook.md](../guides/github_registry_release_runbook.md)
  - iteration fit record: [iteration_fit_v0.9.24_acp_readiness.md](../quality/iteration_fit_v0.9.24_acp_readiness.md)
- Release / Feedback:
  - release impact: `v0.9.24` remains valid as the current ACP release target.
  - remaining risk: `agent-client-protocol` unstable surface and non-`codex` backend fidelity still require follow-up monitoring.
  - next verification command: `scripts/acp_compat_smoke.sh --strict`
