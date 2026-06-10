# Progressive (Staged) Ruleset Parsing — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
> **VCS note:** `chatrpg-rs-v1.20-formula` is NOT a git repo — replace every "commit" with a **checkpoint**: `cargo build -p <crate>` + run the task's tests, confirm green. Do not `git init`.
> **Collision note:** Do NOT edit `reader/module_reader.rs` or `reader/module_reader_loop.rs` (another worker owns them). `run_module_reader` is called black-box.

**Goal:** Turn the ~12-min monolithic ruleset parse into a 3-stage progressive pipeline (identity → character → deep-background) with a live job/progress model, so a user can write a background then fill a character sheet within ~2 min while heavy parsing finishes in the background.

**Architecture:** A new additive orchestrator (`trpg-parser/src/staged.rs`) runs three phases, each persisting artifacts incrementally (partial `RuleKernel` via existing jsonb+serde-default) and updating a `background_jobs` row. `reader/parallel.rs` is split so the character slice can be emitted before resolution/gm. A CLI `parse-staged` drives it in-process (test harness); HTTP+SSE endpoints expose it for a future frontend. Optimization 2 (object schemas discover-now / extract-on-demand) is a separable final phase touching `trpg-material`.

**Tech Stack:** Rust, tokio, axum (SSE), sqlx/Postgres, serde_json; LLM via `trpg-llm` (codex-relay).

---

## File Structure

- `crates/trpg-rule-agent/src/reader/parallel.rs` (MODIFY) — split into `plan_phase` (pub), `read_character_slice`, `read_resolution_and_gm`; `run_reader_parallel` becomes a wrapper. Stays < 200 lines.
- `crates/trpg-rule-agent/src/reader/mod.rs` (MODIFY) — export the new fns.
- `crates/trpg-parser/src/staged.rs` (CREATE, < 400 lines) — `StagedParse` orchestrator, 3 phase fns, `JobStatus` shape + `report()` helper, incremental kernel upsert.
- `crates/trpg-parser/src/lib.rs` (MODIFY, small) — `pub mod staged;`; make `rule_kernel_from_run_kit` / artifact writers reachable from `staged.rs` (or `pub(crate)`), expose a partial-kernel builder.
- `crates/trpg-cli/src/main.rs` (MODIFY) — `ParseStaged` command + `parse_staged_cli`.
- `crates/trpg-api/src/lib.rs` (MODIFY) — 4 routes + SSE handler.
- `crates/trpg-material/src/lib.rs` (MODIFY) — stub → on-demand `extract_category` (Phase E).
- `crates/trpg-parser/src/staged_status.rs` (CREATE, < 120 lines) — `JobStatus`/`StageState` serde types (kept separate so both parser + api + cli share them; re-exported).

---

## Phase A — Split the reader so the character slice can be emitted alone

### Task A1: Extract `read_character_slice` and `read_resolution_and_gm` from `run_reader_parallel`

**Files:**
- Modify: `crates/trpg-rule-agent/src/reader/parallel.rs`
- Modify: `crates/trpg-rule-agent/src/reader/mod.rs`

Context: today `run_reader_parallel` = `plan_phase` then `tokio::join!(res_fut, char_fut, gm_fut)` then assembles `GmRunKit`. `plan_phase` already exists and returns `Plan { identity, hypothesis, res_hint, char_hint, gm_hint }`. Slices are built with `slice_tools(...)` + `seed(...)` + `run_loop(client, units, sys, seed, tools, budget, is_res)`.

- [ ] **Step 1: Make `plan_phase` and `Plan` pub**

In `parallel.rs` change `struct Plan` → `pub struct Plan` (all fields `pub`) and `async fn plan_phase` → `pub async fn plan_phase`.

- [ ] **Step 2: Add `read_character_slice` returning just the character outputs**

