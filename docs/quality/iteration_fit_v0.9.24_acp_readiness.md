# ACP Iteration Fit Record - v0.9.24

## Header
- Task: ACP release-readiness evidence closure for `v0.9.24`
- Date (UTC): 2026-03-29T13:20:56Z
- Owner: Codex
- Branch / commit: `main` / `6aabc683f4f72f8b2a89359ee8bace1b57bf9f37`
- Scope: strict ACP smoke evidence, release evidence hygiene, and ACP registry blocker capture
- Companion SoT: `docs/quality/system_requirements_done_criteria.md`

## Goal
- Close the `v0.9.24` ACP release-readiness evidence gaps by recording strict smoke, release state, and registry blocker status in concrete repo artifacts.

## Research Snapshot
- Files read:
  - `docs/quality/qa_checklist.md`
  - `docs/quality/verification_guidance.md`
  - `docs/quality/iteration_fit_template.md`
  - `docs/releases/release_notes_v0.9.24.md`
  - `docs/guides/github_registry_release_runbook.md`
  - `docs/reference/acp_standard_spec.md`
  - `docs/README.md`
- Commands run:
  - `scripts/acp_compat_smoke.sh --strict` -> generated `logs/smoke/acp_compat_smoke_20260329_221344.md` with `Overall: pass`
  - `gh pr view 93 --repo agentclientprotocol/registry --json headRefOid,url,body` -> confirmed PR `#93`, head `fed906a22c6d667d33006dbb0af5c008c5e0274e`, and `v0.9.24` release URL in the PR body
  - `gh pr checks 93 --repo agentclientprotocol/registry` -> `no checks reported on the 'add-xsfire-camp-agent' branch`
  - `git diff --check -- docs/quality/cross_environment_execution_protocol.md docs/quality/iteration_fit_template.md docs/quality/verification_guidance.md docs/README.md docs/releases/release_notes_v0.9.24.md` -> no diff-format errors
  - `git status --short --branch` -> existing Rust and release-metadata edits remained intact; no user changes were reverted
- External sources:
  - GitHub PR `agentclientprotocol/registry#93` state snapshot, retrieved 2026-03-29
  - GitHub comment `issuecomment-4150073545`, retrieved 2026-03-29

## Rubric
### Must
- [x] The latest strict ACP smoke passes and is archived for traceability.
  - Evidence:
    - `logs/smoke/acp_compat_smoke_20260329_221344.md`
- [x] Release evidence records the current `v0.9.24` published state and links the release workflow.
  - Evidence:
    - `docs/releases/release_notes_v0.9.24.md`
    - `docs/guides/github_registry_release_runbook.md`
- [x] ACP registry blocker state is captured with the current PR head, blocked run, and maintainer-facing English comment evidence.
  - Evidence:
    - `docs/guides/github_registry_release_runbook.md`
    - `https://github.com/agentclientprotocol/registry/pull/93`
    - `https://github.com/agentclientprotocol/registry/pull/93#issuecomment-4150073545`
- [x] The execution protocol and iteration-fit workflow are linked from repo guidance, not left as orphan docs.
  - Evidence:
    - `docs/quality/verification_guidance.md`
    - `docs/README.md`

### Should
- [x] The release checklist points to a concrete fit record rather than the template file itself.
  - Evidence:
    - `docs/quality/qa_checklist.md`
    - `docs/quality/iteration_fit_v0.9.24_acp_readiness.md`
- [x] Remaining gaps are isolated as manual verification or upstream maintainer blockers only.
  - Evidence:
    - `docs/quality/qa_checklist.md`
    - `docs/guides/github_registry_release_runbook.md`

## Sequence Plan (R->P->M->W->A)
- `R1`: re-read release QA checklist and release evidence docs to find unchecked items that already have evidence
- `P1`: decide the smallest durable artifact set
  - depends_on: `R1`
  - done_check: one concrete fit record file plus checklist wording aligned to it
