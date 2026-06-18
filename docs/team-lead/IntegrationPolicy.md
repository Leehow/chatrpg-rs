# Integration Policy

## Dedicated integration lane

The integration train never runs in the human or feature worktree. It uses:

```text
branch:   codex/<work_id>-integration
worktree: .worktrees/<work_id>/integration
```

A dirty existing worktree is not a blocker. Preserve it untouched.

## Current recovery sequence

For `knowledge-runtime-p0`:

1. create the clean integration worktree;
2. cherry-pick `de97b05f31e746e0a1206e3198951e75cc84a960`;
3. run `model-plus-downstream-check`;
4. advance integration HEAD;
5. cherry-pick `d32ccac488dafc84d4bf233ffd8f969613c3643a`;
6. rerun the gate;
7. update WorkGraph;
8. refill implementation WIP to at least four writers;
9. keep the existing `worker-tc-vs-npc-01` lane running unless it violates scope.

Do not wait for all workers before integration.

## Conflict handling

A textual cherry-pick conflict produces a `conflict_resolution` task. The resolver
owns only the conflicting files and uses current integration HEAD plus the worker
commit as inputs.

Ask the user only when the conflict represents a new product semantic choice or
irreversible compatibility decision.
