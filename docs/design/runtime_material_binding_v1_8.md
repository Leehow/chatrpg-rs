# v1.8 Runtime Material Binding & Parameter Hydration

## Problem

Lazy loading works, but retrieved rules/module materials were not being committed back into runtime state. Actors and objects could appear in narration without mechanical parameters. A disarmed weapon could be represented by a generic placeholder, not by the weapon described in the fiction or by a ruleset weapon definition.

## Goals

- Keep lazy loading: do not pre-extract every stat block, item, weapon, armor, spell, vehicle, or anomaly object.
- Hydrate actor parameters when an actor appears.
- Hydrate object parameters when an object is used or appears.
- Treat CharacterTemplate as stable ruleset material, not a per-turn guess.
- Keep exact actor/object state in BP3 dynamic context.
- Prevent object routing from stealing hypothetical, analytical, or GM-secret inputs.

## Runtime flow

```text
player input
  → TurnOrchestrator
  → active gate relevance
  → frame route
  → actor parameter hydration
  → object parameter hydration when object interaction occurs
  → ContextBuilder includes runtime actor/object graphs in BP3
  → check/effect/object patches commit state
```

## Added services

- `trpg-params::RuntimeParameterService`
- `runtime_actor_parameters`
- `material_hydration_events`

## Hydration policy

Actors:
- PC/current actor is hydrated every turn from CharacterTemplate + ruleset defaults.
- Opposition/NPC actors are hydrated when an active frame exists or when the input mentions an NPC/opposition.
- Exact module NPC stat cards can later replace seeded values.

Objects:
- Object ids are session scoped.
- A weapon seed uses the current narration/input and ruleset to choose a definition.
- Cyberpunk sidearms now seed a Heavy Pistol/Medium Pistol/Very Heavy Pistol profile when the text supports it.
- Mechanical profile includes `source_query` for later exact retrieval.

## Cache policy

- CharacterTemplate remains BP1/BP2 ruleset material.
- Runtime actor parameters are BP3.
- Runtime object graph remains BP3.
- Hydration events are audit material and not necessarily prompt material.