- `M1`: add the release-specific fit record and update the checklist references
  - depends_on: `P1`
  - done_check: the checklist points to an evidence file, not only to the template
- `W1`: verify formatting and confirm no unrelated user work was disturbed
  - depends_on: `M1`
  - done_check: `git diff --check` passes and `git status --short --branch` still shows the pre-existing Rust edits untouched
- `A1`: record remaining blockers and stop at the repo-controlled boundary
  - depends_on: `W1`
  - done_check: only manual verification items and upstream ACP registry approval remain open

## Iteration Log

### Iteration 1
- Conclusion: the initial gap was not ACP protocol breakage; it was missing execution-protocol artifacts and missing links from the verification docs.
- Evidence:
  - fact: `docs/quality/verification_guidance.md` referenced `docs/quality/cross_environment_execution_protocol.md` and `docs/quality/iteration_fit_template.md` before those files existed.
  - interpretation: the repo had release/ACP evidence, but no reusable completion framework to close the loop.
  - assumption: adding the protocol docs would let later release evidence use a stable structure.
- Score change: Must `0/4 -> 1/4`
- Next action: add the missing protocol docs and link them from the docs index and verification guidance.

### Iteration 2
- Conclusion: the protocol and template are now real artifacts, and the latest release notes already contain the fit/context-fit/release-feedback section.
- Evidence:
  - fact: `docs/quality/cross_environment_execution_protocol.md` and `docs/quality/iteration_fit_template.md` now exist and are indexed.
  - interpretation: the repo can now express completion as a repeatable process instead of an ad hoc release note paragraph.
  - assumption: a release-specific record file is still needed because the template itself is not execution evidence.
- Score change: Must `1/4 -> 3/4`
- Next action: create a concrete `v0.9.24` fit record and align the release checklist to it.

### Iteration 3
- Conclusion: `v0.9.24` ACP readiness is documented as a concrete record with strict smoke success and the ACP registry blocker isolated as external.
- Evidence:
  - fact: `logs/smoke/acp_compat_smoke_20260329_221344.md` reports `Overall: pass`.
  - fact: ACP registry PR `#93` is on head `fed906a` and its latest fork-workflow run remains `action_required` with no attached check runs.
  - fact: comment `issuecomment-4150073545` is English-only and includes the blocked run URL.
  - interpretation: current repo-controlled release evidence is complete; the only remaining release blocker is upstream maintainer workflow approval.
  - assumption: manual in-client verification remains a release gate for future operator sign-off, but it is outside this document-only evidence pass.
- Score change: Must `3/4 -> 4/4`, Should `0/2 -> 2/2`
- Next action: keep manual verification and upstream registry approval as explicit remaining operational tasks, not hidden checklist debt.

### Iteration 4
- Conclusion: partial manual ACP verification is now evidenced from a real client session, but it does not yet satisfy the `/setup`-driven completion path.
- Evidence:
  - fact: a user-provided ACP session transcript from `2026-04-03` in `docs/quality` shows successful responses for `/status`, `/monitor`, `/vector`, `/model`, and `/approvals`.
  - fact: `/monitor` output includes `Task monitoring: orchestration=parallel, monitor=auto, vector_checks=on, preempt_on_new_prompt=on`, which satisfies the task-snapshot presence check.
  - fact: the same transcript explicitly says `Plan: no plan updates received yet`, so the `/setup` verification step did not run and the setup-plan completion criterion remains open.
  - interpretation: the adapter command surface is alive in a real session, but the higher-value setup/plan progression path still needs an explicit run.
  - assumption: the transcript was captured against the current `xsfire-camp` workspace and can be treated as release evidence for the listed commands only.
- Score change: Must `4/4 -> 4/4`, Should `2/2 -> 2/2`
- Next action: run `/setup` before `/status -> /monitor -> /vector` in the next manual verification pass so the plan-completion path can be closed.

