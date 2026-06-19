# Rolling integration train

## Branch topology

```text
main (untouched)
  \
   codex/<work_id>-integration  <-- always green local integration baseline
      |\
      | claude/<work_id>/<task-a>
      | claude/<work_id>/<task-b>
      | claude/<work_id>/<task-c>
      \
       try/<work_id>/<task-id>  <-- disposable trial merge branch
```

## Worker rule

A code writer works only in its worktree and produces one scoped commit plus a handoff. It never pushes or merges.

## Trial integration

For every accepted task:

1. branch from current integration HEAD;
2. cherry-pick the worker commit;
3. run task-targeted validation;
4. run affected-crate integration smoke;
5. run architecture guards;
6. fast-forward integration only on pass;
7. record task commit and integration commit in WorkGraph.

A merge conflict does not go to the user. Dispatch a bounded conflict-resolution worker with the current integration HEAD and original acceptance IDs. Escalate only if resolving it changes product semantics or public contracts.

## No batch barrier

Do not wait for all concurrently running workers. Integration is event-driven: first acceptable result enters the train first. New ready tasks use the new integration HEAD.
