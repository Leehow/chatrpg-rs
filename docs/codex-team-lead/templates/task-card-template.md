# Task Card: <TASK_ID> — <Title>

## Parent Epic

- epic_id: <EPIC_ID>
- work_id: <WORK_ID>

## Assigned Design Acceptance IDs

- <DA-ID>

## Objective

<Observable outcome, not just code object.>

## Scope Own

```text
<paths/crates>
```

## Scope Off

```text
<paths/crates>
```

## Required Behavior

1. ...
2. ...

## Required Validation

| Acceptance item | Level | Command / fixture / journey | Done evidence |
|---|---:|---|---|
| ... | V1 | ... | ... |

## Not Done If

- Only types exist but no projection/integration where required.
- Only prompt exists for a safety feature.
- No required validation evidence.
- The worker cannot map changes to ledger rows.

## Worker Handoff Requirements

The handoff must include:

- Files changed.
- Architecture choices.
- Validation commands and outcomes.
- Ledger note for lead: Done / Partial / Missing / Blocked for every assigned design ID.
- Follow-up rows unblocked by this task.
