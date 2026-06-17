//! Chargen formula compiler — a focused rule-agent slice that turns the deep
//! reader's PROSE chargen formulas + the duotext `.layout.md` aligned tables into
//! machine-evaluable §4 `derived_values`, validated by a trpg-formula round-trip.
//!
//! Design: docs/superpowers/specs/2026-06-08-chargen-formula-compiler-design.md
//! It is NOT a bolt-on tool — it is the rule agent's focused SECOND pass: small
//! submit schema (only the §4 array), reuse of the reader's retrieval tools, and
//! a deterministic guardrail (round-trip eval → provisional, never fabricate).

use super::tools;
use super::units::Unit;
use serde_json::{json, Map, Value};
use trpg_llm::LlmClient;
use trpg_model::{CharacterField, CharacterTemplate, DerivedValue};

// ---------------------------------------------------------------------------
// read_layout: slice the page-anchored `.layout.md` sidecar by page
// ---------------------------------------------------------------------------

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
    for seg in sidecar_text.split("<!-- source_id=").skip(1) {
        let page = seg
            .split("page=")
            .nth(1)
            .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        if a <= page && page <= b {
            if let Some((_, body)) = seg.split_once("-->") {
                out.push_str(&format!("--- p{page} (layout) ---\n{}\n\n", body.trim()));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Deterministic guardrail: round-trip every record, downgrade unresolved ones
// ---------------------------------------------------------------------------

/// Build round-trip sample inputs: every stat field = 50, at TOP level (matches
/// how `apply_chargen_formulas` flattens stats so `{{con}}` resolves to `con`).
pub fn sample_inputs_from_template(t: &CharacterTemplate) -> Map<String, Value> {
    let mut m = Map::new();
    for f in &t.fields {
        if f.field_type == "stat" || f.field_type == "characteristic" {
            m.insert(f.field_id.clone(), json!(50));
        }
    }
    m
}

/// First STRING key of a lookup table's ranges (`key`/`min`/`max`), if any — the
/// marker that the table is CATEGORICAL (maps a player CHOICE, e.g. class -> hit
/// die, rather than a numeric span). Numeric-only tables return None.
fn first_categorical_key(tbl: &Value) -> Option<String> {
    let ranges = tbl.get("ranges")?.as_array()?;
    for r in ranges {
        for f in ["key", "min", "max"] {
            if let Some(s) = r.get(f).and_then(|v| v.as_str()) {
                let t = s.trim();
                if !t.is_empty() && t.parse::<f64>().is_err() {
                    return Some(t.to_string());
                }
            }
        }
    }
    None
}

/// True when a lookup table keys on a string (categorical), not a numeric range.
fn table_is_categorical(tbl: &Value) -> bool {
    first_categorical_key(tbl).is_some()
}

/// The machine exprs a raw §4 record evaluates (expr + attr_derived).
fn record_exprs_raw(r: &Value) -> Vec<String> {
    ["expr", "attr_derived"].iter()
        .filter_map(|k| r.get(*k).and_then(|v| v.as_str()))
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .collect()
}

/// Give every CATEGORICAL lookup key a representative sample value (the table's
/// first string key), so a `class -> value` record RESOLVES in the round-trip and
/// keeps its source-backed status instead of degrading for want of a sample.
/// Refs that are themselves records (resolve via topo) and numeric stats (already
/// sampled at 50) are left alone. Generic — driven by table shape, not field names.
fn add_categorical_samples(inputs: &mut Map<String, Value>, records: &[Value]) {
    let is_record_id = |k: &str| records.iter().any(|x|
        x.get("id").and_then(|v| v.as_str()).map(|s| s.trim().eq_ignore_ascii_case(k)).unwrap_or(false));
    for r in records {
        let Some(tables) = r.get("lookup_tables").and_then(|v| v.as_object()) else { continue };
        let Some(key) = tables.values().find_map(first_categorical_key) else { continue };
        for e in record_exprs_raw(r) {
            for refr in trpg_formula::extract_refs(&e) {
                let k = refr.trim_start_matches("derived.").trim().to_ascii_lowercase();
                if k.is_empty() || inputs.contains_key(&k) || is_record_id(&k) { continue; }
                inputs.insert(k, json!(key.clone()));
            }
        }
    }
}

/// Normalize an inline lookup-table value: strip a leading "+", and map the
/// "no value" spellings (None / - / em-dash / blank) to "0". Keeps real dice
/// strings ("1d4") and numbers as-is. Generic — no ruleset-specific terms.
fn normalize_lookup_value(v: &mut Value) {
    if let Some(s) = v.as_str() {
        let t = s.trim().trim_start_matches('+').trim();
        let low = t.to_ascii_lowercase();
        if low.is_empty() || matches!(low.as_str(), "none" | "-" | "—" | "–" | "no bonus" | "n/a" | "na") {
            *v = json!("0");
        } else if t != s {
            *v = json!(t);
        }
    }
}

/// Make a raw model record safe to coerce into a DerivedValue: drop any
/// scalar string field that came back as a non-string (a stray number must not
/// fail the whole record), and normalize inline lookup-table values.
fn sanitize_record(obj: &mut Map<String, Value>) {
    for k in ["role", "input_kind", "recompute", "result_type", "status", "notes", "expr", "attr_derived", "formula", "evaluator"] {
        if obj.get(k).map(|v| !v.is_string()).unwrap_or(false) {
            obj.remove(k);
        }
    }
    // Hybrid-skill repair: models sometimes write the player-allocation part as a
    // fake `{{allocations}}` token inside `expr` (which can't resolve → provisional)
    // instead of the §4 hybrid shape. Route the remaining characteristic part to
    // `attr_derived` and let the allocations array carry the points, so it resolves.
    if let Some(expr) = obj.get("expr").and_then(|v| v.as_str()) {
        if expr.contains("{{allocations}}") {
            let cleaned = expr.replace("{{allocations}}", "").trim().trim_matches('+').trim().to_string();
            obj.remove("expr");
            if !cleaned.is_empty() && obj.get("attr_derived").and_then(|v| v.as_str()).map(str::is_empty).unwrap_or(true) {
                obj.insert("attr_derived".into(), json!(cleaned));
            }
            obj.entry("allocations").or_insert_with(|| json!([]));
        }
    }
    if let Some(lt) = obj.get_mut("lookup_tables").and_then(|v| v.as_object_mut()) {
        for (_, tbl) in lt.iter_mut() {
            if let Some(ranges) = tbl.get_mut("ranges").and_then(|v| v.as_array_mut()) {
                for r in ranges {
                    if let Some(val) = r.get_mut("value") {
                        normalize_lookup_value(val);
                    }
                }
            }
        }
    }
}

/// Round-trip every compiled §4 record through the evaluator; downgrade records
/// that do not resolve to `provisional` (guardrail: verify, never fabricate).
/// Maps §4 `id` -> DerivedValue `field_id`. Records are never dropped silently;
/// a record that cannot coerce becomes a gap note. Returns (DerivedValues, gaps).
pub fn finalize_compiled(mut records: Vec<Value>, template: &CharacterTemplate) -> (Vec<DerivedValue>, Vec<String>) {
    for r in records.iter_mut() {
        if let Some(obj) = r.as_object_mut() { sanitize_record(obj); }
    }
    let mut inputs = sample_inputs_from_template(template);
    add_categorical_samples(&mut inputs, &records);
    let report = trpg_formula::evaluate_chargen(&records, &inputs);
    let mut bad: std::collections::HashSet<String> = std::collections::HashSet::new();
    for ev in &report.values {
        if ev.status == "provisional" || ev.value.is_none() {
            bad.insert(ev.id.trim().to_ascii_lowercase());
        }
    }
    let mut out = Vec::new();
    let mut gaps = report.gaps.clone();
    for mut r in records {
        if let Some(obj) = r.as_object_mut() {
            // §4 uses `id`; DerivedValue uses `field_id`.
            if let Some(idv) = obj.remove("id") {
                obj.entry("field_id").or_insert(idv);
            }
            let fid = obj.get("field_id").and_then(Value::as_str).unwrap_or("").trim().to_ascii_lowercase();
            if bad.contains(&fid) {
                obj.insert("status".into(), json!("provisional"));
            }
            // DerivedValue's legacy required fields lack serde(default) — ensure them.
            if !obj.contains_key("formula") {
                let f = obj.get("expr").or_else(|| obj.get("attr_derived")).cloned().unwrap_or_else(|| json!(""));
                obj.insert("formula".into(), f);
            }
            obj.entry("depends_on").or_insert_with(|| json!([]));
            obj.entry("evaluator").or_insert_with(|| json!(""));
        }
        let label = r.get("field_id").and_then(Value::as_str).unwrap_or("?").to_string();
        match serde_json::from_value::<DerivedValue>(r) {
            Ok(dv) => out.push(dv),
            Err(e) => gaps.push(format!("compiled record `{label}` failed to coerce: {e}")),
        }
    }
    (out, gaps)
}

// ---------------------------------------------------------------------------
// The compile slice: a focused LLM loop whose only output is a §4 array
// ---------------------------------------------------------------------------

pub struct CompileCtx<'a> {
    pub units: &'a [Unit],
    /// Contents of `{id}.layout.md`, if the book was duotext-parsed.
    pub sidecar_text: Option<String>,
    /// Hint: pages the reader flagged as formula locators, e.g. "4,11,33".
    pub located_pages: String,
    /// The game's enumerated skill names (from the reader's option catalogs), so
    /// the compiler can check which skills have a characteristic-derived base.
    pub skill_names: Vec<String>,
}

const COMPILE_SYS: &str = r#"You COMPILE a tabletop RPG's character-creation math into machine-evaluable records for a deterministic evaluator. You are given the game's prose chargen formulas (already located) and tools to read pages. Produce ONE array `derived_values` of §4 records — nothing else.

COVERAGE — the prose list you are handed is a HINT, NOT exhaustive (the upstream reader is lossy). Independently read the character-creation chapter and the blank character sheet, and emit a record for EVERY value that is COMPUTED at character creation (not freely rolled or chosen). Cover ALL of these categories:
 - core resources (hit points; any magic / power / energy pool; sanity / stability / humanity-style tracks) AND their caps/maximums (e.g. a max = a constant minus a skill);
 - derived attributes — characteristic halves/fifths/×N rolls: when a pattern repeats over the characteristics, emit it for ALL of them, not just the common ones;
 - movement rate / speed (even when it is a CONDITIONAL formula over several characteristics, or a fixed baseline constant);
 - point BUDGETS / pools computed from a characteristic or level (skill points, stat/attribute points, role-ability ranks — e.g. EDU×N, INT×N);
 - starting WEALTH / money (cash, assets, currency) computed from a rating / role / social class (often a lookup keyed on that rating — inline the table);
 - any skill whose STARTING value comes from a characteristic;
 - secondary combat/defense STANDING values the sheet lists, computed the same way as the primary derived stats (level + an ability modifier) — e.g. accuracy/hit, evasion/dodge-defense, total defense, a magic-power stat: if the sheet derives the DEFENSES this way, cover the OFFENSIVE/secondary ones the SAME way, consistently — don't capture only some.
If a value is on the sheet but computed from other stats, it needs a record. Cover values that are UNIVERSAL to every character of this game; do NOT enumerate per-class / per-subclass / per-race feature values (spell-slot counts, ki / sorcery / rage uses, class resource pools) — those belong to the class/race option data, a separate layer, not the universal chargen derived values.
EXCLUDE (NOT chargen derived values — leave them out): the dice ROLL that USES a standing value (include "accuracy = level + DEX mod" the STANDING value, but exclude "2d6 + accuracy" the roll); in-play / check-time formulas (a skill check = die+stat+skill, an attack/damage roll); gameplay state-change thresholds and rates (wound / insanity / healing / death triggers — those belong to the resource tracks, a separate layer); per-level / level-up and multiclass advancement formulas (chargen is the starting level only); and values produced by RANDOM rolls during creation (e.g. "roll 1d100 to improve EDU").

Each record: {id, role(attribute|skill|resource|resource_max|background), input_kind(player|derived|hybrid), recompute(live|once), result_type(int|float|dice_or_int), expr, attr_derived, base, allocations, lookup_tables, depends_on, min, max, clamp_max, source_ref, status(source_backed|provisional)}.

RULES (generic — apply to THIS game's real fields, do NOT assume any specific game):
- `expr` is MACHINE form with {{characteristic_id}} placeholders (lowercase) + floor/ceil/round/min/max/lookup/if. NOT prose. e.g. floor(({{con}}+{{siz}})/10).
- For a value read FROM A TABLE: read_layout the page, INLINE the actual rows into lookup_tables {name:{ranges:[{min,max,value}]}} (value = number or dice string like "1d4"), and set expr=lookup(name,{{x}}+{{y}}).
- For a value that depends on a player CHOICE (class / role / lineage / ancestry / background / profession — anything the character SELECTS from a list): inline the CHOICE→value mapping as a STRING-keyed table {name:{ranges:[{min:"<choice_id>",max:"<choice_id>",value:N}]}} (one row per option, lowercase ids, value = number or dice string) and set expr=lookup(name,{{<choice_field>}}) where <choice_field> is the actual field the player picks (e.g. {{class}}). The evaluator matches the string key case-insensitively. NEVER invent a bare per-choice token (e.g. {{class_hit_die}}, {{role_base}}) that no field fills — that degrades to provisional; key the table on the player's CHOICE field instead.
- A characteristic-derived SKILL (role=skill, input_kind=hybrid): emit ONE only when an UNTRAINED character's STARTING value for that skill IS a characteristic expression — i.e. on a blank sheet the skill already reads e.g. "½DEX" (CoC Dodge = floor(DEX/2)). Then: leave `expr` EMPTY; put ONLY the characteristic part in `attr_derived` (e.g. floor({{that_char}}/2)); player-spent points go ONLY in `allocations` [{source,input,budget}]; NEVER put a player token ({{..._allocation}}, {{skill_points}}) in expr/attr_derived.
  DO NOT emit a derived record for a skill that merely ADDS a characteristic at CHECK/RESOLUTION time (systems where a check = die + stat + skill level, and the skill itself starts at 0 or a player-bought level) — that stat is a CHECK-TIME link belonging to the resolution mechanic, NOT a chargen starting value. Such skills are `input_kind=player` (the player buys the level); leave them OUT of derived_values. When unsure whether a stat is the skill's STARTING value vs a check-time addend, leave it OUT.
- Ground each record in a page you READ; if you cannot verify it, set status=provisional. NEVER invent a number or a table row.

TOOLS: get_toc, search(keywords), read(pages) for prose; read_layout(pages) for the aligned TABLE view (use it for any table-based value). Then call submit_chargen with {derived_values:[...]}. Budget ~10 tool calls; be targeted, the formula pages are already located for you."#;

/// True when a derived value carries a machine-evaluable segment.
fn is_machine_dv(d: &DerivedValue) -> bool {
    d.expr.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
        || d.attr_derived.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
}

/// Compile the template's chargen math into §4 derived_values, IN PLACE.
///
/// AUGMENT-not-replace: the reader's prose `derived_values` are the REQUIRED set;
/// the compiler machine-compiles each and may add table/skill values, but never
/// drops one (a field the compiler leaves uncompiled keeps its prose record, so
/// coverage is always >= the reader's set). Runs up to 2 rounds — round 2
/// re-prompts only for required fields still lacking a machine `expr` (mini is
/// run-to-run inconsistent). Returns gap notes for logging.
pub async fn compile_chargen_formulas(
    client: &dyn LlmClient,
    template: &mut CharacterTemplate,
    ctx: CompileCtx<'_>,
    budget: usize,
) -> Vec<String> {
    let prose = serde_json::to_string(&template.derived_values).unwrap_or_default();
    let prose_dvs = template.derived_values.clone();
    let required: Vec<String> = prose_dvs.iter().map(|d| d.field_id.clone()).collect();
    let mut skills: Vec<String> = template
        .fields
        .iter()
        .filter(|f| f.field_type == "skill")
        .map(|f| f.field_id.clone())
        .collect();
    skills.extend(ctx.skill_names.iter().cloned());
    skills.sort();
    skills.dedup();
    let base_seed = format!(
        "Game character math to compile.\nLocated formula pages: {}\nReader's PROSE formulas — emit a machine record for EVERY one of these (compile to `expr`; do NOT drop any):\n{}\n\nSKILL LIST — read the skills chapter / character sheet and, for EACH skill whose STARTING/base value is derived from a characteristic (e.g. half a characteristic), emit a hybrid skill record: role=skill, input_kind=hybrid, attr_derived=floor({{{{that_char}}}}/2). Skip skills with a flat numeric base: {:?}\n\nTables are INLINE in the page text — read the located pages and pull table rows from there.",
        ctx.located_pages, prose, skills
    );
    let submit = tools::submit_tool(
        "submit_chargen",
        "Submit the compiled machine-evaluable chargen records.",
        json!({"derived_values":{"type":"array","items":{"type":"object"}}}),
        &["derived_values"],
    );
    let mut tool_schemas = tools::nav_tools();
    tool_schemas.push(json!({"type":"function","function":{"name":"read_layout","description":"(optional) aligned-table view of page(s) like \"33\". Tables are usually already inline in read(); use this only if a table looks misaligned.","parameters":{"type":"object","properties":{"pages":{"type":"string"}},"required":["pages"]}}}));
    tool_schemas.push(submit);

    // Merge raw records across rounds by id (a later round's record wins).
    let mut merged: std::collections::BTreeMap<String, Value> = std::collections::BTreeMap::new();
    for round in 0..2usize {
        let (cur, _) = finalize_compiled(merged.values().cloned().collect(), template);
        let missing: Vec<String> = required
            .iter()
            .cloned()
            .filter(|fid| !cur.iter().any(|d| d.field_id.eq_ignore_ascii_case(fid) && is_machine_dv(d)))
            .collect();
        if round > 0 && missing.is_empty() {
            break;
        }
        let seed = if round == 0 {
            format!("{base_seed}\n\nRead the located pages, then submit_chargen with one record per value above.")
        } else {
            format!("{base_seed}\n\nROUND 2 — your last pass produced NO machine expr for these REQUIRED fields: {missing:?}. Read their pages and submit_chargen records (expr / lookup_tables / attr_derived) for EACH.")
        };
        let round_budget = if round == 0 { budget } else { budget.min(8) };
        if let Some(records) = run_compile_loop(client, COMPILE_SYS, &seed, &tool_schemas, &ctx, round_budget).await {
            for r in records {
                if let Some(id) = r.get("id").and_then(|v| v.as_str()) {
                    if !id.trim().is_empty() {
                        merged.insert(id.trim().to_ascii_lowercase(), r);
                    }
                }
            }
        }
    }

    let (mut compiled, mut gaps) = finalize_compiled(merged.values().cloned().collect(), template);
    // Augment: keep the reader's prose record for any required field the compiler
    // left out entirely (never regress below what the reader already found).
    for p in prose_dvs {
        if !compiled.iter().any(|d| d.field_id.eq_ignore_ascii_case(&p.field_id)) {
            compiled.push(p);
        }
    }
    if compiled.is_empty() {
        gaps.push("chargen compiler produced nothing; kept prose derived_values".into());
    } else {
        template.derived_values = compiled;
    }
    let added = ensure_categorical_input_fields(template);
    if !added.is_empty() {
        gaps.push(format!("declared categorical player-input fields for derived lookups: {added:?}"));
    }
    gaps
}

/// Declare every CATEGORICAL input a derived formula keys on as a player `choice`
/// field, so the character generator fills it with a clean value the evaluator's
/// categorical lookup can resolve. A record that keys a STRING table (class -> hit
/// die, race -> speed) references an input that is neither a derived id nor a
/// numeric stat; without a declared field the generator never emits a clean value
/// and the derived value degrades to provisional. Generic — driven by the lookup
/// table SHAPE, never by any ruleset's field names. Returns the ids added.
fn ensure_categorical_input_fields(template: &mut CharacterTemplate) -> Vec<String> {
    use std::collections::HashSet;
    let derived_ids: HashSet<String> = template
        .derived_values.iter().map(|d| d.field_id.trim().to_ascii_lowercase()).collect();
    let mut field_ids: HashSet<String> = template
        .fields.iter().map(|f| f.field_id.trim().to_ascii_lowercase()).collect();

    // A categorical input is a ref in a record that keys a STRING table, and that
    // is neither a derived id nor an existing field. Preserve discovery order.
    let mut needed: Vec<String> = Vec::new();
    for d in &template.derived_values {
        let has_cat = d.lookup_tables.as_object()
            .map(|m| m.values().any(table_is_categorical)).unwrap_or(false);
        if !has_cat { continue; }
        for e in [d.expr.as_deref(), d.attr_derived.as_deref()].into_iter().flatten() {
            if e.trim().is_empty() { continue; }
            for refr in trpg_formula::extract_refs(e) {
                let k = refr.trim_start_matches("derived.").trim().to_ascii_lowercase();
                if k.is_empty() || derived_ids.contains(&k) || field_ids.contains(&k) { continue; }
                if !needed.iter().any(|x| x == &k) { needed.push(k); }
            }
        }
    }

    let mut added = Vec::new();
    for k in needed {
        if !field_ids.insert(k.clone()) { continue; }
        let title = k.split('_').filter(|w| !w.is_empty()).map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }).collect::<Vec<_>>().join(" ");
        template.fields.push(CharacterField {
            field_id: k.clone(),
            title,
            field_type: "choice".into(),
            notes: Some("Player choice that derived values are computed from (e.g. hit points). Fill with the canonical lowercase id.".into()),
            ..Default::default()
        });
        added.push(k);
    }
    added
}

async fn run_compile_loop(
    client: &dyn LlmClient,
    system: &str,
    seed: &str,
    tool_schemas: &[Value],
    ctx: &CompileCtx<'_>,
    budget: usize,
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
            let args: Value = tc
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_else(|| json!({}));
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

pub(crate) fn compile_dispatch(ctx: &CompileCtx<'_>, name: &str, args: &Value) -> String {
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
            None => "[no layout view available for this book — mark table-based values provisional]".into(),
        },
        _ => format!("unknown tool {name}"),
    }
}

