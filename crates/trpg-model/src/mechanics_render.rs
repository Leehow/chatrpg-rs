//! B1 BP1 catalog-projection renderers — pure functions, no IO. Split out of
//! `mechanics.rs` (already at the 400-line cap) per the plan's Files note; the
//! crate root re-exports this module (`pub use mechanics_render::*`) so call
//! sites address `trpg_model::{kernel_bp1_view, catalog_index_text,
//! passive_projection_line}` exactly as the shared-type contract states.

use crate::{MechanicEntry, MechanicKind, RuleKernel};

/// BP1 projection view: clone the kernel and clear `mechanics_catalog`; every
/// other field is preserved byte-for-byte. The whole kernel JSON is poured
/// into BP1 (runtime `rule_steward_prefix_blocks_for_turn` →
/// `BlockContent::Json(serde_json::to_value(&kernel)?)`); leaving a CoC-sized
/// catalog (50–137 full entries) inline would blow `validate_compiled_budget`
/// (fail-closed Err), so the catalog reaches BP1 only as the compact index.
pub fn kernel_bp1_view(kernel: &RuleKernel) -> RuleKernel {
    let mut view = kernel.clone();
    view.mechanics_catalog = Vec::new();
    view
}

/// Compact index text: one line per entry, `id | name | when_to_use`, in
/// catalog order (deterministic: identical input ⇒ identical bytes). When
/// `entries.len() > limit` the index tiers down DATA-DRIVEN — by entry count
/// and data features, never by ruleset name: only entries with
/// kind==SubsystemProcedure, or non-empty hooks, or a passive_projection are
/// kept, and a tail line `(+N entries omitted; use lookup_mechanic by id)` is
/// appended so the agent knows how to reach the long tail.
pub fn catalog_index_text(entries: &[MechanicEntry], limit: usize) -> String {
    let keep_all = entries.len() <= limit;
    let mut lines: Vec<String> = Vec::new();
    let mut omitted = 0usize;
    for e in entries {
        let keep = keep_all
            || e.kind == MechanicKind::SubsystemProcedure
            || !e.hooks.is_empty()
            || e.passive_projection.is_some();
        if keep {
            lines.push(format!("{} | {} | {}", one_line(&e.id), one_line(&e.name), one_line(&e.when_to_use)));
        } else {
            omitted += 1;
        }
    }
    if omitted > 0 {
        lines.push(format!("(+{omitted} entries omitted; use lookup_mechanic by id)"));
    }
    lines.join("\n")
}

/// Whitespace-normalized single line (an entry must never break the
/// one-line-per-entry invariant of the index).
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// B3 band-semantics line: locate the resolved check's success band in
/// `kernel.dice_core.success_bands` and render `{band_id}——{label}：{semantics}`.
/// The outcome's band id is read from `"success_tier"` first, then `"band"` —
/// both shapes exist in real records (mechanics `on_tier` accounts against
/// `outcome.success_tier`; agent-loop fixtures carry `"band"`). A band entry
/// without a non-empty `semantics` string renders nothing (fail-closed: the
/// caller drops the key and the bare band id/label stays visible in the
/// outcome JSON — never invented prose).
pub fn band_semantics_line(dice_core: &serde_json::Value, outcome: &serde_json::Value) -> Option<String> {
    let band_id = outcome
        .get("success_tier")
        .and_then(|v| v.as_str())
        .or_else(|| outcome.get("band").and_then(|v| v.as_str()))
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let band = dice_core
        .get("success_bands")?
        .as_array()?
        .iter()
        .find(|b| {
            b.get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().eq_ignore_ascii_case(band_id))
                .unwrap_or(false)
        })?;
    let semantics = band
        .get("semantics")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let label = band
        .get("label")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    Some(match label {
        Some(l) => format!("{band_id}——{l}：{semantics}"),
        None => format!("{band_id}：{semantics}"),
    })
}

