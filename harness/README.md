# Rust-native Harness

This project is Rust-first. Regression harnesses live in `crates/trpg-harness`; no Python runner is required.

Build the CLI and harness:

```bash
cargo build -p trpg-cli -p trpg-harness
```

Run the no-spoiler case:

```bash
cargo run -p trpg-harness -- run \
  --case harness/cases/homecoming_first_turn_no_spoiler.json \
  --bin ./target/debug/trpg \
  --cwd . \
  --output text
```

Run every case:

```bash
cargo run -p trpg-harness -- suite \
  --dir harness/cases \
  --bin ./target/debug/trpg \
  --cwd . \
  --output jsonl
```

Case assertions currently support:

- `forbidden_terms`: terms that must not appear in player-facing delta text.
- `required_terms`: terms that must appear in player-facing delta text.
- `required_events`: JSONL/SSE-equivalent events that must appear.

The harness intentionally drives the same non-interactive `trpg turn --request-json - --stream-format jsonl` path used for automated LLM debugging.


## v0.9 agentic check assertions

The harness can now assert agent/check events:

- `forbidden_events`
- `required_event_contains`
- `required_done_reason`
- `require_no_llm_stream_start`

Example:

```json
{
  "required_events": ["phase:agent_plan", "phase:pending_check_created"],
  "require_no_llm_stream_start": true,
  "required_done_reason": "awaiting_player_roll",
  "required_event_contains": [
    {"event": "phase:agent_plan", "contains": "ask_player_roll"}
  ]
}
```

## v1.0 conflict/combat cases

v1.0 adds ConflictFrame/CombatAgent regression cases. These assert that combat-like inputs create a working StateFrame, required reaction windows stop the stream before narration, and completed frames compact into durable aftermath rather than polluting long-term memory with round-by-round details.

```bash
cargo run -p trpg-harness -- run --case harness/cases/cyberpunk_combat_frame_starts.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/cyberpunk_required_reaction_blocks_unrelated_action.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/combat_frame_compaction.json --bin ./target/debug/trpg --cwd . --output text
```

## v1.2 director cases

v1.2 adds Actionable Situation Director regression cases. These assert that scene output has visible facts, pressure, affordances, risks, a guidance ladder, biased NPC advice, and player-facing clue board data rather than a single official route.

```bash
cargo run -p trpg-harness -- run --case harness/cases/director_scene_opening_has_four_anchors.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/director_stuck_player_guidance_ladder.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/director_costed_examples_min_three.json --bin ./target/debug/trpg --cwd . --output text
```

## v1.4 world-time cases

v1.4 adds World Time Spine regression cases. These verify that every turn emits an authoritative `phase:world_time`, and that combat/conflict flow still carries a world-time anchor.

```bash
cargo run -p trpg-harness -- run --case harness/cases/world_time_emitted_on_turn.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/world_time_survives_combat_turn.json --bin ./target/debug/trpg --cwd . --output text
```

## v1.5 lifecycle kernel tests

```bash
cargo run -p trpg-harness -- run --case harness/cases/interaction_enter_exit_reenter_no_stale_gate.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/interaction_frame_close_cascades_children.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/interaction_stale_roll_cannot_resolve_old_check.json --bin ./target/debug/trpg --cwd . --output text
```

## v1.5 interaction lifecycle invariant cases

These cases target stale-state regressions across multiple turns rather than one isolated event.

```bash
cargo run -p trpg-harness -- run --case harness/cases/interaction_enter_exit_reenter_no_stale_gate.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/interaction_ceasefire_supersedes_reaction_gate.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/interaction_pending_check_abandon_reenter_no_stale_roll.json --bin ./target/debug/trpg --cwd . --output text
```

## v1.5 interaction lifecycle cases

```bash
cargo run -p trpg-harness -- run --case harness/cases/interaction_enter_exit_reenter_no_stale_gate.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/interaction_terminal_intent_supersedes_required_reaction.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/interaction_frame_close_cascades_child_gate.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/interaction_abandon_pending_roll_then_new_action.json --bin ./target/debug/trpg --cwd . --output text
```

## v1.6 object cases

```bash
cargo run -p trpg-harness -- run --case harness/cases/object_disarm_creates_contract.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/object_cable_cut_creates_contract.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/object_pickup_ground_weapon.json --bin ./target/debug/trpg --cwd . --output text
```

## v1.7 turn orchestration cases

```bash
cargo run -p trpg-harness -- run --case harness/cases/orchestrator_enter_exit_reenter_frame_created.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/orchestrator_direction_gate_continue_attack.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/orchestrator_object_inside_frame_child_action.json --bin ./target/debug/trpg --cwd . --output text
```

## v1.8 Runtime Material Binding cases

Suggested regression coverage:

```text
runtime_actor_params_created_on_frame_start
object_disarm_hydrates_weapon_profile
object_session_scoped_weapon_transfer
object_hypothetical_does_not_steal_agent_plan
object_secret_request_preserves_gm_roll
use_grabbed_weapon_routes_to_attack
```

## v1.9 semantic rule-binding / ability hydration cases

