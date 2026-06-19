# Lead Evaluation Checklist

Use this after every worker handoff.

## 1. Handoff Completeness

- Worker restated the task.
- Worker listed files changed.
- Worker listed validation commands and outcomes.
- Worker mapped changes to assigned design acceptance IDs.
- For parallel-pipeline lanes, worker confirmed worktree, branch, base SHA,
  read set, write set, hotspot leases, validation profile, and commit policy.
- For `commit_policy: scoped_commit`, worker reported exactly one local commit
  SHA or a clear blocker explaining why no commit was produced.
- Worker listed risks and untested gaps.
- Worker did not claim unrelated rows Done.

## 2. Diff Review

- Scope stays inside `scope_own`.
- Diff stays inside declared `write_set`, except for explicitly leased shared
  hotspots.
- No unleased edits to root manifests, crate facades, central registries,
  migrations, generated outputs, AcceptanceLedger, WorkGraph, or shared status
  docs.
- No destructive git or unrelated cleanup.
- No ruleset/module/sample hardcode in engine/runtime crates.
- No LLM/plugin direct state commit.
- No bypass of NeedBus/BindingResolver where applicable.
- No prompt-only solution for deterministic safety boundary.
- No durable state without event/projection reasoning.
- No huge file growth beyond project policy unless deferred.

## 3. Validation Review

For each assigned design acceptance row:

- Required V-levels are known.
- Evidence exists for every required V-level.
- Output is inspected, not merely asserted.
- If a check failed, the failure is understood.
- If exact validation is blocked, row is Partial/Blocked, not Done.

## 4. Integration Train Review

For accepted scoped commits:

- Create the trial branch from the current WorkGraph integration head.
- Cherry-pick only the worker's scoped commit.
- Run the targeted validation profile and affected integration smoke.
- Advance the local integration branch only after validation passes.
- Record previous head, worker commit, trial result, validation, and new head in
  `IntegrationReport.md`.
- Update the WorkGraph task state to `integrated` or a repair state.

## 5. Design Coverage Review

Update AcceptanceLedger:

- Done only with evidence.
- Partial if some layers exist but design outcome is not proven.
- Missing if not started.
- BlockedByPrerequisite if dependency is missing.
- BlockedByHuman only if user decision is required.
- Deferred only if explicitly approved or epic non-goal.

## 6. Next Action

If accepted and the ledger has unblocked Missing/Partial rows:

- Recompute the WorkGraph ready queue.
- Dispatch the maximal conflict-free ready set within WIP limits.
- Do not ask user for routine continuation.

If rejected:

- Dispatch revision with exact failure rows.

If truly blocked:

- Ask user with one concise blocker statement and recommended options.
