# TC-KNOW-04 — Durable NPC KnowledgeEdges

## Objective

Enable durable `knowledge_edges` for NPC holders after actor identity is stable.

`NpcLearnedFact` should no longer be event-only when the NPC holder can be resolved to a durable validated actor ID.

## Dependencies

- `TC-KNOW-00` actor identity contract.
- `TC-KNOW-01` KnowledgeEdge v1.
- `TC-KNOW-02` reveal event split.

## Architecture contract

- NPC durable knowledge must use validated holder identity.
- Player knowledge must not be granted when an NPC learns a fact.
- NPC knowledge must not be granted to all NPCs.
- GM truth and NPC beliefs must remain separate.
- Unknown/ad-hoc NPC identity remains event-only or unresolved, not durable.

## Scope-own candidates

The lead should narrow these based on current code:

- `crates/trpg-model/src/**`
- `crates/trpg-db/src/**`
- `crates/trpg-runtime/src/truthgraph.rs`
- relevant tests under touched crates

## Scope-off

- NPC profile/relationship system.
- No-spoiler plugin rewrite, except projection helpers required by this task.
- Broad parser changes.
- Broad cargo fmt of unrelated files.

## Required behavior

1. Allow `knowledge_edges(holder_kind='npc')` or equivalent typed holder storage.
2. Persist `NpcLearnedFact` into durable NPC-specific knowledge when holder identity resolves.
3. Preserve event ledger even when durable write is unresolved.
4. Add per-NPC projection query that returns only that NPC's known/believed facts.
5. Keep player_party projection unchanged when NPC learns a fact.
6. Add idempotent upsert behavior for repeated `NpcLearnedFact` events.

## Acceptance criteria

- `npc_a` can know a fact while `npc_b` does not.
- `npc_a` can believe a false/rumor fact without making it GM truth.
- `player_party` does not learn a fact merely because an NPC learned it.
- Replaying or re-running the same event does not duplicate knowledge.
- Unknown NPC identity fails closed without durable write.

## Validation matrix

Minimum:

```bash
cargo test -p trpg-model knowledge
cargo test -p trpg-runtime --lib truthgraph
DATABASE_URL=... cargo test -p trpg-db --test live_semantic_events -- --nocapture
cargo check -p trpg-model -p trpg-db -p trpg-runtime -p trpg-gm
```

If GM tools are touched:

```bash
DATABASE_URL=... cargo test -p trpg-gm --test live_reveal_fact_tool -- --nocapture
```

## Done when

- Durable NPC KnowledgeEdges exist for resolved NPC holders.
- Per-NPC projection test passes.
- Player projection remains isolated.
- Handoff states how this unblocks `TC-NPC-03`.