```bash
cargo run -p trpg-harness -- run --case harness/cases/ability_semantic_spell_materializes.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/ability_rule_binding_packet_created.json --bin ./target/debug/trpg --cwd . --output text
```

## v1.10 materialization cases

```bash
cargo run -p trpg-harness -- run --case harness/cases/materialization_actor_scav_and_boss_different_profiles.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/materialization_weapon_profile_bound_before_use.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/materialization_ability_binding_verifier.json --bin ./target/debug/trpg --cwd . --output text
```

## v1.12 Mechanics Search Skill cases

These cases exercise demand-oriented retrieval planning rather than ruleset-specific hard-coded lookup paths:

```bash
cargo run -p trpg-harness -- run \
  --case harness/cases/mechanics_search_skill_combat_query_plan.json \
  --bin ./target/debug/trpg \
  --cwd . \
  --output text

cargo run -p trpg-harness -- run \
  --case harness/cases/mechanics_search_skill_weapon_parameter_binding.json \
  --bin ./target/debug/trpg \
  --cwd . \
  --output text
```

For stronger validation, inspect `mechanics_query_plans` and `parameter_facet_bindings` after the run.

### v1.13 parameter facet executor cases

New cases verify that state-changing effects do not assume HP-only damage:

```bash
cargo run -p trpg-harness -- run --case harness/cases/parameter_facet_coc_sanity_track.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/parameter_facet_triangle_chaos_track.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/parameter_facet_dnd_spell_effect.json --bin ./target/debug/trpg --cwd . --output text
```

These cases should eventually grow DB-level assertions for `parameter_facet_execution_runs`, `generic_parameter_states`, `parameter_impacts`, and `effect_resolution_packets`.

### v1.13 parameter facet executor checks

```bash
cargo run -p trpg-harness -- run \
  --case harness/cases/parameter_facet_executor_hp_impact.json \
  --bin ./target/debug/trpg \
  --cwd . \
  --output text

cargo run -p trpg-harness -- run \
  --case harness/cases/parameter_facet_executor_non_hp_resource.json \
  --bin ./target/debug/trpg \
  --cwd . \
  --output text

cargo run -p trpg-harness -- run \
  --case harness/cases/parameter_facet_executor_triangle_chaos.json \
  --bin ./target/debug/trpg \
  --cwd . \
  --output text
```

These cases are intended to verify that mechanics are no longer HP-only and that bound/provisional facets produce `ParameterImpact` and `parameter_facet_execution_runs` records.

### v1.13.1 named-weapon combat route cases

- `combat_named_weapon_attack_routes_combat.json` ensures `用重型手枪开火` stays in combat instead of going to the object kernel.
- `combat_named_weapon_first_turn_commits_check.json` ensures a player-initiated opening attack creates a check/effect contract in the same turn.

### v1.13.3 semantic combat loop cases

```bash
cargo run -p trpg-harness -- run \
  --case harness/cases/semantic_paraphrase_attack_routes_combat.json \
  --bin ./target/debug/trpg \
  --cwd . \
  --output text

cargo run -p trpg-harness -- run \
  --case harness/cases/sustained_combat_advisory_gate_does_not_block.json \
  --bin ./target/debug/trpg \
  --cwd . \
  --output text
```

## v1.14 External Playtest Evaluator

`trpg-harness playtest` runs a multi-turn black-box playtest and exports player-visible text, JSONL events, DB snapshots, DB diffs, and optional Claude Code evaluator reports.

```bash
cargo run -p trpg-harness -- playtest \
  --scenario harness/scenarios/homecoming_external_playtest_8turn.json \
  --bin ./target/debug/trpg \
  --cwd . \
  --evaluator none \
  --output text
```

With Claude Code installed and logged in:

```bash
cargo run -p trpg-harness -- playtest \
  --scenario harness/scenarios/homecoming_external_playtest_8turn.json \
  --bin ./target/debug/trpg \
  --cwd . \
  --evaluator claude-code \
  --claude-bin claude \
  --output text
```

The evaluator is intentionally test-only. It does not replace the production LLM GM backend.


## v1.15.1 — Human-like player simulation and debug directives

Product evaluation now distinguishes player behavior from test setup. Simulated players should use short, natural TRPG player actions and avoid JSON, code, SQL, DB table names, Rust type names, phase/event names, and bundled QA assertions.

For rare-state setup, playtest scenarios may include test-only debug blocks:

```text
[debug]add {"id":"npc.scav_boss","kind":"npc","hp":35,"weapon":"shotgun"}[/debug]
我朝拿霰弹枪的头目开火，然后立刻缩回掩体。
```

The harness strips `[debug]...[/debug]` blocks before sending input to the GM, records them as artifacts, and injects the resulting state as GM-only test context. This allows create/delete/modify of test actors, objects, clues, resources, clocks, and scene facts without polluting player-facing speech.

The evaluator now rejects manual dice commands and roll/damage result reports in default product scenarios. Players describe fictional actions only; the system must roll/resolve/persist automatically until the next player decision. Manual-roll compatibility can be tested only with `allow_manual_roll_input=true`.
