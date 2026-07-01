# chatrpg-rs

Rust-native implementation for a TRPG rulebook/module parser and terminal/API-first runtime.

## Current architecture status

This repository has moved beyond the original starter skeleton. The current
implementation is a source-grounded JSON asset runtime with a unified turn
pipeline, layered ports, Rust-owned adjudication, PostgreSQL state, Tantivy
retrieval, SSE/JSONL streaming, and a Rust-native harness.

The README remains an operational overview. For the current architecture map and
known documentation drift corrections, read:

```text
docs/status/CURRENT_RUNTIME_SNAPSHOT.md
docs/status/DOCUMENTATION_DRIFT_RECONCILIATION_2026-07-01.md
docs/architecture/source-grounded-json-runtime.md
```

## Current stack

```text
Axum + Tokio + SQLx + Utoipa-compatible OpenAPI endpoint + Serde + Reqwest + Tracing + Tantivy
PostgreSQL + JSONB
Terminal/API first; no web UI for v0.8
```

The API exposes interfaces only. There is no web page and no Swagger UI bundle in v0.8. A lightweight OpenAPI index is available at:

```text
GET /api-doc/openapi.json
```

## What this repository contains

- PostgreSQL-first storage with JSONB for parsed material, context blocks, bundles, sessions, turns, characters, background jobs, and GM memory.
- JSON/JSONL-only artifact export: no YAML/TOML in the runtime or parser path.
- PDF ingestion through `oxidize-pdf` by default, with raw chunk JSONL sidecars, optional LLM cleaning, and `pdftotext` fallback.
- OpenAI standard API and OpenAI-compatible API support through `/v1/chat/completions`.
- `TRPG_LLM_SEND_TEMPERATURE=false` support for relay/model variants that reject `temperature`.
- Rulebook parser that defaults to GM onboarding, book locators, character template, starter procedures, and cold-data locators. Full page-chunk extraction is opt-in.
- Module parser that defaults to module spine and first-session prep packets. Later chapters/scenes/NPCs/handouts remain locator-backed until needed.
- Runtime context planner and true BP1/BP2/BP3 context compiler.
- GM memory module:
  - `memory_events`: append-only turn memory
  - `memory_facts`: durable known facts
  - `memory_snapshots`: cache-stable BP2 summaries
  - per-turn retrieved memory in BP3
- Axum API with SSE streaming for character creation and play turns.
- Tantivy unified search over configured SQL/file/JSONL sources, with source configuration stored in PostgreSQL rather than hardcoded in Rust.
- Terminal CLI for init, auth, parse-all, Tantivy search, inspect, character creation, play, dice rolling, and memory inspection.

## Requirements

- Rust stable
- Docker or local PostgreSQL
- Optional `pdftotext` from poppler-utils only when using `TRPG_PDF_BACKEND=pdftotext` or fallback mode
- OpenAI API key or OpenAI-compatible endpoint


## v0.8: oxidize-pdf ingestion and product-level design

v0.8 changes the ingestion default from external `pdftotext` to Rust-native `oxidize-pdf` chunks. `parse-all` now writes raw source chunk sidecars under `data/parsed/source_chunks/`, which are indexed by Tantivy and can be inspected or optionally cleaned by the LLM before rule/module onboarding.

```bash
# default in .env.example
TRPG_PDF_BACKEND=auto

# explicit CLI override
cargo run -p trpg-cli -- parse-all --pdf-backend oxidize

# fallback-oriented mode
cargo run -p trpg-cli -- parse-all --pdf-backend auto

# opt into conservative LLM cleanup for noisy chunks
TRPG_INGEST_SEMANTIC_UNITS=true TRPG_INGEST_LLM_CLEAN=false cargo run -p trpg-cli -- parse-all --pdf-backend oxidize
```

Generated ingestion artifacts:

```text
data/markdown/rulebooks/{source_id}.md
data/markdown/modules/{source_id}.md
data/parsed/source_chunks/{source_id}.oxidize_chunks.jsonl
data/parsed/source_chunks/{source_id}.cleanup_manifest.json
data/parsed/source_chunks/{source_id}.llm_cleaned_chunks.jsonl   # optional
data/markdown/cleaned/{source_id}.llm_cleaned.md                # optional
```

See:

```text
docs/design/oxidize_pdf_ingest_and_cleaning_v0_8.md
docs/product/chatrpg_product_design_v1.md
```

The product design document is intentionally written as a product specification: purpose, philosophy, users, product architecture, journeys, success metrics, safety model, and roadmap. The lower-level crate/API details remain in design docs.

## v0.5+: GM onboarding instead of full parse by default

`trpg parse-all` now builds a GM onboarding bundle, book locators, cold-data locators, and first-session module prep packets. It does **not** fully structure every rule, item, spell, monster, NPC, or later chapter unless explicitly requested.

```bash
trpg parse-all
# fast startup: onboard + index + first-session prep

trpg parse-all --full-parse
# opt into expensive full chunk extraction
```

