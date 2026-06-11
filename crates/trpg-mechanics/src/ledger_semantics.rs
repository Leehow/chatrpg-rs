//! B3 read-side semantic attachment for the mechanical-ledger projection.
//!
//! Pure helpers (no IO) that resolve a `generic_parameter_states` projection
//! row's `parameter_path` to a kernel resource track and attach the
//! `track_semantic_line` rendering under a `"semantic"` key, so the GM agent
//! sees what a number MEANS (threshold zone / zero_means), not just the digit.
//! Read-side only — write paths are untouched (the actor/scene owner split in
//! `apply_outcome_resource_tracks` stays as-is). Fail-closed throughout: no
//! kernel track match, no numeric value, or nothing semantic to say ⇒ the row
//! stays bare (never invented).

use serde_json::{json, Value};
use trpg_model::{resolve_resource_track_id, track_semantic_line, RuleKernel};

fn value_to_i32(v: &Value) -> Option<i32> {
    v.as_i64()
        .map(|n| n as i32)
        .or_else(|| v.as_f64().map(|f| f as i32))
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<i32>().ok()))
}

/// Kernel track JSON for a ledger `parameter_path`. The scene/party/world
/// projection shape `tracks.{id}.current` is stripped to its bare id before
/// `resolve_resource_track_id` (which only understands `hp` / `resources.{id}`
/// / bare-name forms — fed raw it would mis-resolve on head=`tracks`).
pub fn kernel_track_for_path<'a>(parameter_path: &str, kernel: &'a RuleKernel) -> Option<&'a Value> {
    let p = parameter_path.trim();
    let normalized = p.strip_prefix("tracks.").unwrap_or(p);
    let track_id = resolve_resource_track_id(normalized, kernel)?;
    kernel.resource_tracks.iter().find(|t| {
        t.get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().eq_ignore_ascii_case(&track_id))
            .unwrap_or(false)
    })
}

/// Semantic status line for a ledger row's current value, via the kernel track
/// the row's `parameter_path` resolves to. None when the path matches no
/// kernel track or the track has nothing semantic to say (fail-closed).
pub fn semantic_for_parameter_path(parameter_path: &str, current: i32, kernel: &RuleKernel) -> Option<String> {
    track_semantic_line(kernel_track_for_path(parameter_path, kernel)?, current)
}

/// Attach a `"semantic"` key to each generic_parameter_states projection row
/// whose path+value resolve to a renderable kernel-track semantic line. Rows
/// are NOT filtered by target_kind: actor / scene / party / world all take the
/// same read path (Triangle chaos is scene-owned — semantics must not be
/// actor-only). Rows without a numeric value or semantic content stay bare.
pub fn attach_generic_state_semantics(rows: &mut [Value], kernel: &RuleKernel) {
    for row in rows.iter_mut() {
        let Some(path) = row.get("parameter_path").and_then(|v| v.as_str()).map(str::to_string) else { continue };
        let Some(current) = row.get("value").and_then(value_to_i32) else { continue };
        if let Some(line) = semantic_for_parameter_path(&path, current, kernel) {
            if let Some(obj) = row.as_object_mut() {
                obj.insert("semantic".into(), json!(line));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use trpg_model::RuleKernel;

    #[test]
    fn ledger_block_attaches_semantic_line_per_owner_kind() {
        let kernel = RuleKernel {
            resource_tracks: vec![
                json!({"id":"sanity","owner_kind":"actor","thresholds":[
                    {"at":40,"direction":"at_or_below","consequence":"理智受创，开始出现幻觉"}]}),
                json!({"id":"chaos_pool","owner_kind":"scene","zero_means":"混乱平息","thresholds":[
                    {"at":6,"direction":"at_or_above","consequence":"场面失控"}]}),
                json!({"id":"plain_counter","owner_kind":"scene"}),
            ],
            ..Default::default()
        };
        // Synthesized generic_parameter_states projection rows: one actor-owned
        // resource row AND one scene-owned track row (Triangle chaos is scene
        // level — the semantic line must NOT be actor-only).
        let mut rows = vec![
            json!({"target_kind":"actor","target_id":"pc.hero",
                   "parameter_path":"resources.sanity.current","value":38}),
            json!({"target_kind":"scene","target_id":"scene.current",
                   "parameter_path":"tracks.chaos_pool.current","value":7}),
            json!({"target_kind":"scene","target_id":"scene.current",
                   "parameter_path":"tracks.plain_counter.current","value":3}),
        ];
        attach_generic_state_semantics(&mut rows, &kernel);
        let actor_sem = rows[0].get("semantic").and_then(|v| v.as_str())
            .expect("actor row must carry a \"semantic\" key");
        assert!(actor_sem.contains("理智受创"), "actor semantic must contain the consequence: {actor_sem}");
        let scene_sem = rows[1].get("semantic").and_then(|v| v.as_str())
            .expect("scene row must carry a \"semantic\" key");
        assert!(scene_sem.contains("场面失控"), "scene semantic must contain the consequence: {scene_sem}");
        // No thresholds / zero_means -> bare number, no "semantic" key.
        assert!(rows[2].get("semantic").is_none(), "row without thresholds/zero_means must stay bare");
    }
}
