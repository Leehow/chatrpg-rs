# chatrpg-rs

Rust starter implementation for a TRPG rulebook/module parser and terminal/API-first runtime.

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

```env
TRPG_RUNTIME_AUTO_SEARCH=true
TRPG_RUNTIME_AUTO_SEARCH_LIMIT=5
TRPG_RUNTIME_AUTO_SEARCH_MAX_SCENE_PINS=1
TRPG_RUNTIME_AUTOPIN_SCENE_RULES=true
TRPG_RUNTIME_AUTO_SEARCH_DOMAINS=learned,rules,modules,rulings,source,parsed
TRPG_SEARCH_INDEX_DIR=./data/search/tantivy_v3
```

See `docs/product/llm_gm_runtime_search_v07_prd.md` and `docs/design/runtime_search_autoload_v07.md`.

## Quick start

```bash
docker compose up -d postgres
cargo run -p trpg-cli -- init
cargo run -p trpg-cli -- auth set
cargo run -p trpg-cli -- migrate
```

Default local database URL is intentionally mapped to port `54323` to avoid conflicts with local Homebrew PostgreSQL:

```text
postgres://chatrpg:chatrpg@localhost:54323/chatrpg
```

Put PDFs here:

```text
data/rulebooks/*.pdf
data/modules/*.pdf
```

Parse all:

```bash
cargo run -p trpg-cli -- parse-all
```

This writes:

```text
data/parsed/project.bundle.json
data/parsed/context_blocks.jsonl
data/parsed/material_index.jsonl
```

Run API:

```bash
cargo run -p trpg-cli -- api
```

Create a character in terminal:

```bash
cargo run -p trpg-cli -- create-character --ruleset cyberpunk_red --module cyberpunk_red.homecoming
```

Start terminal play:

```bash
cargo run -p trpg-cli -- play --ruleset cyberpunk_red --module cyberpunk_red.homecoming
```

Inside the play shell:

```text
/roll 2d6+3
/memory
/compact-memory
/quit
```

## SSE endpoints

Character creation:

```bash
curl -N -X POST http://127.0.0.1:8787/api/characters/create \
  -H 'content-type: application/json' \
  -d '{"ruleset_id":"cyberpunk_red","module_id":"cyberpunk_red.homecoming","user_preferences":"tech/netrunner, local fixer tie-in"}'
```

Play turn:

```bash
curl -N -X POST http://127.0.0.1:8787/api/sessions/<session_id>/turn \
  -H 'content-type: application/json' \
  -d '{"ruleset_id":"cyberpunk_red","module_id":"cyberpunk_red.homecoming","user_input":"我检查无人机背后的线缆。"}'
```

Memory inspection:

```bash
curl http://127.0.0.1:8787/api/sessions/<session_id>/memory
```

Memory retrieval:

```bash
curl -X POST http://127.0.0.1:8787/api/sessions/<session_id>/memory/retrieve \
  -H 'content-type: application/json' \
  -d '{"text":"Athena drone cable", "limit":8}'
```

Manual compaction into a BP2-stable snapshot:

```bash
curl -X POST http://127.0.0.1:8787/api/sessions/<session_id>/memory/compact \
  -H 'content-type: application/json' \
  -d '{"ruleset_id":"cyberpunk_red","module_id":"cyberpunk_red.homecoming"}'
```

Rule lookup through the book locator:

```bash
curl -X POST http://127.0.0.1:8787/api/rules/lookup \
  -H 'content-type: application/json' \
  -d '{"session_id":"<session_id>","ruleset_id":"cyberpunk_red","module_id":"cyberpunk_red.homecoming","query_text":"basic tech check against a drone cable","limit":8}'
```

Record a provisional or source-backed ruling:

```bash
curl -X POST http://127.0.0.1:8787/api/rulings \
  -H 'content-type: application/json' \
  -d '{"session_id":"<session_id>","ruleset_id":"cyberpunk_red","module_id":"cyberpunk_red.homecoming","demand_id":"basic_tech_drone_cable","ruling_text":"Use TECH + Basic Tech against DV14 as a provisional table ruling; audit after session.","status":"provisional","confidence":"low","provisional":true}'
```

Promote a looked-up rule into a learned packet:

```bash
curl -X POST http://127.0.0.1:8787/api/learned-packets \
  -H 'content-type: application/json' \
  -d '{"ruleset_id":"cyberpunk_red","module_id":"cyberpunk_red.homecoming","packet_type":"skill_check","packet_key":"basic_tech_drone_cable","title":"Basic Tech check for drone cabling","summary":"Use the relevant TECH skill check packet when PCs inspect or manipulate Athena-related cabling. Keep exact DV/source refs attached when available.","learning_stage":"used_once","confidence":"medium"}'
```

The SSE stream is intentionally two-stage:

1. Stream the LLM-facing answer immediately.
2. Schedule postprocess in a background job so saving, validation, state handling, and memory writes do not block user output.

## Cache stability

Runtime context is compiled into three independent bands:

- BP1 `prefix_text`: resident engine protocol, ruleset core, style, procedures, character kernel.
- BP2 `pinned_text`: active module/chapter/mission/scene/location/NPC/exact rule material, plus cache-stable memory snapshots.
- BP3 `dynamic_text`: current input, projected state, recent transcript, dice, validator feedback, and turn-only retrieved memory.

Each band has its own hash. Changing only the user input should change BP3, not BP1/BP2. Writing a new `memory_event` should not change BP2. Running `/compact-memory` or `POST /memory/compact` can change BP2 because it creates or updates a pinned snapshot.

## Design docs

- `docs/design/gm_memory_module.md`
- `docs/design/json_jsonl_format.md`
- `docs/design/non_interactive_cli.md`
- `docs/design/llm_gm_onboarding_and_learning.md`
- `docs/design/unified_tantivy_search_product_design.md`
- `docs/product/llm_gm_unified_tantivy_search_prd.md`
- `docs/status/known_issues.md`

## Notes

This is a starter implementation. Parser quality depends on the LLM model and source PDF quality. v0.8 uses oxidize-pdf chunks by default and can optionally run conservative LLM cleanup for noisy chunks. Scanned PDFs and image-only tables are still not a primary target unless your PDF extraction backend can supply text.

The current environment used to assemble this package does not have `cargo` installed, so the package was not compiled in-container. The code is organized for normal Rust compilation, but the first local build may still reveal minor version-specific issues, especially around Tantivy minor-version APIs.

## Non-interactive CLI mode for tests and LLM-driven debugging

The CLI includes automation-safe paths. These commands do not require a real TTY and can be driven by pipes, files, JSON requests, shell scripts, or another LLM.

Create a character from plain stdin:

```bash
echo "tech/netrunner, local fixer tie-in, partial choices only" \
  | cargo run -p trpg-cli -- create-character \
      --ruleset cyberpunk_red \
      --module cyberpunk_red.homecoming \
      --stdin \
      --stream-format jsonl \
      --no-save
```

Create a character from a JSON request:

```bash
printf '%s' '{"ruleset_id":"cyberpunk_red","module_id":"cyberpunk_red.homecoming","user_preferences":"tech/netrunner, local fixer tie-in"}' \
  | cargo run -p trpg-cli -- create-character \
      --request-json - \
      --stream-format jsonl \
      --no-save
```

Run one GM turn without entering the interactive play shell:

```bash
echo "我检查无人机背后的线缆。" \
  | cargo run -p trpg-cli -- turn \
      --ruleset cyberpunk_red \
      --module cyberpunk_red.homecoming \
      --stdin \
      --stream-format jsonl
```

The supported stream formats are:

```text
text   human-readable stream; metadata goes to stderr, LLM deltas go to stdout
jsonl  one JSON event per line; best for tests and tool calls
sse    raw Server-Sent Events, matching the Axum API style
```

For regression tests, prefer `--stream-format jsonl`. Example filter:

```bash
cargo run -p trpg-cli -- create-character --request-json request.json --stream-format jsonl --no-save \
  | jq -r 'select(.event == "delta") | .data'
```

The interactive `play` shell also exits cleanly when stdin reaches EOF, so scripted smoke tests no longer loop forever after a pipe closes.

## v0.8.4 Rust harness and learning write-side

The harness is Rust-native:

```bash
cargo build -p trpg-cli -p trpg-harness
cargo run -p trpg-harness -- suite --dir harness/cases --bin ./target/debug/trpg --cwd .
```

`trpg turn` now emits a `learning_audit` event after memory is saved. The audit writes `rulings_log` and review-gated `learning_candidates`; it does not write durable `learned_packets` unless explicitly approved or `TRPG_LEARNING_AUTO_PROMOTE=true` is set for a controlled experiment.

Review and promote candidates:

```bash
trpg learn candidates --ruleset cyberpunk_red
trpg learn approve <candidate_id> --stage used_once --notes "source checked"
```

## v0.9 Rust GM Agent: Agentic Checks and Dice Flow

v0.9 adds a Rust-owned GM Agent Runtime for check framing, roll visibility, pending player rolls, public GM rolls, private GM rolls, and audit logging. The LLM is not the agent controller; Rust owns the state machine and calls the LLM only for bounded narration or JSON skills.

Key points:

- Dice/check advice is stored as JSON under `data/agent/advice/`, not hardcoded into Rust and not merged into the system prompt.
- `CheckContract`, `PendingCheck`, `DiceRollRecord`, and `AgentTurnPlan` are persisted in PostgreSQL.
- SSE/JSONL can stop at `pending_check_created` when the player must roll.
- SSE/JSONL can emit `dice` and continue narration when a public GM/system roll is allowed.
- Secret rolls emit GM/debug `tool` events but must not reveal metagame information in player-facing text.
- The Rust harness now asserts agent/check events in addition to final text.

Useful cases:

```bash
cargo run -p trpg-harness -- run \
  --case harness/cases/homecoming_tech_analysis_requires_pending_check.json \
  --bin ./target/debug/trpg \
  --cwd .

cargo run -p trpg-harness -- run \
  --case harness/cases/homecoming_secret_roll_no_meta_leak.json \
  --bin ./target/debug/trpg \
  --cwd .
```