### Iteration 5
- Conclusion: automated preflight for setup/monitor verification is now reproducible and passing, with a timestamped operator checklist generated for the remaining in-client checks.
- Evidence:
  - fact: `scripts/manual_verification_setup_monitor.sh` completed successfully and generated a report at `logs/manual_verification/setup_monitor_20260412_010614.md`.
  - fact: the run included `cargo fmt --check`, `cargo test` (`107 passed`), and `node npm/testing/test-platform-detection.js` (`All platform detection tests passed`).
  - interpretation: repo-controlled verification gates are green for the current setup/monitor implementation line.
  - assumption: checklist items that require ACP client UI interaction (Plan panel/agent panel/click-through behavior) still require manual execution in Zed or equivalent ACP client.
- Score change: Must `4/4 -> 4/4`, Should `2/2 -> 2/2`
- Next action: execute checklist steps 1-10 from the generated manual report in a live ACP client session and attach resulting artifacts.

### Iteration 6
- Conclusion: a live Zed session for `xsfire-camp` is indirectly evidenced and the `AgentPanel` state is confirmed, but command-level `/setup` and plan-transition artifacts are still not available.
- Evidence:
  - fact: `sqlite3 "$HOME/Library/Application Support/Zed/db/0-stable/db.sqlite" "select workspace_id, paths, timestamp, right_dock_active_panel, session_id from workspaces where workspace_id=25;"` returned workspace `25` for `/Volumes/Extend/Projects/DevWorkspace/xsfire-camp` with `right_dock_active_panel=AgentPanel` at `2026-04-11 17:24:50`.
  - fact: `strings "$HOME/Library/Application Support/Zed/db/0-stable/db.sqlite-wal" | rg 'agent_panel25|xsfire-camp'` returned `agent_panel25` state with `selected_agent.custom.name=xsfire-camp` and `last_active_thread.session_id=019d7d4a-54e8-75e1-8933-bc506e070f0e`.
  - fact: no fresh `logs/codex_chats/*`, `sessions/*/canonical.jsonl`, or ACP transcript file was located for the same verification window.
  - interpretation: the user did open the project in Zed with the custom agent panel active, which closes the narrow UI-presence check.
  - assumption: without a transcript or screenshot, slash-command execution order and plan-step completion cannot be promoted from inferred to verified evidence.
- Score change: Must `4/4 -> 4/4`, Should `2/2 -> 2/2`
- Next action: keep `/setup -> /status -> /monitor -> /vector` plan progression as the only remaining manual evidence gap, or attach one screenshot/transcript artifact if the session needs to be fully closed.

### Iteration 7
- Conclusion: ACP now keeps raw executable artifact paths non-clickable in outgoing agent text, so Zed should no longer try to launch `target/release/<binary>` paths as macOS apps.
- Evidence:
  - fact: `src/link_paths.rs` now detects local raw executable artifacts by binary magic (`Mach-O`, `ELF`, `MZ`) and rewrites markdown links for those targets as code-text references instead of clickable local file links.
  - fact: `src/thread.rs` includes an ACP session test `test_send_agent_text_keeps_raw_executable_paths_non_clickable`.
  - fact: `./target/debug/deps/xsfire_camp-108d14335f90d408 --test-threads=1` passed `111` tests, including:
    - `link_paths::tests::keeps_raw_executable_paths_non_clickable`
    - `link_paths::tests::keeps_existing_file_uris_non_clickable_for_raw_executables`
    - `thread::tests::test_send_agent_text_keeps_raw_executable_paths_non_clickable`
  - fact: `node npm/testing/test-platform-detection.js` still passed after the change.
  - interpretation: the `-50` failure mode is now blocked at the ACP rendering layer instead of relying on Zed/macOS to reject the click target.
  - assumption: final UI confirmation in Zed is still recommended because the repo cannot assert client-side rendering policy without a fresh screenshot/transcript.
