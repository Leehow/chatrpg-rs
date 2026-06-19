# Continuous Epic Mode

Continuous Epic Mode is the mode to use when the user wants the team-lead loop to run an approved design from start to finish.

It exists because normal Team Lead Mode has a valid checkpoint pattern:

```text
worker finishes one bounded task -> lead reviews -> lead reports
```

That pattern is correct for one-card tasks, but wrong when the user asks for a whole design to be executed without stopping after each card.

## When to use

Use this mode for prompts like:

- “直接用 loop 把你的设计从头跑到尾。”
- “不要每做一个任务卡就停。”
- “除了 hard blocker，不要问我。”
- “按这个 P0/P1/P2 路线自己实现、测试、修复、继续。”

## Main rule

An accepted worker handoff is a checkpoint, not a final answer.

After accepting one task, the lead must update the ledger and dispatch the next unblocked task.

## Lead behavior

The lead owns:

- epic ledger creation and updates;
- worker dispatch order;
- handoff review;
- revision/fresh-worker decisions;
- integrated validation;
- final user report only when the epic stops.

Workers own:

- bounded implementation;
- local design decisions;
- self-repair;
- validation evidence;
- handoff report.

## Progress messages

Use short checkpoint progress messages only:

```text
Checkpoint accepted: TC-KNOW-02. Validation passed. Next: TC-KNOW-00 because NPC durable holder support is still blocked by actor identity.
```

Do not say “完成了” unless the entire ledger is complete or hard-blocked.

## Recommended first epic

For the current GM memory / spoiler / NPC mind design, use:

`docs/agent-loop/epics/EPIC-KNOWLEDGE-NPC-RUNTIME.md`