Append to `parallel.rs`:
```rust
/// The character slice in isolation — yields the buildable character template +
/// option catalogs (Stage 1 of staged parsing). Mirrors the char branch of
/// run_reader_parallel but does not run resolution/gm.
pub struct CharacterSlice {
    pub character: String,
    pub character_template: serde_json::Value,
    pub option_catalogs: serde_json::Value,
    pub source_pages: String,
}

pub async fn read_character_slice(client: &dyn LlmClient, units: &[Unit], ruleset: &str, plan: &Plan, budget: usize) -> Result<CharacterSlice> {
    let toc = tools::toc(units, 40);
    let char_tools = slice_tools("submit_character", char_props(), &["character", "character_template", "source_pages"]);
    let char_sys = char_sys_prompt();
    let char_seed = seed(ruleset, &toc, &plan.hypothesis, "character", &plan.char_hint);
    let c = run_loop(client, units, &char_sys, &char_seed, &char_tools, budget + 6, false).await?;
    Ok(CharacterSlice {
        character: c.run_kit.character,
        character_template: c.run_kit.character_template,
        option_catalogs: c.run_kit.option_catalogs,
        source_pages: c.run_kit.source_pages,
    })
}
```
Refactor the inline `char_tools` JSON schema (the big `json!` in `run_reader_parallel`) into `fn char_props() -> Value` and the char system string into `fn char_sys_prompt() -> String`, and call those from BOTH `run_reader_parallel` and `read_character_slice` (DRY — do not duplicate the schema).

- [ ] **Step 3: Add `read_resolution_and_gm`**
```rust
pub struct ResolutionGm {
    pub core_resolution: String,
    pub state_tracks: String,
    pub core: super::run_kit::CoreRules,
    pub game_identity: String,
    pub subsystem_map: String,
    pub gm_procedures: String,
    pub source_pages: String,
}

pub async fn read_resolution_and_gm(client: &dyn LlmClient, units: &[Unit], ruleset: &str, plan: &Plan, budget: usize) -> Result<ResolutionGm> {
    let toc = tools::toc(units, 40);
    let res_tools = slice_tools("submit_resolution", res_props(), &["core_resolution", "core", "source_pages"]);
    let gm_tools = slice_tools("submit_gm", gm_props(), &["game_identity", "subsystem_map", "gm_procedures", "source_pages"]);
    let res_seed = seed(ruleset, &toc, &plan.hypothesis, "resolution", &plan.res_hint);
    let gm_seed = seed(ruleset, &toc, &plan.hypothesis, "gm+world", &plan.gm_hint);
    let (r, g) = tokio::join!(
        run_loop(client, units, &res_sys_prompt(), &res_seed, &res_tools, budget + 5, true),
        run_loop(client, units, &gm_sys_prompt(), &gm_seed, &gm_tools, budget, false),
    );
    let (r, g) = (r?, g?);
    Ok(ResolutionGm {
        core_resolution: r.run_kit.core_resolution, state_tracks: r.run_kit.state_tracks, core: r.run_kit.core,
        game_identity: g.run_kit.game_identity, subsystem_map: g.run_kit.subsystem_map,
        gm_procedures: g.run_kit.gm_procedures,
        source_pages: [r.run_kit.source_pages, g.run_kit.source_pages].join(" ; "),
    })
}
```
Extract `res_props()`, `gm_props()`, `res_sys_prompt()`, `gm_sys_prompt()` the same DRY way.

- [ ] **Step 4: Rewrite `run_reader_parallel` to use the three pieces (no behavior change)**

`run_reader_parallel` = `plan_phase` → run `read_character_slice` and `read_resolution_and_gm` concurrently via `tokio::join!` → assemble the SAME `GmRunKit` as before (use `pick()` for identity fallback). Verify the assembled fields match the previous version field-for-field.

- [ ] **Step 5: Export in `mod.rs`**
```rust
pub use parallel::{plan_phase, read_character_slice, read_resolution_and_gm, run_reader_parallel, CharacterSlice, Plan, ResolutionGm};
```

