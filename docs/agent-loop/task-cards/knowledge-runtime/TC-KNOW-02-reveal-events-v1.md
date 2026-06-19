---
id: TC-KNOW-02-reveal-events-v1
title: Reveal Events v1 — ContextSurfaced is not PlayerLearnedFact
mode: implementation
owner: claude-worker
priority: P0
risk: medium
expected_backend: tty
subagent_policy: research_only
observability: full
repair_budget: 3
---

# TC-KNOW-02 — Reveal Events v1

## Objective

Separate internal context loading from player-facing knowledge reveal.

## Required event concepts

Implement or map to existing event/state equivalents:

```text
ContextSurfaced
  The system/GM prompt loaded an entity, fact, scene, or block.
  This does not grant player knowledge.

PlayerExposed
  The player-facing narration exposed an entity or perceptible fact.
  This may grant perceived/heard_about knowledge, not hidden truth.

PlayerLearnedFact
  The player party learned a specific fact.

NpcLearnedFact
  A specific NPC learned a specific fact.
```

## Non-goals

- Do not fully rewrite event sourcing.
- Do not make every context block a reveal.
- Do not automatically reveal synopsis/backstory.

## Scope owned

- Domain event types or compatibility wrappers.
- KnowledgeEdge projection updates for PlayerLearnedFact and NpcLearnedFact.
- Tests proving ContextSurfaced does not reveal.

## Scope off

- Full LLM extraction of reveal events.
- Full UI reveal timeline.

## Acceptance criteria

- ContextSurfaced does not update player_party KnowledgeEdge.
- PlayerExposed can mark an entity as perceived without revealing its hidden identity.
- PlayerLearnedFact updates player_party knowledge.
- NpcLearnedFact updates only the target NPC's knowledge.

## Required tests

- `context_surfaced_does_not_grant_player_knowledge`
- `player_exposed_entity_does_not_reveal_secret_fact`
- `player_learned_fact_updates_party_knowledge`
- `npc_learned_fact_updates_one_npc_only`

## Done when

Reveal semantics are explicit enough for spoiler guard and NPC mind to consume.
