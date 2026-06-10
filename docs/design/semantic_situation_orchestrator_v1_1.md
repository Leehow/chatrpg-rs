# Semantic Situation Orchestrator v1.1

v1.1 replaces the v1.0 combat keyword lock with a semantic situation router.

## Problem

v1.0 could create and compact combat frames, but the active frame acted like a mode lock. Once a combat frame was active, every later player input was consumed as combat unless it matched a narrow close-keyword list. The debug report reproduced this with hide, retreat, de-escalation, and leave-scene inputs that remained stuck in `active:combat`.

## Product rule

A frame is not a mode. A frame is a local situation with goals, pressure, participants, and exits. Every player input must first be routed against the active frame:

- inside-frame action
- required gate response
- exit attempt
- de-escalation attempt
- surrender
- flee
- hide to disengage
- pause and observe
- outside-frame action
- clarification / ambiguous

The router is Rust-owned. It may be backed by LLM skills in later versions, but the control flow, event emission, persistence, and safety gates remain in Rust.

## Runtime changes

### Removed keyword-only main path

The old `looks_like_conflict_start`, `looks_like_attack_or_harm`, `looks_like_reaction_trigger`, and `looks_like_frame_end` functions were removed from the main path. v1.1 uses `classify_situation_intent` to produce a typed `ConflictIntent`.

The current implementation is a Rust semantic paraphrase router: it compares player input against multilingual paraphrase clusters using exact containment plus character-bigram similarity. This is intentionally broader than substring triggers and supports Chinese, English, and Japanese paraphrases such as:

- `拔枪还击`
- `draw and return fire`
- `銃を抜いて撃ち返す`
- `躲到集装箱后面，先不打了`
- `举手和解`
- `转身沿小巷撤退，离开这片区域`

Future versions can replace the internal classifier with an LLM JSON skill without changing the public `ConflictIntent` contract.

### ExitContract

Retreat, surrender, de-escalation, and leave-scene actions are now first-class `ExitContract`s, not ordinary combat actions. Clear exit outcomes close and compact the frame. Hide/pause actions open a direction gate instead of continuing the attack loop.

### StalemateContract

Frames now track progress with `FrameProgressTracker`. If there is no decisive change for several turns, or the player repeats the same action, or NPC patience runs out, the system opens a `conflict_direction_gate_opened` InteractionGate. This prevents combat, investigation, negotiation, infiltration, and side-quest loops.

### NPC drive state

Frames now include `NpcDriveState` and `NpcTacticMemory`. NPCs have patience, morale, aggression, self-interest, and repeated tactic counters. The default evaluator lowers patience when the situation does not progress and forces a direction gate instead of letting NPCs repeat the same behavior forever.

## Cache policy

- BP1: ruleset situation protocol and stable GM behavior.
- BP2: current static encounter/scene/NPC materials and ruleset profile.
- BP3: `StateFrame`, `ConflictIntent`, `ExitContract`, `StalemateContract`, current gate, dice, and transient frame events.

HP/status/patience/morale/progress changes must remain BP3 and should not change pinned hashes.

## New events

- `phase:situation_intent_classified`
- `phase:exit_contract_created`
- `phase:conflict_direction_gate_opened`
- `event:combat_event` with `event_kind=situation_intent_classified`
- `event:combat_event` with `event_kind=exit_contract_created`
- `event:combat_event` with `event_kind=conflict_direction_gate_opened`

## Harness direction

v1.1 adds regression cases for:

- Chinese paraphrase combat start (`拔枪还击`)
- Japanese paraphrase combat start
- hide-to-disengage direction gate
- de-escalation exit contract
- retreat/leave closes or exits frame
- stale active frame opens direction gate rather than looping