## v1.0 Interaction Gates and Working State Frames

v1.0 adds a stricter Rust-owned interaction protocol before the full CombatAgent work:

- Pending checks are now represented as InteractionGates.
- Natural-language roll replies such as `我 TECH 6，Basic Tech 4，掷 1d10 出来是 8` resolve the gate.
- Unparseable roll replies emit `gate_reprompt` instead of silently falling through to narration.
- If the player explicitly starts a different action, the previous gate is closed as `abandoned_by_new_action` and the new action continues with a visible acknowledgement.
- New pending checks supersede older open checks, preventing stale `/roll` input from resolving an old abandoned check.
- Active Working State Frames are projected into BP3 for combat/sidequest/investigation state tracking without destabilizing BP1/BP2 caches.

New harness cases:

```bash
cargo run -p trpg-harness -- run   --case harness/cases/homecoming_pending_check_natural_language_roll_resolves.json   --bin ./target/debug/trpg

cargo run -p trpg-harness -- run   --case harness/cases/homecoming_pending_check_new_action_abandons.json   --bin ./target/debug/trpg
```


## v1.0 Conflict Frame & Combat Agent

v1.0 adds a Rust-owned Conflict/Combat Agent. It does not hardcode a single combat engine. Instead it uses:

- `StateFrame` as temporary working memory for combat, chase, netrun, anomaly encounter, horror encounter, social conflict, hazard sequence, investigation node, side quest, and downtime project.
- `InteractionGate` for required/optional choices and player roll interruptions.
- `CheckContract` for uncertainty resolution.
- `EffectContract` for damage, Harm, SAN loss, armor, resource spend, conditions, position, Chaos, Loose Ends, major wounds and other effects.
- `FrameCompaction` to keep only durable consequences after the frame ends.
- `data/ruleset_advice/*.json` for Cyberpunk RED, D&D 5e, CoC/BRP, Triangle Agency, and Sword World combat/conflict profiles.

Useful v1.0 harness cases:

```bash
cargo run -p trpg-harness -- run --case harness/cases/cyberpunk_combat_frame_starts.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/cyberpunk_required_reaction_blocks_unrelated_action.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/combat_frame_compaction.json --bin ./target/debug/trpg --cwd . --output text
```

Relevant docs:

- `docs/product/conflict_frame_combat_agent_prd_v1_0.md`
- `docs/design/conflict_frame_combat_agent_v1_0.md`

## v1.1 Semantic Situation Orchestrator

This build replaces the v1.0 keyword-locked combat path with a Rust-owned semantic situation router. Combat, negotiation, investigation, chase, infiltration, side quests, anomaly encounters, and horror encounters are treated as temporary `StateFrame`s with progress, exits, NPC drive/morale/patience, and compaction.

New stream phases include:

- `situation_intent_classified`
- `exit_contract_created`
- `conflict_direction_gate_opened`

The previous keyword helpers in `trpg-combat` are no longer the main path. The current classifier is a Rust paraphrase-similarity skill over multilingual examples; the public contract is `ConflictIntent`, so a later LLM JSON classifier can be swapped in without changing the rest of the runtime.

New harness cases cover Chinese/Japanese combat-start paraphrases, hide/disengage direction gates, de-escalation, and retreat exits.

## v1.3 Actionable Situation Director

v1.3 adds a Rust-owned director layer before LLM narration. It turns the current frame/context into an `ActionableSituationBrief` so the GM presents playable situations instead of only atmospheric prose or a single official-looking route.

New concepts:

- `ActionableSituationBrief`: visible facts, pressure, affordances, risks, known facts, open questions, guidance level, goal prompt.
- `PlayerFacingClueBoard`: public known facts and open leads without GM-only truth.
- `ConsequenceContract`: fail-forward success/failure consequences.
- `ClockTick`: visible pressure changes when waiting/repetition matters.
- `SpotlightState`: scaffolding for player spotlight rotation.
- `trpg-director`: deterministic director crate that injects BP3 guidance context before narration.

New env:

```env
TRPG_ACTIONABLE_DIRECTOR_ENABLE_V12=true
TRPG_DIRECTOR_BRIEF_EVERY_TURN=false
TRPG_DIRECTOR_DEFAULT_GUIDANCE_LEVEL=ask_goal
TRPG_DIRECTOR_CLOCK_ON_STALL=true
TRPG_DIRECTOR_MIN_COSTED_EXAMPLES=3
```

New harness examples:

```bash
cargo run -p trpg-harness -- run --case harness/cases/director_scene_opening_has_four_anchors.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/director_stuck_player_guidance_ladder.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/director_npc_advice_is_biased.json --bin ./target/debug/trpg --cwd . --output text
```


## v1.3 Situation Novelty Director

v1.3 adds an anti-repetition layer. The system now treats novelty as state, not prose:

