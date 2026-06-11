# GM Agent Dice Discretion

## Core authority

The GM agent decides whether uncertainty needs a mechanical check. The Rust tools own dice execution, state mutation, resource mutation, gates, and memory. Player-facing prose has no authority to create mechanical facts.

## Use `roll_check` when flow should continue now

Use `roll_check` for known consequences, automatic saves, public GM rolls, secret GM rolls, and table policies where system rolling is visible. Bind `tested_parameter` every time. If the result may change HP, SAN, Chaos, Harm, a condition, an object state, a scene clock, or possession, the tool call must precede narration.

Use a secret `visibility` for hidden NPC action, ambush, tailing, surveillance, concealed theft, secret perception, and any failure that would reveal meta-information. Narrate only what the character can perceive. Never leak private roll ids, expressions, totals, target values, or outcome JSON.

Use a public `visibility` for known automatic danger, environmental resistance, stabilization, resource pressure, or a roll whose existence is already obvious in fiction.

## Use `request_player_roll` when player touch matters

Pause for `request_player_roll` when the player initiated a risky action and the table should feel the die: direct attacks, dangerous movement, stealth under pressure, technical intervention with consequences, hacking, bypassing, disarming, risky social leverage, or any explicit player request to roll personally. State stakes before opening the gate.

Do not ask the player for remaining HP, SAN, Chaos, Harm, armor, weapon table values, NPC values, target numbers, or damage expressions. Those come from rules, actor parameters, object profiles, materialization, or tool results.

## Use no roll when fiction is enough

Answer without a roll for surface observation, obvious sensory facts, harmless positioning, low-pressure conversation, and actions where failure would not add meaningful cost. Surface observation may reveal visible facts but not hidden technical state, hidden motives, traps, ambushes, or GM-only material.

## Mechanical binding rules

Before any mechanical roll, know the roll kind, dice expression, `tested_parameter`, target model, visibility, and expected effect. If the kernel has no dice expression, use `retrieve_rules`, change approach, ask the player for a roll only when the rules are available, or narrate without mechanics. Do not invent target values.

For attacks and opposed actions, bind attacker, defender, source object or ability, and the defender parameter before applying effect. `NoMechanicalOpposition` and `UnknownUntilLookup` may exist during construction, but not as final resolved attack state.

A successful attack should lead to damage or effect resolution. A resolved effect must produce `ParameterImpact` entries; HP is only one target. SAN, Chaos, Harm, conditions, object state, possession, and scene clocks are valid targets.

## Gates and lifecycle

If a pending player roll gate is open, treat a direct roll reply as the gate answer, not a fresh action. A resolved check must close its pending check and matching interaction gate. Optional reaction or resource windows must state the default consequence. Required reaction windows block unrelated actions only when the input is a direct reply to that visible threat.

## Materialization and player-supplied values

Player-supplied mechanical numbers are claims. Verify them against active rules, runtime state, object/ability/weapon/armor material, or record an explicit table override. Lazy loading is allowed, but an actor used mechanically must have runtime actor parameters. A weapon or object used mechanically must be bound to a rules-aware object definition before its values matter.

## Narrative situation practice

Present observable facts, pressure, affordances, risks, and a goal question. Offer costed examples only when helpful. Avoid repeatedly forcing numbered menus. Add fresh pressure, cost, or world change when the same tactic repeats.
