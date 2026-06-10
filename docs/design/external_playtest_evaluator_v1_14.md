# v1.14 External Playtest Evaluator

## Goal

`trpg-harness playtest` is a test-only black-box evaluator for the LLM GM. It does not replace the production LLM backend. The production GM, parser, materializer, and API backend continue to use the configured OpenAI/OpenAI-compatible API. Claude Code is used only as an external player/evaluator for richer diagnostics.

## Why this exists

Phase-only harness cases prove that events were emitted, but they do not prove that the GM acted as a referee. Human playtests found failures such as: the GM narrates HP changes but does not persist HP, asks the player for weapon damage or remaining HP, opens stale direction menus, or leaks hidden module facts. The playtest evaluator captures player-visible output, SSE/JSONL events, and DB deltas for each turn, then optionally asks Claude Code to judge the turn.

## Command

```bash
trpg-harness playtest \
  --scenario harness/scenarios/homecoming_external_playtest_8turn.json \
  --bin ./target/debug/trpg \
  --cwd . \
  --evaluator claude-code \
  --output text
```

Use `--evaluator none` to generate the same snapshots without calling Claude Code.

## Per-turn artifacts

Each turn creates:

- `player_input.txt`
- `player_visible_output.txt`
- `events.jsonl`
- `stderr.txt`
- `db_snapshot_before.json`
- `db_snapshot_after.json`
- `db_diff.json`
- `eval_prompt.md`
- `eval_result.json`

The run root also writes `playtest_result.json`.

## Evaluation boundary

The evaluator sees DB snapshots and debug events, but the player driver should not. This keeps black-box playtesting honest: the player can only see player-visible output, while the evaluator checks whether the hidden mechanical state agrees with the narration.

## Claude Code mode

The first implementation uses `claude -p` as a per-turn evaluator. This is suitable for judging snapshots and debug reports. It is not intended as the production LLM GM backend.

The invocation is intentionally minimal:

```bash
claude -p --bare --no-session-persistence --tools "" --output-format json --json-schema <schema>
```

## What this checks

The scenario format supports:

- expected/forbidden phases
- forbidden player-visible text
- minimum DB deltas, such as at least one new `parameter_impacts` row
- external evaluator verdicts

This makes the harness catch failures that event-only tests miss, especially narration-only mechanics.