- Score change: Must `4/4 -> 4/4`, Should `2/2 -> 2/2`
- Next action: rerun the Zed click-path scenario once and confirm raw executables show as non-clickable code text while source/doc links still open through `file:///...`.

### Iteration 8
- Conclusion: the fixed binary is now installed at the exact Zed runtime path, so new ACP replies in Zed will use the non-clickable raw-executable rendering; previously rendered replies in an existing thread remain unchanged.
- Evidence:
  - fact: `./scripts/build_and_install.sh` completed successfully and printed `Installed: /Users/g/.local/bin/xsfire-camp`.
  - fact: `rg -n 'xsfire-camp|CODEX_HOME' /Users/g/.config/zed/settings.json` shows Zed is configured with `command: /Users/g/.local/bin/xsfire-camp` and `CODEX_HOME=/Users/g/.codex`.
  - fact: `cmp -s /Users/g/.local/bin/xsfire-camp target/release/xsfire-camp && echo MATCH` returned `MATCH`, and both files share the same size and timestamp (`49996688 bytes`, `Apr 12 03:22:15 2026`).
  - fact: `cargo test -- --test-threads=1 > /tmp/xsfire_camp_test.log 2>&1 && rg -n 'keeps_raw_executable_paths_non_clickable|keeps_existing_file_uris_non_clickable_for_raw_executables|test_send_agent_text_keeps_raw_executable_paths_non_clickable|test result:' /tmp/xsfire_camp_test.log` reported the three path-rendering tests as `ok` and `111 passed; 0 failed`.
  - interpretation: the fix is not only present in the repo but also deployed to the binary Zed launches for the `xsfire-camp` custom agent.
  - assumption: message bodies already emitted in an older Zed thread are not retroactively rewritten, because link normalization is applied when outgoing ACP agent text is sent.
- Score change: Must `4/4 -> 4/4`, Should `2/2 -> 2/2`
- Next action: trigger one fresh ACP reply in Zed that includes both a source/doc file path and a raw executable artifact path, then verify only the executable path is rendered as code text.

### Iteration 9
- Conclusion: the repeated `-50` report in Zed is consistent with a stale `xsfire-camp` process that started before the new binary was installed, so the client must respawn the agent process before UI verification is meaningful.
- Evidence:
  - fact: `ps -axo pid,lstart,command | rg '[x]sfire-camp|zed'` showed multiple live `/Users/g/.local/bin/xsfire-camp -c model_auto_compact_token_limit=90000` processes with start times `Sat Apr 11 19:42:06 2026`, `Fri Apr 10 22:15:25 2026`, `Sat Apr 11 00:23:49 2026`, and `Sun Apr 12 01:58:06 2026`.
  - fact: the installed binary at `/Users/g/.local/bin/xsfire-camp` was updated later at `Apr 12 03:22:15 2026`.
  - interpretation: any Zed ACP session still bound to one of those older processes can continue emitting the pre-fix clickable raw-binary links even though the file on disk is updated.
  - assumption: restarting the ACP session or restarting Zed is sufficient for the client to spawn a fresh `xsfire-camp` process from the updated command path.
- Score change: Must `4/4 -> 4/4`, Should `2/2 -> 2/2`
- Next action: restart the active Zed ACP session or restart Zed, then request a fresh reply that includes both a source/doc path and a raw executable path.