- `FreshChange` records what is new this turn.
- `NoveltyDecision` records whether an NPC tactic must shift.
- `BeatSignature` records the actor/tactic/target/consequence used this turn.
- `TacticPalette` and `TacticCooldown` prevent NPCs from repeating the same response.
- Direction gates now accept natural “continue attacking / keep firing / /roll” replies instead of reprompt-looping.

New harness cases:

```bash
cargo run -p trpg-harness -- run --case harness/cases/cyberpunk_enemy_initiated_combat_starts.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/stalemate_continue_pressure_resumes.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/combat_repeated_player_attack_npc_changes_tactic.json --bin ./target/debug/trpg --cwd . --output text
```

## v1.4 World Time Spine

v1.4 adds an authoritative in-fiction time spine.

New commands:

```bash
trpg time show --session <id>
trpg time advance --session <id> --minutes 5 --reason "players delay under fire"
trpg time schedule --session <id> --in-minutes 3 --kind system_event --payload-json '{"label":"security arrives"}'
trpg time events --session <id> --since-tick 0
```

Runtime turns now emit `phase:world_time` and record player/assistant turn events on the world timeline. Context compilation includes exact world time in BP3 and uses `context_watermarks` for incremental dynamic event loading.

Configuration:

```env
TRPG_WORLD_TIME_ENABLE_V14=true
TRPG_WORLD_TIME_START_DISPLAY=Day 1, 00:00
TRPG_WORLD_TIME_CALENDAR_ID=relative_default
TRPG_WORLD_TIME_CONTEXT_EVENT_LIMIT=24
TRPG_WORLD_TIME_AUTO_ADVANCE_PER_TURN_SECONDS=0
```

## v1.5 Interaction Lifecycle Kernel

v1.5 adds a Rust-owned lifecycle kernel for all mutable interaction state. The kernel reconciles sessions before gate routing, cascades frame-close cleanup to child gates and pending checks, and increments `interaction_generation` to prevent stale gates from hijacking later turns.

New config:

```env
TRPG_INTERACTION_KERNEL_ENABLE_V15=true
TRPG_INTERACTION_RECONCILE_EACH_TURN=true
TRPG_INTERACTION_BUMP_GENERATION_ON_FRAME_CLOSE=true
TRPG_INTERACTION_CASCADE_CLOSE_CHILDREN=true
```

New docs:

- `docs/design/interaction_lifecycle_kernel_v1_5.md`
- `docs/product/interaction_lifecycle_kernel_prd_v1_5.md`

## v1.5 Interaction Lifecycle Kernel

v1.5 adds a Rust-owned lifecycle kernel for mutable interaction state.

```env
TRPG_INTERACTION_KERNEL_ENABLE_V15=true
TRPG_INTERACTION_RECONCILE_EVERY_TURN=true
TRPG_INTERACTION_CASCADE_CLOSE_FRAME=true
TRPG_INTERACTION_GENERATION_GUARD=true
```

The kernel enforces:

- closed frames cascade-close child gates and pending checks;
- stale-generation gates cannot capture new player input;
- terminal intents such as ceasefire, surrender, retreat, and scene-end may supersede required reaction gates;
- every turn can reconcile stale/orphaned state and record `InvariantRepair` events.

Relevant docs:

- `docs/design/interaction_lifecycle_kernel_v1_5.md`
- `docs/product/interaction_lifecycle_kernel_prd_v1_5.md`

## v1.5 Interaction Lifecycle Kernel

v1.5 hardens the lifecycle of temporary interaction states. The recurring class of bugs was not that combat or direction gates were individually unfinished; it was that open gates, pending checks, and frame-local obligations could survive after their owning frame had ended.

New runtime layer:

```text
InteractionLifecycleKernel
  → reconcile_session
  → ensure_frame_context
  → attach_gate_to_frame
  → attach_pending_check_to_frame
  → close_frame cascade
  → supersede stale/terminal gates
```

New config:

```env
TRPG_INTERACTION_KERNEL_ENABLE_V15=true
TRPG_INTERACTION_RECONCILE_EVERY_TURN=true
TRPG_INTERACTION_GENERATION_GUARD=true
TRPG_INTERACTION_CASCADE_CLOSE_FRAME=true
```

New migration:

```text
migrations/0011_interaction_lifecycle_kernel_v15.sql
```

New harness examples:

```bash
cargo run -p trpg-harness -- run \
  --case harness/cases/interaction_enter_exit_reenter_no_stale_gate.json \
  --bin ./target/debug/trpg \
  --cwd . \
  --output text

cargo run -p trpg-harness -- run \
  --case harness/cases/interaction_ceasefire_supersedes_reaction_gate.json \
  --bin ./target/debug/trpg \
  --cwd . \
  --output text
```

## v1.6 Object & Possession Kernel

v1.6 adds a runtime object graph. It supports weapons, armor, tools, cables, locks, doors, devices, vehicles, clues, documents, quest items, and environmental objects as stateful instances rather than narration-only props.

New env flags:

```env
TRPG_OBJECT_KERNEL_ENABLE_V16=true
TRPG_OBJECT_INTERACTION_AUTOCONTRACT=true
TRPG_OBJECT_CONTEXT_BP3=true
TRPG_OBJECT_COMPACTION_ENABLE=true
TRPG_OBJECT_DEFAULT_DISARM_TARGET=15
```

