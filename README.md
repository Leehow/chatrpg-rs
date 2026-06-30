# chatrpg-rs

Rust-native solo TRPG GM runtime: ingest rulebooks/modules, build a source-backed GM operating model, run terminal/API-first play, and evaluate the gameplay loop with a Rust harness.

This README is a code-checked orientation for the current workspace. Older release-note style sections in historical docs are still useful as provenance, but the current code baseline is the workspace `1.20.0` line.

## Current stack

- Rust workspace, 2021 edition, 29 crates.
- Axum + Tokio API, SQLx + PostgreSQL/JSONB storage, Tantivy search, Reqwest LLM relay, Serde/JSON/JSONL artifacts, tracing.
- Terminal/API first. There is still no web UI and no bundled Swagger UI.
- Lightweight OpenAPI-style index: `GET /api-doc/openapi.json`.
- Rust-native regression/product harness in `crates/trpg-harness`.

## What the project currently does

`chatrpg-rs` is not a generic chat wrapper. It is a stateful TRPG runtime whose Rust layer owns the rules/referee/control-plane work that should not be improvised by the narrator.

Current runtime pillars:

- **Ingestion and onboarding**: `parse-all` scans `data/rulebooks` and `data/modules`, builds rule/module bundles, RuleKernel/CharacterOnboardingPack data, locators, starter packets, and search indexes.
- **Source-backed search**: Tantivy indexes configured SQL/file/JSONL sources. Runtime retrieval and rule lookup use this instead of hardcoded table copies.
- **Context compiler bands**: BP1/BP2/BP3 context blocks keep engine protocol, pinned session/material state, and turn-dynamic state separate for cache stability.
- **Canonical turn pipeline**: CLI/API turns go through one Rust executor and the 15-phase `CANONICAL_TURN_PLAN`.
- **Rust-owned mechanics/control kernels**: interaction gates, object/possession, semantic routing, materialization, ability/rule binding, contest/referee/mechanics, parameter facets, world time, director guidance, and postprocess lifecycle are Rust services, not narrator-only recommendations.
- **Knowledge and world state**: durable memory, domain events, knowledge edges, NPC relationships/profiles, world facts, story state, clue/evidence projection, and no-spoiler projections are active architecture areas.
- **Evaluation**: the harness supports single-case regression runs, suites, multi-turn playtests, deterministic/replay fixtures, DB-diff inspection, and optional external evaluator snapshots.

## Quick start

Run PostgreSQL:

```bash
docker compose up -d postgres
```

Initialize local folders and `.env`:

```bash
cargo run -p trpg-cli -- init
cargo run -p trpg-cli -- auth set
cargo run -p trpg-cli -- migrate
```

Default local database URL:

```text
postgres://chatrpg:chatrpg@localhost:54323/chatrpg
```

Put source PDFs here:

```text
data/rulebooks/*.pdf
data/modules/*.pdf
```

Parse and index:

```bash
cargo run -p trpg-cli -- parse-all
```

Run the API:

```bash
cargo run -p trpg-cli -- api
```

Start terminal play:

```bash
cargo run -p trpg-cli -- play --ruleset cyberpunk_red --module cyberpunk_red.homecoming
```

Run one non-interactive turn:

```bash
cargo run -p trpg-cli -- turn \
  --ruleset cyberpunk_red \
  --module cyberpunk_red.homecoming \
  --input "我检查无人机背后的线缆。" \
  --stream-format jsonl
```

## PDF ingestion defaults

The current product default is **duotext**, not the older v0.8 oxidize-first default.

```bash
cargo run -p trpg-cli -- parse-all --pdf-backend duotext
```

Supported CLI values:

```text
duotext     default; two pdftotext passes, merged into one page-anchored Markdown file
pdftotext   alias for duotext/poppler
oxidize     Rust-native oxidize-pdf path
auto        oxidize first, then duotext/pdftotext fallback
mineru      vision/model-based extractor path when configured
```

`duotext` is optimized for table-heavy TRPG books: prose pages use reading order, table-heavy pages preserve `-layout` alignment. It requires `pdftotext` from poppler-utils. Use `--pdf-backend oxidize` when you explicitly want the Rust-native path, and `--full-parse` only when you want the expensive full chunk extraction path.

## Useful CLI commands

```bash
# Search
cargo run -p trpg-cli -- search reindex
cargo run -p trpg-cli -- rg "Athena drone cable" --module cyberpunk_red.homecoming --reindex
cargo run -p trpg-cli -- grep-table "Glock 17"

# Rules / onboarding
cargo run -p trpg-cli -- rules playability --ruleset cyberpunk_red --module cyberpunk_red.homecoming
cargo run -p trpg-cli -- rules query --ruleset cyberpunk_red "basic tech drone cable"
cargo run -p trpg-cli -- rules character-pack --ruleset cyberpunk_red

# Sessions and turns
cargo run -p trpg-cli -- opening --ruleset cyberpunk_red --module cyberpunk_red.homecoming --session-id <session>
cargo run -p trpg-cli -- bind-character --ruleset cyberpunk_red --module cyberpunk_red.homecoming
cargo run -p trpg-cli -- explain --session <session_id> --turn <turn_id>
cargo run -p trpg-cli -- coverage --session <session_id>

# World time
cargo run -p trpg-cli -- time show --session <session_id>
cargo run -p trpg-cli -- time advance --session <session_id> --minutes 5 --reason "players delay under fire"
cargo run -p trpg-cli -- time events --session <session_id> --since-tick 0
```

## API surface

The API is Axum-based and SSE-friendly. Main groups:

```text
GET  /health
GET  /api-doc/openapi.json
POST /api/ingest/parse-all
POST /api/ingest/ruleset
GET  /api/rulesets/{id}/identity
GET  /api/rulesets/{id}/character-template
POST /api/context/compile
POST /api/checks/resolve
POST /api/conflict/start
POST /api/search
POST /api/search/reindex
POST /api/search/load
GET  /api/search/sources
POST /api/rules/lookup
POST /api/rules/steward/assist
POST /api/rules/playability
GET  /api/rules/character-onboarding/{ruleset_id}
POST /api/characters/create
POST /api/sessions/start
POST /api/sessions/{session_id}/turn
GET  /api/sessions/{session_id}/time
POST /api/sessions/{session_id}/time/advance
GET  /api/sessions/{session_id}/events
GET  /api/sessions/{session_id}/memory
POST /api/sessions/{session_id}/memory/retrieve
POST /api/sessions/{session_id}/memory/compact
```

## Harness and product evaluation

Build the test binary pair:

```bash
cargo build -p trpg-cli -p trpg-harness
```

Run one case:

```bash
cargo run -p trpg-harness -- run \
  --case harness/cases/homecoming_first_turn_no_spoiler.json \
  --bin ./target/debug/trpg \
  --cwd . \
  --output text
```

Run a suite:

```bash
cargo run -p trpg-harness -- suite \
  --dir harness/cases \
  --bin ./target/debug/trpg \
  --cwd . \
  --output jsonl
```

Run a multi-turn playtest:

```bash
cargo run -p trpg-harness -- playtest \
  --scenario harness/scenarios/homecoming_external_playtest_8turn.json \
  --bin ./target/debug/trpg \
  --cwd . \
  --evaluator none \
  --output text
```

The playtest runner also supports:

```text
--mode live            run the public CLI/GM/DB path
--mode deterministic   classify from an authored provider-free fixture
--mode replay          classify from an accepted live cassette
--record-fixture       save an accepted live run as a replay cassette
--enable-debug-directives
--require-human-player-input
```

Offline fixture evaluation:

```bash
cargo run -p trpg-harness -- eval replay \
  --fixture path/to/report.md \
  --output text
```

See `harness/README.md` for more harness details.

## Architecture map

- `trpg-cli` — terminal entry points and automation-safe commands.
- `trpg-api` — Axum API and SSE transports.
- `trpg-gm` — current turn control plane, agent loop, tools, modes, presentation gates, ledger, and postprocess phases.
- `trpg-runtime` — session/runtime services, context assembly, search/load, world time, knowledge/NPC/story/evidence projections.
- `trpg-parser` + `trpg-ingest` — PDF ingestion, onboarding, bundle generation, staged parse.
- `trpg-rule-agent` — Rule Steward first-pass skills, rule assistance, playability checks.
- `trpg-search` — Tantivy search and source registry.
- `trpg-agent`, `trpg-interaction`, `trpg-object`, `trpg-material`, `trpg-ability`, `trpg-referee`, `trpg-mechanics`, `trpg-contest`, `trpg-director`, `trpg-time` — Rust-owned runtime/referee subsystems.
- `trpg-db` — PostgreSQL persistence and migrations.
- `trpg-harness` + `trpg-eval` — regression, playtest, replay, and evaluation tools.

One important current boundary: `trpg-gm` is still business-heavy. The long-term architecture notes aim to slim it down to control-plane/narration responsibilities, but the current code has not completed that migration.

## Current known work

High-ROI work still visible from the code/docs audit:

1. **Engine de-hardcoding**: move remaining ruleset/module-specific names, DVs, aliases, combat defaults, and profile choices out of runtime crates into parsed RuleKernel/ModulePrepPacket/profile data.
2. **Semantic-first routing and need detection**: reduce lexical fallback and replace keyword-sensitive rule/material/module triggers with source-backed semantic classifiers.
3. **API/session consistency**: ensure API turn transport loads server-side recent history by default and defines cancellation behavior for disconnected SSE clients.
4. **Observability and source traces**: keep strengthening TurnFailed/TurnTrace/domain event/source-ref propagation so failures and retrieval decisions do not look like successful empty turns.
5. **Source-backed ruleset completeness**: exact weapon/armor/SP/AC/SAN/Harm/Chaos/power-table/resource facets still depend on richer extraction and verification packs.
6. **Documentation/API drift**: the static OpenAPI info version and older historical product docs should be audited regularly against `Cargo.toml`, CLI flags, API routes, and migrations.

## Validation recipes

For docs-only updates:

```bash
find docs harness -type f -name '*.md' -print
```

For Rust-affecting work, use a focused subset first:

```bash
cargo fmt --check
cargo check -p trpg-model -p trpg-db -p trpg-runtime -p trpg-gm
cargo test -p trpg-model
cargo test -p trpg-db
cargo test -p trpg-runtime --lib
cargo test -p trpg-gm
bash scripts/arch_gates.sh
```

For gameplay/product acceptance, prefer a connected journey through public paths rather than only component tests:

```bash
cargo run -p trpg-harness -- suite --dir harness/cases --bin ./target/debug/trpg --cwd .
cargo run -p trpg-harness -- playtest --scenario harness/scenarios/homecoming_external_playtest_8turn.json --bin ./target/debug/trpg --cwd . --evaluator none
```

## Reference docs

- `docs/status/current_project_state_v1_20.md`
- `docs/architecture/layered-runtime-invariants.md`
- `docs/status/known_issues.md`
- `docs/status/rule_steward_character_onboarding_v1_16.md`
- `docs/status/rule_steward_first_pass_v1_16_1.md`
- `docs/status/rule_steward_mechanics_v1_16_2.md`
- `docs/GPT_Pro_审查_triage_2026-06-16.md`
