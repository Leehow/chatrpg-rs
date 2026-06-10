# Staged Onboarding Completion — `onboarding_compile` Stage-2 pass

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
> **NOTE:** these working copies have NO git. Replace every "commit" step with a **Build + test checkpoint**: run the stated build/test command and confirm green ("Finished" + "test result: ok") before moving on. Do NOT `git commit`.

**Goal (functional bar):** On a **staged-parsed** ruleset, `create-character --auto` produces a **stat-complete, playable PC** (not a narrative draft). Today the staged path builds the onboarding pack via `stage1_onboarding_pack` (thin) — `creation_flows = Vec::new()` and `starter_character_pack = default` — so `start_session` reports "missing ... CharacterCreationFlow.steps, StarterCharacterPack" and the PC degrades to a draft. This plan (1) derives `creation_flows` deterministically from the template the reader already produced, and (2) adds a focused Stage-2 LLM pass (`onboarding_compile`) that extracts the ruleset's starter generation recipes, then re-upserts the complete pack.

**Architecture:** Three slices, mirroring the established chargen/object compile pattern.
- **A (deterministic, Stage 1, no LLM):** `creation_flows_from_template(&CharacterTemplate) -> Vec<CharacterCreationFlow>` wraps the template's ordered `creation_flow` steps into ONE `CharacterCreationFlow`, called inside `stage1_onboarding_pack`.
- **B (LLM, Stage 2):** new `crates/trpg-rule-agent/src/reader/onboarding_compile.rs` (<400 lines) mirrors `object_compile.rs` (loop / `nav_tools` / `read_layout` / `submit` / fail-closed guardrail). `compile_starter_pack(client, &OnboardingCtx, budget) -> serde_json::Value` returns a `StarterCharacterPack` as a JSON object (archetypes + creation_shortcuts; pregens optional). The guardrail DROPS any archetype/shortcut not grounded in a read page or referencing a non-existent template field / option-catalog id; empty extraction → empty pack `{}` (never fabricate).
- **C (wiring, Stage 2):** `staged.rs::stage2_deep`, after the object-schema persist, builds `OnboardingCtx`, calls `reader::compile_starter_pack`, then `crate::persist_stage2_onboarding(...)`, which builds the COMPLETE `CharacterOnboardingPack` (= stage1 pack base + the deserialized starter pack), re-validates, and `db.upsert_character_onboarding_pack`. Independent, non-fatal try — mirrors the object step's error handling.

**Tech Stack:** Rust workspace. Crates: `trpg-rule-agent` (reader/LLM), `trpg-parser` (pipeline + persist), `trpg-model` (structs), `trpg-db` (`upsert_character_onboarding_pack`), `trpg-runtime` (consumes the pack — NOT touched). LLM via codex-relay (`TRPG_LLM_MODEL`, e.g. gpt-5.4-mini). Test DB on `:54347`.

---

## File Structure

- **Create** `crates/trpg-rule-agent/src/reader/onboarding_compile.rs` — `OnboardingCtx`, `compile_starter_pack`, the fail-closed guardrail (`finalize_starter_pack`), tool loop. The LLM loop is live-validated (Task 7); the deterministic guardrail is unit-tested (Task 4).
- **Modify** `crates/trpg-rule-agent/src/reader/mod.rs` — `pub use onboarding_compile::{compile_starter_pack, OnboardingCtx};` and `pub mod onboarding_compile;`.
- **Modify** `crates/trpg-parser/src/lib.rs` — add `creation_flows_from_template`; call it inside `stage1_onboarding_pack` (replace `creation_flows: Vec::new()`); add `persist_stage2_onboarding`.
- **Modify** `crates/trpg-parser/src/staged.rs` — `stage2_deep` builds `OnboardingCtx`, runs `compile_starter_pack`, calls `persist_stage2_onboarding` (independent, non-fatal).

### FROZEN CONTRACTS (use these names EXACTLY)

- `pub(crate) fn creation_flows_from_template(template: &CharacterTemplate) -> Vec<CharacterCreationFlow>` (parser).
- `pub struct OnboardingCtx<'a> { pub units: &'a [Unit], pub sidecar_text: Option<String>, pub role_field: Option<String>, pub skill_fields: Vec<String>, pub option_catalogs: serde_json::Value }` (reader).
- `pub async fn compile_starter_pack(client: &dyn LlmClient, ctx: &OnboardingCtx<'_>, budget: usize) -> serde_json::Value` (reader) — returns the `StarterCharacterPack` as a JSON object, or `{}` when nothing is extractable.
- `pub(crate) async fn persist_stage2_onboarding(db: &Db, ruleset_id: &str, title: &str, template: &CharacterTemplate, option_catalogs: &serde_json::Value, starter_pack: &serde_json::Value) -> Result<()>` (parser).

### Grounded types (read, exact as of this writing)

- `trpg_model::StarterCharacterPack { pack_id, ruleset_id, module_id: Option<String>, pregens: Vec<PregenCharacterRef>, archetypes: Vec<RecommendedArchetype>, creation_shortcuts: Vec<CreationShortcut>, module_fit_notes, source_refs }`.
- `RecommendedArchetype { archetype_id, title, summary, fit_tags: Vec<String>, required_option_refs: Vec<String>, source_refs }`.
- `CreationShortcut { shortcut_id, title, mode: CharacterCreationMode, description, source_refs }`.
- `CharacterCreationFlow { flow_id, ruleset_id, title, mode, supported_modes, steps: Vec<CreationStep>, decision_graph, required_tools, source_refs, validation_profile }`.
- `CharacterTemplate { ..., fields: Vec<CharacterField>, derived_values, creation_flow: Vec<CreationStep>, ... }`; `CharacterField { field_id, title, field_type, choices_material_id: Option<String>, ... }`.
- `CharacterCreationMode` (enum, `Default = Guided`); variants incl. `QuickStart`, `Guided`, `Pregenerated`, `Detailed`, `ImportExistingSheet`, `Randomized`, `HighLevel`.
- `validate_character_onboarding_pack(pack)` errors when `creation_flows` is empty/stepless OR when starter pack `pregens` + `archetypes` + `creation_shortcuts` are ALL empty (parser lib.rs).
- Reader `Unit`, `tools::{nav_tools, submit_tool, toc, search, read}`, `chargen_compile::read_layout`, `trpg_llm::LlmClient`.

