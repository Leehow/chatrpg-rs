# Human Player Simulator Protocol

## Isolation

Use three independent roles and contexts:

```text
GM under test
  Sees runtime-approved GM/player context according to product behavior.

Player simulator
  Sees only player-visible narration/events, its own character sheet, public choices, and prior player transcript.
  Never receives module text, GMTruthView, verifier secrets, fact IDs, expected hidden answer, or DB state.

Evaluator
  May see the scenario contract, trace, state diff, and hidden truth needed to judge leaks.
  Never supplies actions to the GM/player during the run.
```

Do not use one shared LLM conversation for all roles.

## Player modes

### Adaptive scripted player

Preferred for deterministic V3/V4 and CI.

The script is a state machine keyed by observed public events and semantic output conditions, not fixed turn numbers. It may branch on:

- character creation prompt/complete;
- scene introduction received;
- `AwaitingPlayerRoll` or player-visible roll request;
- NPC response category (helpful, evasive, hostile, asks question);
- check success/failure public consequence;
- reveal/non-reveal state visible to the player.

### Live LLM player

Required for V5 on important journeys.

Give the player model:

- a short player persona;
- current character sheet/player view;
- campaign goal;
- instruction to act naturally and avoid test jargon;
- no secrets or expected solution.

It chooses one action at a time after reading the actual GM response.

## Character creation is part of the journey

Before the first play turn, the runner must call the actual character-creation CLI/API. It must prove:

- a character was created and persisted;
- the character is bound to the session/actor;
- required stats/skills/resources exist;
- subsequent turns use that character.

For a feature not specifically about interactive character creation, `create-character --auto` through the public CLI is acceptable. Directly inserting a character row is not valid journey setup.

## Dice behavior

When the product requests a player roll:

1. Read the public pending-check/roll prompt.
2. Read the player's own character sheet through the public/player surface.
3. Roll through the product's public dice surface or configured dice adapter.
4. Send a natural-language player response containing the actual result.
5. Require `DiceRolled` and `CheckResolved` evidence.

Never invent TECH/skill values that do not belong to the created character.

Deterministic mode may seed the dice adapter. Live mode should accept success/failure branches rather than forcing a result. If a reveal requires success, the scenario must define a legitimate alternative branch after failure, not mutate the roll.

## No hard-pushing

Forbidden in normal V3/V5 journeys:

- `[debug]` state mutation;
- direct DB insert/update of the target fact/relationship/check;
- player text such as “emit PlayerLearnedFact” or “create a pending_check”;
- skipping character creation/session start;
- injecting GM-only knowledge into the player action;
- continuing scripted actions after the observed state no longer matches their precondition.

## Human-likeness checks

Reject player input that contains:

- internal event/phase/tool/type names;
- JSON/SQL/code;
- exact hidden names not previously exposed;
- assertions about expected test behavior;
- arbitrary stats not present on the character sheet.
