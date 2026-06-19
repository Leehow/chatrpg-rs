---
name: team-lead-parallel-pipeline
description: Extend Codex Team Lead Mode with conflict-aware parallel scheduling, isolated worktrees, rolling integration, continuous execution, and connected journey validation.
---

# Team Lead Parallel Pipeline

Read the base `team-lead-mode` skill first. This skill tightens orchestration for large, approved implementation epics.

## ChatRPG marker compatibility

This repository currently requires the global `[TEAM_LEAD_WORKER_V1]` marker.
Do not dispatch workers with the sample `[TEAM_LEAD_WORKER_V2]` marker unless
the global Team Lead worker skill and `CLAUDE.md` are updated first. For
ChatRPG, use the existing V1 marker and add the parallel fields documented in
`docs/codex-team-lead/03-worker-dispatch-template.md`:

- `parallel_pipeline`;
- `worktree`, `branch`, `base_sha`;
- `commit_policy`;
- `read_set`, `write_set`, `hotspot_leases`;
- `generated_outputs`, `migration_slot`;
- `validation_profile`;
- `workgraph_path`, `integration_report`.

## Operating invariant

Do not maximize worker count. Maximize **independent write throughput** while keeping the integration branch green.

A worker task may run in parallel only when:

1. all hard dependencies are integrated;
2. its `write_set` does not overlap another active writer;
3. it holds every required `hotspot_lease`;
4. it has its own worktree and branch;
5. its acceptance IDs and validation profile are explicit.

## Required artifacts

For a non-trivial epic, maintain:

- `AcceptanceLedger.md`: design outcomes and required evidence;
- `WorkGraph.yaml`: machine-readable dependency/conflict/scheduling state;
- an integration branch `codex/<work_id>-integration`;
- one isolated worktree per code-affecting lane;
- a rolling integration report.

## Continuous scheduler loop

Repeat without user check-ins:

1. Read the ledger, WorkGraph, integration HEAD, and completed handoffs.
2. Review finished lanes and classify them Accepted / Revision / Rejected / Blocked.
3. Trial-integrate every Accepted lane immediately; do not wait for sibling lanes.
4. Run the lane's targeted gate plus the integration smoke gate.
5. On pass, advance the integration branch and mark the task Integrated.
6. Recompute the ready queue from dependencies and write-set leases.
7. Dispatch the maximal conflict-free set within WIP limits.
8. Keep verification, review, and journey-design lanes occupied while code lanes run.
9. Continue until all acceptance rows are terminal or a true human blocker appears.

A task completion is a checkpoint, never an epic completion condition.

## Default WIP profile

Start with:

- 1 contract/architecture writer;
- up to 4 disjoint implementation writers;
- 1 test/journey design lane;
- 1 adversarial review or verification lane;
- 1 serial integration train.

Increase or decrease code writers from the conflict graph, not token availability.

## Git policy

For code-affecting parallel work, `worktree` is the default rather than an exception.

Allowed when the approved epic contract says so:

- create/delete local task branches and worktrees;
- worker creates one scoped local commit;
- create disposable trial-integration branch/worktree;
- cherry-pick accepted worker commits into the trial branch;
- abort failed trial cherry-picks;
- fast-forward the local integration branch after tests pass.

Never implied:

- push;
- merge or fast-forward `main`;
- rewrite shared history;
- deploy;
- destructive cleanup of user work.

## Conflict policy

`scope_own` is necessary but insufficient. Every task declares:

- `read_set`;
- `write_set`;
- `hotspot_leases`;
- `contract_dependencies`;
- `generated_outputs`;
- `migration_slot` when applicable.

Overlapping write sets are serialized unless one side is converted into a read-only review/test lane.

Shared hotspots are single-writer:

- root/workspace manifests;
- central facade/registry/composition-root files;
- migration ordering;
- generated contracts/assets;
- shared ledgers and narrative status documents.

Workers propose changes to shared hotspots in handoff notes. The contract or integration owner applies them serially.

## Autonomy policy

Routine ambiguity is not a blocker. Resolve it using:

1. approved acceptance behavior;
2. project constitution;
3. existing stable contracts and patterns;
4. smallest reversible change;
5. fail-closed behavior;
6. explicit decision note in the handoff.

Escalate to the human only for product-semantic choices, irreversible migration/data loss, security/privacy/legal/billing, external credentials/resources, public compatibility break, scope expansion/reduction, or exhausted approved limits.

Worker questions are classified as:

- `self_resolve`: worker decides and records;
- `lead_resolve`: Codex decides from the approved contract;
- `human_blocker`: only this category reaches the user.

## Integration train

Never merge a whole batch at once. For each accepted task:

1. create a disposable trial branch from current integration HEAD;
2. cherry-pick the task's scoped commit;
3. run targeted tests and contract checks;
4. run integration smoke appropriate to affected surfaces;
5. advance integration HEAD only on pass;
6. dispatch a conflict-resolution or revision worker on failure.

New tasks start from the latest integration HEAD. Running tasks on older bases may finish when their write sets are disjoint; trial integration decides whether they are still applicable.

## Validation

Component tests are diagnostic evidence. Player-visible acceptance requires a connected Journey that proves:

`human operation -> feature trigger -> runtime mechanism -> committed state -> visible result -> later-turn/reload effect`.

No character/session prelude means `INVALID_SETUP`. No feature trigger means `NOT_TRIGGERED`. A mechanical check without `DiceRolled` and `CheckResolved` cannot pass.

Use one ScenarioSpec for deterministic, live-model, and replay modes.
