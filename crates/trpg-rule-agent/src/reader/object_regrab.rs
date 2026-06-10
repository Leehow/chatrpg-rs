//! Fix (e) — VALDROP "last-cell" re-grab: a focused SECOND LLM pass that recovers
//! a declared, CONFIRMED slot left null in one example while a sibling example
//! captured it. The sweep (docs/object-compile-failure-sweep-2026-06-09.md §2a)
//! found the dominant true defect is a real printed cell on an ALREADY-READ page
//! getting dropped from one row (CoC tomes `spells`, BRP `range`, SW `crit_value`,
//! Triangle `effect`, …). This pass re-scans the cited page(s) for that one row and
//! copies back ONLY values literally printed — never invents.
//!
//! Cheap by design: if NO example has a gap (the common case) it makes ZERO LLM
//! calls. Fail-open everywhere — any error / parse failure / no fills returns the
//! pre-regrab category unchanged. Reuses object_compile's loop + tool schemas.

use super::object_compile::{
    confirmed_slots, is_meaningful, object_tool_schemas, run_object_loop, ObjectCtx,
};
use super::tools;
use serde_json::{json, Value};
use trpg_llm::LlmClient;

const REGRAB_SYS: &str = r#"You recover dropped table cells. A first extraction pass left some entries with EMPTY slots that a SIBLING entry in the SAME table DOES have — so those slots are real columns whose value for THIS entry was missed.

For EACH listed entry: read_layout the cited page(s), find THAT entry's row, and return ONLY the slots whose value is LITERALLY printed for it. Copy the value VERBATIM from the table.

NEVER invent. NEVER guess. NEVER copy a sibling's value. If a slot is genuinely blank / absent for this entry (the cell is empty or "-"), OMIT it — do not fill it. It is correct and expected to return fewer slots than asked, or none at all. This pass exists to recover literally-printed cells, not to fabricate.

TOOLS: get_toc, search(keywords), read(pages), read_layout(pages). Then call submit_regrab with {fills:[{name, slots:{<slot>:<value>}}]} — one object per entry you found values for (skip entries you found nothing for)."#;

/// Recover confirmed-but-null slots for examples that dropped a printed cell.
/// `cat` must already be finalize'd (so `schema_slots[].confirmed` exists).
/// Returns the merged category, or `cat` unchanged when nothing to do / on error.
pub(super) async fn regrab_missing(
    client: &dyn LlmClient,
    ctx: &ObjectCtx<'_>,
    cat: Value,
    budget: usize,
) -> Value {
    let gaps = category_gaps(&cat);
    if gaps.iter().all(|(_, _, g)| g.is_empty()) {
        // Common case: no example dropped a confirmed cell — pay nothing.
        return cat;
    }
    match run_regrab_loop(client, ctx, &cat, &gaps, budget).await {
        Some(fills) if !fills.is_empty() => merge_fills(cat, &fills),
        _ => cat, // fail-open: no fills / LLM error / parse failure
    }
}

/// Per example, the slots that are CONFIRMED for the category (a real column some
/// sibling fills) but null/empty in THIS example — the cells to try to recover.
/// Returns (example_index, example_name, gap_slots). Pure (no LLM) — unit-tested.
fn category_gaps(cat: &Value) -> Vec<(usize, String, Vec<String>)> {
    let confirmed = confirmed_slots(cat);
    let mut out = Vec::new();
    let Some(examples) = cat.get("examples").and_then(Value::as_array) else { return out };
    for (i, ex) in examples.iter().enumerate() {
        let name = ex.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        let slots = ex.get("slots").and_then(Value::as_object);
        let mut gaps: Vec<String> = Vec::new();
        for slot in &confirmed {
            // A gap = confirmed column, but THIS example has it null/empty/absent.
            let filled = slots
                .and_then(|m| m.get(slot))
                .map(is_meaningful)
                .unwrap_or(false);
            if !filled {
                gaps.push(slot.clone());
            }
        }
        gaps.sort();
        out.push((i, name, gaps));
    }
    out
}

