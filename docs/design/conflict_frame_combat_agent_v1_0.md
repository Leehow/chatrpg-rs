# v1.0 Conflict Frame & Combat Agent Design

## Product intent

v1.0 changes "combat" from a fixed rules engine into a Rust-owned Conflict Agent. The agent controls a working-state frame, interaction gates, checks, effects, dice visibility, and frame compaction. Ruleset-specific behavior is data-driven through `data/ruleset_advice/*.json`, not hardcoded in Rust and not pasted wholesale into prompts.

## Why this shape

The supported games have incompatible conflict shapes:

- Cyberpunk RED uses Combat Time, initiative, actions, ranged/melee combat, Netrunning, wound states, armor and critical injuries.
- D&D 5e uses initiative, movement/action/bonus action/reaction, opportunity attacks, attacks versus AC, saves, conditions, monster stat blocks, and spells.
- Call of Cthulhu / BRP uses d100 roll-under, difficulty bands, Keeper hidden information, fight-back/dodge choices, firearms, chases, sanity, wounds and investigation pressure.
- Triangle Agency uses anomaly encounters, 6d4, Chaos, Harm, playwalled visibility, mission reports and aftermath rather than a traditional tactical combat loop.

The common product object is therefore not "attack and damage". The common object is a transient conflict frame with gates, checks, effects and compaction.

## Main objects

### StateFrame

`StateFrame` is the temporary working memory for a local process: combat, chase, netrun, investigation node, social conflict, hazard sequence, side quest, downtime project, anomaly encounter or horror encounter.

During play, a frame is projected into BP3. It should not enter BP1 or BP2 because round number, active actor, hit points, temporary effects and reaction windows change frequently.

### CombatWorkingState

`CombatWorkingState` lives inside `StateFrame.working_state` and records: combat mode, round, phase, active actor, initiative order, participants, sides, zones, range model, action economy, reaction windows, temporary effects, hazards, objectives, rule packet IDs and recent frame events.

### InteractionGate

CombatAgent never lets a required choice silently pass through. If a player is targeted by a visible attack and the profile says a reaction is required, it opens a `RequiredReactionChoice` gate. If a player must roll, it opens a `PlayerRollRequired` gate. Optional reactions may resolve as default/no reaction if the profile says so.

### CheckContract

Any action requiring uncertainty resolution becomes a `CheckContract`. The contract says who acts, what roll is needed, who rolls, whether the roll is public/private/player-required, what target is known, what stakes apply, and which rule/advice/source refs justify it.

### EffectContract

Effects are separate from checks. A hit, Harm, sanity loss, armor ablation, condition, resource spend or loose end is first an `EffectContract`, then a validator may convert it into durable `StatePatch` values.

### FrameCompaction

When a frame ends, the system preserves consequences and discards transient details. Combat retains wounds, deaths, NPC escape/surrender, resource costs, destroyed locations, heat/chaos/loose ends, important clues and open hooks. It discards initiative order, stale reaction windows, expired cover, and round-by-round minutiae.

## Cache policy

- BP1: only resident ruleset protocol, play loop, output contract and stable GM behavior.
- BP2: current encounter packet, current scene/location/NPC static cards, stable learned packets.
- BP3: StateFrame, active actor, round, HP/status/resource deltas, dice results, pending gates, temporary lookups.

Changing HP or round must not change BP1/BP2 hashes.

## Data-driven profiles

Profiles live under `data/ruleset_advice/`:

- `cyberpunk_red.combat.v1.json`
- `dnd5e.combat.v1.json`
- `coc7e.conflict.v1.json`
- `triangle_agency.conflict.v1.json`
- `sword_world_2_5.combat.v1.json`

Each profile describes initiative, action economy, reaction windows, search recipes and compaction policy. Rust implements the executor and validators; profile data supplies the ruleset-specific advice.

## Runtime flow

1. Player input arrives.
2. Rust runtime resolves any open InteractionGate first.
3. Runtime compiles BP1/BP2/BP3.
4. CombatAgent checks active StateFrame or starts a new one if the input is conflict-sensitive.
5. CombatAgent writes frame events, gates, CheckContracts and EffectContracts as required.
6. If a player roll or required reaction is needed, SSE stops with an awaiting reason.
7. Otherwise LLM narration continues using the structured conflict context.
8. Learning audit captures CheckContract and EffectContract details into review-gated learning candidates.
9. On frame completion, compaction writes durable aftermath and archives transient details.

## v1.0 non-goals

v1.0 does not implement a complete Cyberpunk RED damage engine, D&D spell engine, CoC insanity table, or Triangle mission resolver. It implements the common frame/gate/check/effect system needed to run those details through search, learned packets and validators.