// ---------------------------------------------------------------------------
// Tests (deterministic parts only; the LLM loop is validated live)
// ---------------------------------------------------------------------------

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

#[cfg(test)]
mod finalize_tests {
    use super::*;
    use serde_json::json;
    use trpg_model::{CharacterField, CharacterTemplate};

    fn tmpl_with_stats(ids: &[&str]) -> CharacterTemplate {
        let mut t = CharacterTemplate::default();
        t.fields = ids
            .iter()
            .map(|id| CharacterField {
                field_id: id.to_string(),
                title: id.to_string(),
                field_type: "stat".into(),
                ..Default::default()
            })
            .collect();
        t
    }

    #[test]
    fn resolvable_record_keeps_declared_status() {
        let recs = vec![json!({
            "id":"hp_max","role":"resource_max","input_kind":"derived","result_type":"int",
            "expr":"floor(({{con}}+{{siz}})/10)","status":"source_backed"
        })];
        let t = tmpl_with_stats(&["con", "siz"]);
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
        let t = tmpl_with_stats(&["con", "siz"]);
        let (dvs, _gaps) = finalize_compiled(recs, &t);
        assert_eq!(dvs.len(), 1, "record kept, not dropped");
        assert_eq!(dvs[0].status.as_deref(), Some("provisional"));
    }