---

## Task 1: `creation_flows_from_template` — deterministic Stage-1 flow

Wrap the template's ordered `creation_flow` (the `CreationStep`s the reader already produced) into ONE `CharacterCreationFlow`, preserving step order and ids. Pure (no LLM, no `book_map`) so it is deterministic-testable and runs in the fast Stage-1 pack.

**Files:**
- Modify: `crates/trpg-parser/src/lib.rs` (add fn near `stage1_onboarding_pack`, ~line 1726)
- Test: `crates/trpg-parser/src/lib.rs` `#[cfg(test)]` (a new `mod onboarding_flow_tests`)

- [ ] **Step 1: Write the failing test** — append to `crates/trpg-parser/src/lib.rs`:

```rust
#[cfg(test)]
mod onboarding_flow_tests {
    use super::*;

    fn step(id: &str, title: &str) -> CreationStep {
        CreationStep { step_id: id.into(), title: title.into(), required: true, prompt: None, inputs: vec![], outputs: vec![], source_refs: vec![] }
    }

    fn tmpl_with_steps(ruleset: &str, ids: &[&str]) -> CharacterTemplate {
        let mut t = CharacterTemplate::default();
        t.ruleset_id = ruleset.into();
        t.creation_flow = ids.iter().map(|id| step(id, id)).collect();
        t
    }

    #[test]
    fn six_steps_become_one_flow_with_six_steps_in_order() {
        let t = tmpl_with_steps("call_of_cthulhu_7e",
            &["pick_occupation", "roll_characteristics", "derive_attributes", "spend_skill_points", "starting_gear", "background_story"]);
        let flows = creation_flows_from_template(&t);
        assert_eq!(flows.len(), 1, "one template flow -> one CharacterCreationFlow");
        let f = &flows[0];
        assert_eq!(f.steps.len(), 6, "all 6 steps preserved");
        assert_eq!(f.ruleset_id, "call_of_cthulhu_7e");
        // Step order + ids preserved verbatim.
        let ids: Vec<&str> = f.steps.iter().map(|s| s.step_id.as_str()).collect();
        assert_eq!(ids, vec!["pick_occupation", "roll_characteristics", "derive_attributes", "spend_skill_points", "starting_gear", "background_story"]);
        // A non-empty flow has steps -> passes the validator's flow check.
        assert!(!f.flow_id.is_empty());
    }

    #[test]
    fn empty_creation_flow_yields_no_flows() {
        let t = tmpl_with_steps("triangle_agency", &[]);
        assert!(creation_flows_from_template(&t).is_empty(),
            "no steps -> no fabricated flow (validator still flags missing flow, the honest state)");
    }
}
```

- [ ] **Step 2: Run test to verify it fails** — `cargo test -p trpg-parser creation_flows_from_template 2>&1 | tail -20`. Expected: FAIL — `cannot find function creation_flows_from_template`.

- [ ] **Step 3: Write minimal implementation** — add above `stage1_onboarding_pack` in `crates/trpg-parser/src/lib.rs`:

```rust
/// Deterministically wrap the template's ordered `creation_flow` (the CreationSteps
/// the reader already produced) into ONE structured `CharacterCreationFlow`, with
/// step order + ids preserved. No LLM, no book_map — usable inside the fast Stage-1
/// pack so even a thin onboarding pack carries a steps-bearing flow. Empty steps ->
/// no flow (the validator still flags it honestly, rather than fabricating one).
pub(crate) fn creation_flows_from_template(template: &CharacterTemplate) -> Vec<CharacterCreationFlow> {
    if template.creation_flow.is_empty() {
        return Vec::new();
    }
    let ruleset_id = template.ruleset_id.clone();
    vec![CharacterCreationFlow {
        flow_id: format!("{ruleset_id}.guided_creation.v1"),
        ruleset_id,
        title: "Guided playable character creation".into(),
        mode: CharacterCreationMode::Guided,
        supported_modes: vec![
            CharacterCreationMode::Guided,
            CharacterCreationMode::QuickStart,
            CharacterCreationMode::ImportExistingSheet,
        ],
        steps: template.creation_flow.clone(),
        decision_graph: vec![],
        required_tools: vec!["character_validator".into(), "source_backed_formula_resolver".into()],
        source_refs: template.source_refs.clone(),
        validation_profile: json!({"source_backed": true}),
    }]
}
```

- [ ] **Step 4: Run test to verify it passes** — `cargo test -p trpg-parser creation_flows_from_template 2>&1 | tail -20`. Expected: PASS — both tests green.

- [ ] **Step 5: Build + test checkpoint** — `cargo build -p trpg-parser && cargo test -p trpg-parser onboarding_flow_tests 2>&1 | tail -20`. Expected: "Finished" + "test result: ok".

---

## Task 2: Call `creation_flows_from_template` inside `stage1_onboarding_pack`

Replace `creation_flows: Vec::new()` so the fast Stage-1 pack already carries a steps-bearing flow. This fixes gap 1 immediately (zero LLM) and is observable in the pack the persist functions write.

**Files:**
- Modify: `crates/trpg-parser/src/lib.rs` (`stage1_onboarding_pack`, ~line 1729)
- Test: `crates/trpg-parser/src/lib.rs` `mod onboarding_flow_tests`

- [ ] **Step 1: Write the failing test** — add to `mod onboarding_flow_tests`:

```rust
    #[test]
    fn stage1_pack_carries_deterministic_creation_flows() {
        let mut t = CharacterTemplate::default();
        t.ruleset_id = "call_of_cthulhu_7e".into();
        t.fields = vec![CharacterField { field_id: "str".into(), title: "STR".into(), field_type: "stat".into(), ..Default::default() }];
        t.creation_flow = vec![
            CreationStep { step_id: "roll_characteristics".into(), title: "Roll characteristics".into(), required: true, prompt: None, inputs: vec![], outputs: vec![], source_refs: vec![] },
            CreationStep { step_id: "spend_skill_points".into(), title: "Spend skill points".into(), required: true, prompt: None, inputs: vec![], outputs: vec![], source_refs: vec![] },
        ];
        let pack = stage1_onboarding_pack("call_of_cthulhu_7e", "Call of Cthulhu", &t, &json!([]));
        // The thin pack now carries a steps-bearing flow (was Vec::new()).
        assert_eq!(pack.creation_flows.len(), 1, "stage1 pack now has a creation flow");
        assert_eq!(pack.creation_flows[0].steps.len(), 2, "both template steps preserved");
        // The validator no longer reports a missing-flow error for THIS reason.
        assert!(!pack.validation_report.errors.iter().any(|e| e.code == "missing_character_creation_flow"),
            "flow check passes: {:?}", pack.validation_report.errors);
    }
```