Runtime learning is represented by lookup events, ruling logs, and learned packets. The GM can start play with operational familiarity, look up rules during play, record provisional/source-backed rulings, and later promote frequently used rules into learned packets.

New API surfaces for onboarding/learning:

```text
POST /api/ingest/parse-all?force=false&full_parse=false
POST /api/rules/lookup
POST /api/rulings
POST /api/learned-packets
GET  /api/learned-packets/{ruleset_id}
```

See `docs/design/llm_gm_onboarding_and_learning.md`.



## v0.6+: unified Tantivy search

The project now includes `trpg-search`, a Tantivy-backed unified retrieval layer. It indexes rules, modules, source Markdown, parsed JSONL artifacts, GM memory, rulings, book locators, and learned packets through a configurable source registry.

Search scope is **not hardcoded in Rust**. PostgreSQL table `search_source_configs` stores source definitions. Each enabled source is one of:

```text
sql_query   any SQL query that returns the normalized SearchDocument columns
file_glob   page-anchored Markdown or plain text files under data/
jsonl       JSONL artifacts such as context_blocks.jsonl and material_index.jsonl
```

As the database schema evolves, add or update rows in `search_source_configs`; the Tantivy indexer only consumes normalized `SearchDocument` rows.

CLI examples:

```bash
# Reindex from all enabled source configs
cargo run -p trpg-cli -- search reindex

# Inspect configured sources
cargo run -p trpg-cli -- search sources

# Search like ripgrep, but across DB + files + JSONL index
cargo run -p trpg-cli -- rg "Hacking Athena" --domain modules --module cyberpunk_red.homecoming --reindex

# Machine-readable output
cargo run -p trpg-cli -- rg "Basic Tech" --domain rules --ruleset cyberpunk_red --jsonl

# Generic scope/filter, not tied to any table schema
cargo run -p trpg-cli -- rg "Athena" --scope module_id=cyberpunk_red.homecoming --filter origin=book_locator --explain
```

API examples:

```text
POST /api/search
POST /api/search/reindex
GET  /api/search/sources
POST /api/search/sources
POST /api/search/load
```

`/api/rules/lookup` is now implemented on top of the unified search layer and records lookup events as before. Search hits do not directly enter prompt context; `/api/search/load` converts a selected hit into a TTL-scoped `ContextBlock`, preserving BP1/BP2/BP3 cache stability.

See `docs/design/unified_tantivy_search_product_design.md`.


## v0.7: runtime search/load loop

Search is now connected to the live GM turn path. During `trpg play`, `trpg turn`, and `/api/sessions/{session_id}/turn`, `RuntimeEngine.prepare_turn_context` can detect rule/module-sensitive player input, query Tantivy, record a lookup event, and compile selected hits into TTL-scoped `ContextBlock`s.

Default behavior:

```text
turn-only lookup -> BP3 dynamic_tail, ttl=turn
scene-relevant rule/module packet -> BP2 pinned_middle, ttl=scene
learned stable packet -> BP2
BP1 resident prefix -> never changed automatically by search
```

Environment controls:

```text
TRPG_RUNTIME_AUTO_SEARCH=true
TRPG_RUNTIME_AUTO_SEARCH_LIMIT=5
TRPG_RUNTIME_AUTOPIN_SCENE_RULES=true
TRPG_SEARCH_CJK_EXPANSION=true
```

CJK and bilingual query expansion are scaffolded for common TRPG demands, so Chinese inputs such as `我检查无人机背后的线缆，想黑进去` can retrieve English source material about drones, cables, Basic Tech, hacking, and NET architecture.

Incremental indexing is available and uses per-source `search_index_watermarks`, so new/updated source configs do not require search-core code changes:

```bash
cargo run -p trpg-cli -- search reindex --incremental
cargo run -p trpg-cli -- rg "Athena" --reindex --incremental-reindex
```

See `docs/design/runtime_search_load_v0_7.md`.


## v0.7: runtime auto-search and TTL ContextBlocks

v0.7 wires Tantivy search into the GM runtime. When a player turn looks rule-sensitive or module-sensitive, `RuntimeEngine` can automatically query the unified search index, convert relevant `SearchHit`s into TTL-scoped `ContextBlock`s, and load them through the normal BP1/BP2/BP3 planner.

Default behavior:

```text
turn lookup result      -> BP3 DynamicTail, expires after the turn
scene rule/module hit   -> BP2 PinnedMiddle, expires at scene switch
stable learned packet   -> BP2 PinnedMiddle
BP1 resident prefix     -> never changed by automatic search
```

New commands and options:

```bash
# Full Tantivy rebuild
cargo run -p trpg-cli -- search reindex

# Incremental rebuild using the last search_index_runs watermark
cargo run -p trpg-cli -- search reindex --incremental

# Reindex before a query, but only incrementally
cargo run -p trpg-cli -- rg "黑入无人机线缆" --module cyberpunk_red.homecoming --reindex --incremental-reindex --jsonl
```

Runtime search can be tuned by environment variables:
