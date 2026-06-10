# Product Design: Semantic Situation Orchestrator v1.1

## What it is

Semantic Situation Orchestrator is the product layer that prevents the LLM GM from getting trapped in combat, negotiation, investigation, infiltration, and side-quest loops. It treats every local conflict as a temporary situation with goals, pressure, exits, NPC patience, and compaction rules.

## Why it exists

Human players do not speak in fixed keywords. They say things like:

- “我先缩回去。”
- “我不跟它硬刚了。”
- “我把枪压低，试着让对面冷静。”
- “先撤，之后再说。”

A keyword-driven GM misses these intents and traps the session inside a frame. Human NPCs also do not repeat the same tactic forever. They get impatient, change strategy, flee, negotiate, surrender, or escalate.

## Design philosophy

1. A frame is not a mode lock.
2. Every active frame must have exits.
3. NPCs need drive, patience, and morale.
4. No repeated tactic can continue forever without progress.
5. Stalemate must open choices, not generate more generic narration.
6. Frame state is temporary; only durable consequences survive compaction.

## User experience

If a player says “我躲到集装箱后面，先不打了,” the GM should not keep asking for ordinary combat actions. The system should recognize hide/disengage and ask whether the player wants to keep hiding, escape, negotiate, or push the objective.

If a player says “举手和解，别再打了,” the system should create an exit/de-escalation path and either compact the frame as a truce or transition into a social frame.

If a player repeats the same action without progress, the GM should say the situation is stuck and present meaningful directions.

## Architecture

- Rust `CombatAgent` remains the owner of control flow.
- `ConflictIntent` routes player input against the active frame.
- `ExitContract` represents retreat, surrender, de-escalation, and leave-scene actions.
- `StalemateContract` represents a detected loop and opens an InteractionGate.
- `FrameProgressTracker` records progress, repeated actions, and decisive change.
- `NpcDriveState` records patience, morale, aggression, and tactic repetition.
- `FrameCompaction` preserves only durable consequences after the situation ends.

## Success metrics

- Combat start should work with paraphrases and multiple languages.
- Retreat, hide, surrender, and de-escalation should not be swallowed as generic combat actions.
- A frame should never continue indefinitely without decisive progress or a direction gate.
- NPCs should change tactics when patience/morale is exhausted.
- Round-by-round transient details should not enter long-term memory.

