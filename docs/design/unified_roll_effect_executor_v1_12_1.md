# v1.12.1 Unified Roll & Effect Executor

## Goal

v1.12 could find mechanical rules and create contest profiles, but the execution chain was incomplete: `/roll 3d6` was not consistently treated as the one system dice tool in turn mode, and narrative damage could fail to produce a persistent state patch. v1.12.1 connects the chain:

```text
RollPlan → DiceTool → EffectResolutionPacket → ParameterImpact → MechanicalLedger
```

## Key Design Correction

Damage is not always HP damage. A mechanical effect can target any parameter:

- `actor.hp.current`
- `actor.resources.sanity.current`
- `scene.tracks.chaos.current`
- `actor.resources.harm.current`
- `actor.conditions.*`
- `object.durability.current`
- `object.connection_state`
- `scene.clock.*`

`DamagePacket` is now a compatibility specialization for HP impacts. The canonical object is `EffectResolutionPacket`.

## Rule System Fit

- Cyberpunk RED: attacks commonly impact HP, armor/SP, weapon state, and conditions. The RED core table of contents separates statistics, skills, weapons/armor, combat, ranged/melee combat, and damage/armor procedures, matching actor/object/check/effect facets.
- D&D: weapon attacks impact HP; spells may create save rolls and either HP impacts or conditions; AC/save/spell DC are facets rather than separate engines.
- Sword World 2.5: attacks and spells use actor values, object/ability data, resistance/evasion, and table lookups; impacts may be HP or conditions.
- BRP/CoC: abilities are broad 1-100 rated skills/passions; effects can impact HP, SAN, major wounds, conditions, or narrative state.
- Triangle Agency: effects may impact Harm, Chaos, anomaly state, scene state, or playwalled/GM-only information.

## Execution Invariants

1. Every mechanical roll has a `RollPlan`.
2. Every resolved mechanical effect has an `EffectResolutionPacket`.
3. Every effect packet has at least one `ParameterImpact`.
4. HP changes are represented as `ActorHpDelta`, but HP is not assumed as the universal target.
5. System dice are produced through the single dice tool, recorded in `dice_rolls`, `roll_plans`, and `agent_tool_calls`.
6. If an effect target parameter is inferred provisionally, the packet must carry `provisional_reason`.

## Current Scope

This version implements generic roll plans, turn-mode `/roll` and system-roll requests, effect packets, HP compatibility damage packets, and non-HP resource-track impacts for SAN/Chaos/Harm-like effects. It does not yet implement full armor/SP/AC validation, D&D spell engine, Sword World power table executor, or Triangle playwalled executor.
