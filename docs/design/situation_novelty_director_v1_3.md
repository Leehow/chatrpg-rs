# v1.3 Situation Novelty Director

## Purpose

v1.3 turns novelty into runtime state. Players are allowed to repeat an action, but NPCs and the world should not respond with the same beat unless there is a new cost, pressure, position change, or objective progress.

The v1.2 debug report found three issues: enemy-initiated combat openers regressed, direction gates could reprompt-loop, and Director four-anchor output sometimes missed required anchors. v1.3 fixes those and adds an anti-repetition layer.

## Core rule

```text
Same player action does not justify same GM/NPC response.
```

## New contracts

- `BeatSignature`: actor + tactic + target + consequence signature for one beat.
- `FreshChange`: what is new this turn.
- `NoveltyDecision`: whether the system forced a tactic shift.
- `TacticPalette`: available NPC tactics by archetype.
- `TacticCooldown`: per-frame tactic cooldown.
- `NoveltyState`: recent beats and no-repeat state inside `CombatWorkingState`.

## Runtime behavior

1. Enemy-initiated combat such as “无人机开始朝我开火，场面进入战斗” now starts a frame and opens a required reaction gate.
2. Direction gates semantically accept “继续开火”, “keep firing”, `/roll`, and similar replies as `continue_conflict`.
3. After repeated player attacks, NPC tactics shift through the tactic palette instead of mirroring the same attack.
4. Actionable Situation Director repairs missing four anchors and always supplies `fresh_change`.
5. Repeated waiting or repeated pressure creates `clock_tick` / `fresh_change` rather than duplicate narration.

## Cache policy

Novelty state is BP3 only. It contains current frame pressure, recent beats, cooldowns, and fresh changes. It must not alter BP1/BP2 hashes.
