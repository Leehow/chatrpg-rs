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

## Design rule

All future automated playtest harnesses should be Rust crates or Rust integration tests. Shell is acceptable for orchestration. Python is not part of the project runtime or regression surface.
