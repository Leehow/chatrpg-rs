---
id: TC-D3-04-relationship-extraction-social-gate-v1
title: Relationship Extraction Social Gate v1 — active social evidence
mode: implementation
owner: claude-worker
priority: P2
risk: medium
expected_backend: tty
subagent_policy: research_only
observability: full
repair_budget: 3
---

# TC-D3-04 — Relationship Extraction Social Gate v1

## Objective

Expand relationship extraction triggering beyond "new entity surfaced this
turn" so social interactions with already-known NPCs can still produce
relationship evidence/proposals.

## Why this matters

Design3 notes that threats, bargains, help, deception, and repeated NPC
conversation often change relationships without surfacing a new entity.

## Non-goals

- Do not run relationship extraction every turn unconditionally.
- Do not commit relationship state directly from extraction.
- Do not rewrite the relationship extractor prompt wholesale unless needed.
- Do not introduce new provider calls outside the existing extractor path.

## Scope owned

- `crates/trpg-runtime/src/relationship_extraction.rs`
- `crates/trpg-runtime/src/lib.rs` extraction gate/orchestration
- focused tests under `crates/trpg-runtime/tests/**`
- optional small model/runtime gate type

## Scope off

- NoSpoiler plugin
- NPC profile/mind/behavior prompt wiring
- memory proposal commit pipeline except using its proposal adapter if already
  available
- DB schema changes
- shared ledger/task-card edits

## Architecture constraints

- Gate must remain cost-aware and deterministic where possible.
- Gate should prefer source-backed signals: active NPC ids, player input,
  narration, interaction/tool events, or explicit social intent markers.
- Existing "new entity surfaced" behavior must remain supported.
- If extraction runs, output must still preserve evidence/source event ids.

## Acceptance criteria

- New surfaced entity still triggers extraction.
- No new entity but clear active NPC social interaction can trigger extraction.
- Non-social narration without new entity does not trigger extraction.
- Extraction can map relationship triples into proposal form when available.

## Required tests

- `relationship_gate_triggers_on_new_entity`
- `relationship_gate_triggers_on_active_npc_social_interaction`
- `relationship_gate_skips_non_social_no_new_entity`
- `relationship_gate_preserves_evidence_refs`

## Validation commands

```bash
cargo test -p trpg-runtime relationship_extraction
cargo test -p trpg-runtime memory_proposal
cargo check -p trpg-runtime -p trpg-model
git diff --check
```

## Escalation triggers

- There is no source-backed way to identify active NPC/social interaction.
- Cost control would require a product decision.
- Existing extractor behavior must be broadly rewritten.

## Done when

Relationship extraction can run for meaningful repeated NPC interactions
without requiring a newly surfaced entity, while still skipping ordinary turns.

## Handoff requirements

Include gate inputs, assumptions, validation evidence, and any unsupported
social signals.