    #[test]
    fn stray_non_string_field_does_not_kill_the_record() {
        // model returned an integer where a string field is expected (the
        // real `credit_rating` failure) — sanitize drops it, record survives.
        let recs = vec![json!({
            "id":"credit_rating","role":"background","input_kind":"player","result_type":0
        })];
        let t = tmpl_with_stats(&["edu"]);
        let (dvs, gaps) = finalize_compiled(recs, &t);
        assert_eq!(dvs.len(), 1, "record survives the bad field");
        assert_eq!(dvs[0].field_id, "credit_rating");
        assert!(gaps.iter().all(|g| !g.contains("credit_rating")), "no coerce gap: {gaps:?}");
    }

    #[test]
    fn hybrid_skill_allocations_token_is_repaired_and_resolves() {
        // model wrote dodge as expr with a fake {{allocations}} token → must be
        // routed to attr_derived so it RESOLVES (not provisional).
        let recs = vec![json!({
            "id":"dodge","role":"skill","input_kind":"hybrid","result_type":"int",
            "expr":"{{allocations}}+floor({{dex}}/2)","status":"source_backed"
        })];
        let t = tmpl_with_stats(&["dex"]);
        let (dvs, _gaps) = finalize_compiled(recs, &t);
        assert_eq!(dvs.len(), 1);
        assert_eq!(dvs[0].attr_derived.as_deref(), Some("floor({{dex}}/2)"));
        assert!(dvs[0].expr.as_deref().unwrap_or("").is_empty(), "expr cleared → hybrid path");
        assert_eq!(dvs[0].status.as_deref(), Some("source_backed"), "now resolves: floor(50/2)=25");
    }

