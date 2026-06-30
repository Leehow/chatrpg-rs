# Rust-native Harness

The harness is Rust-first. Regression cases, product playtests, deterministic/replay cassettes, and offline evaluation live in `crates/trpg-harness`; Python is not a product runner.

## Build

```bash
cargo build -p trpg-cli -p trpg-harness
```

## Single-case regression

```bash
cargo run -p trpg-harness -- run \
  --case harness/cases/homecoming_first_turn_no_spoiler.json \
  --bin ./target/debug/trpg \
  --cwd . \
  --output text
```

Useful flags:

```text
--stream-format jsonl|sse|text
--timeout-secs 120
--verbose
--output text|json|jsonl
```

## Suite

```bash
cargo run -p trpg-harness -- suite \
  --dir harness/cases \
  --bin ./target/debug/trpg \
  --cwd . \
  --output jsonl
```

Use `--fail-fast` when narrowing a regression.

## Multi-turn playtest

```bash
cargo run -p trpg-harness -- playtest \
  --scenario harness/scenarios/homecoming_external_playtest_8turn.json \
  --bin ./target/debug/trpg \
  --cwd . \
  --evaluator none \
  --output text
```

Playtest modes:

```text
live           default; drives the public CLI/GM/DB path
deterministic  classifies from an authored provider-free fixture
replay         classifies from an accepted live cassette, with no live fallback
```

Fixture and replay flags:

```text
--fixture <path>         required for deterministic/replay
--record-fixture <path>  in live mode, write an accepted run as a replay cassette
```

Human-player and debug controls:

```text
--require-human-player-input true   default; rejects JSON/code/DB/event-name style player text
--enable-debug-directives           allow test-only [debug]add/patch/delete[/debug] setup blocks
```

`[debug]...[/debug]` blocks are stripped before sending player text to the GM. They are test setup only and must not pollute player-facing speech.

## Optional external evaluator

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

The evaluator is test-only. It does not replace the production LLM GM backend.

## Offline evaluation replay

```bash
cargo run -p trpg-harness -- eval replay \
  --fixture path/to/eval_fixture.md \
  --output text
```

The fixture is a Markdown report containing a fenced `eval-fixture` JSON block. This path evaluates recorded evidence without running the GM.

## Assertion and checkpoint coverage

Case assertions include:

```text
forbidden_terms
required_terms
required_events
forbidden_events
required_event_contains
required_done_reason
require_no_llm_stream_start
```

The current harness also classifies persisted and product-level evidence such as:

```text
check checkpoints
memory checkpoints
knowledge checkpoints
NPC social/relationship checkpoints
flight-recorder checkpoints
player-visible roll/effect findings
human-player-input lint
DB-diff / persisted verification
```

For stronger gameplay acceptance, prefer a connected journey:

```text
human operation
→ public character creation and session binding
→ natural player action
→ intended runtime mechanism
→ committed state/projection
→ player-visible consequence
→ later-turn/reload consumption when persistence is claimed
```

## Representative case families

```bash
# Agent/check flow
cargo run -p trpg-harness -- run --case harness/cases/homecoming_tech_analysis_requires_pending_check.json --bin ./target/debug/trpg --cwd . --output text

# Combat / interaction lifecycle
cargo run -p trpg-harness -- run --case harness/cases/cyberpunk_combat_frame_starts.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/interaction_enter_exit_reenter_no_stale_gate.json --bin ./target/debug/trpg --cwd . --output text

# Object/materialization/mechanics
cargo run -p trpg-harness -- run --case harness/cases/materialization_weapon_profile_bound_before_use.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/mechanics_search_skill_weapon_parameter_binding.json --bin ./target/debug/trpg --cwd . --output text

# Parameter facets and non-HP effects
cargo run -p trpg-harness -- run --case harness/cases/parameter_facet_executor_non_hp_resource.json --bin ./target/debug/trpg --cwd . --output text

# Semantic combat route
cargo run -p trpg-harness -- run --case harness/cases/semantic_paraphrase_attack_routes_combat.json --bin ./target/debug/trpg --cwd . --output text
```

Manual dice commands and roll/damage reports are compatibility-mode behavior. Default product scenarios should use action-only player input: the player describes fictional action, and the system plans, rolls, resolves, persists state, narrates consequences, and stops at the next meaningful player decision.
