# Iteration Fit Template

Use this template to record one execution cycle with evidence-backed completion logic.

## Header
- Task:
- Date (UTC):
- Owner:
- Branch / commit:
- Scope:

## Goal
- Write one sentence with a verifiable completion condition.

Example:
- "Close ACP release-readiness evidence gaps by linking the latest strict smoke report, release workflow status, and registry blocker state in repo docs."

## Research Snapshot
- Files read:
  - `<path>`
- Commands run:
  - `<command>` -> `<result summary>`
- External sources:
  - `<source + date>` or `none`

## Rubric
### Must
- [ ] Criterion:
  - Evidence:
- [ ] Criterion:
  - Evidence:

### Should
- [ ] Criterion:
  - Evidence:
- [ ] Criterion:
  - Evidence:

## Sequence Plan (R->P->M->W->A)
- `R1`:
- `P1`:
- `M1`:
- `W1`:
- `A1`:
  - depends_on:
  - done_check:

Add more only when the dependency chain truly branches.

## Iteration Log

### Iteration 1
- Conclusion:
- Evidence:
  - fact:
  - interpretation:
  - assumption:
- Score change:
- Next action:

### Iteration 2
- Conclusion:
- Evidence:
  - fact:
  - interpretation:
  - assumption:
- Score change:
- Next action:

Duplicate more sections as needed.

## Current Score
- Must: `x / y`
- Should: `x / y`
- Failed items:
  - `<item>`

## Context-Fit Decision
- `Fit`: the current solution still matches the task purpose and operating constraints.
- `Context-Fit`: note whether environment, backend, client, or release context changed during execution.
- Decision:
  - `retain`
  - `adjust plan`
  - `stop and escalate`
- Reason:

## Release / Feedback
- Release impact:
- Regression risk:
- Follow-up owner:
- Next verification command:

## Done Gate
- [ ] All `Must` items passed.
- [ ] Evidence paths and command summaries are recorded.
- [ ] Remaining gaps are either `Should` items or explicit blockers.

## ACP Release-Readiness Example
- Goal:
  - "Confirm the current ACP release line needs no urgent protocol upgrade by passing strict smoke and recording release evidence."
- Must evidence example:
  - [acp_compat_smoke_20260329_221344.md](../../logs/smoke/acp_compat_smoke_20260329_221344.md)
  - [acp_standard_spec.md](../reference/acp_standard_spec.md)
  - [github_registry_release_runbook.md](../guides/github_registry_release_runbook.md)