/// Passive-modifier (4th expressiveness tier) always-on projection line:
/// renders `entry.passive_projection` as a template, substituting `{value}`
/// with the viewer's current value for `entry.tested_parameter`. The value is
/// looked up case-insensitively across the sheet buckets
/// stats/skills/resources/tracks plus the top-level categorical "field"
/// bucket (npc_synth / chargen `apply_track_change_to_sheet` conventions).
/// No template / no tested_parameter / value not found → None (fail-closed:
/// the line is skipped, never invented).
pub fn passive_projection_line(entry: &MechanicEntry, sheet_json: &serde_json::Value) -> Option<String> {
    let template = entry.passive_projection.as_deref()?;
    let param = entry.tested_parameter.as_deref()?;
    let value = sheet_value_text(sheet_json, param)?;
    Some(template.replace("{value}", &value))
}

/// Case-insensitive sheet lookup, first match wins: the stats/skills/
/// resources/tracks buckets, then top-level categorical fields ("field"
/// bucket = scalar top-level keys, per chargen). Objects contribute their
/// inner `value`/`current` scalar (tracks `{value,..}` / resources
/// `{current,..}` shapes); anything else non-scalar → None (fail-closed).
fn sheet_value_text(sheet: &serde_json::Value, param: &str) -> Option<String> {
    for bucket in ["stats", "skills", "resources", "tracks"] {
        if let Some(found) = sheet
            .get(bucket)
            .and_then(|v| v.as_object())
            .and_then(|obj| lookup_ci(obj, param))
            .and_then(scalar_text)
        {
            return Some(found);
        }
    }
    // "field" bucket: top-level categorical inputs (race/class choices) are
    // scalars directly on the sheet root — never descend into bucket objects.
    sheet
        .as_object()
        .and_then(|obj| lookup_ci(obj, param))
        .filter(|v| !v.is_object() && !v.is_array())
        .and_then(scalar_text)
}

fn lookup_ci<'a>(obj: &'a serde_json::Map<String, serde_json::Value>, key: &str) -> Option<&'a serde_json::Value> {
    if let Some(v) = obj.get(key) {
        return Some(v);
    }
    obj.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v)
}

/// Scalar rendering: numbers/strings/bools verbatim; objects via their
/// `value` then `current` member (one structured hop); null/array/empty → None.
fn scalar_text(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        serde_json::Value::Object(m) => m
            .get("value")
            .or_else(|| m.get("current"))
            .filter(|inner| !inner.is_object() && !inner.is_array())
            .and_then(scalar_text),
        _ => None,
    }
}

/// C3: per-line cap on the intent description — layout detail (keeps one
/// runaway intent from blowing the BP2 budget), NOT a behavior switch, so it
/// is a constant rather than an env var.
const INTENT_DESCRIPTION_MAX_CHARS: usize = 200;

/// Current-scene intents index renderer (BP2): one line per intent,
/// `intent_id | description | tested_parameter | difficulty-summary`.
/// Difficulty summary = compact `serde_json::to_string` (None → "-").
/// effect_policy is NEVER rendered — resolution-time data only (C4 pulls it
/// from the graph by intent_id). Empty slice → None (caller emits no block).
pub fn scene_intents_text(intents: &[crate::SceneMechanicIntent]) -> Option<String> {
    if intents.is_empty() {
        return None;
    }
    let lines: Vec<String> = intents
        .iter()
        .map(|i| {
            let desc: String = one_line(&i.description)
                .chars()
                .take(INTENT_DESCRIPTION_MAX_CHARS)
                .collect();
            let difficulty = i
                .difficulty
                .as_ref()
                .and_then(|d| serde_json::to_string(d).ok())
                .unwrap_or_else(|| "-".to_string());
            format!(
                "{} | {} | {} | {}",
                one_line(&i.intent_id),
                desc,
                one_line(&i.tested_parameter),
                difficulty
            )
        })
        .collect();
    Some(lines.join("\n"))
}

#[cfg(test)]
#[path = "mechanics_render_tests.rs"]
mod tests;
