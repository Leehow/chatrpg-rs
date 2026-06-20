//! Pool-scaling parameter compiler — a deterministic chargen post-pass that
//! turns a CATEGORICAL character CHOICE (e.g. Triangle's `competency`) into the
//! NUMERIC rated parameter a kernel's `dice_core.pool_scaling_parameter` names
//! (e.g. `competency_rank`), so the param-driven dice pool (trpg-runtime
//! `scale_pool_if_param_driven`, TRPG_PARAM_DRIVEN_POOL) can size the pool from
//! the actor sheet.
//!
//! WHY this exists (Q-2 data gap, "case b"): a ruleset can carry an agent's
//! competence as a categorical LABEL (a `choice` field) with NO numeric rating
//! anywhere in the parsed data — Triangle's `competency` is exactly this (the
//! template field is `field_type:"choice"`, `derived_values` is empty). The
//! kernel NAMES the scaling param; this pass COMPILES the numeric rating from
//! the ruleset's OWN enumerated option list, deriving the rank from the option's
//! ORDINAL position in that list (the data source: the ruleset's character
//! option catalog, NOT a hand-authored per-label constant table in Rust).
//!
//! Generic & deterministic: driven entirely by DATA SHAPE — (a) the kernel
//! declares the param name, (b) the template has a `choice` field that stems the
//! param, (c) the option catalog enumerates that choice's options. No ruleset /
//! field-name literals; no LLM. When any piece is absent, returns None and the
//! pool stays flat (byte-identical legacy behavior).

use serde_json::{json, Value};
use trpg_model::CharacterTemplate;

/// Suffixes a kernel scaling param may append to the underlying choice field id.
/// `competency_rank` scales the `competency` choice; `role_rating` scales `role`.
/// Generic — these are rating-noun suffixes, not ruleset terms.
const RANK_SUFFIXES: &[&str] = &["_rank", "_rating", "_level", "_score", "_tier", "_value"];

/// The choice field id that a scaling param scales: the param itself if a choice
/// field already carries that exact id, else the param with a trailing
/// rating-noun suffix stripped (`competency_rank` -> `competency`). Returns the
/// matched `choice` field's id (preserving its original casing), or None.
fn choice_field_for_param(param: &str, template: &CharacterTemplate) -> Option<String> {
    let p = param.trim().to_ascii_lowercase();
    if p.is_empty() {
        return None;
    }
    let is_choice = |fid: &str| {
        template.fields.iter().find_map(|f| {
            (f.field_type.eq_ignore_ascii_case("choice")
                && f.field_id.trim().eq_ignore_ascii_case(fid))
            .then(|| f.field_id.trim().to_string())
        })
    };
    if let Some(hit) = is_choice(&p) {
        return Some(hit);
    }
    for suf in RANK_SUFFIXES {
        if let Some(stem) = p.strip_suffix(suf) {
            if let Some(hit) = is_choice(stem) {
                return Some(hit);
            }
        }
    }
    None
}

/// True when any derived value already PRODUCES this param (case-insensitive on
/// `field_id`). If so, the LLM/source-backed compile owns it and this pass MUST
/// NOT clobber it (augment-not-replace).
fn already_compiled(param: &str, template: &CharacterTemplate) -> bool {
    template
        .derived_values
        .iter()
        .any(|d| d.field_id.trim().eq_ignore_ascii_case(param.trim()))
}

/// One enumerated option's stable id, normalized lowercase. Prefers an explicit
/// id (`option_id`/`id`/`locator_id`), falls back to a slugged title/label.
fn option_id(opt: &Value) -> Option<String> {
    for k in ["option_id", "id", "locator_id"] {
        if let Some(s) = opt.get(k).and_then(Value::as_str) {
            let t = s.trim().to_ascii_lowercase();
            if !t.is_empty() {
                return Some(t);
            }
        }
    }
    for k in ["title", "label", "name"] {
        if let Some(s) = opt.get(k).and_then(Value::as_str) {
            let t = slug(s);
            if !t.is_empty() {
                return Some(t);
            }
        }
    }
    None
}

