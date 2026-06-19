---
id: TC-KNOW-00-actor-identity-gate-v1
title: Actor Identity Gate v1 — stable NPC holder identity
mode: implementation
owner: claude-worker
priority: P0
risk: high
expected_backend: tty
subagent_policy: research_only
observability: full
repair_budget: 3
---

# TC-KNOW-00 — Actor Identity Gate v1

## Objective

Define and test the stable actor identity contract required before durable NPC
KnowledgeEdge holders, NpcLearnedFact projections, NpcMindView, or NPC behavior
planning can be considered complete.

## Why this matters

NPC knowledge is only useful if "which NPC knows this" is durable across module
graph assets, runtime scene participants, combat opposition, domain events, and
database projections. Display names, labels, or encounter-local opposition
strings are not stable enough to own knowledge state.

## Non-goals

- Do not implement the full NPC mind system.
- Do not rewrite the entire module parser.
- Do not infer stable ids from display names when the source data is ambiguous.
- Do not make NPCs omniscient to avoid identity gaps.

## Scope owned

- Actor identity model or adapter for NPC-like actors.
- Mapping helpers between module graph NPC ids, runtime actor refs, and event
  or KnowledgeEdge holder ids.
- Fail-closed behavior for ambiguous or missing actor identity.
- Focused tests.

## Scope off

- Full relationship model.
- Full NPC profile extraction.
- Full behavior planner.
- UI.

## Architecture constraints

- Stable actor id must be source-backed or runtime-owned.
- Display name, combat opposition label, or narration text must not be used as
  a durable knowledge holder id.
- Ambiguous actor resolution must fail closed and report evidence.
- Existing player and GM knowledge projections must keep working.

## Acceptance criteria

- A module/source NPC can be mapped to a stable runtime actor id.
- A runtime actor id can be stored as a KnowledgeEdge NPC holder id.
- Ambiguous display-name-only NPC input is rejected or marked unresolved.
- NpcLearnedFact can target a stable actor id without granting knowledge to
  other NPCs.
- Existing player_party reveal behavior is unchanged.

## Required tests

- `actor_identity_maps_source_npc_to_runtime_actor`
- `actor_identity_rejects_display_name_only_holder`
- `npc_learned_fact_requires_stable_actor_id`
- `npc_learned_fact_updates_only_target_actor`
- `player_reveal_behavior_unchanged_by_actor_identity`

## Validation commands

```bash
cargo fmt --check
cargo check -p trpg-model -p trpg-db -p trpg-runtime -p trpg-gm
cargo test -p trpg-model
cargo test -p trpg-db
cargo test -p trpg-runtime --lib
```

## Escalation triggers

- Existing data lacks any stable source id for the target NPC path.
- Implementing the mapping would require a destructive migration.
- Two existing systems claim incompatible actor identity ownership.

## Done when

Durable NPC KnowledgeEdge holders can be keyed by a stable actor id, and
ambiguous NPC identity fails closed rather than silently creating unsafe holder
state.