- [ ] **Step 2: Run test to verify it fails** — `cargo test -p trpg-parser stage1_pack_carries_deterministic_creation_flows 2>&1 | tail -20`. Expected: FAIL — `pack.creation_flows.len()` is 0 (`assertion left == right` 0 vs 1).

- [ ] **Step 3: Write minimal implementation** — in `stage1_onboarding_pack`, replace the literal:

```rust
        creation_flows: Vec::new(),
```

with:

```rust
        creation_flows: creation_flows_from_template(template),
```

- [ ] **Step 4: Run test to verify it passes** — `cargo test -p trpg-parser stage1_pack_carries_deterministic_creation_flows 2>&1 | tail -20`. Expected: PASS.

- [ ] **Step 5: Build + test checkpoint** — `cargo build -p trpg-parser && cargo test -p trpg-parser onboarding_flow_tests 2>&1 | tail -20`. Expected: "Finished" + "test result: ok" (3 tests in the module).

---

## Task 3: `onboarding_compile.rs` scaffold + `compile_starter_pack` LLM loop

New reader module mirroring `object_compile.rs`: `OnboardingCtx`, the DISCOVER-style system prompt, the tool loop (`nav_tools` + `read_layout` + `submit_starter_pack`), and `compile_starter_pack`. This task lands the structure + the loop; the deterministic guardrail (`finalize_starter_pack`) is added and unit-tested in Task 4. The LLM loop itself is **live-validated** (Task 7), NOT unit-tested — note this in the file.

**Files:**
- Create: `crates/trpg-rule-agent/src/reader/onboarding_compile.rs`
- Modify: `crates/trpg-rule-agent/src/reader/mod.rs`

- [ ] **Step 1: Write the failing test** (compile-shape only; the guardrail tests come in Task 4) — at the bottom of the new file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // The LLM loop (`compile_starter_pack`) is LIVE-VALIDATED via parse-staged +
    // create-character (Task 7), not unit-tested — the deterministic guardrail
    // (`finalize_starter_pack`) carries the unit coverage (Task 4). This first test
    // just pins the context + empty-extraction contract so the module compiles.
    #[test]
    fn empty_records_yield_empty_pack() {
        let ctx = OnboardingCtx { units: &[], sidecar_text: None, role_field: None, skill_fields: vec![], option_catalogs: serde_json::json!([]) };
        let pack = finalize_starter_pack(serde_json::json!({}), &ctx);
        assert_eq!(pack, serde_json::json!({}), "no archetypes/shortcuts -> empty pack, never fabricated");
    }
}
```

- [ ] **Step 2: Run test to verify it fails** — `cargo test -p trpg-rule-agent empty_records_yield_empty_pack 2>&1 | tail -20`. Expected: FAIL — module/`finalize_starter_pack` does not exist yet.

- [ ] **Step 3: Write minimal implementation** — create `crates/trpg-rule-agent/src/reader/onboarding_compile.rs`:

```rust
//! Starter-character onboarding compiler — the reader's focused Stage-2 pass that
//! extracts a ruleset's STARTER GENERATION recipes (recommended archetypes +
//! quick-build shortcuts) so `create-character` builds a stat-complete PC instead
//! of a narrative draft. Mirrors `object_compile` (loop / nav_tools / read_layout /
//! submit + a fail-closed guardrail), applied to character onboarding.
//! Design: docs/superpowers/specs/2026-06-09-staged-onboarding-completion-design.md
//!
//! Data-driven + fail-closed: recipes are EXTRACTED from the book per ruleset; an
//! archetype/shortcut not grounded in a read page (or referencing a non-existent
//! template field / option-catalog id) is DROPPED — never fabricated. An empty
//! extraction returns an empty pack `{}` (create-character degrades to a draft,
//! never invents a build). The LLM loop is LIVE-VALIDATED (parse-staged +
//! create-character); only the deterministic guardrail is unit-tested here.

use super::chargen_compile::read_layout;
use super::tools;
use super::units::Unit;
use serde_json::{json, Value};
use trpg_llm::LlmClient;

/// Context for the starter-pack compile pass (mirrors `ObjectCtx`). The template's
/// role/class field + skill fields + option catalogs give the guardrail the legal
/// ids an archetype/shortcut may reference.
pub struct OnboardingCtx<'a> {
    pub units: &'a [Unit],
    /// The duotext merged `.md` (page-anchored) — read_layout's aligned table view.
    pub sidecar_text: Option<String>,
    /// The template's role/class field_id, if any (e.g. "occupation", "class").
    pub role_field: Option<String>,
    /// The template's skill field_ids — legal `signature skill` references.
    pub skill_fields: Vec<String>,
    /// The reader's option catalogs (Roles/classes, skills, starter gear) — legal
    /// option ids an archetype/shortcut may reference.
    pub option_catalogs: Value,
}

const STARTER_SYS: &str = r#"You compile a tabletop RPG's STARTER CHARACTER GENERATION recipes for a deterministic engine, so a new player can build a stat-complete starter character fast. Using the tools, read the character-creation chapter (the part that tells a NEW player how to make a first character), then produce two lists:

 - archetypes: 2-4 recommended STARTER builds. Each = a concrete starting concept tied to this game's REAL options: {archetype_id (snake_case), title, summary (1-2 sentences: which role/class, the stat priorities, the signature skills), fit_tags (e.g. combat/investigation/social), required_option_refs (the role/class + key skill ids it needs — use the REAL template field ids and option-catalog ids you were given), source_pages (where you read it)}.
 - creation_shortcuts: 1-3 quick-build recipes. Each = {shortcut_id (snake_case), title, mode (quick_start|guided|pregenerated), description (the quick path: pick role -> stat method (point-buy budget N, or roll XdY) -> starting skill picks -> starting gear), source_pages}.
 - pregens (OPTIONAL): only if the book prints ready-to-play sample characters; each {pregen_id, title, summary, source_pages}.

