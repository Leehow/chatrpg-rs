# Continuous Epic Protocol

Use this protocol when the user asks the lead to run an approved design from start to finish rather than stopping after each task card.

## Lead invariant

A worker handoff is a checkpoint, not a finish line.

The lead must continue until the ledger reaches a terminal state or a hard stop condition occurs.

## Required setup

1. Select `run_mode: continuous_epic`.
2. Create or update a ledger.
3. Convert the approved design into ordered task cards.
4. Mark dependencies explicitly.
5. Dispatch the first unblocked task.

## Dispatch loop

```text
while true:
  read latest ledger
  if all items terminal:
      final report
      stop

  next = first item with status NotStarted and dependencies Done
  if no next:
      if any item BlockedByPrerequisite and prerequisite in scope:
          dispatch prerequisite
          continue
      if hard blocker:
          ask human with recommended resolution
          stop
      final report with Missing/Blocked reason
      stop

  dispatch worker(next)
  wait for handoff
  review handoff + final response + diff + validation

  if accepted:
      mark Done with evidence
      continue

  if needs revision:
      dispatch revision worker
      continue

  if rejected:
      dispatch fresh replacement worker or mark BlockedByHuman if unsafe
      continue
```

## User-facing progress notes

During continuous epic work, progress notes should be short and should not look like final completion reports. Use:

```text
Checkpoint accepted: <task_id>. Evidence: <tests>. Next dispatch: <next_task_id>.
```

Do not end a user-facing message with “完成了” unless the whole ledger is complete or a hard stop condition was reached.

## Stop conditions

Stop only for:

- all ledger items terminal;
- hard blocker requiring user;
- unsafe working tree;
- explicit budget/stop request;
- repeated task failure with no safe bounded next action.

## Recovery when the loop accidentally stopped

If the user says the loop stopped too early:

1. Acknowledge that the previous behavior was checkpoint-mode, not continuous-epic.
2. Switch the current work to `continuous_epic`.
3. Reconstruct the ledger from accepted handoffs and task cards.
4. Mark completed tasks Done.
5. Dispatch the next unblocked task.

Do not ask the user to restate the design unless the ledger cannot be reconstructed.