    #[test]
    fn categorical_record_resolves_source_backed_via_sample() {
        // class -> hit die keyed on a STRING table. The round-trip must SAMPLE a
        // categorical key (first table key) so the record RESOLVES instead of
        // degrading to provisional for want of a `class` value.
        let recs = vec![json!({
            "id":"hp_max","role":"resource_max","input_kind":"hybrid","result_type":"int",
            "expr":"lookup(hd,{{class}})+{{con}}","status":"source_backed",
            "lookup_tables":{"hd":{"ranges":[
                {"min":"wizard","max":"wizard","value":6},
                {"min":"fighter","max":"fighter","value":10}]}}
        })];
        let t = tmpl_with_stats(&["con"]);
        let (dvs, _gaps) = finalize_compiled(recs, &t);
        assert_eq!(dvs.len(), 1);
        assert_eq!(dvs[0].status.as_deref(), Some("source_backed"),
            "categorical record resolves with a sampled class key");
    }

    #[test]
    fn ensure_declares_only_categorical_refs_as_choice_fields() {
        let mut t = tmpl_with_stats(&["con"]);
        t.derived_values = vec![serde_json::from_value(json!({
            "field_id":"hp_max","formula":"","depends_on":[],"evaluator":"",
            "expr":"lookup(hd,{{class}})+{{con}}",
            "lookup_tables":{"hd":{"ranges":[{"min":"wizard","max":"wizard","value":6}]}}
        })).unwrap()];
        let added = ensure_categorical_input_fields(&mut t);
        // `con` is a stat field (numeric) -> NOT declared; `class` is the categorical key -> declared.
        assert_eq!(added, vec!["class".to_string()]);
        let f = t.fields.iter().find(|f| f.field_id == "class").expect("class field added");
        assert_eq!(f.field_type, "choice");
        // idempotent: a second pass adds nothing.
        assert!(ensure_categorical_input_fields(&mut t).is_empty());
    }

