# chatrpg-rs Validation Rubric — Journey-Driven Edition

The generic V0-V4 vocabulary still applies, but V3/V5 evidence is valid only after the Journey Qualification Gate passes.

## Validation levels

| Level | Name | Purpose | Evidence |
|---:|---|---|---|
| V0 | Static architecture guard | Detect architecture violations | grep, boundary scan, no-hardcode, no direct LLM/plugin commit |
| V1 | Component logic | Prove a pure model/projection/calculation | unit/property/fixture test |
| V2 | Contract/storage/build | Prove serialization, DB, CLI/API contract, compile surface | cargo check/test, migration roundtrip, schema/JSONL contract |
| V3 | Deterministic black-box journey | Prove the feature through the real public product path | clean setup, character creation, session, human-like inputs, real turn pipeline, events/state/output |
| V4 | Adversarial journey | Prove fail-closed behavior under leaks, false beliefs, retries, invalid state, provider failure | black-box negative journey and state inspection |
| V5 | Live-model journey | Prove the real GM model and human-like player can naturally reach and use the feature | actual LLM, isolated player simulator, public interface, adaptive branches, semantic evaluator |
| V6 | Accepted replay cassette | Make a reviewed live journey reproducible in CI | captured provider/tool/dice cassette replayed through the same scenario and product path |

V1/V2 are necessary engineering checks. They are never sufficient for a player-facing design claim.

## Journey Qualification Gate

A V3/V4/V5/V6 run is invalid unless all applicable gates pass.

| Gate | Requirement | Invalid when |
|---|---|---|
| Q0 Environment | isolated DB/schema, expected assets available, current binary/API starts | stale/shared state or missing source assets |
| Q1 Character | character created through public character-creation path, persisted, complete enough for play, bound to session/actor | turn begins with no real player character |
| Q2 Session | session started through public path and current scene is playable | test injects an arbitrary session or starts in no scene |
| Q3 Human input | player input contains no internal event names, DB instructions, debug mutation, or GM-only knowledge | test-engineering prose drives behavior |
| Q4 Trigger reached | intended capability is actually triggered by player action | feature path never runs |
| Q5 Mechanism exercised | required internal mechanism emits evidence | expected check has no DiceRolled/CheckResolved; expected reveal has no knowledge event |
| Q6 Outcome observed | player-visible behavior and state/projection reflect the mechanism | only a DB row or struct exists |
| Q7 Continuity | when memory/persistence is claimed, a later turn or reload consumes the committed result | state was written but never used later |

## Result states

Use these result states for journey checkpoints and acceptance rows:

- `INVALID_SETUP`: Q0-Q3 failed. The test says nothing about the feature.
- `NOT_TRIGGERED`: natural player actions did not reach the intended path. This is not Pass.
- `TRIGGERED_NO_MECHANISM`: intent reached the area, but required tool/event/dice/state path did not execute.
- `MECHANISM_NO_USER_EFFECT`: internals ran but player-facing or next-turn behavior did not change.
- `PASS`: all required gates and assertions passed.
- `FAIL`: a required assertion was violated.
- `FLAKY`: repeated live runs disagree beyond approved tolerance.
- `BLOCKED`: external dependency prevented execution.

## Non-negotiable examples

- No character was created: `INVALID_SETUP`, never Pass.
- A technical action that should require a check produced no pending/GM roll, no `DiceRolled`, and no `CheckResolved`: `NOT_TRIGGERED` or `TRIGGERED_NO_MECHANISM`.
- `KnowledgeEdge` unit tests pass but no real turn reads the projection: design row remains Partial.
- NPC relationship fields change in DB but the NPC's later dialogue/behavior is unchanged: `MECHANISM_NO_USER_EFFECT`.
- A no-spoiler test directly inserts `PlayerLearnedFact`: invalid for reveal-flow acceptance. The reveal must be caused by a player action in the journey.
- Debug directives or direct DB mutation may be used for narrow V1/V4 fault injection, never as V3/V5 proof of normal gameplay.
