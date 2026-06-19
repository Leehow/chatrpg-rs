# Rust-native Harness v0.8.4

The regression harness is now a Rust crate: `crates/trpg-harness`.

The project should not depend on Python for core validation. Harness cases drive the same non-interactive CLI path used by tests and LLM-assisted debugging:

```text
trpg turn --request-json - --stream-format jsonl
```

## Goals

- Validate player-visible stream behavior from JSONL deltas.
- Catch spoiler leaks before humans see them.
- Assert required stream events such as `context_compiled`, `memory_event_saved`, and `learning_audit`.
- Support required/forbidden text assertions without adding a web frontend.

## Usage

```bash
cargo build -p trpg-cli -p trpg-harness
cargo run -p trpg-harness -- suite --dir harness/cases --bin ./target/debug/trpg --cwd .
```

## Case format

```json
{
  "name": "homecoming_first_turn_no_spoiler",
  "ruleset_id": "cyberpunk_red",
  "module_id": "cyberpunk_red.homecoming",
  "user_input": "我检查无人机背后的线缆，看能不能切断。",
  "forbidden_terms": ["Athena"],
  "required_events": ["context_compiled", "memory_event_saved", "learning_audit", "done"],
  "stream_format": "jsonl"
}
```

## Golden cases (TC-P2-03)

Three representative golden cases live under `harness/cases/` and cover the
main play styles with phrasing-drift-resistant assertions (event names, fact
ids, and short forbidden terms — never long prose):

- `homecoming_no_spoiler_golden.json` — Cyberpunk RED Homecoming, with a
  no-spoiler `forbidden_terms` guard.
- `triangle_required_events_golden.json` — Triangle Agency, with a required
  `phase:conflict_agent` stream event plus a `required_event_contains` check.
- `coc_investigation_golden.json` — CoC-style investigation, with a
  clue/reveal no-spoiler `forbidden_terms` guard.

The case schema, JSONL parser, and assertion evaluator are exported from the
`trpg_harness` library so they can be exercised offline. `crates/trpg-harness/tests/golden_cases.rs`
validates that these cases parse and that the shared evaluator catches a spoiler
leak and a missing required event — with no `trpg` process, network, or LLM:

```bash
cargo test -p trpg-harness
```

Running the cases live (against a real `trpg` binary and provider) still uses the
`suite` command shown above.

## Design rule

All future automated playtest harnesses should be Rust crates or Rust integration tests. Shell is acceptable for orchestration. Python is not part of the project runtime or regression surface.
