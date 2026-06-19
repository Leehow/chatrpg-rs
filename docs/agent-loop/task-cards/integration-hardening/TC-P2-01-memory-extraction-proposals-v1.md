---
id: TC-P2-01-memory-extraction-proposals-v1
title: Memory Extraction Proposals v1 — facts, knowledge, and relationships
mode: implementation
owner: claude-worker
priority: P2
risk: high
expected_backend: tty
subagent_policy: research_only
observability: full
repair_budget: 3
---

# TC-P2-01 — Memory Extraction Proposals v1

## Objective

Add a proposal-only extraction layer that can represent candidate world facts, holder knowledge updates, and NPC relationship deltas from turn evidence without committing state directly.

## Why this matters

P0/P1 created durable fact/holder separation, reveal semantics, NPC knowledge, and NPC relationship state. The next hardening layer needs a safe extraction surface so later background memory jobs can propose updates while runtime-owned commit paths still decide what becomes domain events, knowledge edges, relationship rows, or memory facts.

## Non-goals

- Do not wire proposals into the GM turn loop.
- Do not write DB rows, append domain events, or mutate relationship state.
- Do not require a real LLM provider for tests.
- Do not redesign `KnowledgeEdge`, `NpcRelationship`, or `MemoryFact`.
- Do not extract new module entities from raw source text.

## Scope owned

- `crates/trpg-model/src/*memory*proposal*.rs`
- `crates/trpg-model/src/lib.rs`
- `crates/trpg-model/tests/*memory*proposal*.rs`
- `crates/trpg-runtime/src/*memory*proposal*.rs`
- `crates/trpg-runtime/src/lib.rs`
- `crates/trpg-runtime/src/relationship_extraction.rs` only if a tiny adapter or reuse hook is needed
- `crates/trpg-runtime/tests/*memory*proposal*.rs`

## Scope off

- Migrations and `crates/trpg-db/**`
- GM turn loop / streaming / plugin host wiring
- NoSpoiler plugin behavior
- Existing P0/P1 model semantics except additive imports/helpers
- Broad `cargo fmt`
- Generated artifacts and unrelated dirty files

## Architecture constraints

- Agents and extractors propose; runtime-owned commit paths commit.
- A world fact and who knows that fact must remain separate.
- Holder identities must use the TC-KNOW-00 actor identity contract and fail closed on unstable NPC/PC/faction ids.
- NPC relationship deltas must preserve the TC-NPC-02 evidence-required gate.
- Proposals must preserve source/evidence references and be deterministic in tests.
- Player-facing narration must not receive proposal contents that include player-unknown secrets.

## Implementation guidance

Prefer a compact model enum/struct family such as `MemoryExtractionProposal`, with variants for:

- `WorldFactCandidate` / fact definition candidate;
- `KnowledgeUpdateCandidate` / holder learns or believes fact;
- `NpcRelationshipDeltaCandidate` / bounded evidence-backed relationship change;
- optional `MemoryFactCandidate` adapter for the existing relationship triple path.

The runtime helper may parse deterministic JSON-like proposal payloads or map existing relationship triples into proposals. Keep the layer pure and validation-heavy: reject missing evidence, unstable holders, unknown relationship targets, and unsupported proposal kinds.

## Acceptance criteria

- Fact identity and holder knowledge are represented as separate proposal concepts.
- NPC knowledge proposals require stable NPC holder ids and do not create durable edges.
- Relationship proposals require evidence/source refs and do not mutate `NpcRelationship`.
- Existing relationship triple extraction can be represented or bridged without changing its current behavior.
- Unsupported or malformed proposal payloads fail closed with a typed error or empty proposal list.
- Proposals preserve `source_event_ids` / `source_refs` needed for later commit review.
- Public API is additive and serde round-trippable where relevant.

## Required tests

- `memory_proposal_separates_world_fact_from_holder_knowledge`
- `memory_proposal_rejects_unstable_npc_holder`
- `memory_proposal_relationship_requires_evidence`
- `memory_proposal_does_not_commit_state`
- `memory_proposal_preserves_source_refs`
- `relationship_triple_can_be_mapped_to_memory_proposal`

## Validation commands

```bash
cargo test -p trpg-model memory_proposal
cargo test -p trpg-runtime memory_proposal
cargo test -p trpg-runtime relationship_extraction
cargo check -p trpg-model -p trpg-db -p trpg-runtime -p trpg-gm
git diff --check
```

## Escalation triggers

- The implementation requires a destructive migration or immediate durable write path.
- The only viable design would make memory/transcript state authoritative over domain events/projections.
- A public contract must break instead of using an additive adapter.
- Existing relationship extraction behavior must be rewritten broadly.

## Done when

There is a tested proposal-only layer for facts, knowledge, and relationships, with no state commits and with clear handoff to future commit/review tasks.

## Handoff requirements

Include assumptions, files changed, proposal schema examples, validation output, repair loops, scope ledger, and an acceptance-criteria ledger.