- [ ] **Step 6: Checkpoint** — `cargo build -p trpg-rule-agent` green; existing reader tests pass (`cargo test -p trpg-rule-agent`). The split is behavior-preserving; the LLM loop is validated live later via `parse-staged`.

---

## Phase B — Job-status types + staged orchestrator

### Task B1: Shared job-status types

**Files:**
- Create: `crates/trpg-parser/src/staged_status.rs`
- Modify: `crates/trpg-parser/src/lib.rs` (add `pub mod staged_status;`)
- Test: inline `#[cfg(test)]` in `staged_status.rs`

- [ ] **Step 1: Write the failing test**
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn progress_and_transitions() {
        let mut s = JobStatus::new("call_of_cthulhu_7e");
        assert_eq!(s.progress_pct, 0);
        s.begin("identity"); s.finish("identity", "premise ready");
        s.begin("character");
        assert_eq!(s.stage, "character");
        assert!(s.stages.iter().any(|x| x.name == "identity" && x.status == "done"));
        assert!(s.progress_pct > 0 && s.progress_pct < 100);
        s.fail("character", "boom");
        assert_eq!(s.error.as_deref(), Some("boom"));
        assert!(s.stages.iter().any(|x| x.name == "character" && x.status == "failed"));
    }
}
```

- [ ] **Step 2: Run test → fails** (`cargo test -p trpg-parser staged_status` → compile error, no `JobStatus`).

- [ ] **Step 3: Implement**
```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const STAGES: &[(&str, u8)] = &[("identity", 5), ("character", 35), ("deep", 100)];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageState { pub name: String, pub status: String, pub detail: String,
    #[serde(default)] pub started_at: Option<String>, #[serde(default)] pub finished_at: Option<String> }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobStatus { pub ruleset_id: String, pub stage: String, pub stages: Vec<StageState>,
    pub progress_pct: u8, pub current_detail: String, #[serde(default)] pub error: Option<String> }

impl JobStatus {
    pub fn new(ruleset_id: &str) -> Self {
        let stages = STAGES.iter().map(|(n, _)| StageState { name: n.to_string(), status: "pending".into(), detail: String::new(), started_at: None, finished_at: None }).collect();
        Self { ruleset_id: ruleset_id.into(), stage: "identity".into(), stages, progress_pct: 0, current_detail: String::new(), error: None }
    }
    fn at(&mut self, name: &str) -> Option<&mut StageState> { self.stages.iter_mut().find(|s| s.name == name) }
    pub fn begin(&mut self, name: &str) { self.stage = name.into(); self.current_detail = format!("{name} started"); if let Some(s) = self.at(name) { s.status = "running".into(); } }
    pub fn finish(&mut self, name: &str, detail: &str) {
        if let Some(s) = self.at(name) { s.status = "done".into(); s.detail = detail.into(); }
        self.progress_pct = STAGES.iter().find(|(n, _)| *n == name).map(|(_, p)| *p).unwrap_or(self.progress_pct);
        self.current_detail = detail.into();
    }
    pub fn fail(&mut self, name: &str, err: &str) { if let Some(s) = self.at(name) { s.status = "failed".into(); s.detail = err.into(); } self.error = Some(err.into()); }
    pub fn note(&mut self, detail: &str) { self.current_detail = detail.into(); }
    pub fn to_value(&self) -> Value { serde_json::to_value(self).unwrap_or_default() }
}
```
(`started_at`/`finished_at` are filled by the orchestrator via an injected timestamp — `staged.rs` passes `chrono::Utc::now()` strings; `staged_status.rs` stays time-free for deterministic tests.)

- [ ] **Step 4: Run test → passes.**
- [ ] **Step 5: Checkpoint** — build + test green.

### Task B2: The staged orchestrator

**Files:**
- Create: `crates/trpg-parser/src/staged.rs`
- Modify: `crates/trpg-parser/src/lib.rs` (`pub mod staged;` + make needed helpers `pub(crate)`: `rule_kernel_from_run_kit`, `write_rule_kernel_artifact`, `coerce_character_template`, `build_compiler_llm`, onboarding-pack assembly).

Context: `ProjectParseService { db: Db, llm: Arc<dyn LlmClient>, config: ParserConfig }`. Kernel persists via `db.upsert_rule_kernel(&kernel)`. Job updates via `db.update_background_job(job_id, status, result_json, error)`.

- [ ] **Step 1: Define the orchestrator skeleton**
```rust
use crate::staged_status::JobStatus;
use anyhow::Result;
use std::sync::Arc;
use trpg_db::Db;
use trpg_llm::LlmClient;
use trpg_rule_agent::reader;

pub struct StagedParse { pub db: Db, pub llm: Arc<dyn LlmClient>, pub ruleset_id: String,
    pub units: Vec<reader::Unit>, pub sidecar_text: Option<String>, pub title: String, pub job_id: String }

impl StagedParse {
    async fn report(&self, st: &JobStatus, status: &str) {
        let _ = self.db.update_background_job(&self.job_id, status, st.to_value(), st.error.as_deref()).await;
    }
    pub async fn run(&self, budget: usize) -> JobStatus {
        let mut st = JobStatus::new(&self.ruleset_id);
        self.report(&st, "running").await;
        let plan = match self.stage0_identity(&mut st).await { Ok(p) => p, Err(e) => { st.fail("identity", &e.to_string()); self.report(&st, "failed").await; return st; } };
        let char = match self.stage1_character(&mut st, &plan, budget).await { Ok(c) => c, Err(e) => { st.fail("character", &e.to_string()); self.report(&st, "failed").await; return st; } };
        // Stage 2 is non-fatal: stage1 success => job 'done' even if deep partially fails.
        self.stage2_deep(&mut st, &plan, &char, budget).await;
        let final_status = if st.error.is_some() && st.progress_pct < 35 { "failed" } else { "done" };
        self.report(&st, final_status).await;
        st
    }
}
```

- [ ] **Step 2: Stage 0 — identity (plan_phase → partial kernel)**
```rust
async fn stage0_identity(&self, st: &mut JobStatus) -> Result<reader::Plan> {
    st.begin("identity"); self.report(st, "running").await;
    let toc = reader::tools_toc(&self.units); // thin pub helper OR reader::plan_phase reads toc itself
    let plan = reader::plan_phase(self.llm.as_ref(), &self.ruleset_id, &toc).await?;
    // Persist a partial kernel carrying identity/premise so /identity can serve it.
    let mut kernel = trpg_model::RuleKernel { kernel_id: format!("{}.rule_kernel.v1", self.ruleset_id), ruleset_id: self.ruleset_id.clone(), version: "v1_staged".into(), ..Default::default() };
    kernel.game_identity = serde_json::json!({"summary": plan.identity, "hypothesis": plan.hypothesis});
    self.db.upsert_rule_kernel(&kernel).await.ok();
    st.finish("identity", &plan.identity); self.report(st, "running").await;
    Ok(plan)
}
```
(`reader::plan_phase` currently takes `(client, ruleset, toc)`. Add a thin `reader::toc_for(units)` pub wrapper over `tools::toc(units, 40)` OR have stage0 call `reader::tools::toc` — expose `tools` as pub if not already. Check: `tools` is `pub mod tools;` already.)

- [ ] **Step 3: Stage 1 — character slice → partial kernel + onboarding artifacts**
```rust
async fn stage1_character(&self, st: &mut JobStatus, plan: &reader::Plan, budget: usize) -> Result<reader::CharacterSlice> {
    st.begin("character"); self.report(st, "running").await;
    let slice = reader::read_character_slice(self.llm.as_ref(), &self.units, &self.ruleset_id, plan, budget).await?;
    // Coerce + persist the character template into the kernel (character_sheet_schema)
    // and write the onboarding-pack artifacts, REUSING the existing parser helpers.
    let template = crate::coerce_character_template(slice.character_template.clone(), &self.ruleset_id, &self.title);
    let mut kernel = self.db.load_rule_kernel(&self.ruleset_id).await.ok().flatten()
        .unwrap_or_else(|| trpg_model::RuleKernel { kernel_id: format!("{}.rule_kernel.v1", self.ruleset_id), ruleset_id: self.ruleset_id.clone(), version: "v1_staged".into(), ..Default::default() });
    kernel.character_sheet_schema = serde_json::to_value(&template).unwrap_or_default();
    self.db.upsert_rule_kernel(&kernel).await.ok();
    crate::persist_stage1_character_artifacts(&self.db, &self.config_data_dir(), &self.ruleset_id, &self.title, &template, &slice.option_catalogs).await.ok();
    st.finish("character", "character sheet ready"); self.report(st, "running").await;
    Ok(slice)
}
```
Add `pub(crate) fn persist_stage1_character_artifacts(...)` in `lib.rs` that writes `parsed/rules/{ruleset}.character_sheet_template.json` + `.character_option_catalogs.json` (reuse `write_character_onboarding_pack_artifact`'s file-writing logic, factored out) and upserts the onboarding pack row so `/character-template` can serve it. `self.config_data_dir()` returns the data dir (passed into StagedParse or carried).

- [ ] **Step 4: Stage 2 — deep background (each sub-step independent)**
```rust
async fn stage2_deep(&self, st: &mut JobStatus, plan: &reader::Plan, char: &reader::CharacterSlice, budget: usize) {
    st.begin("deep"); st.note("resolution + gm"); self.report(st, "running").await;
    let compiler = crate::build_compiler_llm().unwrap_or_else(|| self.llm.clone());
    // 2a resolution+gm
    let rg = reader::read_resolution_and_gm(self.llm.as_ref(), &self.units, &self.ruleset_id, plan, budget).await.ok();
    // 2b chargen compile (derived value formulas) — backfill into kernel + template
    st.note("compiling character formulas"); self.report(st, "running").await;
    let mut template = crate::coerce_character_template(char.character_template.clone(), &self.ruleset_id, &self.title);
    let skills = crate::skill_ids(&template, &char.option_catalogs);
    let tracks = rg.as_ref().map(|r| crate::track_ids(&r.core.resource_tracks)).unwrap_or_default();
    let ctx = reader::CompileCtx { units: &self.units, sidecar_text: self.sidecar_text.clone(), located_pages: String::new(), skill_names: skills.clone() };
    let _ = reader::compile_chargen_formulas(compiler.as_ref(), &mut template, ctx, budget).await;
    // 2c object schemas: DISCOVER only (stubs) — Phase E adds on-demand extract
    st.note("discovering object categories"); self.report(st, "running").await;
    let obj_ctx = reader::ObjectCtx { units: &self.units, sidecar_text: self.sidecar_text.clone(), skills, resource_tracks: tracks };
    let object_stubs = reader::discover_object_categories(compiler.as_ref(), &obj_ctx, budget).await; // NEW pub fn (Task E1)
    // 2d assemble + persist FULL kernel (reuse rule_kernel_from_run_kit-equivalent)
    crate::persist_stage2_kernel(&self.db, &self.ruleset_id, &self.title, rg.as_ref(), &template, object_stubs).await.ok();
    // 2e module reader (black box) + reindex — best-effort, non-fatal
    st.note("module + index"); self.report(st, "running").await;
    crate::run_module_and_index_for(&self.db, &self.config, &self.ruleset_id).await.ok();
    st.finish("deep", "deep parse complete"); self.report(st, "running").await;
}
```
Add `pub(crate)` helpers in `lib.rs`: `skill_ids(template, catalogs)`, `track_ids(&[Value])` (the same extraction logic already inline in parse_source lines 544–578 — factor it out, DRY), `persist_stage2_kernel(...)` (builds the full `RuleKernel` from resolution+template+object_stubs and upserts), `run_module_and_index_for(...)` (calls the existing module + reindex path for one ruleset, black-box). `discover_object_categories` is a new pub fn in `object_compile.rs` (Task E1) returning the stub list.

- [ ] **Step 5: Unit test (mock LLM) — stage order + partial-kernel-after-stage1 + stage2-failure-non-fatal**

**Files:** Test: `crates/trpg-parser/src/staged.rs` `#[cfg(test)]`
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use trpg_llm::MockLlmClient; // exists (trpg-llm lib.rs:317 impl LlmClient for MockLlmClient)
    // Build a StagedParse with a MockLlmClient scripted to return a minimal character_template,
    // an in-memory/throwaway Db (or a test Db), run().await, then assert:
    //  - st.stages identity->done, character->done before deep
    //  - after stage1, db.load_rule_kernel has non-empty character_sheet_schema, empty object_schemas
    //  - if mock makes stage2 chargen error, st.error set but final_status == "done" and stage1 artifacts intact
}
```
If a real test `Db` is unavailable in unit scope, split the assertions: test `JobStatus` transitions deterministically (done in B1) and validate the live DB path via the CLI harness (Task C) instead — note this explicitly in the test file so coverage intent is clear (no silent gap).

- [ ] **Step 6: Checkpoint** — `cargo build -p trpg-parser` green; `cargo test -p trpg-parser` green.

---

## Phase C — CLI test harness `parse-staged`

### Task C1: `parse-staged` subcommand (in-process driver)

**Files:**
- Modify: `crates/trpg-cli/src/main.rs`

Context: `Commands` enum at line 32; dispatch match at ~457. `connect_db()`, `make_llm()`, `default_data_dir()` exist (used by `ParseAll`). Units load via `trpg_rule_agent::reader::load_units(path)`; sidecar = the merged duotext `.md` at `<data_dir>/markdown/rulebooks/<source>.md`.

- [ ] **Step 1: Add the command variant**
```rust
/// Run the STAGED ruleset parse in-process and stream stage progress to the terminal.
ParseStaged {
    #[arg(long)] ruleset: String,
    #[arg(long)] data_dir: Option<PathBuf>,
    #[arg(long, default_value_t = false)] stage1_only: bool,
    #[arg(long, default_value_t = false)] json: bool,
    #[arg(long, default_value_t = 9)] budget: usize,
},
```

- [ ] **Step 2: Dispatch + handler**
Add to the match: `Commands::ParseStaged { ruleset, data_dir, stage1_only, json, budget } => parse_staged_cli(ruleset, data_dir, stage1_only, json, budget).await,`
```rust
async fn parse_staged_cli(ruleset: String, data_dir: Option<PathBuf>, stage1_only: bool, json: bool, budget: usize) -> Result<()> {
    let db = connect_db().await?; db.migrate().await?;
    let llm = make_llm()?;
    let dir = data_dir.unwrap_or_else(default_data_dir);
    // locate the rulebook source_id (largest semantic_units.jsonl under parsed/source_units)
    let (units_path, source_id) = largest_units_file(&dir.join("parsed/source_units"))?;
    let units = trpg_rule_agent::reader::load_units(&units_path)?;
    let sidecar = std::fs::read_to_string(dir.join(format!("markdown/rulebooks/{source_id}.md"))).ok();
    let job_id = format!("staged_{}", uuid::Uuid::new_v4().simple());
    db.insert_background_job(&job_id, "ruleset_parse_staged", serde_json::json!({"ruleset": ruleset})).await?;
    let sp = trpg_parser::staged::StagedParse { db: db.clone(), llm, ruleset_id: ruleset.clone(), units, sidecar_text: sidecar, title: ruleset.clone(), job_id: job_id.clone(), data_dir: dir, stage1_only };
    let t0 = std::time::Instant::now();
    // The CLI prints transitions by polling the job row between phases via a callback OR by
    // running sp.run() and printing the returned JobStatus stages with elapsed.
    let st = sp.run(budget).await;
    for s in &st.stages { eprintln!("[{:>6.1}s] {:<10} {:<7} {}", t0.elapsed().as_secs_f32(), s.name, s.status, s.detail); }
    if json { println!("{}", serde_json::to_string_pretty(&st.to_value())?); }
    else if let Some(k) = db.load_rule_kernel(&ruleset).await? { println!("{}", serde_json::to_string_pretty(&k.character_sheet_schema)?); }
    Ok(())
}
```
Add `stage1_only: bool` + `data_dir: PathBuf` fields to `StagedParse`; in `run()`, return right after Stage 1 (status "done") when `stage1_only`. Implement `largest_units_file(dir) -> Result<(PathBuf, String)>` (pick the biggest `*.semantic_units.jsonl`, derive `source_id` from filename).

- [ ] **Step 3: Live smoke (manual)** — Run:
`DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg TRPG_LLM_MODEL=gpt-5.4-mini ./target/debug/trpg parse-staged --ruleset call_of_cthulhu_7e --data-dir /Users/haoli/leehow/code/chatrpgv2/_rstest_coc --stage1-only`
Expected: prints `identity done` (~10s) then `character done` (~1.5min), then dumps a `character_sheet_schema` with `fields`. Then without `--stage1-only`: also `deep done` and a populated kernel (`object_schemas` has discovered stubs).

- [ ] **Step 4: Checkpoint** — `cargo build -p trpg-cli` green; smoke output as expected.

---

## Phase D — HTTP API + SSE

### Task D1: Routes + SSE handler

**Files:**
- Modify: `crates/trpg-api/src/lib.rs`

Context: `router(state)` builds the axum `Router`; `AppState` carries a `Db` (+ llm). SSE types already imported. `background_jobs` read: add `Db::load_background_job(job_id) -> Result<Option<Value>>` returning `result_json` if missing.

- [ ] **Step 1: Add `Db::load_background_job` (trpg-db)** returning the row's `status`,`result_json`,`error` as a small struct/Value.

- [ ] **Step 2: Routes**
```rust
.route("/api/ingest/ruleset", post(ingest_ruleset))
.route("/api/ingest/{job_id}/status", get(ingest_status))
.route("/api/ingest/{job_id}/events", get(ingest_events))      // SSE
.route("/api/rulesets/{id}/identity", get(ruleset_identity))
.route("/api/rulesets/{id}/character-template", get(ruleset_character_template))
```

- [ ] **Step 3: `ingest_ruleset` handler** — builds `StagedParse` (load units+sidecar like the CLI), inserts the job, `tokio::spawn`s `sp.run(budget)`, returns `Json({job_id, ruleset_id})` immediately (202 Accepted).

- [ ] **Step 4: `ingest_events` SSE** — stream that, every ~750ms, reads `load_background_job(job_id)`, emits `Event::default().json_data(result_json)`, and ends when `status` ∈ {`done`,`failed`}. Use `KeepAlive`.

- [ ] **Step 5: `ruleset_character_template`** — `load_rule_kernel(id)`; if `character_sheet_schema` non-empty → 200 with it; else `202` with `{stage, progress_pct}` from the latest job. `ruleset_identity` similarly gated on `game_identity.summary`.

- [ ] **Step 6: API smoke test** — a `#[tokio::test]` (or scripted curl in a shell test) hitting an ephemeral server: POST ruleset → poll `/status` until `character` done → assert `/character-template` was 202 before and 200 after. Mark `#[ignore]` if it needs the live LLM; provide a mock-LLM `AppState` variant if feasible.