GUARDRAIL (fail-closed): GROUND every recipe in a page you READ — record source_pages. Reference ONLY the role/class field, skill fields, and option-catalog ids you were given; do NOT invent a role, class, skill, or option that is not in those lists. If you cannot read a recipe's basis, OMIT it — never fabricate a build, a stat, or a number. If the game has NO printed starter guidance you can ground, submit EMPTY lists.

TOOLS: get_toc, search(keywords), read(pages), read_layout(pages) for tables. Then call submit_starter_pack with {archetypes:[...], creation_shortcuts:[...], pregens:[...]}. Budget ~10 tool calls; be targeted — find the "creating a character" / "character creation" chapter."#;

/// Compile this ruleset's starter-character pack (recommended archetypes +
/// quick-build shortcuts; pregens optional). Returns the StarterCharacterPack as a
/// JSON OBJECT, fail-closed: every recipe is grounded + references only legal ids,
/// or it is dropped; an empty extraction returns `{}`. The LLM loop is
/// live-validated — only `finalize_starter_pack` is unit-tested.
pub async fn compile_starter_pack(client: &dyn LlmClient, ctx: &OnboardingCtx<'_>, budget: usize) -> Value {
    let toc = tools::toc(ctx.units, 40);
    let submit = tools::submit_tool(
        "submit_starter_pack",
        "Submit the starter-character recipes (empty lists if the game has no groundable starter guidance).",
        json!({
            "archetypes": {"type": "array", "items": {"type": "object"}},
            "creation_shortcuts": {"type": "array", "items": {"type": "object"}},
            "pregens": {"type": "array", "items": {"type": "object"}}
        }),
        &["archetypes", "creation_shortcuts"],
    );
    let schemas = onboarding_tool_schemas(submit);
    let option_ids = legal_option_ids(ctx);
    let seed = format!(
        "Role/class field: {:?}\nSkill fields (legal signature-skill refs): {:?}\nOption-catalog ids (legal required_option_refs): {:?}\n\nTOC (already fetched):\n{toc}\n\nRead this game's character-creation chapter and submit_starter_pack with 2-4 grounded archetypes + 1-3 quick-build shortcuts (pregens only if the book prints sample characters).",
        ctx.role_field, ctx.skill_fields, option_ids
    );
    let raw = run_onboarding_loop(client, STARTER_SYS, &seed, &schemas, ctx, budget, "submit_starter_pack")
        .await
        .unwrap_or_else(|| json!({}));
    finalize_starter_pack(raw, ctx)
}

/// nav tools + the read_layout table view + the given submit tool (mirrors
/// `object_tool_schemas`).
fn onboarding_tool_schemas(submit: Value) -> Vec<Value> {
    let mut t = tools::nav_tools();
    t.push(json!({"type": "function", "function": {
        "name": "read_layout",
        "description": "aligned-table view of page(s) like \"30\" or \"30-32\" — use for point-buy / starting-gear tables that look misaligned in read().",
        "parameters": {"type": "object", "properties": {"pages": {"type": "string"}}, "required": ["pages"]}
    }}));
    t.push(submit);
    t
}

/// The focused tool loop (mirrors `run_object_loop`). Returns the submit args.
async fn run_onboarding_loop(
    client: &dyn LlmClient,
    system: &str,
    seed: &str,
    tool_schemas: &[Value],
    ctx: &OnboardingCtx<'_>,
    budget: usize,
    submit_name: &str,
) -> Option<Value> {
    let mut msgs = vec![
        json!({"role": "system", "content": system}),
        json!({"role": "user", "content": seed}),
    ];
    for _ in 0..(budget + 8) {
        let resp = client.complete_with_tools(msgs.clone(), tool_schemas.to_vec()).await.ok()?;
        let message = resp.pointer("/choices/0/message").cloned().unwrap_or_else(|| json!({}));
        let tcs = message.get("tool_calls").and_then(Value::as_array).cloned().unwrap_or_default();
        if tcs.is_empty() {
            msgs.push(message);
            msgs.push(json!({"role": "user", "content": format!("Use the tools, then call {submit_name}.")}));
            continue;
        }
        msgs.push(message);
        for tc in &tcs {
            let name = tc.pointer("/function/name").and_then(Value::as_str).unwrap_or("");
            let args: Value = tc
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_else(|| json!({}));
            let id = tc.get("id").and_then(Value::as_str).unwrap_or("");
            if name == submit_name {
                return Some(args);
            }
            let out = onboarding_dispatch(ctx, name, &args);
            msgs.push(json!({"role": "tool", "tool_call_id": id, "content": out}));
        }
    }
    None
}

fn onboarding_dispatch(ctx: &OnboardingCtx<'_>, name: &str, args: &Value) -> String {
    let cap = |s: String| if s.len() <= 3000 { s } else { s.chars().take(3000).collect() };
    match name {
        "get_toc" => tools::toc(ctx.units, 40),
        "search" => {
            let kws: Vec<String> = args
                .get("keywords")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            cap(tools::search(ctx.units, &kws, 8))
        }
        "read" => cap(tools::read(ctx.units, args.get("pages").and_then(Value::as_str).unwrap_or(""))),
        "read_layout" => match &ctx.sidecar_text {
            Some(s) => cap(read_layout(s, args.get("pages").and_then(Value::as_str).unwrap_or(""))),
            None => "[no layout view available — read() the page instead]".into(),
        },
        _ => format!("unknown tool {name}"),
    }
}

/// The legal id vocabulary an archetype/shortcut may reference: the role/class
/// field, every skill field, and every option-catalog option/group/locator id.
/// Pure (no LLM) so the guardrail is unit-testable. Lowercased.
fn legal_option_ids(ctx: &OnboardingCtx<'_>) -> std::collections::HashSet<String> {
    let mut ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let Some(rf) = &ctx.role_field {
        ids.insert(rf.to_ascii_lowercase());
    }
    for s in &ctx.skill_fields {
        ids.insert(s.to_ascii_lowercase());
    }
    if let Some(cats) = ctx.option_catalogs.as_array() {
        for c in cats {
            collect_ids(c, &mut ids);
        }
    }
    ids
}

