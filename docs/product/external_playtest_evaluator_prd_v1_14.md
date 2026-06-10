# v1.14 External Playtest Evaluator PRD

## Product purpose

Give developers a repeatable way to test whether the LLM GM feels like a real GM/referee at the table, not merely whether it emits internal events.

## User story

As a developer running Claude Code locally, I want Claude Code to judge each played turn from the outside, so that I can catch failures like stale menus, missing DB writeback, player-facing rule burden, and spoiler leaks without using production API tokens for evaluator reasoning.

## Scope

In scope:

- `trpg-harness playtest`
- multi-turn scenario JSON
- per-turn player-visible output capture
- per-turn SSE/JSONL event capture
- DB snapshot and DB diff capture
- optional Claude Code evaluator using `claude -p`
- Homecoming 8-turn golden playtest scenario

Out of scope:

- Claude Code as production GM backend
- Codex bridge
- full MCP server
- autonomous long-session player driver

## Success criteria

A run must produce turn-level artifacts that let a human or Claude Code answer:

- Did the GM ask the player for a rule-table value?
- Did the GM narrate a mechanical change without DB writeback?
- Did a stale gate or direction menu block a clear action?
- Did hidden module information leak into player text?
- Did a pending roll/effect bind to the correct mechanical gate?

## Default scenario

`harness/scenarios/homecoming_external_playtest_8turn.json` covers:

1. technical assessment of the drone cable;
2. required reaction under fire;
3. reaction/cover;
4. named-weapon attack;
5. system attack roll;
6. damage/effect roll and DB impact;
7. sustained pressure without direction-menu lock;
8. natural-language exit/movement.
