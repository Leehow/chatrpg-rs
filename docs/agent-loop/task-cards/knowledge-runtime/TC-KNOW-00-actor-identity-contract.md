# TC-KNOW-00 — Actor Identity Contract

## Objective

Create the durable identity contract needed for knowledge holders, especially NPC holders.

This task unblocks durable `knowledge_edges(holder_kind='npc')` by ensuring every NPC holder can be mapped to a stable actor/entity identity rather than an ad-hoc string from a scene or LLM output.

## Background

`NpcLearnedFact` events may exist before durable NPC knowledge edges are opened. That is acceptable as a fail-closed interim state. This task defines the identity contract needed to make durable NPC knowledge safe.

## Architecture contract

- Do not make NPC identity ruleset/module-specific.
- Do not hardcode Homecoming, Athena, Odessa, CoC, D&D, Triangle, etc. in runtime logic.
- Do not let LLM text alone create a durable actor identity without runtime validation.
- Prefer stable IDs from ModuleEntity / Actor / EntityGraph / existing session actors.
- Unknown NPC identity must fail closed or remain event-only.

## Scope-own candidates

The lead should narrow these based on current code:

- `crates/trpg-model/src/**`
- `crates/trpg-db/src/**`
- `crates/trpg-runtime/src/**`
- relevant tests under touched crates

## Scope-off

- Broad parser rewrites.
- Full NPC mind system.
- No-spoiler plugin rewrite.
- Broad cargo fmt of unrelated files.

## Required behavior

1. Define a durable holder/actor identity type or contract that can represent:
   - `gm`
   - `player_party`
   - `pc:<id>`
   - `npc:<actor_id>`
   - `faction:<id>`
   - future extensible holder kinds.
2. Provide validation rules for NPC holder IDs.
3. Provide a resolver function that maps event actor/entity references to a durable knowledge holder when safe.
4. When unresolved, return a typed unresolved result instead of inventing a holder.
5. Keep existing GM/player_party behavior compatible.

## Acceptance criteria

- NPC holder IDs are stable and validated before use in durable knowledge edges.
- Unknown NPC references do not create durable knowledge holders.
- Existing gm/player_party tests still pass.
- A unit test shows two NPC IDs remain distinct and cannot collide with player_party/gm.
- A unit test shows unresolved/ad-hoc NPC text fails closed.

## Validation matrix

Minimum:

```bash
cargo test -p trpg-model holder
cargo test -p trpg-runtime holder
cargo check -p trpg-model -p trpg-db -p trpg-runtime
```

If DB code changes:

```bash
DATABASE_URL=... cargo test -p trpg-db --test live_semantic_events -- --nocapture
```

## Done when

- Identity type/contract exists.
- NPC durable holder validation exists.
- Unsafe NPC IDs fail closed.
- Tests cover gm/player_party/npc/faction-like extensibility or equivalent.
- Handoff explains how this unblocks `TC-KNOW-04`.
