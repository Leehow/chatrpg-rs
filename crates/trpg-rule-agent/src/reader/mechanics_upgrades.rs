//! A4 配套升级写回（mechanics_finalize 的物理拆分子模块，守 ≤400 行）：
//! thresholds followup 链接、success_bands 语义化补全、fields.notes 补写 +
//! kernel round-trip 护栏。经 `#[path] mod upgrades; pub use ...` 引入，
//! 公开路径仍是 `mechanics_finalize::apply_*`（共享类型契约不变）。
use super::{norm_key, snippet, warn};
use serde_json::Value;
use trpg_model::{MechanicEntry, RuleKernel, ValidationMessage};

// ---------------------------------------------------------------------------
// A4 — companion kernel upgrades, same pass: thresholds followup links,
// success-band semantic completion, sheet-field usage notes. Discipline: the
// existing kernel is pass-one's source of truth — every merge is ADDITIVE
// (only missing keys are inserted; nothing existing is ever overwritten, no
// threshold/field is ever created). All pure functions, zero IO/LLM.
// ---------------------------------------------------------------------------

/// upgrades[*]: `{track_id, at?|loss_in_one_go?, direction?, followup_procedure_id}`.
/// Matches an EXISTING threshold of the named track (id case-insensitive) by
/// shape — `at` equal AND `direction` equal (both-absent counts equal), OR
/// `loss_in_one_go` equal — then only inserts a missing `followup_procedure_id`
/// (the canonical catalog id). Broken ref -> `threshold_followup_broken`;
/// no matching track/threshold -> `threshold_upgrade_unmatched`.
pub fn apply_thresholds_upgrades(
    kernel: &mut RuleKernel,
    upgrades: &[Value],
    catalog: &[MechanicEntry],
) -> Vec<ValidationMessage> {
    let ids: std::collections::HashMap<String, &str> = catalog
        .iter()
        .map(|e| (e.id.trim().to_ascii_lowercase(), e.id.trim()))
        .collect();
    let mut msgs = Vec::new();
    for u in upgrades {
        let follow = u
            .get("followup_procedure_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let Some(canonical) = follow.and_then(|f| ids.get(&f.to_ascii_lowercase()).copied()) else {
            msgs.push(warn(
                "threshold_followup_broken",
                follow,
                format!("followup_procedure_id is missing or not in the final catalog; upgrade skipped: {}", snippet(u)),
            ));
            continue;
        };
        match find_threshold_mut(&mut kernel.resource_tracks, u) {
            Some(th) => {
                if th.get("followup_procedure_id").map(|v| !v.is_null()).unwrap_or(false) {
                    msgs.push(warn(
                        "threshold_followup_exists",
                        Some(canonical),
                        format!("threshold already carries a followup_procedure_id; kept as-is (never overwritten): {}", snippet(u)),
                    ));
                } else if let Some(o) = th.as_object_mut() {
                    o.insert("followup_procedure_id".into(), Value::String(canonical.to_string()));
                }
            }
            None => msgs.push(warn(
                "threshold_upgrade_unmatched",
                Some(canonical),
                format!("no existing threshold matches; thresholds are pass-one facts, never created here: {}", snippet(u)),
            )),
        }
    }
    msgs
}

/// First threshold of the upgrade's track whose shape matches (see
/// `threshold_shape_matches`); None when track/thresholds/shape miss.
fn find_threshold_mut<'a>(tracks: &'a mut [Value], u: &Value) -> Option<&'a mut Value> {
    let want = norm_key(u.get("track_id").and_then(Value::as_str)?)?;
    let track = tracks.iter_mut().find(|t| {
        t.get("id")
            .and_then(Value::as_str)
            .and_then(norm_key)
            .as_deref()
            == Some(&want)
    })?;
    track
        .get_mut("thresholds")?
        .as_array_mut()?
        .iter_mut()
        .find(|th| threshold_shape_matches(th, u))
}

/// Shape match = `at` numerically equal AND `direction` equal (both absent
/// counts as equal), OR `loss_in_one_go` numerically equal.
fn threshold_shape_matches(th: &Value, u: &Value) -> bool {
    let num = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64);
    let dir = |v: &Value| {
        v.get("direction")
            .and_then(Value::as_str)
            .and_then(norm_key)
    };
    let at =
        matches!((num(u, "at"), num(th, "at")), (Some(a), Some(b)) if a == b) && dir(u) == dir(th);
    let ligo = matches!((num(u, "loss_in_one_go"), num(th, "loss_in_one_go")), (Some(a), Some(b)) if a == b);
    at || ligo
}

