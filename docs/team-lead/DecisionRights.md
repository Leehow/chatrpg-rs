# Decision Rights

## Worker decides

- private helper names and internal module layout;
- reversible local refactors inside `write_set`;
- test fixture organization;
- serde defaults that preserve compatibility and fail closed;
- choice among implementations that satisfy the same acceptance contract;
- whether another self-repair iteration is needed.

## Codex lead decides

- task decomposition and IDs;
- ready queue order;
- WIP level inside configured bounds;
- model/backend selection;
- worktree/branch creation;
- hotspot leases;
- revision versus fresh worker;
- rolling integration order;
- focused validation commands;
- conflict-resolution dispatch;
- ledger and WorkGraph updates.

## Human decides

- new or changed product semantics;
- scope expansion/reduction;
- public API compatibility break;
- irreversible migration/data loss;
- security/privacy/legal/billing;
- unavailable credentials or external resources;
- push, main merge, deployment, or shared-history rewrite;
- approved resource/stop-limit expansion.