/// Recursively harvest every `*_id` / `option_id` / `group_id` / `locator_id` /
/// `field_id` / `catalog_id` string from a catalog value (and option titles).
fn collect_ids(v: &Value, out: &mut std::collections::HashSet<String>) {
    match v {
        Value::Object(map) => {
            for (k, val) in map {
                if (k.ends_with("_id") || k == "title") && val.is_string() {
                    if let Some(s) = val.as_str() {
                        if !s.trim().is_empty() {
                            out.insert(s.trim().to_ascii_lowercase());
                        }
                    }
                }
                collect_ids(val, out);
            }
        }
        Value::Array(arr) => {
            for val in arr {
                collect_ids(val, out);
            }
        }
        _ => {}
    }
}

// finalize_starter_pack + guardrail helpers land in Task 4.
fn finalize_starter_pack(_raw: Value, _ctx: &OnboardingCtx<'_>) -> Value {
    json!({}) // replaced in Task 4
}
```

- [ ] **Step 4: Wire the module exports** — in `crates/trpg-rule-agent/src/reader/mod.rs`, add the module declaration (after `pub mod object_compile;`):

```rust
pub mod onboarding_compile;
```

and the export (after the `object_compile` re-export):

```rust
pub use onboarding_compile::{compile_starter_pack, OnboardingCtx};
```

- [ ] **Step 5: Run test to verify it passes** — `cargo test -p trpg-rule-agent empty_records_yield_empty_pack 2>&1 | tail -20`. Expected: PASS (the stub `finalize_starter_pack` returns `{}`).

- [ ] **Step 6: Build + test checkpoint** — `cargo build -p trpg-rule-agent 2>&1 | tail -20`. Expected: "Finished". (`compile_starter_pack` is unused by callers yet — that's fine; it's `pub`. The stub `finalize_starter_pack` may warn "unused" until Task 4 wires the guardrail — acceptable mid-plan.)

---

## Task 4: Fail-closed guardrail `finalize_starter_pack`

Replace the stub with the real guardrail: DROP any archetype/shortcut not grounded in a `source_pages` value, OR whose `required_option_refs` reference an id not in the legal vocabulary (`legal_option_ids`). After dropping, if NO archetype/shortcut/pregen survives, return `{}` (never fabricate). Otherwise return the cleaned `StarterCharacterPack`-shaped object. Pure (no LLM) → unit-tested.

**Files:**
- Modify: `crates/trpg-rule-agent/src/reader/onboarding_compile.rs`

- [ ] **Step 1: Write the failing tests** — extend `mod tests` in `onboarding_compile.rs`:

```rust
    fn ctx_with(role: Option<&str>, skills: &[&str], catalogs: Value) -> OnboardingCtx<'static> {
        OnboardingCtx {
            units: &[],
            sidecar_text: None,
            role_field: role.map(String::from),
            skill_fields: skills.iter().map(|s| s.to_string()).collect(),
            option_catalogs: catalogs,
        }
    }

    #[test]
    fn archetype_referencing_bogus_field_is_dropped() {
        // One archetype references a real skill ("library_use"); the other references
        // a non-existent field ("warp_drive") -> dropped. The good one survives.
        let ctx = ctx_with(Some("occupation"), &["library_use", "spot_hidden"], json!([]));
        let raw = json!({
            "archetypes": [
                {"archetype_id": "scholar", "title": "Scholar", "summary": "reads things",
                 "required_option_refs": ["occupation", "library_use"], "source_pages": "30-31"},
                {"archetype_id": "pilot", "title": "Pilot", "summary": "flies",
                 "required_option_refs": ["warp_drive"], "source_pages": "30"}
            ],
            "creation_shortcuts": []
        });
        let pack = finalize_starter_pack(raw, &ctx);
        let arch = pack["archetypes"].as_array().expect("archetypes array");
        assert_eq!(arch.len(), 1, "bogus-ref archetype dropped: {pack}");
        assert_eq!(arch[0]["archetype_id"], json!("scholar"));
    }

    #[test]
    fn ungrounded_recipe_is_dropped() {
        // An archetype with no source_pages is not grounded -> dropped.
        let ctx = ctx_with(Some("class"), &["athletics"], json!([]));
        let raw = json!({
            "archetypes": [
                {"archetype_id": "fighter", "title": "Fighter", "required_option_refs": ["class"], "source_pages": "12"},
                {"archetype_id": "floater", "title": "Floater", "required_option_refs": ["class"]}
            ],
            "creation_shortcuts": [
                {"shortcut_id": "quick", "title": "Quick build", "mode": "quick_start", "description": "pick class then go", "source_pages": "12"},
                {"shortcut_id": "ghost", "title": "Ungrounded", "mode": "quick_start", "description": "no page"}
            ]
        });
        let pack = finalize_starter_pack(raw, &ctx);
        assert_eq!(pack["archetypes"].as_array().unwrap().len(), 1, "ungrounded archetype dropped");
        assert_eq!(pack["archetypes"][0]["archetype_id"], json!("fighter"));
        assert_eq!(pack["creation_shortcuts"].as_array().unwrap().len(), 1, "ungrounded shortcut dropped");
        assert_eq!(pack["creation_shortcuts"][0]["shortcut_id"], json!("quick"));
    }

    #[test]
    fn empty_after_dropping_yields_empty_pack() {
        // Everything is ungrounded -> all dropped -> empty pack {}, NOT fabricated.
        let ctx = ctx_with(Some("class"), &[], json!([]));
        let raw = json!({
            "archetypes": [{"archetype_id": "x", "title": "X", "required_option_refs": ["class"]}],
            "creation_shortcuts": [{"shortcut_id": "y", "title": "Y", "mode": "guided", "description": "no page"}]
        });
        assert_eq!(finalize_starter_pack(raw, &ctx), json!({}), "all dropped -> {{}}");
    }

    #[test]
    fn ref_validated_against_option_catalog_ids() {
        // required_option_refs may also reference option-catalog ids, not just template fields.
        let catalogs = json!([
            {"catalog_id": "coc.options.v1", "option_groups": [
                {"group_id": "occupations", "options": [
                    {"option_id": "antiquarian", "title": "Antiquarian"}
                ]}
            ]}
        ]);
        let ctx = ctx_with(Some("occupation"), &[], catalogs);
        let raw = json!({
            "archetypes": [
                {"archetype_id": "a", "title": "A", "required_option_refs": ["antiquarian"], "source_pages": "30"}
            ],
            "creation_shortcuts": []
        });
        let pack = finalize_starter_pack(raw, &ctx);
        assert_eq!(pack["archetypes"].as_array().unwrap().len(), 1, "catalog option id is legal");
    }
