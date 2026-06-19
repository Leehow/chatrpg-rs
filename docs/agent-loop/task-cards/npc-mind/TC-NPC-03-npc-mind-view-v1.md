---
id: TC-NPC-03-npc-mind-view-v1
title: NPC Mind View v1 — knowledge + persona + relationship projection
mode: implementation
owner: claude-worker
priority: P1
risk: high
expected_backend: tty
subagent_policy: research_only
observability: full
repair_budget: 3
---

# TC-NPC-03 — NPC Mind View v1

## Objective

Build a projected view for a specific NPC that includes only that NPC's knowledge/beliefs plus persona and relationship state.

## Non-goals

- Do not make NPC omniscient.
- Do not expose GM truth to NPC speech prompts.
- Do not implement final dialogue generation.

## Scope owned

- NpcMindView type/helper.
- KnowledgeEdge integration.
- NpcProfile integration.
- NpcRelationship integration.
- Tests.

## Acceptance criteria

- NPC mind view includes facts known/believed by that NPC.
- NPC mind view excludes facts unknown to that NPC.
- False beliefs are marked as beliefs, not truth.
- Persona and relationship are included in compact form.
- Prompt-safe NPC speech context can be produced from this view.

## Required tests

- `npc_mind_view_excludes_unknown_fact`
- `npc_mind_view_includes_false_belief_as_belief`
- `npc_speech_prompt_uses_npc_knowledge_not_gm_truth`
- `npc_mind_view_includes_persona_and_relationship`

## Done when

A GM/NPC speech prompt can be built from NpcMindView without giving the NPC facts it should not know.