/// Slug a free-text label to a stable lowercase id token (alnum runs joined by
/// `_`), matching the catalog's own locator-id convention.
fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut prev_us = true; // suppress leading underscore
    for ch in s.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_us = false;
        } else if !prev_us {
            out.push('_');
            prev_us = true;
        }
    }
    out.trim_matches('_').to_string()
}

/// Enumerated option ids for a choice field, in catalog ORDER, de-duplicated.
/// Reads the matching option group (by `category` / `group_id` containing the
/// field id) and pulls `options[]` then `locators[]`. Empty when no group.
fn enumerated_options(field_id: &str, option_catalogs: &Value) -> Vec<String> {
    let fid = field_id.trim().to_ascii_lowercase();
    let mut out: Vec<String> = Vec::new();
    let mut push = |id: String| {
        if !id.is_empty() && !out.iter().any(|x| x == &id) {
            out.push(id);
        }
    };
    let Some(cats) = option_catalogs.as_array() else {
        return out;
    };
    let group_matches = |g: &Value| -> bool {
        ["category", "group_id", "title"].iter().any(|k| {
            g.get(*k)
                .and_then(Value::as_str)
                .map(|s| s.to_ascii_lowercase().contains(&fid))
                .unwrap_or(false)
        })
    };
    for cat in cats {
        // A catalog is either a flat option list or holds `option_groups`.
        let groups: Vec<&Value> = cat
            .get("option_groups")
            .and_then(Value::as_array)
            .map(|a| a.iter().collect())
            .unwrap_or_else(|| vec![cat]);
        for g in groups {
            if !group_matches(g) {
                continue;
            }
            for arr_key in ["options", "locators"] {
                if let Some(arr) = g.get(arr_key).and_then(Value::as_array) {
                    for o in arr {
                        if let Some(id) = option_id(o) {
                            push(id);
                        }
                    }
                }
            }
        }
    }
    out
}

/// Build a §4 derived record that compiles a categorical choice into the
/// kernel-named numeric scaling param via an ordinal lookup table sourced from
/// the ruleset's enumerated options. Returns None (pool stays flat) unless ALL
/// hold: param non-empty, not already compiled, a `choice` field stems it, and
/// the catalog enumerates >= 2 distinct options for that choice.
///
/// The record routes `role=attribute` -> sheet `stats.<param>` (read by the
/// runtime's `numeric_param_from_profile`); `expr=lookup(<param>_rank_table,
/// {{<choice_field>}})` with one row per option (id -> 1-based ordinal).
pub fn pool_scaling_choice_record(
    param_name: &str,
    template: &CharacterTemplate,
    option_catalogs: &Value,
) -> Option<Value> {
    let param = param_name.trim();
    if param.is_empty() || already_compiled(param, template) {
        return None;
    }
    let field = choice_field_for_param(param, template)?;
    let options = enumerated_options(&field, option_catalogs);
    if options.len() < 2 {
        return None; // not a real scale -> leave the pool flat (fail-soft)
    }
    let table_name = format!("{param}_table");
    let ranges: Vec<Value> = options
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let rank = (i + 1) as i64;
            json!({"min": id, "max": id, "value": rank})
        })
        .collect();
    let field_lc = field.to_ascii_lowercase();
    Some(json!({
        "id": param,
        "role": "attribute",
        "input_kind": "derived",
        "recompute": "once",
        "result_type": "int",
        "expr": format!("lookup({table_name},{{{{{field_lc}}}}})"),
        "lookup_tables": { table_name: { "ranges": ranges } },
        "depends_on": [field_lc],
        "status": "source_backed",
        "notes": format!(
            "Pool-scaling rank derived from the ruleset's enumerated `{field}` \
             options (ordinal position = rank). Data source: character option \
             catalog. Generic ordinal compile, not a per-label constant table."
        ),
    }))
}

#[cfg(test)]
#[path = "pool_scaling_tests.rs"]
mod pool_scaling_tests;
