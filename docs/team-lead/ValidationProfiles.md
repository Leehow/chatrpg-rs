# Validation Profiles

## trpg-model-contract

```bash
cargo check -p trpg-model
cargo test -p trpg-model
cargo check -p trpg-db -p trpg-runtime -p trpg-gm -p trpg-api -p trpg-cli -p trpg-parser -p trpg-material
```

## model-plus-downstream-check

```bash
cargo check -p trpg-model
cargo test -p trpg-model
cargo check -p trpg-db -p trpg-runtime -p trpg-gm -p trpg-api -p trpg-cli -p trpg-parser -p trpg-material
```

## knowledge-db

- model/DB compile;
- repository round-trip against isolated DB;
- holder-specific projection;
- idempotent event replay;
- no player grant from `ContextSurfaced`.

## runtime-knowledge

- PlayerKnowledgeView excludes unknown GM facts;
- NpcMindView is holder-specific;
- context compiler consumes the intended view;
- no direct DB/search bypass around NeedBus/projector contracts.

## journey-prelude

A journey is invalid unless it:

1. creates a character through the public product path;
2. persists and binds the character to a session;
3. enters a playable scene;
4. performs a natural player action;
5. reaches real turn execution.

## no-spoiler-adversarial

Connected chain:

```text
character/session
-> player-unknown fact exists
-> natural observation/action
-> player-facing context/SSE excludes secret
-> reveal-triggering action/check
-> DiceRolled + CheckResolved when mechanical
-> PlayerLearnedFact
-> later turn permits the revealed fact
-> reload preserves the result
```

## npc-connected-journey

Connected chain:

```text
character/session
-> meet NPC naturally
-> baseline speech/stance
-> player help/threaten/trade action
-> evidence-backed relationship delta
-> NpcMindView/NpcBehaviorPlan changes
-> later dialogue visibly changes
-> NPC never uses a fact it does not know
-> reload preserves relationship and knowledge
```