New event phases include:

```text
object_kernel
object_interaction_contract_created
object_event
```

Example harness cases:

```bash
cargo run -p trpg-harness -- run --case harness/cases/object_disarm_creates_contract.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/object_cable_cut_creates_contract.json --bin ./target/debug/trpg --cwd . --output text
```

## v1.7 Turn Orchestration Kernel

v1.7 adds a single turn reducer before gate, object, combat, director, and generic agent routing. This prevents frame-worthy player inputs from being captured by stale gates or generic pending checks.

Configuration:

```env
TRPG_TURN_ORCHESTRATOR_ENABLE_V17=true
TRPG_TURN_ORCHESTRATOR_GATE_RELEVANCE=true
TRPG_TURN_ORCHESTRATOR_FRAME_FIRST=true
TRPG_TURN_ORCHESTRATOR_OBJECT_AS_CHILD=true
TRPG_TURN_ORCHESTRATOR_DISABLE_GENERIC_CHECK_FOR_FRAME_ACTION=true
```

Important harness cases:

```bash
cargo run -p trpg-harness -- run --case harness/cases/orchestrator_enter_exit_reenter_frame_created.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/orchestrator_direction_gate_continue_attack.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/orchestrator_object_inside_frame_child_action.json --bin ./target/debug/trpg --cwd . --output text
```

## v1.8 Runtime Material Binding & Parameter Hydration

v1.8 adds a runtime parameter hydrator. Lazy loading remains the policy, but actors and objects now receive runtime parameters when they appear.

- `trpg-params` creates `runtime_actor_parameters` from CharacterTemplate + ruleset defaults.
- `trpg-object` now uses session-scoped object ids and seeds ruleset-aware weapon profiles from narration/input.
- Object self-selection is narrowed: hypothetical assessment, analytical questions, and GM-secret requests no longer become object contracts.
- Required reaction gates block unrelated object/attack intents unless the player gives a terminal/exit intent.

Relevant tables:

```text
runtime_actor_parameters
material_hydration_events
object_definitions
object_instances
```

## v1.9 Semantic Rule Binding & Ability Hydration Kernel

v1.9 adds semantic materialization and ability hydration. New crates:

```text
crates/trpg-semantics
crates/trpg-ability
```

New runtime tables:

```text
semantic_classification_events
rule_binding_packets
ability_definitions
ability_instances
ability_trigger_bindings
ability_activation_contracts
```

Configuration:

```env
TRPG_SEMANTIC_PRIMARY=true
TRPG_SEMANTIC_CLASSIFIER_ENABLE_V19=true
TRPG_SEMANTIC_EXTRACTOR_ENABLE_V19=true
TRPG_LEXICAL_FALLBACK_ENABLE=false
TRPG_LEXICAL_FALLBACK_AUDIT_ONLY=true
TRPG_ABILITY_KERNEL_ENABLE_V19=true
TRPG_RULE_BINDING_ENABLE_V19=true
TRPG_RULE_BINDING_REQUIRE_RUNTIME_WRITEBACK=true
TRPG_ABILITY_CONTEXT_BP3=true
```

Principle: grep/Tantivy can retrieve candidates, but semantic tools decide intent, extraction, and runtime writeback.

## v1.9.1 Semantic Route Stability Hotfix

v1.9.1 keeps semantic classification but makes Rust the final route arbiter. It fixes the v1.9 report findings where LLM-primary routing made identical inputs route differently across runs.

Key behavior:

- disarm/grab reliably routes to the object kernel;
- technical risk assessment routes to agentic checks instead of free narration;
- required reaction gates cannot be superseded by ordinary object/attack intent;
- direction gates accept continued pressure/attack;
- object, ability, conflict, director, and agent paths only run when selected by `TurnOrchestrator`.

New configuration:

```env
TRPG_SEMANTIC_ROUTE_REDUCER_STRICT=true
TRPG_SEMANTIC_ROUTE_CACHE_ENABLE=true
TRPG_SEMANTIC_RECORD_REPLAY_ENABLE=false
TRPG_DIRECTION_GATE_AUTO_DEFAULT_AFTER_REPROMPTS=2
```

## v1.10 Real Materialization Extractor

v1.10 adds `trpg-material`, a runtime materialization pipeline for actor, object, ability, check, and effect parameters.

New environment flags:

```env
TRPG_REAL_MATERIALIZATION_ENABLE_V110=true
TRPG_REAL_MATERIALIZATION_EXTRACTOR_ENABLE_V110=true
TRPG_MATERIALIZATION_REQUIRE_SOURCE_OR_PROVISIONAL_REASON=true
TRPG_MATERIALIZATION_BLOCKING_FOR_MECHANICS=true
TRPG_MATERIALIZATION_CONTEXT_BP3=true
TRPG_MATERIALIZATION_PREFER_MODULE_CARDS=true
```

Typical stream phases:

```text
materialization_kernel
materialization_demand_created
source_evidence_collected
extraction_run_created
rule_binding_packet_created
binding_verification_created
runtime_writeback_applied
```

## v1.10.1 Simulation-Guided Hotfix

v1.10.1 fixes several route/lifecycle regressions found in the v1.10 report:

- `trpg-material` no longer double-wraps `narration_context`.
- Technical risk assessment now forces an agentic check and blocks free narration.
- Enemy-initiated combat frame starts open a required reaction gate immediately.
- Required reaction reprompts emit `reaction_window_opened` and `awaiting_required_reaction`.
- Direct incoming attacks take priority over ordinary object intentions.

A manual golden trace for the Homecoming opening is recorded at:

```text
harness/scenarios/homecoming_opening_semantic_trace.json
```

Recommended smoke tests:

```bash
cargo run -p trpg-harness -- run --case harness/cases/homecoming_tech_analysis_requires_pending_check.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/cyberpunk_required_reaction_blocks_unrelated_action.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/interaction_terminal_intent_supersedes_required_reaction.json --bin ./target/debug/trpg --cwd . --output text
cargo run -p trpg-harness -- run --case harness/cases/stalemate_continue_pressure_resumes.json --bin ./target/debug/trpg --cwd . --output text
```

## v1.10.2 Player-Supplied Value Referee

Player-supplied mechanical numbers are now treated as claims, not automatic truth. The runtime records and verifies supplied damage totals, damage expressions, DV/DC/TN values, HP, armor, and related parameters before narration consumes them.

New events:

```text
phase:player_value_referee
phase:player_value_claim_detected
phase:player_value_verified
phase:table_override_proposed
```

New config:

```env
TRPG_PLAYER_VALUE_REFEREE_ENABLE_V1102=true
TRPG_PLAYER_VALUE_ALLOW_TABLE_OVERRIDE=true
TRPG_PLAYER_VALUE_REQUIRE_RULE_OR_TABLE_CHECK=true
```

The GM should check rules and relevant object/ability tables first. If a player insists on an out-of-band value, the system records a table override and warns that balance and rules design may be affected.

## v1.10.2 Referee Combat Slice

v1.10.2 now includes the intended referee-combat work, not only player-value verification.

New crate:

```text
crates/trpg-mechanics
```

New runtime tables:

```text
actor_mechanical_states
attack_resolution_contracts
damage_packets
combat_round_events
```

Behavior:

- Successful attack checks create an `AttackResolutionContract`.
- A successful hit opens a follow-up damage pending check.
- Damage rolls create a `DamagePacket` and persist HP changes in `actor_mechanical_states`.
- The BP3 context includes a Mechanical Ledger so the GM does not ask players for remaining HP.
- Player-supplied values are still checked by the referee policy, but that policy is a submodule, not the whole release.

## v1.11 Contest / Opposition Kernel

v1.11 adds typed contest profiles between `CheckContract` and final resolution. Attacks and other mechanical checks should no longer resolve as final `NoMechanicalOpposition` / `UnknownUntilLookup`; the kernel creates static-DV, attack-vs-defense, opposed, percentile roll-under, saving throw, or provisional ruleset-procedure models.

New config:

```env
TRPG_CONTEST_KERNEL_ENABLE_V111=true
# TRPG_CONTEST_DEFAULT_PERCENTILE_SKILL=50  # optional table override only; default unset
TRPG_CONTEST_CONTEXT_BP3=true
TRPG_CONTEST_REQUIRE_MODEL_FOR_CHECK=true
```

Representative harness cases:

```bash
cargo run -p trpg-harness -- run \
  --case harness/cases/contest_attack_creates_profile.json \
  --bin ./target/debug/trpg \
  --cwd . \
  --output text

cargo run -p trpg-harness -- run \
  --case harness/cases/contest_tech_assessment_static_dv.json \
  --bin ./target/debug/trpg \
  --cwd . \
  --output text
```

## v1.12 Mechanics Search Skills & Parameter Binding

v1.12 adds reusable, multi-step search skills over the generic GrepSearch/Tantivy retriever. These skills are demand-oriented rather than ruleset-specific: combat resolution, weapon parameters, armor/defense, ability activation, condition/resource, NPC statblocks, module cards, and scene objects.

Search skills produce `MechanicsQueryPlan` records and write extracted results back as `ParameterFacetBinding` records on the existing actor/object/ability/check/effect parameter system. They do not create a parallel procedure engine.

New runtime records:

```text
mechanics_query_plans
parameter_facet_bindings
search_skill_profiles
```

New env flags:

```env
TRPG_MECHANICS_SEARCH_SKILLS_ENABLE_V112=true
TRPG_MECHANICS_SEARCH_MULTI_STEP=true
TRPG_MECHANICS_SEARCH_USE_RULESET_LOCATORS=true
TRPG_MECHANICS_SEARCH_WRITE_FACETS=true
TRPG_MECHANICS_SEARCH_CONTEXT_BP3=true
TRPG_MECHANICS_SEARCH_GREP_CANDIDATES_ONLY=true
```