### Iteration 10
- Conclusion: core ACP runtime acceptance is now closed by deterministic repo-controlled evidence; the remaining gap is target ACP client rendering/open behavior only.
- Evidence:
  - fact: `src/thread.rs` now includes `test_core_runtime_acceptance_setup_status_monitor_vector_and_config_updates`, which runs `/setup -> /status -> /monitor -> /vector`, changes `task_orchestration_mode` to `sequential`, and verifies `Plan`, `ConfigOptionUpdate`, `/status`, `/monitor`, `/vector`, and canonical log evidence together.
  - fact: `docs/reference/acp_standard_spec.md` and `docs/quality/qa_checklist.md` now align on the actual `session/update` contract (`ConfigOptionUpdate`, not `CurrentModeUpdate`).
  - interpretation: the repo can now prove the core ACP runtime flow without depending on a fresh Zed transcript.
  - assumption: target client UI rendering/open policy still requires live client evidence because ACP stdio tests cannot prove editor-side click/open behavior.
- Score change: Must `4/4 -> 4/4`, Should `2/2 -> 2/2`
- Next action: collect one fresh target-client pass for local-link open policy and plan/progress rendering, then close the remaining external gate.

### Iteration 11
- Conclusion: the repo-controlled completion gate is fully revalidated on the current tree, and the only remaining work is external ACP client evidence.
- Evidence:
  - fact: `cargo fmt --check` passed on `2026-04-13`.
  - fact: `cargo test` passed with `118 passed, 0 failed` on `2026-04-13`.
  - fact: `cargo build --release` passed on `2026-04-13`.
  - fact: `scripts/acp_compat_smoke.sh --strict` generated `logs/smoke/acp_compat_smoke_20260413_081510.md` with `Overall: pass`.
  - interpretation: repo-controlled `Must` criteria are satisfied by current evidence, not only by historical smoke/test artifacts.
  - assumption: external client rendering/open behavior still cannot be promoted from repo evidence to verified acceptance without a fresh target-client session.
- Score change: Must `4/4 -> 4/4`, Should `2/2 -> 2/2`
- Next action: keep `EG-01..02` as the only live-client gate.

## Current Score
- Must: `4 / 4`
- Should: `2 / 2`
- Failed items:
  - none

## Context-Fit Decision
- `Fit`: yes, the current solution matches the task purpose of closing evidence and completion logic for the current ACP release line.
- `Context-Fit`: unchanged at the protocol level; only release evidence and registry status moved during execution.
- Decision:
  - `retain`
- Reason: strict smoke shows no urgent ACP schema drift, so the correct action is to preserve the implementation line and close the evidence/operational gap instead of forcing a speculative ACP upgrade.

## Release / Feedback
- Release impact: `v0.9.24` remains the active ACP release target with a published GitHub release and a passing strict smoke report (`logs/smoke/acp_compat_smoke_20260413_081510.md`).
- Regression risk: low for ACP contract drift in the current release line; medium for future unstable-schema drift and non-`codex` backend parity gaps.
- Failure-log attachment status: `N/A (strict pass)`
- Follow-up owner: repo owner for manual verification, ACP registry maintainers for fork-workflow approval
- Manual verification status: core ACP runtime acceptance is now covered by automated repo evidence (`thread::tests::test_core_runtime_acceptance_setup_status_monitor_vector_and_config_updates` plus strict smoke); partial pass from a real ACP client session remains for `/status`, `/monitor`, `/vector`, `/model`, and `/approvals`; automated preflight now pass with report `logs/manual_verification/setup_monitor_20260412_010614.md`; indirect Zed evidence confirms `xsfire-camp` opened with `AgentPanel` active on `2026-04-11 17:24:50`; the fixed runtime binary is installed at Zed's configured command path `/Users/g/.local/bin/xsfire-camp`; raw executable local-path rendering is covered by automated tests, but a fresh Zed ACP reply is still needed because older thread messages are not retroactively rewritten and at least one live Zed ACP process predates the reinstall, so the client session must be restarted before editor-side link/rendering checks can be meaningfully rechecked
- Next verification command: restart the ACP client session, then request one reply containing both a source/doc path and a raw executable path while observing plan/progress rendering

## Done Gate
- [x] All `Must` items passed.
- [x] Evidence paths and command summaries are recorded.
- [x] Remaining gaps are either `Should` items or explicit blockers.