```

- [ ] **Step 2: Run test to verify it fails** — `cargo test -p trpg-rule-agent -- onboarding_compile::tests 2>&1 | tail -30`. Expected: FAIL — the stub returns `{}` so all non-empty-pack asserts fail.

- [ ] **Step 3: Write minimal implementation** — replace the stub `finalize_starter_pack` with:

```rust
/// Fail-closed guardrail (pure, no LLM): keep only archetypes/shortcuts that are
/// GROUNDED (have a non-empty `source_pages`) AND whose `required_option_refs` all
/// resolve to a legal id (role/class field, a skill field, or an option-catalog id).
/// Pregens are kept when grounded. If nothing survives, return `{}` (never
/// fabricate). Otherwise return a StarterCharacterPack-shaped object.
fn finalize_starter_pack(raw: Value, ctx: &OnboardingCtx<'_>) -> Value {
    let legal = legal_option_ids(ctx);
    let archetypes = keep_grounded(raw.get("archetypes"), |a| refs_are_legal(a, &legal));
    let shortcuts = keep_grounded(raw.get("creation_shortcuts"), |_| true);
    let pregens = keep_grounded(raw.get("pregens"), |_| true);
    if archetypes.is_empty() && shortcuts.is_empty() && pregens.is_empty() {
        return json!({});
    }
    let mut pack = json!({
        "archetypes": archetypes,
        "creation_shortcuts": shortcuts,
    });
    if !pregens.is_empty() {
        pack["pregens"] = json!(pregens);
    }
    pack
}

/// Keep array entries that are grounded (non-empty `source_pages`) AND pass `extra`.
fn keep_grounded(arr: Option<&Value>, extra: impl Fn(&Value) -> bool) -> Vec<Value> {
    arr.and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| is_grounded(item) && extra(item))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Grounded = the recipe records the page(s) it was read from.
fn is_grounded(item: &Value) -> bool {
    item.get("source_pages")
        .and_then(Value::as_str)
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}

/// Every `required_option_refs` id must resolve to a legal id. Empty refs are OK
/// (a concept that names no specific option). Unknown id -> drop (fail-closed).
fn refs_are_legal(item: &Value, legal: &std::collections::HashSet<String>) -> bool {
    let refs = item.get("required_option_refs").and_then(Value::as_array);
    match refs {
        None => true,
        Some(rs) => rs.iter().all(|r| {
            r.as_str()
                .map(|s| {
                    let id = s.trim().to_ascii_lowercase();
                    id.is_empty() || legal.contains(&id)
                })
                .unwrap_or(true)
        }),
    }
}
```

- [ ] **Step 4: Run tests to verify they pass** — `cargo test -p trpg-rule-agent -- onboarding_compile::tests 2>&1 | tail -30`. Expected: PASS — all 5 guardrail tests green (incl. `empty_records_yield_empty_pack` from Task 3).

- [ ] **Step 5: Build + test checkpoint** — `cargo build -p trpg-rule-agent && cargo test -p trpg-rule-agent onboarding_compile 2>&1 | tail -20`. Expected: "Finished" + "test result: ok".

---

## Task 5: `persist_stage2_onboarding` — build + re-upsert the COMPLETE pack

A parser helper that builds the COMPLETE `CharacterOnboardingPack` (reuse `stage1_onboarding_pack` for the base — which now carries `creation_flows`), sets `starter_character_pack` by deserializing the compiled `starter_pack` JSON, re-validates, and `db.upsert_character_onboarding_pack`. The base-vs-merge logic is pure and unit-tested via an extracted helper `merge_starter_into_pack`; the DB call itself is live-validated (no test `Db` in this scope — same convention as `staged.rs`).

**Files:**
- Modify: `crates/trpg-parser/src/lib.rs` (add near `persist_stage2_kernel`, ~line 1773)
- Test: `crates/trpg-parser/src/lib.rs` `mod onboarding_flow_tests`

- [ ] **Step 1: Write the failing test** — add to `mod onboarding_flow_tests`:

```rust
    #[test]
    fn merge_starter_into_pack_lands_starter_and_keeps_flows() {
        // Base pack (stage1) has the deterministic creation_flows; merging a compiled
        // starter pack must (a) land archetypes/shortcuts and (b) preserve the flows.
        let mut t = CharacterTemplate::default();
        t.ruleset_id = "call_of_cthulhu_7e".into();
        t.fields = vec![CharacterField { field_id: "str".into(), title: "STR".into(), field_type: "stat".into(), ..Default::default() }];
        t.creation_flow = vec![CreationStep { step_id: "roll_characteristics".into(), title: "Roll".into(), required: true, prompt: None, inputs: vec![], outputs: vec![], source_refs: vec![] }];
        let base = stage1_onboarding_pack("call_of_cthulhu_7e", "Call of Cthulhu", &t, &json!([]));
        assert_eq!(base.creation_flows.len(), 1, "precondition: base has a flow");

        let starter = json!({
            "archetypes": [{"archetype_id": "investigator", "title": "Investigator", "summary": "follows clues", "required_option_refs": [], "source_refs": []}],
            "creation_shortcuts": [{"shortcut_id": "quick", "title": "Quick start", "mode": "quick_start", "description": "pick occupation then go", "source_refs": []}]
        });
        let pack = merge_starter_into_pack(base, &starter);

        // (a) starter pack landed.
        assert_eq!(pack.starter_character_pack.archetypes.len(), 1, "archetype landed");
        assert_eq!(pack.starter_character_pack.archetypes[0].archetype_id, "investigator");
        assert_eq!(pack.starter_character_pack.creation_shortcuts.len(), 1, "shortcut landed");
        // (b) creation_flows preserved.
        assert_eq!(pack.creation_flows.len(), 1, "flows preserved through merge");
        // (c) the validator now passes BOTH the flow check and the starter-path check.
        let report = validate_character_onboarding_pack(&pack);
        assert!(!report.errors.iter().any(|e| e.code == "missing_character_creation_flow"), "flow ok: {:?}", report.errors);
        assert!(!report.errors.iter().any(|e| e.code == "missing_starter_character_path"), "starter ok: {:?}", report.errors);
    }

    #[test]
    fn merge_empty_starter_leaves_base_starter_empty() {
        // An empty compiled pack ({}) must NOT fabricate a starter path: the base
        // starter pack stays empty (validator still reports missing_starter, the
        // honest degraded state). The flows still pass.
        let mut t = CharacterTemplate::default();
        t.ruleset_id = "triangle_agency".into();
        t.fields = vec![CharacterField { field_id: "arc".into(), title: "Arc".into(), field_type: "choice".into(), ..Default::default() }];
        t.creation_flow = vec![CreationStep { step_id: "pick_arc".into(), title: "Pick ARC".into(), required: true, prompt: None, inputs: vec![], outputs: vec![], source_refs: vec![] }];
        let base = stage1_onboarding_pack("triangle_agency", "Triangle", &t, &json!([]));
        let pack = merge_starter_into_pack(base, &json!({}));
        assert!(pack.starter_character_pack.archetypes.is_empty(), "empty compiled pack -> no fabricated archetypes");
        assert!(pack.starter_character_pack.creation_shortcuts.is_empty());
    }
