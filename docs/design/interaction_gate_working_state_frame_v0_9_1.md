# v0.9.1 Interaction Gate + Working State Frame

## Why this exists

v0.9 proved the Rust-owned GM Agent architecture, but the long-session report showed that the interaction handshake was too thin: a pending check only resolved when the next input was a clean `/roll` command or a bare number. Natural table speech such as `我 TECH 6，Basic Tech 4，掷 1d10 出来是 8` bypassed structured resolution and left `pending_checks` open. That made long tests brittle and created a stale-check tail.

v0.9.1 introduces two product primitives before the full combat system:

1. `InteractionGate`: explicit, auditable gates for required rolls, required reaction choices, optional windows, confirmation prompts, target selection, and ambiguity resolution.
2. `StateFrame`: temporary working memory for combat, side quests, investigations, chases, netruns, hazards, and other local procedures.

## InteractionGate

An `InteractionGate` is the answer to: “what must the player do before the table can continue?”

It is not hardcoded combat logic. Rust owns the state machine; advice/rules packets define which gate to open and which options exist.

Gate kinds:

- `player_roll_required`
- `required_reaction_choice`
- `optional_reaction_window`
- `confirm_risky_action`
- `choose_action_mode`
- `spend_resource_window`
- `select_target`
- `resolve_ambiguous_intent`

Important behaviors:

- Natural-language roll replies are parsed.
- Unparseable replies re-prompt instead of silently narrating.
- New actions can abandon the bound action, and the old pending check is closed.
- New pending gates supersede older open gates.
- Harness can assert gate events: `gate_reprompt`, `gate_abandoned`, `pending_check_resolved`.

## Working State Frame

`StateFrame` is not long-term memory. It is local operational state:

- Combat round, active actor, temporary effects, reaction windows.
- Side quest progress, local clocks, temporary suspicions.
- Investigation-node state, unresolved local clues, current pressure.
- Netrun / chase / hazard sequence state.

Frames go to BP3, not BP1 or BP2. This preserves cache stability:

- BP1: ruleset protocols and stable system behavior.
- BP2: current static encounter/scene/NPC packets.
- BP3: active `StateFrame`, gates, dice, current HP/status/resources, recent frame events.

When a frame ends, a later `FrameCompactor` should preserve only durable consequences: injuries, destroyed objects, NPC attitude changes, discovered facts, resources spent, clues gained, open hooks, and learning candidates.

## v0.9.1 implemented scope

- Added `InteractionGate`, gate statuses, fallback policy, expected input, and action options to `trpg-model`.
- Added `StateFrame`, `FrameEvent`, and `FrameCompaction` models.
- Added PostgreSQL tables for gates and frames.
- Added natural-language roll parsing.
- Added explicit gate reprompt/abandon behavior.
- Added stale pending-check closure.
- Added BP3 projection for active state frames.
- Added harness multi-turn support and cases for natural-language roll resolution and new-action abandonment.
- Added check-contract target/DV capture into learning candidates.

## Next step

v1.0 should build `CombatFrame` and `CombatAgent` on top of `StateFrame`, not beside it. Combat reactions should become `InteractionGate` instances; attack/defense/damage should become `CheckContract` + `StatePatch` + `FrameEvent`.
