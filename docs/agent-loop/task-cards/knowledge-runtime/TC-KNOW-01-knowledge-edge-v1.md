---
id: TC-KNOW-01-knowledge-edge-v1
title: KnowledgeEdge v1 — separate facts from who knows them
mode: implementation
owner: claude-worker
priority: P0
risk: medium
expected_backend: tty
subagent_policy: research_only
observability: full
repair_budget: 3
---

# TC-KNOW-01 — KnowledgeEdge v1

## Objective

Implement the first foundation of Knowledge Runtime: world facts must be separate from who knows, suspects, believes, or is misinformed about those facts.

## Why this matters

Spoiler prevention, player knowledge, NPC knowledge, NPC speech, and NPC behavior should all rely on the same knowledge ledger.

## Non-goals

- Do not build a graph database.
- Do not rewrite all memory systems.
- Do not make LLM summaries authoritative state.
- Do not grant player knowledge just because a fact was loaded into context.

## Scope owned

- Models/types for KnowledgeEdge and holder/view enums.
- Persistence layer for inserting/upserting/listing knowledge edges.
- Projection functions for GM, player party, and NPC views.
- Focused tests.

## Scope off

- Full spoiler verifier implementation.
- Full NPC behavior planner.
- Broad memory extractor rewrite.
- UI.

## Architecture constraints

- Fact truth and holder knowledge are distinct.
- GM truth does not automatically enter player view.
- NPC knowledge is holder-specific.
- False beliefs are represented as beliefs, not world truth.
- Every knowledge edge should carry evidence/source event or a clearly nullable compatibility field.

## Implementation guidance

Create or adapt structures equivalent to:

```text
KnowledgeEdge
  session_id
  holder_kind: gm | player_party | pc | npc | faction | system
  holder_id
  fact_id
  knowledge_state: unknown | perceived | heard_about | suspects | believes_true | believes_false | knows_true | misinformed | withheld | forgotten
  confidence
  learned_at_turn_id
  source_event_id
  disclosure_policy
  updated_at
```

Expose projection APIs equivalent to:

```text
gm_truth_view(session_id)
player_knowledge_view(session_id)
npc_knowledge_view(session_id, npc_id)
```

## Acceptance criteria

- Same fact can be known by GM and NPC A, unknown by player party, and falsely believed by NPC B.
- Player projection hides unknown facts.
- NPC projection is holder-specific.
- Reveal/upsert to player party does not grant knowledge to all NPCs.
- Existing memory/fact code remains compatible.

## Required tests

- `knowledge_edge_roundtrip`
- `player_projection_hides_unknown_fact`
- `npc_projection_is_holder_specific`
- `false_belief_not_world_truth`
- `player_reveal_updates_player_party_only`

## Validation commands

Use the current repo's smallest meaningful backend test command. If no central runner exists, add a focused standalone test script under the existing debug test pattern and run it directly.

## Escalation triggers

- Existing schema cannot support fact IDs or event IDs without destructive migration.
- Two existing memory systems conflict on ownership.
- Public API shape would break.

## Done when

All acceptance criteria are implemented, tested, and documented in worker handoff.
