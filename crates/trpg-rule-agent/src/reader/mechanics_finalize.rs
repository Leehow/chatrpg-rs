//! Deterministic guardrails for the mechanics-catalog compile pass (A3):
//! raw `submit_mechanics` records -> qualified `MechanicEntry` set + validation
//! messages, applied before anything enters the kernel.
//!
//! Two disciplines (spec §4 / plan A3):
//! - An entry is DROPPED for exactly two reasons: empty `source_refs`
//!   (invention suspicion) and uncoercible garbage that not even an id can
//!   rescue, plus the unknown-`tested_parameter` safety drop. Everything else
//!   is downgraded and KEPT — "drop because unrecognized" is forbidden here.
//! - Every drop/downgrade emits exactly one `ValidationMessage`; never silent.
//!
//! Pure functions, zero IO/LLM — replayable offline on any dumped raw array
//! (the `mechanics_proto` bin and the A7 audit call this directly).

use serde_json::Value;
use std::collections::HashSet;
use trpg_model::{EngineHook, MechanicEntry, RuleKernel, SourceRef, ValidationMessage};

/// Guardrail pipeline (execution order is load-bearing — see plan A3 §4):
/// dedup by normalized id (last wins) -> coercion/salvage -> source_refs ->
/// tested_parameter -> hooks vocabulary -> followup closure (which must run
/// AFTER all drops, or a link aimed at a doomed entry would falsely pass).
/// `extra_parameter_keys` = option-catalog skill names that may not appear in
/// the sheet-schema fields; pure-kernel callers pass `&[]`.
pub fn finalize_catalog(
    raw: Vec<Value>,
    kernel: &RuleKernel,
    extra_parameter_keys: &[String],
) -> (Vec<MechanicEntry>, Vec<ValidationMessage>) {
    let mut msgs: Vec<ValidationMessage> = Vec::new();
    let keys = sheet_parameter_keys(kernel, extra_parameter_keys);

    // Dedup by normalized id, last submission wins (A2 merge-by-id semantics).
    let mut order: Vec<String> = Vec::new();
    let mut by_id: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
    for r in raw {
        match norm_id(&r) {
            Some(id) => {
                if by_id.insert(id.clone(), r).is_none() {
                    order.push(id);
                }
            }
            None => msgs.push(warn(
                "mechanic_uncoercible",
                None,
                format!("record without a usable id dropped: {}", snippet(&r)),
            )),
        }
    }

    let mut entries: Vec<MechanicEntry> = Vec::new();
    for id in &order {
        let mut r = by_id.remove(id).expect("deduped record present");
        // Hooks are physically lifted out BEFORE coercion: an unknown event is
        // a hard serde Err on the whole entry, but guardrail #5 strips just
        // the hook and keeps the entry (validated further below).
        let raw_hooks = r.as_object_mut().and_then(|o| o.remove("hooks"));

        // Guardrail #4 — coercion with salvage: malformed substructures are
        // stripped and the entry retried as a semantic five-key rebuild;
        // only an id-less record is truly uncoercible (handled above). The
        // salvaged entry still walks the remaining guardrails (source_refs!).
        let (mut entry, salvaged) = match serde_json::from_value::<MechanicEntry>(r.clone()) {
            Ok(e) => (e, false),
            Err(err) => {
                let e = salvage_semantic(&r);
                msgs.push(warn(
                    "mechanic_uncoercible",
                    Some(&e.id),
                    format!("salvaged as semantic entry (invalid substructures stripped): {err}"),
                ));
                (e, true)
            }
        };

        // Guardrail #3 — empty source_refs = invention suspicion: drop.
        if entry.source_refs.is_empty() {
            msgs.push(warn(
                "mechanic_dropped_no_source",
                Some(&entry.id),
                "entry has no source_refs (nothing grounds it in a read page)".into(),
            ));
            continue;
        }

        // Guardrail #1 — tested_parameter must exist on the sheet (None is
        // legal: hook/semantic/passive entries test no parameter).
        if let Some(p) = entry.tested_parameter.as_deref() {
            if !parameter_known(p, &keys) {
                msgs.push(warn(
                    "mechanic_dropped_unknown_parameter",
                    Some(&entry.id),
                    format!("tested_parameter `{p}` matches no sheet field / derived value / track / skill"),
                ));
                continue;
            }
        }

        // Guardrail #5 — hooks vocabulary: the EngineHook enum IS the engine
        // event vocabulary; anything else is stripped (entry kept, the tier
        // degrades naturally by data shape). Salvaged entries stay semantic:
        // their stripped substructures are covered by the salvage message.
        if !salvaged {
            if let Some(raw_hooks) = raw_hooks {
                entry.hooks = validate_hooks(&raw_hooks, &entry.id, &mut msgs);
            }
        }

        entries.push(entry);
    }

    // Guardrail #2 — followup closure over the SURVIVING id set (intra-batch
    // mutual references are legal; links to dropped/unknown ids are removed,
    // the entry stays).
    let surviving: HashSet<String> = entries
        .iter()
        .map(|e| e.id.trim().to_ascii_lowercase())
        .collect();
    for e in &mut entries {
        let id = e.id.clone();
        e.followup_links.retain(|l| {
            let ok = surviving.contains(&l.procedure_id.trim().to_ascii_lowercase());
            if !ok {
                msgs.push(warn(
                    "followup_link_broken",
                    Some(&id),
                    format!(
                        "followup link points at `{}` which is not in the final catalog",
                        l.procedure_id
                    ),
                ));
            }
            ok
        });
    }

    (entries, msgs)
}

