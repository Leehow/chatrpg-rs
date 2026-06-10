# Chargen Formula Compiler Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
> **NOTE:** these working copies have NO git. Replace every "commit" step with a "checkpoint": run the stated build/test command and confirm green before moving on. Do NOT `git commit`.

**Goal:** Auto-compile the rule reader's prose chargen formulas + duotext `.layout.md` tables into the machine-evaluable §4 `derived_values` (in place), so `trpg-formula` computes chargen deterministically without a hand-authored spec.

**Architecture:** A focused second slice in the rule agent (`trpg-rule-agent/src/reader/chargen_compile.rs`) runs after the deep reader. It reuses the reader's retrieval helpers (`toc`/`search`/`read`) + adds `read_layout` (reads the page-anchored `.layout.md` sidecar), drives a small LLM loop whose only output is a §4 `derived_values` array, then a DETERMINISTIC finalize step round-trips every record through `trpg_formula::evaluate_chargen` (unresolved → `provisional`, never fabricated) and upgrades `template.derived_values` in place. Runtime `load_chargen_spec` reads the template + an optional per-id override file.

**Tech Stack:** Rust workspace; `trpg-rule-agent` (reader/LLM), `trpg-formula` (pure evaluator), `trpg-parser` (pipeline), `trpg-runtime` (chargen). LLM via codex-relay (gpt-5.4-mini). DBs :54347 (CoC/Triangle), :54346 (Cyberpunk).

---

## File Structure

- **Create** `crates/trpg-rule-agent/src/reader/chargen_compile.rs` — the compile slice: `read_layout` (pure), `compile_chargen_formulas` (async LLM loop), `finalize_compiled` (pure round-trip+provisional), `sample_inputs_from_template` (pure).
- **Modify** `crates/trpg-rule-agent/src/reader/mod.rs` — export the new module's public fn.
- **Modify** `crates/trpg-rule-agent/Cargo.toml` — add `trpg-formula` dependency.
- **Modify** `crates/trpg-parser/src/lib.rs:~519` — call `compile_chargen_formulas` after `coerce_character_template`, before `normalize_character_sheet_template_schema`.
- **Modify** `crates/trpg-runtime/src/chargen.rs` — `chargen_spec_from_template` + `merge_override` + change `load_chargen_spec` signature; update `create_and_bind_character` call site.
- **Modify** `crates/trpg-parser/src/lib.rs:80` — add `chargen_compiler=v1` to `parse_config_hash`.

Round-trip guardrail and `field_id↔id` mapping reuse existing types: `trpg_model::{CharacterTemplate, DerivedValue, CharacterField}` (DerivedValue already carries the §4 fields), `trpg_formula::evaluate_chargen`.

---

## Task 1: `read_layout` — slice the `.layout.md` sidecar by page

**Files:**
- Create: `crates/trpg-rule-agent/src/reader/chargen_compile.rs`
- Test: same file, `#[cfg(test)]`