/// upgrades[*]: `{id, semantics?, mechanical_effects?, …}`, merged by band id
/// (case-insensitive) into `dice_core.success_bands`: an existing band only
/// gains its MISSING keys; an unknown id is appended verbatim as a whole new
/// band + `success_band_completed` (the historical hand-fix for missing
/// critical/failure bands, promoted to a compile guardrail). Note on the
/// restrained completeness heuristic (plan A4 §4): the live CoC band shape is
/// `{id, rank, test{kind,…}, label, unless?}` — no structured failure-side
/// marker exists, so no deterministic `success_bands_incomplete` warning is
/// emitted (shape not judgeable -> stay silent; completeness is owned by the
/// prompt instructions + the A7 batch audit).
pub fn apply_success_bands_upgrades(
    kernel: &mut RuleKernel,
    upgrades: &[Value],
) -> Vec<ValidationMessage> {
    let mut msgs = Vec::new();
    let usable: Vec<(String, &Value)> = upgrades
        .iter()
        .filter_map(
            |u| match u.get("id").and_then(Value::as_str).and_then(norm_key) {
                Some(id) => Some((id, u)),
                None => {
                    msgs.push(warn(
                        "success_band_upgrade_unusable",
                        None,
                        format!("band upgrade without a usable id skipped: {}", snippet(u)),
                    ));
                    None
                }
            },
        )
        .collect();
    if usable.is_empty() {
        return msgs; // empty/garbage upgrades: a malformed dice_core stays byte-for-byte untouched
    }
    if !kernel.dice_core.is_object() {
        kernel.dice_core = Value::Object(Default::default());
    }
    let obj = kernel
        .dice_core
        .as_object_mut()
        .expect("object ensured above");
    if !obj
        .get("success_bands")
        .map(Value::is_array)
        .unwrap_or(false)
    {
        obj.insert("success_bands".into(), Value::Array(Vec::new()));
    }
    let bands = obj
        .get_mut("success_bands")
        .and_then(Value::as_array_mut)
        .expect("array ensured above");
    for (id, u) in usable {
        let existing = bands.iter_mut().find(|b| {
            b.get("id")
                .and_then(Value::as_str)
                .and_then(norm_key)
                .as_deref()
                == Some(&id)
        });
        match existing {
            Some(band) => {
                if let (Some(bo), Some(uo)) = (band.as_object_mut(), u.as_object()) {
                    for (k, v) in uo {
                        if !bo.contains_key(k) {
                            bo.insert(k.clone(), v.clone());
                        }
                    }
                }
            }
            None => {
                bands.push(u.clone());
                msgs.push(warn(
                    "success_band_completed",
                    Some(&id),
                    "kernel lacked this success band; whole object appended from the compile pass"
                        .into(),
                ));
            }
        }
    }
    msgs
}

/// notes[*]: `{field_id, notes}`. Matches `character_sheet_schema.fields[*]`
/// by field_id (case-insensitive) and writes `notes` ONLY when it is missing /
/// null / empty-string. Never creates a field; unmatched (including fallback
/// schema shapes without a fields array) -> `field_note_unmatched`.
pub fn apply_field_notes(kernel: &mut RuleKernel, notes: &[Value]) -> Vec<ValidationMessage> {
    let mut msgs = Vec::new();
    for n in notes {
        let fid = n
            .get("field_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let text = n
            .get("notes")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let (Some(fid), Some(text)) = (fid, text) else {
            msgs.push(warn(
                "field_note_unmatched",
                fid,
                format!("note without usable field_id/notes skipped: {}", snippet(n)),
            ));
            continue;
        };
        let want = fid.to_ascii_lowercase();
        let field = kernel
            .character_sheet_schema
            .get_mut("fields")
            .and_then(Value::as_array_mut)
            .and_then(|a| {
                a.iter_mut().find(|f| {
                    f.get("field_id")
                        .and_then(Value::as_str)
                        .and_then(norm_key)
                        .as_deref()
                        == Some(&want)
                })
            });
        match field {
            Some(f) => {
                let fillable = match f.get("notes") {
                    None => true,
                    Some(v) => {
                        v.is_null() || v.as_str().map(|s| s.trim().is_empty()).unwrap_or(false)
                    }
                };
                if fillable {
                    if let Some(o) = f.as_object_mut() {
                        o.insert("notes".into(), Value::String(text.to_string()));
                    }
                }
            }
            None => msgs.push(warn(
                "field_note_unmatched",
                Some(fid),
                format!("field_id `{fid}` matches no sheet field; fields are never created here"),
            )),
        }
    }
    msgs
}

/// Round-trip guard, run AFTER all write-backs: the whole kernel must survive
/// `to_value` -> `from_value::<RuleKernel>`, and `normalize_resource_tracks`
/// (id/name filter only) must keep every upgraded `followup_procedure_id`.
/// Err -> the caller reverts the WHOLE pass (fail-closed integrity).
pub fn kernel_round_trip_guard(kernel: &RuleKernel) -> Result<(), String> {
    let v = serde_json::to_value(kernel).map_err(|e| format!("kernel to_value failed: {e}"))?;
    let rt: RuleKernel = serde_json::from_value(v)
        .map_err(|e| format!("kernel from_value round-trip failed: {e}"))?;
    let count = |tracks: &[Value]| -> usize {
        trpg_model::normalize_resource_tracks(tracks)
            .iter()
            .filter_map(|t| t.get("thresholds").and_then(Value::as_array).cloned())
            .flatten()
            .filter(|th| th.get("followup_procedure_id").is_some())
            .count()
    };
    let (before, after) = (count(&kernel.resource_tracks), count(&rt.resource_tracks));
    if before != after {
        return Err(format!(
            "normalize_resource_tracks dropped upgraded followup keys: {before} -> {after}"
        ));
    }
    Ok(())
}