## v1.12.1 Unified Roll & Effect Executor

v1.12.1 connects the mechanics search/parameter facet layer to actual roll execution and runtime state patches:

```text
RollPlan → DiceTool → EffectResolutionPacket → ParameterImpact → MechanicalLedger
```

The key product change is that damage is no longer assumed to mean HP damage. HP damage is represented as an `ActorHpDelta` and compatibility `DamagePacket`, but the canonical result is now an `EffectResolutionPacket` containing one or more `ParameterImpact` rows. This supports HP, SAN, Chaos, Harm, conditions, object durability/connection state, scene clocks, and other target parameters.

New tables:

- `roll_plans`
- `effect_resolution_packets`
- `parameter_impacts`

New configuration:

```env
TRPG_UNIFIED_ROLL_EFFECT_EXECUTOR_ENABLE_V1121=true
TRPG_AGENT_TABLE_DICE_POLICY=system_rolls_visible
TRPG_EFFECT_CONTEXT_BP3=true
TRPG_EFFECT_ALLOW_PROVISIONAL_TARGET_PARAMETER=false
TRPG_STRICT_SOURCE_BACKED_MATERIALIZATION=true
TRPG_FAIL_ON_MISSING_SOURCE_BACKED_PARAMS=true
TRPG_ALLOW_SYNTHETIC_ACTOR_SEEDS=false
TRPG_EFFECT_REQUIRE_PARAMETER_IMPACT=true
```

A player can still report a roll, but `/roll 3d6`, `/roll`, “你来投”, or “roll for me” now route to the Rust dice tool in turn mode. The GM should not ask the player for remaining HP/SAN/Chaos/Harm or for rules-table parameters when the system can read or materialize them.

### v1.12.2 Roll Binding & Mechanical Gate Priority

This patch binds `/roll` to the current unresolved mechanical check/effect, prevents direction gates from eating damage rolls, and makes system-visible dice policy operational for combat/effect checks. It is a hotfix on top of v1.12.1, not a full rules engine.

New defaults:

```env
TRPG_ROLL_BINDING_HOTFIX_ENABLE_V1122=true
TRPG_ROLL_BINDING_USE_LATEST_UNRESOLVED_CHECK=true
TRPG_DIRECTION_GATE_IS_ADVISORY=true
TRPG_AUTO_RESOLVE_SYSTEM_EFFECT_ROLLS=true
```

### v1.13 Parameter Facet Executor

v1.13 keeps the existing parameter system as the single runtime source of truth. Cyberpunk RED armor/SP, D&D AC/save/spell effects, Sword World power tables, CoC/BRP SAN, and Triangle Harm/Chaos are modeled as facets and starter mechanical profiles rather than separate hardcoded rules engines.

New tables:

- `parameter_facet_execution_runs`
- `generic_parameter_states`
- `ruleset_mechanical_profiles`

New config:

```env
TRPG_PARAMETER_FACET_EXECUTOR_ENABLE_V113=true
TRPG_PARAMETER_FACET_EXECUTOR_USE_BOUND_FACETS_FIRST=true
TRPG_PARAMETER_FACET_EXECUTOR_USE_RULESET_STARTER_PROFILES=true
TRPG_PARAMETER_FACET_EXECUTOR_WRITE_GENERIC_STATES=true
TRPG_PARAMETER_FACET_EXECUTOR_CONTEXT_BP3=true
TRPG_PARAMETER_FACET_EXECUTOR_AUDIT_PROVISIONAL=true
```

The executor resolves effect targets in this order: bound parameter facets, runtime object/ability/actor facets, ruleset starter profile, provisional fallback. It emits `EffectResolutionPacket`, `ParameterImpact`, and a facet execution audit row.

## v1.13 Parameter Facet Executor

v1.13 consumes the parameter facets written by Mechanics Search Skills instead of creating ruleset-specific hardcoded engines. It executes existing `actor/object/ability/check/effect` facets and writes the result back through `EffectResolutionPacket`, `ParameterImpact`, `actor_mechanical_states`, and `generic_parameter_states`.

New runtime tables:

```text
parameter_facet_execution_runs
generic_parameter_states
ruleset_mechanical_profiles
```

New defaults:

```env
TRPG_PARAMETER_FACET_EXECUTOR_ENABLE_V113=true
TRPG_PARAMETER_FACET_EXECUTOR_USE_BOUND_FACETS_FIRST=true
TRPG_PARAMETER_FACET_EXECUTOR_USE_RULESET_STARTER_PROFILES=true
TRPG_PARAMETER_FACET_EXECUTOR_WRITE_GENERIC_STATES=true
TRPG_PARAMETER_FACET_EXECUTOR_CONTEXT_BP3=true
TRPG_PARAMETER_FACET_EXECUTOR_AUDIT_PROVISIONAL=true
```

The executor treats HP damage as one possible effect target. It can also apply effects to `resources.sanity.current`, `resources.harm.current`, `tracks.chaos.current`, conditions, object state, anomaly state, scene clocks, and other generic parameters. This keeps Cyberpunk RED armor/SP, D&D AC/save/spell effects, Sword World power tables/resistance, CoC SAN/Major Wound, and Triangle Harm/Chaos as data/facet bindings rather than separate Rust engines.