The sidecar is page-anchored markdown: each page begins with `<!-- source_id=… page=N text_hash=… -->` then `# Page N`. `read_layout` returns the concatenated text of pages whose N is in the requested range.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const SIDECAR: &str = "---\nextractor: duotext-layout\n---\n\n\
<!-- source_id=x page=32 text_hash=a -->\n\n# Page 32\n\nDEX table intro\n\n\
<!-- source_id=x page=33 text_hash=b -->\n\n# Page 33\n\nSTR+SIZ | Damage Bonus | Build\n2-64 | -2 | -2\n65-84 | -1 | -1\n\n\
<!-- source_id=x page=34 text_hash=c -->\n\n# Page 34\n\nnext page\n";

    #[test]
    fn read_layout_single_page() {
        let out = read_layout(SIDECAR, "33");
        assert!(out.contains("Damage Bonus"), "got: {out}");
        assert!(out.contains("65-84 | -1 | -1"));
        assert!(!out.contains("next page"), "must not bleed into p34");
        assert!(!out.contains("DEX table intro"), "must not bleed into p32");
    }

    #[test]
    fn read_layout_range() {
        let out = read_layout(SIDECAR, "32-33");
        assert!(out.contains("DEX table intro") && out.contains("Damage Bonus"));
        assert!(!out.contains("next page"));
    }

    #[test]
    fn read_layout_missing_page_is_empty_not_panic() {
        let out = read_layout(SIDECAR, "999");
        assert!(out.trim().is_empty() || out.contains("[no layout"), "got: {out}");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p trpg-rule-agent read_layout 2>&1 | tail -20`
Expected: FAIL — `cannot find function read_layout`.

- [ ] **Step 3: Write minimal implementation** (top of the new file)

```rust
//! Chargen formula compiler — a focused rule-agent slice that turns the deep
//! reader's PROSE chargen formulas + the duotext `.layout.md` tables into the
//! machine-evaluable §4 `derived_values`, validated by a trpg-formula round-trip.

use serde_json::Value;

/// Parse a page range like "33" or "32-34".
fn parse_pages(s: &str) -> (u32, u32) {
    let s = s.trim();
    if let Some((a, b)) = s.split_once('-') {
        (a.trim().parse().unwrap_or(0), b.trim().parse().unwrap_or(u32::MAX))
    } else {
        let p = s.parse().unwrap_or(0);
        (p, p)
    }
}

/// Return the text of the requested pages from a page-anchored `.layout.md`
/// sidecar (the duotext aligned-table view). Pages are delimited by
/// `<!-- … page=N … -->` markers. Out-of-range / missing → empty string.
pub fn read_layout(sidecar_text: &str, pages: &str) -> String {
    let (a, b) = parse_pages(pages);
    let mut out = String::new();
    // Split on the page-anchor comment; each segment after the first carries one page.
    for seg in sidecar_text.split("<!-- source_id=").skip(1) {
        // seg starts like: "x page=33 text_hash=b -->\n\n# Page 33\n\n<body>"
        let page = seg
            .split("page=").nth(1)
            .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        if a <= page && page <= b {
            if let Some(body) = seg.split_once("-->").map(|(_, rest)| rest) {
                out.push_str(&format!("--- p{page} (layout) ---\n{}\n\n", body.trim()));
            }
        }
    }
    out
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p trpg-rule-agent read_layout 2>&1 | tail -20`
Expected: PASS (3 tests).

- [ ] **Step 5: Checkpoint** — `cargo build -p trpg-rule-agent` green.

---

## Task 2: round-trip guardrail (`finalize_compiled` + `sample_inputs_from_template`)

**Files:**
- Modify: `crates/trpg-rule-agent/Cargo.toml`
- Modify: `crates/trpg-rule-agent/src/reader/chargen_compile.rs`
- Test: same file

Deterministic: take the agent's raw §4 records (as `Vec<Value>`), build sample inputs from the template's stat fields, run `trpg_formula::evaluate_chargen`, and for each record set `status=provisional` when it does not resolve (unresolved refs / cycle / eval error). Map each record into a `DerivedValue` (the `id` field becomes `field_id`). NEVER drop a record; NEVER invent a value.

- [ ] **Step 1: Add the dependency**

In `crates/trpg-rule-agent/Cargo.toml` under `[dependencies]`:

```toml
trpg-formula = { path = "../trpg-formula" }
```

- [ ] **Step 2: Write the failing test**

```rust
#[cfg(test)]
mod finalize_tests {
    use super::*;
    use serde_json::json;
    use trpg_model::{CharacterTemplate, CharacterField};

    fn tmpl_with_stats(ids: &[&str]) -> CharacterTemplate {
        let mut t = CharacterTemplate::default();
        t.fields = ids.iter().map(|id| CharacterField {
            field_id: id.to_string(), title: id.to_string(),
            field_type: "stat".into(), ..Default::default()
        }).collect();
        t
    }

    #[test]
    fn resolvable_record_keeps_declared_status() {
        let recs = vec![json!({
            "id":"hp_max","role":"resource_max","input_kind":"derived","result_type":"int",
            "expr":"floor(({{con}}+{{siz}})/10)","status":"source_backed"
        })];
        let t = tmpl_with_stats(&["con","siz"]);
        let (dvs, gaps) = finalize_compiled(recs, &t);
        assert_eq!(dvs.len(), 1);
        assert_eq!(dvs[0].field_id, "hp_max");
        assert_eq!(dvs[0].status.as_deref(), Some("source_backed"));
        assert!(gaps.is_empty());
    }

    #[test]
    fn unresolved_ref_is_downgraded_to_provisional_not_dropped() {
        let recs = vec![json!({
            "id":"weird","role":"attribute","input_kind":"derived","result_type":"int",
            "expr":"floor({{nonexistent_stat}}/2)","status":"source_backed"
        })];
        let t = tmpl_with_stats(&["con","siz"]); // no `nonexistent_stat`
        let (dvs, _gaps) = finalize_compiled(recs, &t);
        assert_eq!(dvs.len(), 1, "record kept, not dropped");
        assert_eq!(dvs[0].status.as_deref(), Some("provisional"));
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p trpg-rule-agent finalize 2>&1 | tail -20`
Expected: FAIL — `cannot find function finalize_compiled`.

- [ ] **Step 4: Write the implementation**

Add to `chargen_compile.rs` (imports at top: `use serde_json::{json, Map, Value};`, `use trpg_model::{CharacterTemplate, DerivedValue};`):

```rust
/// Build round-trip sample inputs: every stat field = 50 (resolves {{con}} etc.).
pub fn sample_inputs_from_template(t: &CharacterTemplate) -> Map<String, Value> {
    let mut m = Map::new();
    let mut stats = Map::new();
    for f in &t.fields {
        if f.field_type == "stat" || f.field_type == "characteristic" {
            stats.insert(f.field_id.clone(), json!(50));
        }
    }
    m.insert("stats".into(), Value::Object(stats));
    m
}

/// Round-trip every compiled §4 record through the evaluator; downgrade records
/// that do not resolve to `provisional` (guardrail: verify, never fabricate).
/// Returns (DerivedValues in §4 form, gap notes). Records are never dropped.
pub fn finalize_compiled(records: Vec<Value>, template: &CharacterTemplate) -> (Vec<DerivedValue>, Vec<String>) {
    let inputs = sample_inputs_from_template(template);
    let report = trpg_formula::evaluate_chargen(&records, &inputs);
    // id -> resolved? (provisional when unresolved/cycle/error)
    let mut bad: std::collections::HashSet<String> = std::collections::HashSet::new();
    for ev in &report.values {
        if ev.status == "provisional" || ev.value.is_none() {
            bad.insert(ev.id.trim().to_ascii_lowercase());
        }
    }
    let mut out = Vec::new();
    for mut r in records {
        if let Some(obj) = r.as_object_mut() {
            // §4 uses `id`; DerivedValue uses `field_id`.
            if let Some(idv) = obj.remove("id") { obj.entry("field_id").or_insert(idv); }
            let fid = obj.get("field_id").and_then(Value::as_str).unwrap_or("").trim().to_ascii_lowercase();
            if bad.contains(&fid) { obj.insert("status".into(), json!("provisional")); }
            // DerivedValue requires `formula`; mirror `expr` into it when absent.
            if !obj.contains_key("formula") {
                let f = obj.get("expr").or_else(|| obj.get("attr_derived")).cloned().unwrap_or(json!(""));
                obj.insert("formula".into(), f);
            }
        }
        match serde_json::from_value::<DerivedValue>(r) {
            Ok(dv) => out.push(dv),
            Err(e) => report_push(&mut out, e), // see helper below
        }
    }
    (out, report.gaps)
}

fn report_push(_out: &mut Vec<DerivedValue>, _e: serde_json::Error) { /* malformed record dropped silently is NOT ok */ }
```

NOTE: replace the `report_push` stub — a record that fails to deserialize must surface as a gap, not vanish. Use this instead:

```rust
pub fn finalize_compiled(records: Vec<Value>, template: &CharacterTemplate) -> (Vec<DerivedValue>, Vec<String>) {
    let inputs = sample_inputs_from_template(template);
    let report = trpg_formula::evaluate_chargen(&records, &inputs);
    let mut bad = std::collections::HashSet::new();
    for ev in &report.values {
        if ev.status == "provisional" || ev.value.is_none() { bad.insert(ev.id.trim().to_ascii_lowercase()); }
    }
    let mut out = Vec::new();
    let mut gaps = report.gaps.clone();
    for mut r in records {
        if let Some(obj) = r.as_object_mut() {
            if let Some(idv) = obj.remove("id") { obj.entry("field_id").or_insert(idv); }
            let fid = obj.get("field_id").and_then(Value::as_str).unwrap_or("").trim().to_ascii_lowercase();
            if bad.contains(&fid) { obj.insert("status".into(), json!("provisional")); }
            if !obj.contains_key("formula") {
                let f = obj.get("expr").or_else(|| obj.get("attr_derived")).cloned().unwrap_or(json!(""));
                obj.insert("formula".into(), f);
            }
        }
        let label = r.get("field_id").and_then(Value::as_str).unwrap_or("?").to_string();
        match serde_json::from_value::<DerivedValue>(r) {
            Ok(dv) => out.push(dv),
            Err(e) => gaps.push(format!("compiled record `{label}` failed to coerce: {e}")),
        }
    }
    (out, gaps)
}
```

(Delete the first `finalize_compiled` + `report_push` stub; keep only this second version.)

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p trpg-rule-agent finalize 2>&1 | tail -20`
Expected: PASS (2 tests).

- [ ] **Step 6: Checkpoint** — `cargo build -p trpg-rule-agent` green.

---

## Task 3: the compile agent slice (`compile_chargen_formulas`)

**Files:**
- Modify: `crates/trpg-rule-agent/src/reader/chargen_compile.rs`
- Modify: `crates/trpg-rule-agent/src/reader/mod.rs`

LLM loop (no deterministic unit test — validated live in Task 7). Reuses `tools::{toc,search,read}` over `units` + `read_layout` over the sidecar; small submit schema = only the §4 `derived_values` array. Generic prompt (body/will/ref examples — NO CoC hit-words). On success, calls `finalize_compiled` and writes back into `template.derived_values`.

- [ ] **Step 1: Implement the public entry + loop**

```rust
use trpg_llm::LlmClient;
use super::units::Unit;
use super::tools;
use serde_json::json;

pub struct CompileCtx<'a> {
    pub units: &'a [Unit],
    pub sidecar_text: Option<String>,   // contents of {id}.layout.md, if duotext-parsed
    pub located_pages: String,          // hint: pages the reader flagged as formula locators, e.g. "4,11,33"
}

const COMPILE_SYS: &str = r#"You COMPILE a tabletop RPG's character-creation math into machine-evaluable records for a deterministic evaluator. You are given the game's prose chargen formulas (already located) and tools to read pages. Produce ONE array `derived_values` of §4 records — nothing else.

Each record: {id, role(attribute|skill|resource|resource_max|background), input_kind(player|derived|hybrid), recompute(live|once), result_type(int|float|dice_or_int), expr, attr_derived, base, allocations, lookup_tables, depends_on, min, max, clamp_max, source_ref, status(source_backed|provisional)}.

RULES (generic — apply to THIS game's real fields, do NOT assume any specific game):
- `expr` is MACHINE form with {{characteristic_id}} placeholders (lowercase) + floor/ceil/round/min/max/lookup/if. NOT prose. e.g. floor(({{con}}+{{siz}})/10).
- For a value read FROM A TABLE: read_layout the page, INLINE the actual rows into lookup_tables {name:{ranges:[{min,max,value}]}} (value = number or dice string like "1d4"), and set expr=lookup(name,{{x}}+{{y}}).
- For a SKILL whose starting value is derived from a characteristic (e.g. a dodge/evade skill = half a characteristic): role=skill, input_kind=hybrid, attr_derived=floor({{that_char}}/2), allocations for any player-spent points.
- Ground each record in a page you READ; if you cannot verify it, set status=provisional. NEVER invent a number or a table row.

TOOLS: get_toc, search(keywords), read(pages) for prose; read_layout(pages) for the aligned TABLE view (use it for any table-based value). Then call submit_chargen with {derived_values:[...]}. Budget ~10 tool calls; be targeted, the formula pages are already located for you."#;

/// Compile the template's chargen math into §4 derived_values, IN PLACE.
/// Returns the gaps (provisional/uncoercible records) for logging. On total
/// failure the template's existing (prose) derived_values are left untouched.
pub async fn compile_chargen_formulas(
    client: &dyn LlmClient,
    template: &mut trpg_model::CharacterTemplate,
    ctx: CompileCtx<'_>,
    budget: usize,
) -> Vec<String> {
    let prose = serde_json::to_string(&template.derived_values).unwrap_or_default();
    let skills: Vec<&str> = template.fields.iter()
        .filter(|f| f.field_type == "skill").map(|f| f.field_id.as_str()).collect();
    let seed = format!(
        "Game character math to compile.\nLocated formula pages: {}\nExisting PROSE formulas (compile these to machine form, fix/extend as needed):\n{}\nSkill fields (check which have a characteristic-derived base): {:?}\nRead the located pages (use read_layout for tables), then submit_chargen.",
        ctx.located_pages, prose, skills);
    let submit = tools::submit_tool(
        "submit_chargen",
        "Submit the compiled machine-evaluable chargen records.",
        json!({"derived_values":{"type":"array","items":{"type":"object"}}}),
        &["derived_values"]);
    let mut tool_schemas = tools::nav_tools();
    tool_schemas.push(json!({"type":"function","function":{"name":"read_layout","description":"Read the ALIGNED TABLE view of page(s) like \"33\" or \"33-34\". Use for any table-based value (damage bonus, build, carry).","parameters":{"type":"object","properties":{"pages":{"type":"string"}},"required":["pages"]}}}));
    tool_schemas.push(submit);

    match run_compile_loop(client, COMPILE_SYS, &seed, &tool_schemas, &ctx, budget).await {
        Some(records) if !records.is_empty() => {
            let (dvs, gaps) = finalize_compiled(records, template);
            if !dvs.is_empty() { template.derived_values = dvs; }
            gaps
        }
        _ => vec!["chargen compiler produced nothing; kept prose derived_values".into()],
    }
}
```

- [ ] **Step 2: Implement the loop + dispatch** (mirrors `agent::run_loop` but units+sidecar dispatch and a §4-array submit)

```rust
async fn run_compile_loop(
    client: &dyn LlmClient, system: &str, seed: &str,
    tool_schemas: &[Value], ctx: &CompileCtx<'_>, budget: usize,
) -> Option<Vec<Value>> {
    let mut msgs = vec![
        json!({"role":"system","content":system}),
        json!({"role":"user","content":seed}),
    ];
    for _ in 0..(budget + 8) {
        let resp = client.complete_with_tools(msgs.clone(), tool_schemas.to_vec()).await.ok()?;
        let message = resp.pointer("/choices/0/message").cloned().unwrap_or_else(|| json!({}));
        let tcs = message.get("tool_calls").and_then(Value::as_array).cloned().unwrap_or_default();
        if tcs.is_empty() {
            msgs.push(message);
            msgs.push(json!({"role":"user","content":"Use the tools, then call submit_chargen."}));
            continue;
        }
        msgs.push(message);
        for tc in &tcs {
            let name = tc.pointer("/function/name").and_then(Value::as_str).unwrap_or("");
            let args: Value = tc.pointer("/function/arguments").and_then(Value::as_str)
                .and_then(|s| serde_json::from_str(s).ok()).unwrap_or_else(|| json!({}));
            let id = tc.get("id").and_then(Value::as_str).unwrap_or("");
            if name == "submit_chargen" {
                return args.get("derived_values").and_then(Value::as_array).cloned();
            }
            let out = compile_dispatch(ctx, name, &args);
            msgs.push(json!({"role":"tool","tool_call_id":id,"content":out}));
        }
    }
    None
}

fn compile_dispatch(ctx: &CompileCtx<'_>, name: &str, args: &Value) -> String {
    let cap = |s: String| if s.len() <= 3000 { s } else { s.chars().take(3000).collect() };
    match name {
        "get_toc" => tools::toc(ctx.units, 40),
        "search" => {
            let kws: Vec<String> = args.get("keywords").and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
            cap(tools::search(ctx.units, &kws, 8))
        }
        "read" => cap(tools::read(ctx.units, args.get("pages").and_then(Value::as_str).unwrap_or(""))),
        "read_layout" => match &ctx.sidecar_text {
            Some(s) => cap(read_layout(s, args.get("pages").and_then(Value::as_str).unwrap_or(""))),
            None => "[no layout view available for this book — mark table-based values provisional]".into(),
        },
        _ => format!("unknown tool {name}"),
    }
}
```

- [ ] **Step 3: Export from `mod.rs`**

In `crates/trpg-rule-agent/src/reader/mod.rs` add:

```rust
mod chargen_compile;
pub use chargen_compile::{compile_chargen_formulas, CompileCtx};
```

And ensure the parent `crates/trpg-rule-agent/src/lib.rs` re-exports what parser needs (it already does `pub use reader::...`; add `compile_chargen_formulas, CompileCtx` to that re-export list if present).

- [ ] **Step 4: Checkpoint** — `cargo build -p trpg-rule-agent` green. (No unit test; live-validated in Task 7.)

---

## Task 4: wire the compiler into `parse_rulebook`

**Files:**
- Modify: `crates/trpg-parser/src/lib.rs:~519` (after `coerce_character_template`, before `normalize_…`)

- [ ] **Step 1: Insert the compile call**

Right after the `let mut character_template = match reader_char_template { … };` block (currently ending at line ~519) and BEFORE `normalize_character_sheet_template_schema(&mut character_template, …)`:

```rust
        // §4 chargen compiler: upgrade the reader's PROSE derived_values to
        // machine-evaluable form (expr + inlined tables + char-derived skills),
        // round-trip-validated. Only when the reader produced a template.
        if reader_run_kit.is_some() {
            let units_path = self.config.data_dir.join("parsed/source_units").join(format!("{}.semantic_units.jsonl", doc.source_id));
            if let Ok(units) = reader::load_units(&units_path) {
                let sidecar_text = doc.metadata.get("layout_sidecar_path")
                    .and_then(|v| v.as_str())
                    .and_then(|p| std::fs::read_to_string(p).ok());
                let located_pages = character_template.source_refs.iter()
                    .filter_map(|r| r.page.map(|p| p.to_string()))
                    .collect::<Vec<_>>().join(",");
                let ctx = reader::CompileCtx { units: &units, sidecar_text, located_pages };
                let gaps = reader::compile_chargen_formulas(self.llm.as_ref(), &mut character_template, ctx, 10).await;
                if !gaps.is_empty() { tracing::info!(?gaps, "chargen compiler gaps (provisional/uncoerced)"); }
            }
        }
```

NOTE on `source_refs.page`: confirm the `SourceRef` field name/type at `trpg-model` (the restored template showed `source_refs[*].page`). If it's `Option<u32>`, the `.map(|p| p.to_string())` above is correct; if it's `u32`, drop the `filter_map`/`map` wrapping.

- [ ] **Step 2: Checkpoint** — `cargo build -p trpg-parser` green.

---

## Task 5: runtime — read the template, layer the override

**Files:**
- Modify: `crates/trpg-runtime/src/chargen.rs`
- Test: same file

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod compiler_runtime_tests {
    use super::*;
    use serde_json::json;
    use trpg_model::{CharacterTemplate, DerivedValue};

    fn dv(field_id: &str, expr: &str) -> DerivedValue {
        DerivedValue { field_id: field_id.into(), formula: expr.into(),
            expr: Some(expr.into()), role: Some("resource_max".into()),
            input_kind: Some("derived".into()), ..Default::default() }
    }

    #[test]
    fn spec_from_template_maps_field_id_to_id_and_skips_prose_only() {
        let mut t = CharacterTemplate::default();
        t.derived_values = vec![
            dv("hp_max", "floor(({{con}}+{{siz}})/10)"),
            DerivedValue { field_id: "luck".into(), formula: "3D6 x 5".into(), ..Default::default() }, // prose-only, no expr
        ];
        let spec = chargen_spec_from_template(&t);
        assert_eq!(spec.len(), 1, "prose-only luck skipped");
        assert_eq!(spec[0]["id"], json!("hp_max"));
        assert_eq!(spec[0]["expr"], json!("floor(({{con}}+{{siz}})/10)"));
    }

    #[test]
    fn override_replaces_by_id_keeps_others() {
        let base = vec![json!({"id":"hp_max","expr":"floor(({{con}}+{{siz}})/10)"}),
                        json!({"id":"sanity","expr":"{{pow}}"})];
        let ovr = vec![json!({"id":"sanity","expr":"{{pow}}","max":99,"status":"source_backed"})];
        let merged = merge_override(base, ovr);
        assert_eq!(merged.len(), 2);
        let san = merged.iter().find(|r| r["id"]=="sanity").unwrap();
        assert_eq!(san["max"], json!(99), "override won");
        assert!(merged.iter().any(|r| r["id"]=="hp_max"), "non-overridden kept");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p trpg-runtime compiler_runtime 2>&1 | tail -20`
Expected: FAIL — `cannot find function chargen_spec_from_template` / `merge_override`.

- [ ] **Step 3: Implement** (add to `chargen.rs`; needs `use trpg_model::CharacterTemplate;`)

```rust
/// Convert a template's compiled derived_values into §4 value records. Only
/// MACHINE-evaluable rows (have `expr` or `attr_derived`) are emitted; prose-only
/// rows are skipped (they would otherwise evaluate to 0). `field_id` -> `id`.
pub fn chargen_spec_from_template(t: &CharacterTemplate) -> Vec<Value> {
    let mut out = Vec::new();
    for d in &t.derived_values {
        let machine = d.expr.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
            || d.attr_derived.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false);
        if !machine { continue; }
        let mut v = serde_json::to_value(d).unwrap_or(Value::Null);
        if let Some(obj) = v.as_object_mut() {
            if let Some(fid) = obj.remove("field_id") { obj.insert("id".into(), fid); }
        }
        out.push(v);
    }
    out
}

/// Layer an override list over a base spec: a record whose `id` matches replaces
/// the base record; new ids are appended.
pub fn merge_override(base: Vec<Value>, overrides: Vec<Value>) -> Vec<Value> {
    let idof = |v: &Value| v.get("id").and_then(Value::as_str).unwrap_or("").trim().to_ascii_lowercase();
    let mut out = base;
    for o in overrides {
        let oid = idof(&o);
        if oid.is_empty() { continue; }
        if let Some(slot) = out.iter_mut().find(|b| idof(b) == oid) { *slot = o; }
        else { out.push(o); }
    }
    out
}
```

- [ ] **Step 4: Change `load_chargen_spec` to read the template + override**

Replace the current `load_chargen_spec(ruleset_id)` body so the TEMPLATE is the default source and `{ruleset}.chargen.override.json` (then legacy `{ruleset}.chargen.json`) layers on top:

```rust
/// §4 chargen spec for a ruleset: compiled template derived_values (default),
/// with an optional hand-authored override layered per-id. Graceful when absent.
pub fn load_chargen_spec(ruleset_id: &str, template: &CharacterTemplate) -> Vec<Value> {
    let base = chargen_spec_from_template(template);
    let dir = std::env::var("TRPG_DATA_DIR").unwrap_or_else(|_| "data".into());
    let read = |name: String| -> Vec<Value> {
        let p = std::path::Path::new(&dir).join("parsed").join("characters").join(name);
        std::fs::read_to_string(&p).ok().and_then(|s| serde_json::from_str::<Vec<Value>>(&s).ok()).unwrap_or_default()
    };
    let mut overrides = read(format!("{}.chargen.override.json", safe_id(ruleset_id)));
    if overrides.is_empty() { overrides = read(format!("{}.chargen.json", safe_id(ruleset_id))); }
    merge_override(base, overrides)
}
```

- [ ] **Step 5: Update the call site in `create_and_bind_character`**

Change `let chargen_spec = load_chargen_spec(ruleset_id);` to:

```rust
        let chargen_spec = load_chargen_spec(ruleset_id, &template);
```

(`template` is already in scope at that point — `let template = pack.sheet_template.clone();`.)

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p trpg-runtime compiler_runtime 2>&1 | tail -20`
Expected: PASS (2 tests). Then `cargo test -p trpg-runtime chargen 2>&1 | tail -20` — the existing 2 chargen tests still pass (they call `apply_chargen_formulas` directly, unaffected).

- [ ] **Step 7: Checkpoint** — `cargo build -p trpg-runtime` green.

---

## Task 6: cache invalidation (compiler version in parse hash)

**Files:**
- Modify: `crates/trpg-parser/src/lib.rs:80`

- [ ] **Step 1: Add the compiler version token**

In `ParserConfig::new`'s `parse_config_hash` format string, insert `chargen_compiler=v1;` after `prompt=rulebook_reader_agent_v1;`:

```rust
            parse_config_hash: sha256_hex(format!("parser=v1.16.2;schema=v1;prompt=rulebook_reader_agent_v1;chargen_compiler=v1;full_chunks={parse_full_chunks};pdf_backend={};clean_mode={};oxidize_chunk_target_chars={oxidize_chunk_target_chars};llm_clean={llm_clean_extraction};semantic_units={semantic_unit_conditioning};semantic_unit_max_chars={semantic_unit_max_chars};llm_semantic_wash={llm_semantic_wash};rule_steward_first_pass={rule_steward_first_pass}", pdf_backend.as_str(), oxidize_clean_mode.as_str())),
```

- [ ] **Step 2: Checkpoint** — `cargo build --workspace 2>&1 | grep -E "^error|Finished" | tail -5` green.

---

## Task 7: live validation across 3 rulesets (duotext)

**Files:** none (operational). Re-parse each ruleset with the duotext default (reuses `.md`/`.layout.md` cache; bumped hash → bundle re-parses → compiler runs). Watch that the MinerU vision model does NOT spawn.

- [ ] **Step 1: Re-parse CoC (duotext)** — DB :54347, mini

```bash
cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
cargo build -p trpg-cli --bin trpg 2>&1 | grep -E "^error|Finished" | tail -2
DATABASE_URL="postgres://chatrpg:chatrpg@localhost:54347/chatrpg" TRPG_LLM_MODEL=gpt-5.4-mini TRPG_PDF_BACKEND=duotext \
  ./target/debug/trpg parse-all --pdf-backend duotext --data-dir /Users/haoli/leehow/code/chatrpgv2/_rstest_coc \
  > /Users/haoli/leehow/code/chatrpgv2/mineru_parseall_logs/coc_compiler.log 2>&1
```

- [ ] **Step 2: Verify CoC compiled spec vs the gold standard**

```bash
python3 - <<'PY'
import json
t=json.load(open("/Users/haoli/leehow/code/chatrpgv2/_rstest_coc/parsed/characters/call_of_cthulhu_7e.character_sheet_template.json"))
dv={d["field_id"]:d for d in t.get("derived_values",[])}
print("compiled ids:", list(dv))
def chk(fid, key, want):
    got=dv.get(fid,{}).get(key)
    print(("OK " if (want in str(got) if want else got is not None) else "!! "), fid, key, "=", got)
chk("hit_points" if "hit_points" in dv else "hp_max","expr","floor")
chk("magic_points" if "magic_points" in dv else "mp_max","expr","{{pow}}")
chk("sanity_points" if "sanity_points" in dv else "sanity","expr","{{pow}}")
chk("dodge","attr_derived","floor({{dex}}")          # char-derived skill
chk("damage_bonus","lookup_tables","ranges")          # inlined table
chk("build","lookup_tables","ranges")
PY
```
Expected: `hp/mp/san` have machine `expr`; `dodge` has `attr_derived=floor({{dex}}/2)`; `damage_bonus`/`build` have inlined `lookup_tables.ranges`. Provisional is acceptable where the page couldn't be verified — but values must NOT be fabricated.

- [ ] **Step 3: Re-parse Cyberpunk RED** — DB :54346, data-dir `chatrpg-rs-v1.16.2/data`. Verify the compiled spec has machine `expr` for HP-from-BODY+WILL / Humanity and round-trips (no crash, no fabricated numbers). Near-miss on exact formula is OK (no gold standard); fabrication is NOT.

- [ ] **Step 4: Re-parse Triangle Agency** — DB :54347, data-dir `_rstest_triangle`. Verify it compiles without crashing; a near-empty `derived_values` is ACCEPTABLE (Triangle has few numeric chargen formulas) — confirm nothing was fabricated.

- [ ] **Step 5: Checkpoint** — all three parsed without panics; CoC matches gold standard; Cyberpunk/Triangle compiled-or-empty with no fabrication. Record counts in the handoff.

---

## Self-Review (done at write time)

- **Spec coverage:** §5.1 compile slice → Task 3; §5.2 round-trip → Task 2; §5.3 in-place upgrade/parse_rulebook → Task 4; §5.4 read_layout → Task 1; §5.5 load_chargen_spec+override → Task 5; §5.6 cache → Task 6; §9 3-ruleset validation → Task 7. All covered.
- **Placeholders:** none — every code step has real code; the one `report_push` stub is explicitly flagged for replacement with the full version shown directly below it.
- **Type consistency:** `read_layout(&str,&str)->String`, `finalize_compiled(Vec<Value>,&CharacterTemplate)->(Vec<DerivedValue>,Vec<String>)`, `compile_chargen_formulas(&dyn LlmClient,&mut CharacterTemplate,CompileCtx,usize)->Vec<String>`, `chargen_spec_from_template(&CharacterTemplate)->Vec<Value>`, `merge_override(Vec<Value>,Vec<Value>)->Vec<Value>`, `load_chargen_spec(&str,&CharacterTemplate)->Vec<Value>` — used consistently across tasks. `id`↔`field_id` mapping handled in both `finalize_compiled` (id→field_id) and `chargen_spec_from_template` (field_id→id), symmetric.
- **Open verifications flagged inline:** `SourceRef.page` type (Task 4 Step 1), `CharacterField`/`DerivedValue` `..Default::default()` availability (both derive Default — confirmed), `lib.rs` re-export list (Task 3 Step 3).
