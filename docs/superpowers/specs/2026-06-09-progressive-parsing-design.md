# Progressive (staged) ruleset parsing + live progress

Status: design (2026-06-09). Approved direction: turn the ~12-minute monolithic
ruleset parse into a 3-stage progressive pipeline so the user can start *doing
something* (writing a character background, then filling a character sheet)
within seconds–minutes, while the heavy parameter extraction finishes in the
background — with parse progress reported at all times.

## Problem
`ProjectParseService::parse_all` is synchronous and monolithic: per source it runs
ingest → `reader_parallel` → `chargen_compile` → `object_compile` → assemble
kernel → `module_reader` → reindex, then returns. Measured on CoC (40th Anniv.,
~430 pp): **~12 min total** — ingest+reader ≈ 2 min, chargen+object ≈ 8 min,
module+index ≈ 1.5 min. The user stares at a blank screen the whole time. Yet
everything needed to *create a character* (the character-sheet schema + the dice
notation in the creation flow) is ready after the ~2-min reader; the remaining
~8 min is play-time data (derived-value formulas, weapon/spell schemas, module
cold data, search index) the user does not need to start.

## Goal
After upload, surface a usable minimum fast, defer heavy parameter parsing to the
background, keep the user busy (write background → fill sheet), and always show
progress. Deliver this as **backend pipeline + progress API + a CLI test harness
(no UI — the API is the integration surface).**

## Out of scope / parked
- **Editable markdown kernel doc + reuse/audit-edit** (load-from-doc, hand-edit
  authority): parked at user's request ("之后再说"). Today a `rule_kernel.json`
  artifact already exists under `data_dir/parsed/rules/`, and reuse already works
  via `source_hash + parse_config_hash` bundle cache. Not touched here.
- **Module reader internals.** `reader/module_reader.rs` / `module_reader_loop.rs`
  are under active development by another worker. This design treats
  `run_module_reader` as a **black-box Stage-2 step** — sequenced, never edited.

## Current state (grounded — read 2026-06-09)
- `trpg-parser` `parse_all` (line ~114) loops sources; `parse_source` (~480–880)
  per source: `reader::run_reader_parallel` (501) → `compile_chargen_formulas`
  (559) → `compile_object_schemas` (580) → assemble `CharacterOnboardingPack` +
  `rule_kernel_from_run_kit` (692) → `run_module_reader` (865) → reindex (in
  `parse_all` tail). Artifacts written to `data_dir/parsed/rules/*.json`.
- `reader_parallel` = `plan_phase` (1 LLM call on the TOC → identity/hypothesis +
  3 slice hints) → 3 slices (`resolution`, `character`, `gm`) run concurrently,
  awaited together → `GmRunKit`. The `character` slice alone yields
  `character_template` (+ `option_catalogs`).
- `RuleKernel` (trpg-model ~1370) is `Serialize/Deserialize/Default`, persisted via
  `Db::upsert_rule_kernel` → `rule_kernels.content_json` jsonb; every field is
  `#[serde(default)]` → **partial kernels round-trip cleanly** (missing fields
  default). Loaded by `Db::load_rule_kernel(ruleset_id)`.
- Job infra EXISTS: `Db::insert_background_job(job_id, kind, input_json)` /
  `update_background_job(job_id, status, result_json, error)` (used today by
  `postprocess_character`).
- `trpg-api` is an axum `Router` with SSE already imported
  (`axum::response::sse::{Event, KeepAlive, Sse}`); routes include
  `POST /api/ingest/parse-all`.

## Design

### Staged orchestrator (new — `trpg-parser/src/staged.rs`)
A new entry point `parse_ruleset_staged(service, source, job_id)` runs three phases,
each: do work → incrementally persist artifacts → `update_background_job` with a
structured status. It REUSES the existing reader/compile calls; it does NOT rewrite
`parse_source`. `reader_parallel` is refactored into three separately-callable
entry points so phases can own them:
- `reader::plan_phase(...)` (already exists, made `pub`) → identity/premise/hints.
- `reader::read_character_slice(client, units, ruleset, hints, budget)` → the
  `character` slice result (template + option_catalogs).  [extracted from `parallel.rs`]