### v1.13.1 Combat Route Source Object Hotfix

v1.13.1 fixes the named-weapon combat regression: an utterance such as `用重型手枪开火` now remains an attack/combat action. The weapon is treated as a source object to materialize and bind, not as a top-level object interaction that steals the turn. First-turn player attacks also commit their `CheckContract`/`EffectContract` immediately so `/roll` and system auto-roll have a locked mechanical target.

### v1.13.2 Roll/Effect Gate Closure Hotfix

This build fixes the remaining v1.13.1 combat execution-chain bug class: damage/effect rolls honor dice policy, fallback facet matching no longer self-matches prompt boilerplate, roll replies are protected from semantic override, and auto-resolved checks close their matching interaction gates.

### v1.13.3 Semantic Combat Loop & Roll Authority

This release makes sustained combat semantic-first and prevents advisory direction/stalemate gates from blocking clear in-frame actions. Named weapons remain source objects for attacks. `TRPG_AGENT_TABLE_DICE_POLICY=system_rolls_visible` is enforced at the CLI/API execution boundary so combat checks do not intermittently fall back to player-reported rolls.

New toggles:

```env
TRPG_SEMANTIC_COMBAT_INTENT_ENABLE_V1133=true
TRPG_SEMANTIC_COMBAT_LEXICAL_FALLBACK_AUDIT=true
TRPG_SUSTAINED_COMBAT_LOOP_POLICY_V1133=true
TRPG_DETERMINISTIC_ROLL_AUTHORITY_V1133=true
TRPG_DIRECTION_GATE_ADVISORY_FOR_FRAME_ACTIONS=true
```

## v1.14 External Playtest Evaluator

This version adds a test-only external playtest evaluator. It keeps the production LLM GM on the configured API backend, but allows `trpg-harness` to call Claude Code as an outside judge after each turn.

```bash
cargo run -p trpg-harness -- playtest \
  --scenario harness/scenarios/homecoming_external_playtest_8turn.json \
  --bin ./target/debug/trpg \
  --cwd . \
  --evaluator none
```

Use `--evaluator claude-code` to call `claude -p` for a structured per-turn judgment. Each turn writes `player_visible_output.txt`, `events.jsonl`, `db_snapshot_before.json`, `db_snapshot_after.json`, `db_diff.json`, `eval_prompt.md`, and `eval_result.json` under `data/playtests/<run_id>/`.

This is designed to catch problems that phase-only harness cases miss, such as narration-only HP changes, stale menus, missing parameter impacts, hidden spoiler leaks, or the GM asking the player for rule-table values.

## v1.15 Product Evaluation Skill

The repo includes a Claude Code / subagent skill for evaluating chatrpg as a full solo LLM-GM TRPG product rather than only as a Rust codebase:

```text
.claude/skills/chatrpg-product-evaluator/SKILL.md
```

Use it when asking Claude Code or a subagent to assess a build. The skill requires build checks, harness checks, black-box playtest artifacts, DB/event verification, GM/referee scoring, UX/fun scoring, NPC/world simulation review, and weird-player robustness testing.

Key support files:

```text
harness/evaluation/product_rubric_v1.json
harness/evaluation/evaluation_report_template.md
harness/evaluation/strange_player_action_bank.json
prompts/evaluation/trpg_product_evaluator.md
schemas/product_evaluation_report.schema.json
```


## v1.15.1 — Human-like player simulation and debug directives

Product evaluation now distinguishes player behavior from test setup. Simulated players should use short, natural TRPG player actions and avoid JSON, code, SQL, DB table names, Rust type names, phase/event names, and bundled QA assertions.

For rare-state setup, playtest scenarios may include test-only debug blocks:

```text
[debug]add {"id":"npc.scav_boss","kind":"npc","hp":35,"weapon":"shotgun"}[/debug]
我朝拿霰弹枪的头目开火，然后立刻缩回掩体。
```

The harness strips `[debug]...[/debug]` blocks before sending input to the GM, records them as artifacts, and injects the resulting state as GM-only test context. This allows create/delete/modify of test actors, objects, clues, resources, clocks, and scene facts without polluting player-facing speech.

The evaluator now treats manual dice commands and reported roll/damage totals as compatibility-mode behavior, not default product play. In product mode, the player describes fictional action only; the system plans, rolls, resolves, persists state, narrates consequences, and stops at the next player decision.


## v1.15.2 — Action-only player protocol

Product evaluation now enforces the intended single-player LLM-GM loop: players describe fictional actions only. They should not type `/roll`, report `1d10`/`3d6` results, provide damage totals, or supply DV/SP/AC/SAN/Chaos/Harm values. The GM/system is expected to infer when mechanics are needed, call the dice tool, resolve hit/effect/resource changes, write the ledger, narrate the outcome, and continue until the next meaningful player decision. Manual-roll tests are allowed only when a scenario explicitly sets `allow_manual_roll_input=true`.
