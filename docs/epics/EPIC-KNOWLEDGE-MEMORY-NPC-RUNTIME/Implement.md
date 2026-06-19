# Implement — EPIC-KNOWLEDGE-MEMORY-NPC-RUNTIME

## Loop Mode

Use Codex Team Lead autonomous goal loop with the parallel pipeline enabled.
This is not a one-card checkpoint.

Required pipeline artifacts:

- `AcceptanceLedger.md` is the Done authority.
- `WorkGraph.yaml` is the scheduling, dependency, lease, and integration-state
  authority.
- `IntegrationReport.md` records rolling integration events and recovery
  decisions.
- `.agents/skills/team-lead-parallel-pipeline/SKILL.md` defines the local
  conflict-aware scheduler rules.

The lead repeats:

```text
read AcceptanceLedger + WorkGraph + completed handoffs
  → review finished lanes and classify accepted/revision/rejected/blocked
  → trial-integrate each accepted scoped commit immediately
  → run targeted gate + affected integration smoke
  → update AcceptanceLedger + WorkGraph + IntegrationReport
  → recompute ready queue from dependencies, write_set, hotspot leases, and WIP
  → dispatch maximal conflict-free ready set
```

Stop only when:

- all approved rows are terminal;
- true BlockedByHuman occurs;
- destructive or irreversible action requires permission;
- model/backend/tooling is unavailable;
- dirty integration debt cannot be mapped to handoffs/tasks safely;
- approved stop limit is reached.

## Worker Model Policy

- Opus: implementation, debugging, architecture, integration, high-risk review.
- Sonnet: narrow verification, handoff synthesis, simple documentation/process updates.

## Dispatch Shape

Every dispatch must include:

- assigned design acceptance IDs;
- scope_own / scope_off;
- `parallel_pipeline: enabled` for code-affecting multi-lane work;
- `worktree`, `branch`, `base_sha`, and `commit_policy`;
- `read_set`, `write_set`, `hotspot_leases`, `generated_outputs`, and
  `migration_slot`;
- `workgraph_path` and `integration_report`;
- validation levels required;
- exact handoff path;
- instruction not to claim whole epic Done.

## Lead-Owned Work

Lead owns:

- AcceptanceLedger updates.
- WorkGraph state transitions, lease assignment, and ready-set selection.
- IntegrationReport updates.
- Final synthesis.
- Single-lane focused validation.
- Deciding the next maximal conflict-free ready set.
- Determining Done/Partial/Missing from evidence.

## Worker-Owned Work

Workers own:

- Code-affecting implementation inside scope.
- Tests inside scope.
- Local validation.
- One scoped local commit when `commit_policy: scoped_commit`.
- Shared hotspot proposals when they need unleased central edits.
- Handoff with ledger note.

## Current Recovery Gate

Before dispatching new code-affecting lanes, run
`TC-PIPE-00-integration-debt-classification` from `WorkGraph.yaml`.

The current worktree already contains substantial dirty integration output from
prior workers. Treat that as integration debt, not as accepted product state.
Do not start new Knowledge/NPC/Memory implementation lanes until dirty slices
are mapped to handoffs, reviewed, and either accepted into the rolling
integration train or turned into revision/replacement tasks.

## WorkGraph Scheduling Rules

A task may enter `running` only when:

1. all `depends_on` tasks are integrated or intentionally terminal;
2. its `write_set` does not overlap another active writer;
3. every `hotspot_leases` entry is free;
4. it has an explicit validation profile and handoff path;
5. code-affecting work has an isolated worktree and branch.

The lead should prefer one contract/architecture writer, up to four disjoint
implementation writers, one or two verification lanes, and one serial
integration train. Increase worker count only when the conflict graph supports
it.

## Rolling Integration

For every accepted scoped worker commit:

1. Create a disposable trial branch from the current integration head.
2. Cherry-pick the scoped worker commit.
3. Run the task validation profile and affected integration smoke.
4. Fast-forward the local integration branch only after validation passes.
5. Record the event in `IntegrationReport.md`.
6. Update the WorkGraph task with `commit_sha`, `integration_sha`, and state.
7. Recompute the ready queue and backfill newly available lanes.

Merge conflicts are not user questions by default. Dispatch a bounded
conflict-resolution worker unless the conflict changes product semantics,
public compatibility, irreversible migration policy, security/privacy posture,
or approved scope.

## Revision Strategy

If a worker delivers type/storage only for a behavior row:

1. Accept foundation only if correct.
2. Mark behavior row Partial, not Done.
3. Dispatch integration task.

If validation is missing:

1. Dispatch revision for validation.
2. Do not accept Done.

If architecture violates project constitution:

1. Reject or request revision.
2. Consider fresh worker with narrower scope.
