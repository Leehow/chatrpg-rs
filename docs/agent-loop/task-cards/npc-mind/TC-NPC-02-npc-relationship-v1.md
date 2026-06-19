---
id: TC-NPC-02-npc-relationship-v1
title: NPC Relationship v1 — dynamic attitude toward player/PC/NPC/faction
mode: implementation
owner: claude-worker
priority: P1
risk: medium
expected_backend: tty
subagent_policy: research_only
observability: full
repair_budget: 3
---

# TC-NPC-02 — NPC Relationship v1

## Objective

Create a structured relationship state for NPC attitudes and interaction desire.

## Non-goals

- Do not make the LLM directly set final relationship values without runtime validation.
- Do not implement full social combat.
- Do not rewrite combat morale.

## Scope owned

- NpcRelationship model/storage or equivalent extension.
- Bounded delta application helper.
- Evidence event requirement.
- Tests.

## Suggested fields

```text
session_id
npc_id
target_kind: player_party | pc | npc | faction
target_id
trust
respect
fear
affection
suspicion
hostility
debt
leverage
stance
interaction_desire
talkativeness
last_interaction_turn_id
evidence_event_ids
```

## Acceptance criteria

- Relationship values are bounded.
- Deltas require evidence_event_ids.
- Helping an NPC can increase trust/respect/debt.
- Threatening an NPC can increase fear/suspicion and reduce trust.
- Stance and interaction_desire can be derived or updated consistently.

## Required tests

- `relationship_delta_is_bounded`
- `relationship_delta_requires_evidence_event`
- `help_increases_trust_or_debt`
- `threat_decreases_trust_and_increases_fear`

## Done when

Runtime can store and update an NPC's attitude toward the player without relying on free-form memory summary.