- `reader::read_resolution_and_gm(...)` → resolution + gm slices (Stage 2).

The legacy `run_reader_parallel` stays as a thin wrapper (plan + all three) so the
synchronous CLI `parse-all` path is unchanged.

### The three stages
| Stage | Runs | Persists (incremental) | Unblocks | ~time |
|---|---|---|---|---|
| **0 identity** | `plan_phase` | ruleset stub: id/title/`game_identity` (partial kernel) | write **background/persona** | ~10 s |
| **1 character** | `read_character_slice` (opt. 1: emitted without waiting for resolution/gm) | `character_template` (prose derived_values) + `option_catalogs` → onboarding-pack artifacts + partial kernel (`character_sheet_schema`) | **fill the sheet** (stats/skills/gear) | ~1.5 min |
| **2 deep (bg)** | resolution+gm slices (∥) → `chargen_compile` → object **discover** (opt. 2) → `run_module_reader` (black box) → reindex; finalize kernel | full kernel: `dice_core`/`check_model`/`resource_tracks`, **compiled derived formulas**, object-schema **stubs**, module cold data, Tantivy index | derived values backfill; **playable** | +6–9 min |

Rationale for `resolution` in Stage 2: creation-time rolls (e.g. CoC 3d6 for STR)
use only the dice notation carried in `creation_flow`; the full check model /
success bands are play-time. So Stage 1 needs no resolution slice → fastest sheet.

### Derived-value backfill
Stage 1's `character_template` carries the reader's **prose** `derived_values` (HP =
…, Sanity = …) but not machine formulas. The user fills INPUT fields immediately;
DERIVED fields show "computing…". `chargen_compile` is ordered **first in Stage 2**,
so its compiled formulas land (kernel + template) typically before the user finishes
the sheet; the client recomputes derived values once Stage 2 reports `chargen` done.

### Optimization 1 — character slice emitted early
Stage 1 awaits only the `character` slice (≈1.5 min) instead of all three reader
slices (≈2 min). `resolution`+`gm` move into Stage 2 and run concurrently there.

### Optimization 2 — object schemas: discover-now, extract-on-demand
Stage 2 runs only `object_compile` **discover** (the 2-round category list +
`source_pages` + `couples_to`), persisting each category as a **stub** in
`kernel.object_schemas` (no `schema_slots`/`examples` yet) — ~1 min instead of the
~5–6 min full extract of all categories. The per-category schema is extracted
**lazily on first materialization need**: `MaterializationService` detects a stub
(category matched by `object_schema_guidance` but missing `schema_slots`), runs
`extract_category` for it, and upserts the filled schema back into the kernel
(cached for next time). A stub carries `{category_id, kind, source_pages, status:
"discovered"}`; a filled schema flips `status: "compiled"`. This is a separable
final phase: if deferred, Stage 2 can fall back to full extract.

### Job / progress model
`background_jobs` row, `job_kind = "ruleset_parse_staged"`, `result_json`:
```json
{ "ruleset_id": "...", "stage": "stage1",
  "stages": [ {"name":"identity","status":"done","started_at":..,"finished_at":..,"detail":".."},
              {"name":"character","status":"running","detail":"reading character sheet"},
              {"name":"deep","status":"pending"} ],
  "progress_pct": 35, "current_detail": "...", "error": null }
```
`status` ∈ pending|running|done|failed. Updated after each phase (and at coarse
sub-steps of Stage 2: chargen / object-discover / module / index).

### API surface (trpg-api)
- `POST /api/ingest/ruleset` — body identifies the uploaded source; inserts the
  job, `tokio::spawn`s `parse_ruleset_staged`, returns `{ job_id, ruleset_id }`
  immediately.
- `GET /api/ingest/{job_id}/events` — **SSE** stream of the `result_json` snapshots
  on each update (+ keep-alive); closes when stage=`done`/`failed`.
- `GET /api/ingest/{job_id}/status` — one-shot JSON (poll fallback for non-SSE).
- `GET /api/rulesets/{id}/identity` — `{title, game_identity}` once Stage 0 done
  (else `202`).
