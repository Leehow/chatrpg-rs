# v1.13.1 Combat Route Source Object Hotfix

This hotfix repairs the v1.13 regression where an attack sentence that names a weapon (for example `用重型手枪开火`) was routed to the Object Kernel instead of the Combat/Conflict reducer.

## Invariant

A weapon named in an attack is a **source object**, not an object-interaction turn.

- `我用重型手枪打它` => combat route, weapon materialization may run as supporting material.
- `我缴它的械` / `我抢他手里的枪` => object route.

## Changes

1. The semantic ObjectDefinition materialization merge no longer promotes a turn to `object_interaction_first` when Rust has already classified it as attack, enemy-initiated conflict, or terminal/exit.
2. `route_for` gives frame-worthy actions priority over object materialization evidence.
3. Player-initiated frame-opening attacks now also commit a `CheckContract` and `EffectContract` in the same turn. Previously the frame could open without a locked mechanical check, leaving `/roll` with nothing to bind to.

## Expected outcome

A first-turn attack with a named weapon should produce:

- `turn_route` with `start_frame_first`
- `combat_frame_created`
- `check_contract_created`
- `effect_contract_created`
- if `TRPG_AGENT_TABLE_DICE_POLICY=system_rolls_visible`, a system roll and downstream contest/effect handling.
