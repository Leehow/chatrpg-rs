# Acceptance Ledger Contract

The acceptance ledger is the source of truth for whether the design is complete.

## Status Terms

| Status | Meaning |
|---|---|
| Missing | No meaningful implementation or evidence exists. |
| Partial | Some layer exists, but at least one required acceptance/evidence row is missing. |
| Implemented | Code exists but required validation is incomplete. Do not use for final completion. |
| Done | Required behavior is implemented and required evidence passed. |
| Deferred | Explicitly postponed by user or epic non-goal. Must name reason. |
| BlockedByHuman | Needs user decision, secrets, destructive action, or scope change. |
| BlockedByPrerequisite | Cannot proceed until another design row is done. Not terminal. |
| Untested | Code exists but evidence is absent or blocked. Not terminal. |
| OutOfScope | Excluded by the approved Prompt.md. |

## Ledger Row Template

```markdown
| ID | Design acceptance item | Required evidence | Status | Implemented by | Evidence | Gaps / next action |
|---|---|---|---|---|---|---|
| DA-KNOW-01 | Holder-specific knowledge projection works for GM/player/NPC. | V1,V2,V4 | Missing | - | - | Needs model/storage/projection tests. |
```

## Lead Update Rules

1. Workers do not edit the ledger unless explicitly scoped.
2. Workers include a `Ledger note for lead` in handoff.
3. Lead applies ledger updates after diff review and validation.
4. A row cannot move to Done if required evidence is absent.
5. A downstream row cannot move to Done only because a prerequisite was implemented.
6. A row with compile-only evidence can be at most Implemented or Partial unless the row was compile-only.
7. A row with prompt-only implementation cannot satisfy a safety/policy design unless context filter and verifier rows are also done or explicitly deferred.

## Design Coverage Audit

Before final report, the lead must answer:

1. Which design rows are Done?
2. Which are Partial and why?
3. Which are Missing?
4. Which are Deferred, and did the user approve deferral?
5. Which are BlockedByHuman?
6. Which rows have only V2 evidence but require V3/V4/V5?
7. Which user-facing claims lack V3?
8. Which safety claims lack V4?
9. Which TRPG parser/playability claims lack V5?
