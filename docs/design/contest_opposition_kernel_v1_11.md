# v1.11 Contest / Opposition Kernel

## Purpose

v1.10.2 introduced a Referee Combat Slice: attack checks can request damage, damage packets can update HP, and the mechanical ledger is projected into BP3. The next missing layer is contest ownership. A CheckContract cannot remain at `NoMechanicalOpposition` or `UnknownUntilLookup` when the result will affect HP, equipment, resources, conditions, or scene state.

v1.11 adds a Contest/Opposition Kernel between CheckContract and Effect/Damage resolution.

## Design

```text
CheckContract + DiceRoll
  -> ContestProfile
  -> CheckResolutionModel
  -> ContestResolutionRecord
  -> CheckResultRecord.outcome
  -> RefereeCombat / Object / Ability / Effect handling
```

The kernel does not try to implement every ruleset's full combat math. It creates a typed resolution model that downstream systems can consume and audit. Exact rules can replace provisional models as Mechanical Source Packs improve.

## Resolution models

- Static target number.
- Opposed roll.
- Attack vs defense.
- Percentile roll-under.
- Saving throw.
- Ruleset procedure lookup.
- Provisional model with audit reason.

## BP cache policy

Contest profiles and recent contest resolution events are BP3 dynamic context. Stable procedure definitions, once extracted, belong in BP2.

## Cross-ruleset fit

Cyberpunk RED separates statistics, skills, weapons/armor, resolving actions, and combat procedures. D&D separates equipment, ability checks, combat, and spellcasting. Sword World 2.5 separates skill checks, contested checks, combat flow, weapon attacks, damage, magic, and data sections. BRP defines abilities broadly as skills, passions, or other percentile-rated factors. The ContestProfile sits between those materials and the final effect patch.
