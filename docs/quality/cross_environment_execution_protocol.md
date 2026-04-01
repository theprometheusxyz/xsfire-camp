# Cross-Environment Execution Protocol

Purpose: define one reusable execution contract for implementation, refactor, debugging, and release-readiness work across ACP clients, backend choices, and local environments.

## Goal
- Fix the task goal in one sentence before implementation.
- The goal must include a verifiable completion condition.

Example:
- "Prove the current ACP release line has no urgent protocol-drift blocker by passing `scripts/acp_compat_smoke.sh --strict` and recording the result in release evidence."

## Rubric
- Split criteria into `Must` and `Should`.
- Every `Must` item must cite evidence:
  - repository file path,
  - command output summary,
  - or primary external source with date.
- `Should` items improve quality or operations but do not block completion.

Template:

```md
## Rubric
### Must
- <criterion>
  - Evidence: <file path | command | source>

### Should
- <criterion>
  - Evidence: <file path | command | source>
```

## Sequence Dependency Standard
- Use one explicit execution path with dependencies.
- Default ordering is `Research -> Rubric -> Plan -> Implement -> Verify -> Score -> Next Action`.
- When task order matters, expand the plan as `R -> P -> M -> W -> A`:
  - `R`: research and current-state capture
  - `P`: phase or priority selection
  - `M`: milestone or major checkpoint
  - `W`: work package
  - `A`: atomic action with one owner and one done check

Example:

```md
R1 Current-state research
P1 Fix completion criteria
M1 Close documentation gap
W1 Add missing template docs
A1 Create `docs/quality/iteration_fit_template.md`
```

## Iteration Loop
Each iteration should target the smallest failed rubric item first.

1. Research
   - read only the files and logs needed to classify the gap
2. Rubric
   - write or update `Must` and `Should` with evidence anchors
3. Plan
   - choose one sequence-dependent path and avoid parallel work that breaks dependency order
4. Implement
   - make the minimum safe change
5. Verify
   - rerun the exact command or inspect the exact file that proves the change
6. Score
   - mark each rubric item `pass` or `fail`
7. Next Action
   - either choose the next failed item or declare done

## Completion Rule
- A task is complete only when all `Must` items pass.
- If a `Must` item fails, do not declare completion.
- If the time or iteration budget is exceeded, record:
  - blocker,
  - current score,
  - best workaround,
  - next verification command.

## Evidence Rules
- Prefer direct evidence over inference.
- Keep evidence close to the work:
  - release work: release notes, checklist, runbook
  - ACP contract work: spec mapping doc, smoke report, targeted tests
  - backend work: source file + focused regression tests
- If a referenced file is missing, that missing path is itself a valid failure signal and should be fixed or explicitly retired.

## Blocker Handling
- Ask only when blocked by:
  - destructive or irreversible action,
  - missing credentials or permissions,
  - unresolved product choice with no supporting evidence.
- When blocked, ask one concise question and include one recommended default.

## Required Iteration Output
Use this order for execution reports:

1. `Goal`
2. `Rubric (Must/Should)`
3. `Iteration N Result`
4. `Current Score / Remaining Gaps`
5. `Next Action or Done`

## ACP-Oriented Verification Defaults
- `cargo test`
- `scripts/acp_compat_smoke.sh --strict`
- `git diff --check`
- release or registry verification commands from `docs/guides/github_registry_release_runbook.md`

## Done Decision
- `Done` means:
  - every `Must` item is backed by current evidence,
  - required docs are updated,
  - the next operator can continue without re-discovering the same state.