/// Run ONE focused loop: seed the category kind + cited pages + per-entry gap list,
/// then collect `submit_regrab.fills`. Returns the raw fills array on success.
async fn run_regrab_loop(
    client: &dyn LlmClient,
    ctx: &ObjectCtx<'_>,
    cat: &Value,
    gaps: &[(usize, String, Vec<String>)],
    budget: usize,
) -> Option<Vec<Value>> {
    let submit = tools::submit_tool(
        "submit_regrab",
        "Submit recovered cells: fills:[{name, slots:{<slot>:<value>}}], only literally-printed values.",
        json!({"fills": {"type": "array", "items": {"type": "object"}}}),
        &["fills"],
    );
    let schemas = object_tool_schemas(submit);

    let kind = cat.get("kind").and_then(Value::as_str).unwrap_or("");
    let cat_pages = cat.get("source_pages").and_then(Value::as_str).unwrap_or("");
    let examples = cat.get("examples").and_then(Value::as_array);
    // One bullet per entry-with-gaps: name, the page(s) to look on, the gap slots.
    let mut entries = String::new();
    for (i, name, gap_slots) in gaps {
        if gap_slots.is_empty() {
            continue;
        }
        let ex_pages = examples
            .and_then(|a| a.get(*i))
            .and_then(|e| e.get("source_pages"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(cat_pages);
        entries.push_str(&format!(
            "- entry \"{name}\" (pages {ex_pages}): recover if printed -> {gap_slots:?}\n"
        ));
    }
    let seed = format!(
        "Category kind: {kind}. Master table around pages: {cat_pages}.\nSome entries dropped a cell that a sibling entry HAS. For each entry below, read_layout the cited page(s), find that entry's row, and return ONLY slots whose value is literally printed for it (omit any truly blank). Copy verbatim — never invent.\n\nEntries to re-check:\n{entries}\nThen call submit_regrab with fills:[{{name, slots:{{<slot>:<value>}}}}]."
    );

    run_object_loop(client, REGRAB_SYS, &seed, &schemas, ctx, budget, "submit_regrab")
        .await
        .and_then(|v| v.get("fills").and_then(Value::as_array).cloned())
}

/// Merge recovered fills into the category's examples. For each fill matching an
/// example by `name`, set only the slots that are (a) currently null/empty AND
/// (b) present in the fill — NEVER overwriting an already-meaningful value, and
/// ignoring unknown / non-gap slots. Any newly-filled slot is dropped from
/// `provisional_slots`. Pure (no LLM) — unit-tested.
fn merge_fills(mut cat: Value, fills: &[Value]) -> Value {
    // name -> {slot: value} from the fills (last write wins on dup names).
    let mut by_name: std::collections::HashMap<String, &Value> = std::collections::HashMap::new();
    for f in fills {
        if let Some(name) = f.get("name").and_then(Value::as_str) {
            by_name.insert(name.to_string(), f);
        }
    }
    // Declared columns: a fill slot outside this set is unknown -> ignored. When a
    // category has no schema_slots (test fixtures), allow any slot.
    let declared: Option<std::collections::HashSet<String>> = cat
        .get("schema_slots")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|s| s.get("slot").and_then(Value::as_str).map(String::from)).collect());
    let is_declared = |slot: &str| declared.as_ref().map(|d| d.contains(slot)).unwrap_or(true);
    let mut newly_filled: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let Some(examples) = cat.get_mut("examples").and_then(Value::as_array_mut) {
        for ex in examples.iter_mut() {
            let name = ex.get("name").and_then(Value::as_str).unwrap_or("").to_string();
            let Some(fill) = by_name.get(name.as_str()) else { continue };
            let Some(fill_slots) = fill.get("slots").and_then(Value::as_object) else { continue };
            let Some(ex_slots) = ex.get_mut("slots").and_then(Value::as_object_mut) else { continue };
            for (slot, new_val) in fill_slots {
                // (a) a declared column, (b) currently empty here, (c) fill has a value.
                let cur_empty = ex_slots.get(slot).map(|v| !is_meaningful(v)).unwrap_or(true);
                if is_declared(slot) && cur_empty && is_meaningful(new_val) {
                    ex_slots.insert(slot.clone(), new_val.clone());
                    newly_filled.insert(slot.clone());
                }
            }
        }
    }
    // Drop any newly-filled slot from provisional_slots (it now has a value).
    if !newly_filled.is_empty() {
        if let Some(prov) = cat.get_mut("provisional_slots").and_then(Value::as_array_mut) {
            prov.retain(|v| v.as_str().map(|s| !newly_filled.contains(s)).unwrap_or(true));
        }
    }
    cat
}