- [ ] **Step 7: Checkpoint** — `cargo build -p trpg-api` green.

---

## Phase E — Optimization 2: object schemas discover-now / extract-on-demand (separable)

### Task E1: `discover_object_categories` (split discover from extract)

**Files:**
- Modify: `crates/trpg-rule-agent/src/reader/object_compile.rs`
- Modify: `crates/trpg-rule-agent/src/reader/mod.rs`

Context: `compile_object_schemas` currently does `discover_categories` then `extract_category` per cat. Split so Stage 2 can call discover alone.

- [ ] **Step 1:** Make `discover_categories` pub as `pub async fn discover_object_categories(client, ctx, budget) -> Vec<Value>` returning the raw category objects, each annotated `{..., status:"discovered"}`. Keep `compile_object_schemas` (full) working by calling discover then extract (used by the legacy synchronous path).
- [ ] **Step 2:** Make `extract_category` pub as `pub async fn extract_object_category(client, ctx, cat, budget) -> Option<Value>` (it already exists private — rename/expose; the returned schema sets `status:"compiled"`).
- [ ] **Step 3:** Export both in `mod.rs`.
- [ ] **Step 4: Test** — `discover_object_categories` on a fixture returns stubs with `status:"discovered"` and `source_pages`, no `schema_slots` required. (Deterministic part only; live-validated by `parse-staged`.)
- [ ] **Step 5: Checkpoint** — build + `cargo test -p trpg-rule-agent` green.

