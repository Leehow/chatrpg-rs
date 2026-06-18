# `journey-prelude-character-session`

The smallest end-to-end journey the harness foundation supports, kept here under
`harness/scenarios/prelude/` (not `docs/epics/**`) so the artifacts live with the
harness they exercise. Everything in this directory is **provider-free**: the
consuming test (`crates/trpg-harness/tests/journey_prelude_scenario.rs`) reaches a
terminal classification without spawning the CLI, an LLM, or a database.

## Artifacts

| File | Role |
|---|---|
| `journey-prelude-character-session.json` | The reviewable scenario contract: identity, the persisted/session-binding setup the prelude requires, and the first check-dependent turn (with its check checkpoint). |
| `character-creation.jsonl` | A sample `trpg create-character --auto --stream-format jsonl` stream that the prelude readiness parser accepts as ready (persisted + bound). |
| `journey-prelude-character-session.replay.json` | A `recorded_mode: "live"` cassette (a [`Fixture`]) whose turn matches the contract's turn input and records a *resolved* contest row. |

## How a terminal result is reached, provider-free

1. **Prelude readiness** — `parse_character_creation_jsonl(character-creation.jsonl, require_persisted, require_session_binding)` ⇒ ready, with a non-empty `character_id`, `session_id`, and `actor_id`.
2. **Persisted verdict** — feeding those ids (rows present) into `evaluate_persisted_verification` ⇒ `Ok`. The same function returns `Blocked` when no DB is reachable and `Invalid` when a required row is missing, so a real run fails closed.
3. **Replay verification** — `verify_fixture_plan(replay cassette, identity, [turn inputs], Replay)` ⇒ no mismatches: the cassette's identity, `live` provenance, and turn input all match the contract.
4. **Check classification** — `fixture_turn_evidence` + `classify_check_checkpoint` over the recorded resolved contest row and the contract's check checkpoint ⇒ `PASS`. A provisional (`awaiting_binding`) row would instead classify `TRIGGERED_NO_MECHANISM`, never `PASS`.

## Live regeneration (requires DB + provider — not run here)

The live executor records the cassette automatically on an accepted run via a
`--record-fixture` flag on the binary's `playtest` runner; a failed run writes only
a `diagnostic_failed` cassette that the replay executor rejects. That wiring is the
provider-dependent layer and is intentionally not exercised by the provider-free
test in this slice.
