---
id: TC-NPC-01-npc-profile-v1
title: NPC Profile v1 — static persona and speech style
mode: implementation
owner: claude-worker
priority: P1
risk: medium
expected_backend: tty
subagent_policy: research_only
observability: full
repair_budget: 3
---

# TC-NPC-01 — NPC Profile v1

## Objective

Create a structured NPC profile model that can represent static persona, goals, fears, secrets, and speech style.

## Non-goals

- Do not implement full relationship dynamics.
- Do not generate NPC profiles from every module automatically in this task.
- Do not allow player-facing prompts to include GM-only secrets.

## Scope owned

- NpcProfile model/storage or equivalent existing model extension.
- Persona/speech-style projection helpers.
- Tests for serialization and safe prompt view.

## Scope off

- Combat AI rewrite.
- Full NPC behavior planner.
- Full module parser NPC extraction rewrite.

## Suggested fields

```text
actor_id
name
role
archetype
personality_traits
values
drives
fears
goals
secrets
speech_style
behavioral_boundaries
source_refs
```

Speech style may include:

```text
formality
directness
sentence_length
emotionality
slang_level
humor
threat_style
taboo_topics
catchphrases
```

## Acceptance criteria

- Profile round-trips through storage/serde.
- Prompt-safe view hides GM-only secrets.
- Speech style can be compiled into a small prompt block or behavior hint.
- Source refs are preserved when available.

## Required tests

- `npc_profile_roundtrip`
- `npc_profile_safe_view_hides_secret`
- `npc_speech_style_projection_contains_expected_traits`

## Done when

NPC profile can be loaded independently of combat and safely used by GM narration.