### Task E2: Materialization fills a stub on first use

**Files:**
- Modify: `crates/trpg-material/src/lib.rs`

Context: `object_schema_guidance(&self, demand)` (added earlier) loads `kernel.object_schemas`, filters by family, injects `schema_slots`+`examples`. A stub has `status:"discovered"` and no `schema_slots`.

- [ ] **Step 1: Write the failing test** — a kernel with a `discovered` weapon stub; assert that after `ensure_category_compiled` the kernel’s weapon entry has `status:"compiled"` + `schema_slots`. (Use a mock LLM returning a schema.)
- [ ] **Step 2: Implement `ensure_category_compiled`** — in `object_schema_guidance`, for each matched stub lacking `schema_slots`: build an `ObjectCtx` (units+sidecar from the ruleset’s data_dir; skills/tracks from the kernel), call `reader::extract_object_category`, and upsert the filled schema back into `kernel.object_schemas` (replace the stub by `category_id`) via `db.upsert_rule_kernel`. Then proceed with the now-filled schemas. Guard: only compile categories whose `kind` family matches the demand (don’t compile all). If extraction fails, keep the stub and fall back to free-form (existing path).
- [ ] **Step 3:** Loading units+sidecar at play time: add a small helper resolving the ruleset’s `parsed/source_units` + `markdown/rulebooks` from the configured data dir (env `TRPG_DATA_DIR` or the bundle’s stored path). If the data dir is unavailable, skip on-demand compile and fall back (log it — no silent cap).
- [ ] **Step 4: Run test → passes.**
- [ ] **Step 5: Live verify** — fresh CoC kernel with stubs (from `parse-staged`), run a `.38 revolver` turn, assert the weapon stub flips to `compiled` and `damage=1D10` extracted (reuses the `.38` verification from this session).
- [ ] **Step 6: Checkpoint** — build + tests green.

---

## Self-Review notes (gaps to watch during execution)
- `StagedParse` needs `data_dir` + `config` for Stage-2 module/index and Phase-E unit loading — thread it in (added in C1).
- `coerce_character_template`, `build_compiler_llm`, `write_*_artifact`, skill/track extraction, module+reindex-for-one-ruleset are currently inline/private in `lib.rs` parse_source; factor them into `pub(crate)` helpers WITHOUT changing `parse_source` behavior (DRY; keep parse_source calling the same helpers).
- Keep every modified file < 400 lines; `staged.rs` < 400 (move status types to `staged_status.rs`).
- LLM-dependent stages are validated **live** via `parse-staged` (Task C3) + the API smoke; deterministic logic (status state machine, reader-split assembly equivalence, discover stub shape, stub→compiled upsert) is unit-tested.
