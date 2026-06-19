---
id: TC-NPC-04-npc-behavior-plan-v1
title: NPC Behavior Plan v1 — structured action/speech guidance
mode: implementation
owner: claude-worker
priority: P1
risk: high
expected_backend: tty
subagent_policy: research_only
observability: full
repair_budget: 3
---

# TC-NPC-04 — NPC Behavior Plan v1

## Objective

Create a structured NpcBehaviorPlan that constrains NPC action and speech before GM narration renders it.

## Non-goals

- Do not fully automate social encounters.
- Do not override player agency.
- Do not allow behavior plan to commit state directly.

## Scope owned

- NpcBehaviorPlan type/helper.
- Deterministic derivation from NpcMindView where possible.
- Tests for hostile/fearful/trusting behaviors.

## Suggested output

```text
npc_id
stance
interaction_desire
willingness_to_help
willingness_to_lie
willingness_to_fight
willingness_to_reveal_secret
risk_tolerance
current_goal
emotional_state
facts_knows
facts_can_reveal
facts_will_withhold
preferred_actions
forbidden_actions
speech_style_prompt
dialogue_guidance
source_event_ids
```

## Acceptance criteria

- Hostility lowers help willingness.
- Fear lowers reveal willingness unless fear makes compliance plausible.
- Trust raises cooperation.
- NPC cannot reveal a fact it does not know.
- NPC can know a fact but withhold it based on secrets/relationship.
- Output is proposal/guidance, not committed state.

## Required tests

- `hostile_npc_has_lower_help_willingness`
- `fearful_npc_has_lower_reveal_willingness`
- `trusting_npc_has_higher_cooperation`
- `npc_cannot_reveal_unknown_fact`
- `npc_can_withhold_known_secret`

## Done when

GM narration can consume behavior plan to produce stable NPC speech/action without personality drift or omniscience.
