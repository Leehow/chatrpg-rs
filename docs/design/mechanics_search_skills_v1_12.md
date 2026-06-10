# v1.12 Mechanics Search Skills & Parameter Binding

## Purpose

v1.12 adds a retrieval-planning layer between generic GrepSearch/Tantivy candidate recall and the existing actor/object/ability/check/effect parameter systems. It does not introduce a separate rules engine. It extends the current parameter system with mechanical facets and multi-step search strategies.

## Core rule

Keywords may help retrieve candidates. They must not make the final business decision.

```text
MechanicalDemand
  → SearchSkillProfile
  → multi-step MechanicsQueryPlan
  → GrepSearch/Tantivy candidates
  → semantic extraction
  → RuleBindingPacket
  → ParameterFacetBinding
  → actor/object/ability/check/effect runtime parameters
```

## Search skills

The initial skill set is demand-oriented, not ruleset-oriented:

- `combat_resolution_search`
- `weapon_parameter_search`
- `armor_defense_search`
- `ability_activation_search`
- `condition_resource_search`
- `npc_statblock_search`
- `module_card_search`
- `scene_object_search`

A Cyberpunk RED gunfight, a D&D attack roll, a Sword World resisted spell, a BRP firearm check, and a Triangle Agency anomaly consequence all go through the same planning shape. The ruleset locator and alias pack change the queries; the runtime writeback target remains the shared parameter system.

## Multi-step planning

Each mechanics query plan can include:

1. Locator search: find relevant rulebook/module regions.
2. Exact entity search: find the named weapon, spell, NPC, vehicle, condition, or scene object.
3. Field search: retrieve requested fields such as damage, range, AC, DV, SP, SAN, Harm, Chaos, trigger, cost.
4. Procedure search: find the resolution process, not merely a numeric value.
5. Module-card search: prefer module-local NPC cards, object cards, vehicle cards, encounter cards, and scene objects.
6. Broad fallback: use OR/should-style recall when exact field queries are too narrow.
7. Semantic rerank/extraction.
8. Runtime writeback.

## Parameter facets

Search results write back as facets on existing systems:

- Actor facets: check, defense, resource, condition.
- Object facets: attack, damage, armor, durability.
- Ability facets: activation, trigger, cost, effect.
- Check/contest facets: resolution bindings.
- Effect facets: damage/resource/condition bindings.
- Visibility facets: GM-only and playwalled constraints.

## Cache policy

Recent search plans and parameter facets are BP3 dynamic context. Verified stable object/ability definitions may later graduate to BP2, but per-turn demand, candidate evidence, and facet writebacks remain dynamic.
