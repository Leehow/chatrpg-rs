---
id: TC-P2-03-golden-harness-cases-v1
title: Golden Harness Cases v1 — Homecoming, Triangle, and CoC investigation
mode: implementation
owner: claude-worker
priority: P2
risk: medium
expected_backend: tty
subagent_policy: research_only
observability: full
repair_budget: 3
---

# TC-P2-03 — Golden Harness Cases v1

## Objective

Add Rust-native golden harness coverage for three representative play styles: Cyberpunk RED Homecoming, Triangle Agency, and a CoC-style investigation.

## Why this matters

P0/P1 added knowledge projections and NPC behavior foundations, while P2-01/02 harden extraction and verification. Golden harness cases ensure those invariants remain visible in end-to-end style fixtures: no player-unknown leak, required stream events appear, and knowledge/NPC behavior regressions have runnable evidence.

## Non-goals

- Do not use Python.
- Do not require live LLM providers or external network.
- Do not repair large CLI/harness architecture unless a tiny additive helper is required.
- Do not make assertions depend on exact full prose when event/term assertions suffice.
- Do not import large copyrighted scenario text into fixtures.

## Scope owned

- `crates/trpg-harness/**`
- `harness/cases/**`
- `crates/trpg-harness/tests/**`
- `docs/design/rust_native_harness_v0_8_4.md` only if command/case docs need an additive update
- `docs/agent-loop/**` only for task evidence/handoff references

## Scope off

- GM turn loop implementation
- NoSpoiler plugin implementation
- Knowledge/NPC model redesign
- Real module ingestion or real provider calls
- Python scripts
- Broad `cargo fmt`

## Architecture constraints

- Harness must exercise the Rust-native path described in `docs/design/rust_native_harness_v0_8_4.md`.
- Cases should assert player-visible stream behavior with required/forbidden terms/events.
- Fixtures must be small, deterministic, and source-safe.
- Golden assertions should be resilient to phrasing drift; use event names, fact ids, forbidden terms, and structural JSONL checks.

## Implementation guidance

Prefer adding minimal fixture cases under `harness/cases/` plus focused harness tests that validate case parsing and assertion behavior without requiring a real GM run. If the existing harness can run mock/fake CLI output, use that for deterministic tests. If not, add the smallest local fixture parser/assertion tests and document the command for live/manual suite execution.

## Acceptance criteria

- There is one Homecoming-themed case with a no-spoiler forbidden term assertion.
- There is one Triangle-themed case with a required event/stream assertion.
- There is one CoC-style investigation case with a clue/reveal or no-spoiler assertion.
- Case format is documented or examples are self-explanatory.
- `trpg-harness` tests validate parsing/assertion logic without live providers.
- The suite command remains Rust-native.

## Required tests

- `golden_case_parses_homecoming_no_spoiler`
- `golden_case_parses_triangle_required_events`
- `golden_case_parses_coc_investigation`
- `harness_forbidden_terms_fail_on_leak`
- `harness_required_events_fail_when_missing`

## Validation commands

```bash
cargo test -p trpg-harness
cargo check -p trpg-cli -p trpg-harness
git diff --check
```

## Escalation triggers

- The harness cannot be tested without a live provider/API key.
- Meaningful coverage requires importing copyrighted scenario text rather than small synthetic fixtures.
- A broad CLI/harness rewrite is required.

## Done when

Three golden harness cases exist with deterministic parser/assertion tests and a documented Rust-native validation path.

## Handoff requirements

Include assumptions, files changed, fixture summaries, validation output, repair loops, scope ledger, and an acceptance-criteria ledger.