```

- [ ] **Step 2: Run test to verify it fails** — `cargo test -p trpg-parser merge_starter_into_pack 2>&1 | tail -20`. Expected: FAIL — `cannot find function merge_starter_into_pack`.

- [ ] **Step 3: Write minimal implementation** — add near `persist_stage2_kernel` in `crates/trpg-parser/src/lib.rs`:

```rust
/// Build the COMPLETE onboarding pack from a Stage-1 base + the compiled starter
/// pack. Pure (no DB) so the merge is unit-testable. An empty compiled pack `{}`
/// leaves the base starter pack untouched (fail-closed: never fabricate a build).
/// Re-validates so the returned pack carries an honest `validation_report`.
pub(crate) fn merge_starter_into_pack(mut pack: CharacterOnboardingPack, starter_pack: &Value) -> CharacterOnboardingPack {
    if let Ok(starter) = serde_json::from_value::<StarterCharacterPack>(starter_pack.clone()) {
        if !starter.archetypes.is_empty() || !starter.creation_shortcuts.is_empty() || !starter.pregens.is_empty() {
            let mut starter = starter;
            // Preserve the pack's stable identity even when the compiler omits ids.
            if starter.pack_id.is_empty() { starter.pack_id = format!("{}.starter_characters.v1", pack.ruleset_id); }
            if starter.ruleset_id.is_empty() { starter.ruleset_id = pack.ruleset_id.clone(); }
            pack.starter_character_pack = starter;
        }
    }
    pack.validation_report = validate_character_onboarding_pack(&pack);
    pack
}

/// Stage-2 onboarding persistence: build the COMPLETE `CharacterOnboardingPack`
/// (Stage-1 base — now carrying deterministic creation_flows — plus the compiled
/// `starter_pack`), re-validate, and re-upsert it. Independent + non-fatal at the
/// call site (mirrors the object-schema sub-step). The DB path is live-validated.
pub(crate) async fn persist_stage2_onboarding(db: &Db, ruleset_id: &str, title: &str, template: &CharacterTemplate, option_catalogs: &Value, starter_pack: &Value) -> Result<()> {
    let base = stage1_onboarding_pack(ruleset_id, title, template, option_catalogs);
    let pack = merge_starter_into_pack(base, starter_pack);
    db.upsert_character_onboarding_pack(&pack).await?;
    Ok(())
}
```

- [ ] **Step 4: Run tests to verify they pass** — `cargo test -p trpg-parser merge_starter_into_pack 2>&1 | tail -20`. Expected: PASS.

- [ ] **Step 5: Build + test checkpoint** — `cargo build -p trpg-parser && cargo test -p trpg-parser onboarding_flow_tests 2>&1 | tail -20`. Expected: "Finished" + "test result: ok" (5 tests).

---

## Task 6: Wire the Stage-2 step into `staged.rs::stage2_deep`

After the object-schema persist (`persist_stage2_kernel`), build `OnboardingCtx` (role_field + skill_fields from `template.fields`; option_catalogs from `char_slice.option_catalogs`; units/sidecar from `self`), call `reader::compile_starter_pack`, then `crate::persist_stage2_onboarding`. Independent + non-fatal — a failure records a note and leaves the thin pack intact (mirrors the kernel persist's error handling).

**Files:**
- Modify: `crates/trpg-parser/src/staged.rs` (`stage2_deep`, after the `persist_stage2_kernel` block, ~line 174)

- [ ] **Step 1: Verify the build is the failing signal** — `staged.rs` has no test `Db` (see its `#[cfg(test)]` note), so this step's correctness is carried by the build (the new call must compile against the frozen signatures) + the live verify (Task 7). Confirm it does not yet compile a call to `compile_starter_pack`: `grep -n compile_starter_pack crates/trpg-parser/src/staged.rs` → no match (expected before the edit).

- [ ] **Step 2: Write minimal implementation** — in `stage2_deep`, insert AFTER the `if let Err(e) = crate::persist_stage2_kernel(...) { ... }` block and BEFORE the "2e module reader" comment:

```rust
        // 2d-bis onboarding compile: extract starter-character recipes + re-upsert
        // the COMPLETE onboarding pack (creation_flows are already deterministic in
        // the Stage-1 base; this fills the starter pack). Independent + non-fatal.
        st.note("compiling starter character pack");
        self.report(st, "running").await;
        let role_field = template
            .fields
            .iter()
            .find(|f| matches!(f.field_type.as_str(), "role" | "class") || matches!(f.field_id.as_str(), "role" | "class" | "occupation" | "profession" | "career" | "arc"))
            .map(|f| f.field_id.clone());
        let skill_fields: Vec<String> = template
            .fields
            .iter()
            .filter(|f| f.field_type == "skill")
            .map(|f| f.field_id.clone())
            .collect();
        let onboarding_ctx = reader::OnboardingCtx {
            units: &self.units,
            sidecar_text: self.sidecar_text.clone(),
            role_field,
            skill_fields,
            option_catalogs: char_slice.option_catalogs.clone(),
        };
        let starter_pack = reader::compile_starter_pack(compiler.as_ref(), &onboarding_ctx, budget).await;
        if let Err(e) = crate::persist_stage2_onboarding(&self.db, &self.ruleset_id, &self.title, &template, &char_slice.option_catalogs, &starter_pack).await {
            st.note(&format!("onboarding persist failed: {e}"));
        }
```

