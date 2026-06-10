# v1.13 Parameter Facet Executor

v1.13 completes the current LEGO-style mechanics path without creating ruleset-specific engines. The executor consumes the existing `Actor`, `Object`, `Ability`, `Check`, `Contest`, and `Effect` parameter facets written by v1.12 Mechanics Search Skills, then applies them through the unified roll/effect pipeline.

## Goal

The goal is not to write `CyberpunkDamageEngine`, `DndSpellEngine`, `SwordWorldPowerTableEngine`, `CocSanityEngine`, or `TriangleChaosEngine`. Instead, v1.13 makes existing parameter facets executable:

```text
SearchSkill → ParameterFacetBinding → RollPlan/DiceTool → EffectResolutionPacket → ParameterImpact → MechanicalLedger
```

## Runtime behavior

The executor selects facets in this order:

1. Bound `parameter_facet_bindings` for the current session and target.
2. Ruleset starter mechanical profile.
3. Provisional fallback with explicit audit reason.

It supports these impact targets:

- `actor.hp.current`
- `actor.resources.sanity.current`
- `actor.resources.harm.current`
- `scene.tracks.chaos.current`
- `actor.conditions.*`
- `object.*` generic parameter state
- `anomaly.*` generic parameter state
- `scene/clock/world` generic parameter state

## Storage

New tables:

```text
parameter_facet_execution_runs
generic_parameter_states
ruleset_mechanical_profiles
```

`actor_mechanical_states` remains the authority for actor HP/resources/conditions. `generic_parameter_states` covers non-actor targets such as scenes, clocks, objects, anomalies, and campaign tracks.

## Cross-system mapping

- Cyberpunk RED: weapon damage and armor/SP are object/actor facets; HP is an actor impact.
- D&D 5e: AC/save/spell DC are actor/ability/check facets; spell effects are ability effect facets.
- Sword World 2.5: power tables, evasion, resistance, and armor are facets rather than a hardcoded SW engine.
- BRP/CoC: abilities are skill/passions/ratings, with HP, SAN, and Major Wound represented as resource/condition facets.
- Triangle Agency: Harm, Chaos, anomaly effects, and playwalled visibility are resource/scene/anomaly/visibility facets.

## Invariant

An effect may not be considered applied unless at least one `ParameterImpact` or `parameter_facet_execution_run` exists. `runtime_writeback_applied` must correspond to actual DB state change or a recorded no-op reason.
