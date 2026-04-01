# ACP Iteration Fit Record - v0.9.24

## Header
- Task: ACP release-readiness evidence closure for `v0.9.24`
- Date (UTC): 2026-03-29T13:20:56Z
- Owner: Codex
- Branch / commit: `main` / `6aabc683f4f72f8b2a89359ee8bace1b57bf9f37`
- Scope: strict ACP smoke evidence, release evidence hygiene, and ACP registry blocker capture

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
- Release impact: `v0.9.24` remains the active ACP release target with a published GitHub release and a passing strict smoke report.
- Regression risk: low for ACP contract drift in the current release line; medium for future unstable-schema drift and non-`codex` backend parity gaps.
- Failure-log attachment status: `N/A (strict pass)`
- Follow-up owner: repo owner for manual verification, ACP registry maintainers for fork-workflow approval
- Next verification command: `scripts/acp_compat_smoke.sh --strict`

## Done Gate
- [x] All `Must` items passed.
- [x] Evidence paths and command summaries are recorded.
- [x] Remaining gaps are either `Should` items or explicit blockers.