(Note: `template` here is the formula-compiled binding already in `stage2_deep` from the chargen-compile step — reuse it; do NOT re-coerce. `compiler` is the same `build_compiler_llm()` binding the object step uses.)

- [ ] **Step 3: Build to verify it compiles** — `cargo build -p trpg-parser 2>&1 | tail -20`. Expected: "Finished" — the call type-checks against `reader::OnboardingCtx` / `reader::compile_starter_pack` / `crate::persist_stage2_onboarding`.

- [ ] **Step 4: Build + test checkpoint (whole pipeline crate set)** — `cargo build -p trpg-rule-agent -p trpg-parser && cargo test -p trpg-parser 2>&1 | tail -20`. Expected: "Finished" + "test result: ok" (existing staged-state-machine test + the new onboarding tests all green).

---

## Task 7: LIVE verify — fresh staged parse of CoC → `create-character --auto` → stat-complete PC

The LLM `compile_starter_pack` loop is NOT unit-tested (it is live-validated here — note already in the module doc-comment). This task is the functional bar: a fresh staged parse of CoC, then `create-character --auto`, must produce a stat-complete PC with NO "missing" / NO "starter character path" validation error. Run the FULL build first so the binary carries Tasks 1-6.

**Files:**
- None (manual verification). Commands assume the repo root `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula` and the full `.env.example` sourced with the test overrides below.

- [ ] **Step 1: Build the workspace** — `cargo build 2>&1 | tail -20`. Expected: "Finished".

- [ ] **Step 2: Source env + set the test DB / data dir** — from the repo root:

```bash
set -a; source .env.example; set +a
export DATABASE_URL='postgres://chatrpg:chatrpg@localhost:54347/chatrpg'
export TRPG_DATA_DIR='_rstest_coc'
# TRPG_LLM_* come from .env.example; ensure TRPG_LLM_MODEL points at the codex-relay
# model in use (e.g. gpt-5.4-mini) and TRPG_LLM_BASE_URL/API_KEY are the relay's.
```

- [ ] **Step 3: Fresh staged parse of CoC** — (assumes the CoC source/units already ingested into `_rstest_coc`/the `:54347` DB; if not, run the project's normal ingest into `TRPG_DATA_DIR` first):

```bash
cargo run -q -p trpg-cli -- parse-staged --ruleset call_of_cthulhu_7e --data-dir _rstest_coc --budget 9 2>&1 | tail -40
```

Expected: the job reaches `done`; Stage-2 logs include `compiling starter character pack` (tracing note) and an onboarding pack re-upsert with NO `onboarding persist failed`.

- [ ] **Step 4: Inspect the persisted pack** — confirm Stage-2 filled both gaps:

```bash
psql "$DATABASE_URL" -c "select jsonb_array_length(pack->'creation_flows') as flows, jsonb_array_length(pack->'starter_character_pack'->'archetypes') as archetypes, jsonb_array_length(pack->'starter_character_pack'->'creation_shortcuts') as shortcuts, pack->'validation_report'->>'status' as status from character_onboarding_packs where ruleset_id='call_of_cthulhu_7e';"
```

Expected: `flows >= 1`, `archetypes + shortcuts >= 1`, `status` is `ok` (or `warning`, NOT `error` for `missing_character_creation_flow` / `missing_starter_character_path`).

- [ ] **Step 5: `create-character --auto` → stat-complete PC** —

```bash
cargo run -q -p trpg-cli -- create-character --ruleset call_of_cthulhu_7e --auto 2>&1 | tail -60
```

Expected (the BAR): the command succeeds and prints a stat-complete starter PC (characteristics + skills filled). It must NOT error with "is not mechanically startable yet; missing ... CharacterCreationFlow.steps, StarterCharacterPack" and must NOT print a narrative-only DRAFT. Contrast with the pre-fix behavior recorded in the 2026-06-09 playtest ("missing creation flow extraction and starter character path").

- [ ] **Step 6: Regression sweep** — `cargo test -p trpg-parser -p trpg-rule-agent 2>&1 | tail -20`. Expected: "test result: ok" across both crates.

---

## Self-review — spec coverage map

- **A. Deterministic creation_flows** — Tasks 1-2 (`creation_flows_from_template` + call inside `stage1_onboarding_pack`). Unit-tested: 6 steps → 1 flow with 6 ordered steps; empty → no flow; Stage-1 pack now carries flows.
- **B. `onboarding_compile.rs`** — Tasks 3-4 (`OnboardingCtx`, `compile_starter_pack` loop mirroring `object_compile`, `finalize_starter_pack` fail-closed guardrail). Exports wired in `reader/mod.rs`. The LLM loop is live-validated (noted in the module doc-comment); the guardrail is unit-tested.
- **Fail-closed** — Task 4 tests: bogus-field archetype dropped; ungrounded recipe dropped; everything dropped → `{}`; catalog-id refs accepted. Task 5 test: empty compiled pack leaves base starter empty (no fabrication).
- **C. Stage-2 wiring** — Task 6 (`stage2_deep` builds `OnboardingCtx`, runs `compile_starter_pack`, calls `persist_stage2_onboarding`, independent + non-fatal). Task 5 (`persist_stage2_onboarding` + `merge_starter_into_pack`, pack-merge unit-tested: starter lands, creation_flows present, validator passes both checks).
- **Tests** — deterministic for `creation_flows_from_template` (Task 1), the onboarding guardrail (Task 4), and `persist_stage2_onboarding` pack-merge (Task 5). LIVE verify (Task 7): fresh staged CoC parse → `create-character --auto` → stat-complete PC with the exact commands.

All four FROZEN CONTRACT names used verbatim: `creation_flows_from_template`, `OnboardingCtx`/`compile_starter_pack`, `persist_stage2_onboarding`. No placeholders; every code step is complete real Rust grounded in the read structs.
