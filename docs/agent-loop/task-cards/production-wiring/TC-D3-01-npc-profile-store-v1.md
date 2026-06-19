---
id: TC-D3-01-npc-profile-store-v1
title: NPC Profile Store v1 — durable profile load/upsert
mode: implementation
owner: claude-worker
priority: P1
risk: high
expected_backend: tty
subagent_policy: research_only
observability: full
repair_budget: 3
---

# TC-D3-01 — NPC Profile Store v1

## Objective

Add durable storage and load/upsert helpers for `NpcProfile` so NPC profiles can
be reused by runtime mind/behavior projections instead of existing only as
transient test structs.

## Why this matters

Design3 calls for a Persona Store as part of GM Memory System. TC-NPC-01 added
the model and safe view, but there is no `npc_profiles` table or DB/runtime
profile load path.

## Non-goals

- Do not build full module NPC-card extraction.
- Do not generate profiles with an LLM.
- Do not wire profiles into GM prompt assembly yet.
- Do not alter relationship or knowledge schemas.

## Scope owned

- additive migration for `npc_profiles`
- `crates/trpg-db/src/lib.rs`
- DB tests under `crates/trpg-db/tests/**`
- optional tiny runtime adapter in `crates/trpg-runtime/src/npc_profile.rs`
- model tests only if serde compatibility requires it

## Scope off

- GM turn loop
- NoSpoiler plugin
- NPC behavior prompt injection
- memory extraction pipeline
- broad profile extraction from source assets
- destructive migrations
- shared ledger/task-card edits

## Architecture constraints

- Profile identity must use the TC-KNOW-00 stable NPC actor id contract.
- GM-only secrets must remain in durable profile JSON but only safe views may
  enter prompt-facing adapters.
- Migration must be additive and idempotent.
- Existing `NpcProfile` serde shape should remain compatible.

## Implementation guidance

Mirror existing DB helper style for `npc_relationships`. Store the complete
profile as JSON plus a few indexed identity columns if useful. Validate actor id
on upsert/load and fail closed on unstable ids or mismatched profile actor ids.

## Acceptance criteria

- `upsert_npc_profile` persists a full `NpcProfile`.
- `load_npc_profile` round-trips a profile by session + actor id.
- Unstable actor ids fail closed and write no row.
- A loaded profile's `safe_view()` excludes GM-only secret content.
- Existing NPC profile tests still pass.

## Required tests

- `live_npc_profile_roundtrip`
- `live_npc_profile_rejects_unstable_actor_id`
- `loaded_npc_profile_safe_view_hides_secret`

## Validation commands

```bash
cargo test -p trpg-model npc_profile
DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg cargo test -p trpg-db --test live_npc_profiles -- --nocapture
cargo check -p trpg-model -p trpg-db -p trpg-runtime
git diff --check
```

## Escalation triggers

- A destructive migration is needed.
- Current `NpcProfile` JSON shape cannot be persisted without a public model
  breaking change.
- Live DB is unavailable; report exact skipped command and run non-DB fallback.

## Done when

NPC profiles have a durable, source-safe DB path ready for NPC mind/behavior
production wiring.

## Handoff requirements

Include migration summary, helper APIs, validation evidence, and any remaining
profile source-ingestion follow-up.
