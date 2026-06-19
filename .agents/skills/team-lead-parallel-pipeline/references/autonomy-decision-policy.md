# Autonomy decision policy

## Worker may decide

- private names and file layout inside owned domain;
- helper functions and local refactors;
- test organization inside assigned test surface;
- serde defaults that preserve backward compatibility and fail closed;
- bounded error types and observability fields;
- whether to use an existing dependency already in the workspace;
- implementation alternatives that satisfy the same acceptance behavior;
- revision strategy after a deterministic failure.

## Lead decides without asking the user

- task IDs, worker model/backend, dispatch order, WIP level;
- worktree creation and lease assignment;
- whether to revise, replace, or verify a worker;
- local trial integration ordering;
- targeted validation commands inside the approved validation class;
- conservative conflict resolution that preserves contracts;
- ledger and WorkGraph state transitions.

## Human-only decisions

- a new user-visible semantic or UX tradeoff;
- public API break or compatibility policy change;
- irreversible migration/data loss;
- new security/privacy/legal/billing posture;
- credentials or unavailable external resources;
- new large dependency or deployment architecture not covered by the approved plan;
- scope expansion/reduction;
- push/main merge/deploy when not pre-authorized;
- approved time/token/turn limit exceeded.

## Assumption record

For non-human ambiguity, continue and record:

```text
Assumption:
Decision:
Why this is reversible and consistent with the constitution:
Validation proving it:
```