- `GET /api/rulesets/{id}/character-template` — the onboarding pack / sheet schema
  once Stage 1 done (else `202` with `{stage, progress_pct}`).
The legacy `POST /api/ingest/parse-all` (synchronous, all-rulesets) stays for batch.

### Quick test harness — CLI (no UI)
A `trpg parse-staged --ruleset <id> --data-dir <dir>` subcommand that runs the
**same** `parse_ruleset_staged` orchestrator **in-process** (no server, no browser)
and streams the result to the terminal: prints each stage transition with elapsed
time, and at Stage 1 dumps the resolved `character_template` (so the fast-path
output is eyeballable immediately). Flags: `--stage1-only` (stop after Stage 1 to
test the fast path in isolation), `--json` (emit the raw job-status snapshots for
scripting). This is the fast, convenient test environment; the HTTP API + SSE
endpoints below are the real integration surface for a future frontend, validated
separately by an API smoke test.

## Error handling
- Per-stage isolation: a failed stage marks itself `failed` + records `error`, but
  **already-`done` stages are never rolled back** — Stage 1 success means the user
  can still build a character even if Stage 2 fails.
- Within Stage 2, each sub-step (chargen / object-discover / module / index) is an
  independent `try`; one failing does not abort the others. Object stubs that never
  get extracted simply fall back to free-form materialization (existing path).
- The job ends `done` if Stage 1 succeeded (Stage 2 partials are non-fatal),
  `failed` only if Stage 0/1 failed.

## Component boundaries / files
- `trpg-parser/src/staged.rs` (NEW, < 400 lines): orchestrator + 3 phase fns +
  job-status helpers. Calls existing reader/compile fns.
- `trpg-parser/src/reader/parallel.rs`: split into `plan_phase` (pub),
  `read_character_slice`, `read_resolution_and_gm`; `run_reader_parallel` becomes a
  wrapper. (Touches a file the module-reader work does not own.)
- `trpg-parser/src/lib.rs`: small — expose incremental kernel upsert helpers; reuse
  `rule_kernel_from_run_kit` with partial inputs.
- `trpg-api/src/lib.rs`: 4 new routes + SSE handler (no HTML).
- `trpg-cli/src/main.rs`: new `parse-staged` subcommand (in-process orchestrator
  driver — the quick test harness; prints stage transitions + Stage-1 template).
- `trpg-material/src/lib.rs`: object-schema **stub → on-demand extract** branch in
  `object_schema_guidance` / `extract` (optimization 2).
- `trpg-db/src/lib.rs`: reuse `background_jobs`; add a tiny typed wrapper for the
  staged-parse status shape if convenient (no schema change).
- **Not touched**: `reader/module_reader.rs`, `reader/module_reader_loop.rs`.

## Testing
- `staged.rs` unit test with a mock LLM: asserts stage ORDER, that the partial
  kernel is readable after Stage 1 (character_sheet_schema present, object_schemas
  empty), and that a forced Stage-2 failure leaves Stage-1 artifacts intact.
- Materialization test: a `discovered` stub triggers `extract_category` once, then
  the filled schema is reused (no second extract).
- API smoke (CLI or test): `POST /api/ingest/ruleset` → poll status → assert
  stage transitions; `/character-template` returns 202 before Stage 1, 200 after.
- Manual: `trpg parse-staged --ruleset call_of_cthulhu_7e --data-dir _rstest_coc`
  on CoC — watch Stage 0/1/2 transitions + elapsed times in the terminal, confirm
  the Stage-1 character_template dumps correctly (and `--stage1-only` returns fast).

## Philosophy guardrails
- Reuse existing primitives (background_jobs, SSE, partial-kernel serde, reader
  slices) — no new infra where the codebase already has it.
- Data-driven & fail-open: object stubs degrade to today's free-form path; missing
  Stage-2 data never blocks character creation.
- Isolation: orchestrator is additive (`staged.rs`); the synchronous `parse-all`
  and the in-flux `module_reader` are left working as-is.