    #[test]
    fn ensure_skips_when_no_categorical_table() {
        // a numeric-range lookup must NOT spawn any choice field.
        let mut t = tmpl_with_stats(&["str", "siz"]);
        t.derived_values = vec![serde_json::from_value(json!({
            "field_id":"db","formula":"","depends_on":[],"evaluator":"",
            "expr":"lookup(db,{{str}}+{{siz}})",
            "lookup_tables":{"db":{"ranges":[{"min":85,"max":124,"value":"0"}]}}
        })).unwrap()];
        assert!(ensure_categorical_input_fields(&mut t).is_empty());
    }

    #[test]
    fn lookup_values_are_normalized() {
        let recs = vec![json!({
            "id":"damage_bonus","role":"attribute","input_kind":"derived","result_type":"dice_or_int",
            "expr":"lookup(db,{{str}}+{{siz}})","status":"source_backed",
            "lookup_tables":{"db":{"ranges":[
                {"min":85,"max":124,"value":"None"},
                {"min":125,"max":164,"value":"+1d4"}
            ]}}
        })];
        let t = tmpl_with_stats(&["str", "siz"]);
        let (dvs, _gaps) = finalize_compiled(recs, &t);
        let ranges = dvs[0].lookup_tables["db"]["ranges"].as_array().unwrap();
        assert_eq!(ranges[0]["value"], json!("0"), "None -> 0");
        assert_eq!(ranges[1]["value"], json!("1d4"), "+1d4 -> 1d4");
    }
}
