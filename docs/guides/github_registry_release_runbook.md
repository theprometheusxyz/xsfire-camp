# GitHub Registry and Release Runbook

Purpose: provide one canonical operational flow for GitHub release checks, ACP registry PR tracking, and registry PR communication rules.

## Scope
- Repository: `theprometheusxyz/xsfire-camp`
- ACP registry: `agentclientprotocol/registry`
- ACP registry entry: `registry/xsfire-camp/agent.json`

## Non-Negotiable Communication Rule
- All comments on ACP registry PRs must be written in English.
- If clarification is needed, post one concise English comment with evidence links (run URL, log excerpt, changed file path).

## Standard Flow
1. Verify the latest GitHub release and release workflow status in this repo.
2. Verify ACP registry entry version and ACP registry PR state/checks.
3. Resolve blockers with evidence-first updates.
4. Record outcome in release notes/checklists.

## Current State Snapshot
- Snapshot time (UTC): `2026-03-29T12:35:31Z`.
- Latest product release target is `v0.9.24`.
- `v0.9.24` GitHub release is published (`draft=false`, `prerelease=false`):
  - `https://github.com/theprometheusxyz/xsfire-camp/releases/tag/v0.9.24`
- Latest product `Release` workflow run is `23708294063` on branch `v0.9.24` with `conclusion=success`:
  - `https://github.com/theprometheusxyz/xsfire-camp/actions/runs/23708294063`
- ACP registry PR `#93` is `OPEN` on head `fed906a` and now carries the `v0.9.24` entry update:
  - `https://github.com/agentclientprotocol/registry/pull/93`
- Latest ACP `Build Registry` run on branch `add-xsfire-camp-agent` is `23709093662` with `conclusion=action_required` on head `fed906a`:
  - `https://github.com/agentclientprotocol/registry/actions/runs/23709093662`
- Latest status comment requesting maintainer re-run:
  - `https://github.com/agentclientprotocol/registry/pull/93#issuecomment-4150073545`
- Current gap:
  - product release and ACP registry entry are both on `v0.9.24`, but the fork-originated registry workflow still requires maintainer approval/re-run.

## Verification Commands
```bash
# 1) Latest release and release workflow in product repo
gh release view v0.9.24 --repo theprometheusxyz/xsfire-camp --json name,tagName,isDraft,isPrerelease,url,publishedAt
gh run list --repo theprometheusxyz/xsfire-camp --workflow release.yml --limit 5 --json databaseId,workflowName,headBranch,status,conclusion,url,createdAt

# 2) ACP registry PR status/checks (replace PR number if needed)
gh pr view 93 --repo agentclientprotocol/registry --json number,state,mergeStateStatus,headRefName,baseRefName,url,updatedAt
gh pr checks 93 --repo agentclientprotocol/registry

# 3) If a specific registry workflow run is blocked/action_required
gh run list --repo agentclientprotocol/registry --branch add-xsfire-camp-agent --limit 5 --json databaseId,workflowName,status,conclusion,url,createdAt
gh run view 23709093662 --repo agentclientprotocol/registry
```

## Blocker Handling
### A. ACP registry PR blocked (`action_required`)
- Typical cause:
  - upstream maintainer approval/re-run is required for a fork-originated workflow.
- Action:
  - Add one English status comment on the PR with:
    - current blocker (`action_required`)
    - run URL/ID
    - exact requested maintainer action (approve/re-run)
- Do not spam repeated comments without new evidence.

## Evidence Log Template
Use this compact structure in release docs/checklists:
- `release_run`: `<repo run URL or ID>`
- `registry_pr`: `<PR URL + state>`
- `registry_checks`: `<check summary>`
- `blocker`: `<none | description>`
- `next_action`: `<single actionable step>`

## Done Criteria
- `release.yml` latest relevant run is successful or failure is documented with owner/action.
- ACP registry PR entry matches the latest release assets and has no unresolved maintainer-action blocker, or the blocker is explicitly tracked with a single English status comment and next action owner.