#[cfg(test)]
mod tests {
    use super::*;

    // (e) gap targeting: `range` is filled by one sibling (confirmed) but null in
    // the other example -> that example has it as a gap. `soak` is filled by both
    // -> never a gap. `bogus` is never filled (unconfirmed) -> never a gap even
    // though it is null everywhere. Deterministic — no LLM.
    #[test]
    fn category_gaps_targets_confirmed_nulls() {
        let cat = json!({
            "kind": "weapon",
            "schema_slots": [
                {"slot": "soak", "confirmed": true},
                {"slot": "range", "confirmed": true},
                {"slot": "bogus", "confirmed": false}
            ],
            "examples": [
                {"name": "A", "slots": {"soak": "2", "range": "Touch", "bogus": null}},
                {"name": "B", "slots": {"soak": "3", "range": "", "bogus": null}}
            ]
        });
        let gaps = category_gaps(&cat);
        assert_eq!(gaps.len(), 2);
        // Entry A is complete on confirmed columns -> no gaps.
        assert_eq!(gaps[0].1, "A");
        assert!(gaps[0].2.is_empty(), "A has no gaps: {:?}", gaps[0].2);
        // Entry B dropped `range` (confirmed) -> exactly that, never `bogus`.
        assert_eq!(gaps[1].1, "B");
        assert_eq!(gaps[1].2, vec!["range".to_string()], "B's only gap is range");
    }

    // No example has a gap -> empty per-entry gap lists (regrab_missing then skips
    // the LLM entirely and returns cat unchanged).
    #[test]
    fn category_gaps_empty_when_all_filled() {
        let cat = json!({
            "schema_slots": [{"slot": "dmg", "confirmed": true}],
            "examples": [
                {"name": "A", "slots": {"dmg": "1d6"}},
                {"name": "B", "slots": {"dmg": "1d8"}}
            ]
        });
        let gaps = category_gaps(&cat);
        assert!(gaps.iter().all(|(_, _, g)| g.is_empty()), "no gaps anywhere: {gaps:?}");
    }

    // (e) merge: a recovered value lands in the null slot; an already-filled slot
    // is left untouched; an unknown (undeclared) slot in the fill is ignored; and
    // the newly-filled slot is dropped from provisional_slots. Deterministic — no LLM.
    #[test]
    fn merge_fills_only_fills_gaps() {
        let cat = json!({
            "schema_slots": [{"slot": "range"}, {"slot": "duration"}],
            "provisional_slots": ["range", "other"],
            "examples": [
                {"name": "Wounding", "slots": {"range": "", "duration": "instant"}}
            ]
        });
        let fills = vec![json!({
            "name": "Wounding",
            "slots": {
                "range": "Touch",        // gap -> should land
                "duration": "fabricated",// already meaningful -> must NOT overwrite
                "unknown": "junk"        // not a declared slot -> ignored
            }
        })];
        let out = merge_fills(cat, &fills);
        let slots = &out["examples"][0]["slots"];
        assert_eq!(slots["range"].as_str(), Some("Touch"), "gap filled verbatim");
        assert_eq!(slots["duration"].as_str(), Some("instant"), "filled slot untouched");
        assert!(slots.get("unknown").is_none(), "undeclared slot ignored: {slots}");
        // provisional_slots drops the newly-filled `range`, keeps `other`.
        let prov: Vec<String> = out["provisional_slots"].as_array().unwrap()
            .iter().filter_map(|v| v.as_str().map(String::from)).collect();
        assert_eq!(prov, vec!["other".to_string()], "range dropped, other kept: {prov:?}");
    }

    // The merge must never clobber a meaningful value even when the fill insists.
    #[test]
    fn merge_fills_never_overwrites_meaningful() {
        let cat = json!({"examples": [{"name": "X", "slots": {"dmg": "1d6"}}]});
        let fills = vec![json!({"name": "X", "slots": {"dmg": "9d9"}})];
        let out = merge_fills(cat, &fills);
        assert_eq!(out["examples"][0]["slots"]["dmg"].as_str(), Some("1d6"));
    }

    // The LLM re-grab behavior itself (read_layout the page, copy the printed cell,
    // never invent) is validated LIVE later against real rulesets — it is NOT and
    // CANNOT be faked here, so these tests only cover the deterministic helpers.
}