/// Legal parameter-key set, all lowercase: character_sheet_schema
/// `fields[*].field_id` ∪ `derived_values[*].field_id` ∪
/// `resource_tracks[*].id` ∪ `extra`. The schema is read by Value paths (it
/// may be a fallback shape like `{"title":…}` — then only tracks/extra
/// contribute, and entries with a tested_parameter all drop, loudly).
pub fn sheet_parameter_keys(kernel: &RuleKernel, extra: &[String]) -> HashSet<String> {
    let mut keys: HashSet<String> = HashSet::new();
    let schema = &kernel.character_sheet_schema;
    for bucket in ["fields", "derived_values"] {
        if let Some(arr) = schema.get(bucket).and_then(Value::as_array) {
            keys.extend(
                arr.iter()
                    .filter_map(|f| f.get("field_id").and_then(Value::as_str))
                    .filter_map(norm_key),
            );
        }
    }
    keys.extend(
        kernel
            .resource_tracks
            .iter()
            .filter_map(|t| t.get("id").and_then(Value::as_str))
            .filter_map(norm_key),
    );
    keys.extend(extra.iter().filter_map(|s| norm_key(s)));
    keys
}

/// Case-insensitive match; a "skills." prefix on the candidate is stripped
/// for a second try (skill keys live unprefixed in the legal set).
fn parameter_known(p: &str, keys: &HashSet<String>) -> bool {
    let q = p.trim().to_ascii_lowercase();
    keys.contains(&q)
        || q.strip_prefix("skills.")
            .map(|s| keys.contains(s))
            .unwrap_or(false)
}

/// Semantic five-key rebuild (id/name/description/when_to_use/source_refs);
/// source_refs elements are kept individually when coercible — an entry whose
/// refs are all garbage then falls to the no-source drop, still loudly.
fn salvage_semantic(r: &Value) -> MechanicEntry {
    let s = |k: &str| {
        r.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let source_refs: Vec<SourceRef> = r
        .get("source_refs")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| serde_json::from_value(v.clone()).ok())
                .collect()
        })
        .unwrap_or_default();
    MechanicEntry {
        id: s("id"),
        name: s("name"),
        description: s("description"),
        when_to_use: s("when_to_use"),
        source_refs,
        ..Default::default()
    }
}

/// Per-element EngineHook validation: Err -> hook stripped + one message
/// (the entry itself is kept; its tier downgrades by data shape).
fn validate_hooks(
    raw_hooks: &Value,
    entry_id: &str,
    msgs: &mut Vec<ValidationMessage>,
) -> Vec<EngineHook> {
    let Some(arr) = raw_hooks.as_array() else {
        msgs.push(warn(
            "hook_downgraded_no_engine_event",
            Some(entry_id),
            format!(
                "hooks is not an array, all stripped: {}",
                snippet(raw_hooks)
            ),
        ));
        return Vec::new();
    };
    let mut hooks = Vec::new();
    for h in arr {
        match serde_json::from_value::<EngineHook>(h.clone()) {
            Ok(hook) => hooks.push(hook),
            Err(_) => {
                let event = h.get("event").and_then(Value::as_str).unwrap_or("?");
                msgs.push(warn(
                    "hook_downgraded_no_engine_event",
                    Some(entry_id),
                    format!("hook event `{event}` has no engine event point; hook stripped"),
                ));
            }
        }
    }
    hooks
}

/// A4 companion-upgrade write-backs (thresholds followup / success-band
/// semantics / field notes / round-trip guard) live in a physically split
/// submodule to keep this file <=400 lines; the public path stays
/// `mechanics_finalize::apply_*` per the shared type contract.
#[path = "mechanics_upgrades.rs"]
mod upgrades;
pub use upgrades::{
    apply_field_notes, apply_success_bands_upgrades, apply_thresholds_upgrades,
    kernel_round_trip_guard,
};

/// On-outcome `=field` reference guard — same physical-split pattern as the
/// A4 upgrades; the public path stays `mechanics_finalize::apply_*`.
#[path = "mechanics_outcome_refs.rs"]
mod outcome_refs;
pub use outcome_refs::apply_on_outcome_ref_guard;

/// Normalized dedup key: trim + ascii-lowercase; empty -> None.
fn norm_id(r: &Value) -> Option<String> {
    r.get("id")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
}

fn norm_key(s: &str) -> Option<String> {
    let k = s.trim().to_ascii_lowercase();
    (!k.is_empty()).then_some(k)
}

fn warn(code: &str, target: Option<&str>, message: String) -> ValidationMessage {
    ValidationMessage {
        code: code.into(),
        message,
        target: target.map(str::to_string),
    }
}

fn snippet(v: &Value) -> String {
    v.to_string().chars().take(120).collect()
}

#[cfg(test)]
#[path = "mechanics_finalize_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "mechanics_upgrade_tests.rs"]
mod upgrade_tests;
