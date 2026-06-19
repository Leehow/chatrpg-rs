# Autonomous Team-Lead Development Loop

## Goal

This document defines the development loop for AiChatTrpg work that should be executed through Team Lead Mode without repeatedly asking the human routine implementation questions.

The loop is designed for:

- source-grounded JSON asset runtime work;
- Knowledge Runtime and spoiler policy work;
- NPC mind/persona/relationship work;
- plugin/policy systems;
- runtime transaction and validation work;
- parser/asset/binding work.

## Operating model

```text
Codex lead
  owns task decomposition, dispatch, review, validation, final report

Claude Code worker
  owns bounded implementation, local design, tests, self-repair, handoff

Task card
  owns acceptance criteria and validation matrix

Tests and diffs
  are the final judge, not intermediate chat claims
```

## Non-goal

This loop does not make workers unconstrained. It makes them autonomous inside a bounded task card.

Workers must still obey:

- AGENTS.md;
- CLAUDE.md;
- the Team Lead worker skill;
- scope_own / scope_off;
- risk budget;
- repository architecture rules.

## One-line protocol

```text
Dispatch with a task card, let the worker design/implement/test/repair, review evidence, revise through workers, then report status from the acceptance ledger.
```
