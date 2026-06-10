# v1.10.2 Referee Combat Slice

This version restores the intended v1.10.2 scope. The player-supplied-value referee is retained as a sub-policy, but it is not the whole release.

## Product target

Homecoming's opening firefight must be playable as a referee-led mechanical loop, not just as narration:

1. declare attack;
2. resolve hit/miss;
3. request or roll damage;
4. apply damage to a persistent mechanical ledger;
5. remember the target's current HP on the next turn;
6. allow natural-language withdrawal or movement without forcing a stale direction menu.

## New runtime concepts

- `MechanicalLedger`: authoritative BP3 combat state for actor HP, armor, conditions, morale, and recent damage.
- `ActorMechanicalState`: persistent per-session state for HP and combat condition.
- `AttackResolutionContract`: links source actor, target, source object/ability, attack check, and follow-up damage.
- `DamagePacket`: normalized damage result and HP delta.
- `CombatRoundAction`: typed in-frame action such as `continue_pressure`, `attack`, `withdraw`, `reload`, `use_object`.

## Rules-first player values

Player-supplied values are claims, not automatic truth. If a player reports a damage total, the referee may use the total after validating that it is plausible for the active damage expression. If a player reports a weapon damage expression or DV, the GM should verify it against rules, source packs, object definitions, or a table override before relying on it.

## Boundaries

This release does not implement full armor/SP/AC/SAN/Harm/Chaos validation. That belongs in the later Damage / Condition / Resource Patch Validator. v1.10.2 only ensures that mechanical outcomes become persistent state instead of being lost between narration turns.
