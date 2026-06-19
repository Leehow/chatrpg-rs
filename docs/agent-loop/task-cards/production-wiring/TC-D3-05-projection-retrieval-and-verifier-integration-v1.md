---
id: TC-D3-05-projection-retrieval-and-verifier-integration-v1
title: Projection Retrieval and Verifier Integration v1
mode: implementation
owner: claude-worker
priority: P2
risk: high
expected_backend: tty
subagent_policy: research_only
observability: full
repair_budget: 3
---

# TC-D3-05 — Projection Retrieval and Verifier Integration v1

## Objective

Introduce explicit viewer/speaker projection retrieval APIs and connect
production verifier hooks to deterministic knowledge/NPC consistency helpers.

## Why this matters

Design3 asks to replace one generic memory retrieve with projections:
`for_player_narration`, `for_gm_adjudication`, `for_npc_speech`, and
`for_npc_action`. TC-P2-02 added verifier helpers, but they are not fully wired
into production output checks.

## Non-goals

- Do not rewrite all retrieval/search providers.
- Do not introduce LLM-based semantic verification.
- Do not block streaming before narration is delivered.
- Do not expose GM-only truth in player/NPC projections.

## Scope owned

- `crates/trpg-runtime/src/knowledge_projection.rs`
- `crates/trpg-runtime/src/npc_mind.rs`
- `crates/trpg-runtime/src/knowledge_leak_verifier.rs`
- `crates/trpg-gm/src/turn_loop.rs` only for narrow verifier hook integration
- focused tests in runtime/gm crates

## Scope off

- WorldFact store migration
- profile store migration
- memory proposal commit pipeline
- broad search/retrieval rewrite
- real provider calls
- shared ledger/task-card edits

## Architecture constraints

- Player projection must include only player-known / player-safe facts.
- NPC speech/action projections must use that NPC's mind view and behavior plan.
- GM adjudication projection may include hidden truth but must be separated from
  player-facing narration.
- Verifier findings must be advisory/errata-compatible and must not echo secret
  text.

## Acceptance criteria

- Runtime exposes clear projection helpers for player narration, GM
  adjudication, NPC speech, and NPC action.
- NPC projections do not include GM-only or other-NPC facts.
- Production AfterLlmStream can call deterministic leak/NPC consistency helpers
  when supplied with markers/plans.
- Existing NoSpoiler and NPC mind tests still pass.

## Required tests

- `projection_for_player_narration_excludes_unknown_fact`
- `projection_for_npc_speech_uses_npc_mind_only`
- `projection_for_gm_adjudication_keeps_hidden_truth_separate`
- `after_stream_verifier_uses_projection_findings`

## Validation commands

```bash
cargo test -p trpg-runtime projection
cargo test -p trpg-runtime verifier
cargo test -p trpg-gm no_spoiler
cargo check -p trpg-model -p trpg-runtime -p trpg-gm
git diff --check
```

## Escalation triggers

- A broad retrieval rewrite is required.
- Production verifier requires unavailable secret/provider data.
- The integration would leak secret text into traces or prompt.

## Done when

Projection retrieval APIs exist and production verifier hooks can consume them
without collapsing GM/player/NPC knowledge boundaries.

## Handoff requirements

Include projection API summary, verifier hook scope, validation evidence, and
remaining broad retrieval follow-ups.
