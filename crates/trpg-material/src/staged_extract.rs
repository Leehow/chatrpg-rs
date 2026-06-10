//! On-demand object-schema compilation (Optimization 2 of staged parsing).
//!
//! STATUS: DORMANT / DEFERRED (2026-06-09). The activation hook (a call to
//! `ensure_category_compiled` at the top of `object_schema_guidance`) is intentionally
//! NOT wired. Staged Stage 2 currently does the FULL object compile eagerly in the
//! background (it doesn't block the user, who is mid character-creation), so schemas
//! are ready by play time with no mid-action hiccup. Enable this lazy path later for
//! SPELL-HEAVY rulesets (e.g. D&D's hundreds of spells) where eager extraction of every
//! category is genuinely too slow — then add the one-line hook into object_schema_guidance.
//! Until then this module is compiled + unit-tested but never invoked.
//!
//! Staged parsing's Stage 2 only DISCOVERS object/ability categories — each lands
//! in `kernel.object_schemas` as a `status:"discovered"` STUB (category_id / kind /
//! source_pages / couples_to, but no `schema_slots`/`examples`). The expensive
//! per-category extract is deferred to PLAY time: the FIRST turn that needs a stub's
//! family materializes it. `ensure_category_compiled` is that lazy filler — it
//! detects a matched stub, runs `reader::extract_object_category` once, upserts the
//! filled schema back into the kernel (cached for next time), then `object_schema_guidance`
//! proceeds with the now-typed slots. Fail-open: if the data dir is unavailable or the
//! extract fails, the stub is left as-is and the caller falls back to free-form.

use crate::MaterializationService;
use serde_json::{json, Value};
use trpg_model::{MaterialTargetKind, MaterializationDemand, RuleKernel};
use trpg_rule_agent::reader::{self, ObjectCtx, Unit};

/// Ability-family category kinds — kept in sync with `object_schema_guidance`'s
/// own split so on-demand compile fills exactly the categories that guidance will
/// then surface (don't compile the whole catalog on a single weapon turn).
pub(crate) const ABILITY_KINDS: &[&str] =
    &["spell", "psychic", "ability", "power", "discipline", "maneuver", "talent", "magic"];

impl MaterializationService {
    /// Fill any DISCOVERED stub matching this demand's family on first use: extract
    /// its schema (one LLM pass over the ruleset's source) and upsert the filled
    /// category back into the kernel. Best-effort — never errors out the turn.
    pub(crate) async fn ensure_category_compiled(&self, demand: &MaterializationDemand) {
        let Some(client) = self.llm.as_ref() else { return };
        let Some(mut kernel) = self.db.load_rule_kernel(&demand.ruleset_id).await.ok().flatten() else { return };
        if kernel.object_schemas.is_empty() {
            return;
        }
        let want_ability = demand_wants_ability(demand);
        // Which categories of this family are still bare stubs (no schema_slots)?
        let pending: Vec<usize> = kernel
            .object_schemas
            .iter()
            .enumerate()
            .filter(|(_, s)| category_in_family(s, want_ability) && is_stub(s))
            .map(|(i, _)| i)
            .collect();
        if pending.is_empty() {
            return; // already compiled (or no stub in this family) — nothing to do.
        }
        // Resolve the ruleset's source units + sidecar from the configured data dir.
        // Without it we cannot read the table pages — skip on-demand compile and let
        // the caller fall back to free-form (logged, never a silent cap).
        let Some((units, sidecar)) = resolve_ruleset_units(&demand.ruleset_id) else {
            tracing::info!(ruleset = %demand.ruleset_id, "on-demand object compile skipped: data dir / source units unavailable");
            return;
        };
        let skills = kernel_skill_ids(&kernel);
        let resource_tracks = kernel_track_ids(&kernel);
        let ctx = ObjectCtx { units: &units, sidecar_text: sidecar, skills, resource_tracks };
        let budget = 9usize;
        let mut changed = false;
        for idx in pending {
            let stub = kernel.object_schemas[idx].clone();
            match reader::extract_object_category(client, &ctx, &stub, budget).await {
                Some(filled) => {
                    merge_compiled_schema(&mut kernel.object_schemas, filled);
                    changed = true;
                }
                None => {
                    let cat_id = stub.get("category_id").and_then(|v| v.as_str()).unwrap_or("");
                    tracing::info!(ruleset = %demand.ruleset_id, category = %cat_id, "on-demand object extract returned nothing; keeping stub");
                }
            }
        }
        if changed {
            if let Err(e) = self.db.upsert_rule_kernel(&kernel).await {
                tracing::warn!(ruleset = %demand.ruleset_id, error = %e, "failed to persist on-demand-compiled object schemas");
            }
        }
    }
}

/// A demand is in the ABILITY family (spells/powers) vs the OBJECT family
/// (weapons/gear/armor/…). Mirrors `object_schema_guidance`'s `want_ability`.
fn demand_wants_ability(demand: &MaterializationDemand) -> bool {
    matches!(demand.target_kind, MaterialTargetKind::AbilityDefinition | MaterialTargetKind::AbilityInstance)
}

/// Does this category belong to the requested family? Data-driven over the
/// schema's own `kind` (no per-ruleset category names hardcoded), identical to
/// the predicate `object_schema_guidance` uses to pick.
pub(crate) fn category_in_family(schema: &Value, want_ability: bool) -> bool {
    let kind = schema.get("kind").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
    ABILITY_KINDS.iter().any(|k| kind.contains(k)) == want_ability
}

/// A bare stub: `status` is not yet `compiled` AND it carries no `schema_slots`.
/// `discover_object_categories` stamps `status:"discovered"`; older kernels may
/// have no marker — treat "no schema_slots" as the source of truth either way.
pub(crate) fn is_stub(schema: &Value) -> bool {
    let compiled = schema.get("status").and_then(Value::as_str) == Some("compiled");
    let has_slots = schema
        .get("schema_slots")
        .and_then(Value::as_array)
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    !compiled && !has_slots
}

/// Upsert a freshly-extracted schema into the kernel's `object_schemas`, replacing
/// the matching stub by `category_id` (case-insensitive). Appends if no stub matched.
pub(crate) fn merge_compiled_schema(schemas: &mut Vec<Value>, filled: Value) {
    let id = filled.get("category_id").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
    if let Some(slot) = schemas.iter_mut().find(|s| {
        s.get("category_id").and_then(Value::as_str).map(|x| x.to_ascii_lowercase()) == Some(id.clone())
    }) {
        *slot = filled;
    } else {
        schemas.push(filled);
    }
}

/// Character skill field ids carried in the kernel's `character_sheet_schema`
/// (a serialized CharacterTemplate). Used so an `attack_skill` hook reuses the
/// real id (mirrors the parser's `skill_ids`).
fn kernel_skill_ids(kernel: &RuleKernel) -> Vec<String> {
    let mut out: Vec<String> = kernel
        .character_sheet_schema
        .get("fields")
        .and_then(Value::as_array)
        .map(|fs| {
            fs.iter()
                .filter(|f| f.get("field_type").and_then(Value::as_str) == Some("skill"))
                .filter_map(|f| f.get("field_id").and_then(Value::as_str).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out.dedup();
    out
}

/// Resource-track ids from the kernel's `resource_tracks` (mirrors `track_ids`).
fn kernel_track_ids(kernel: &RuleKernel) -> Vec<String> {
    kernel
        .resource_tracks
        .iter()
        .filter_map(|t| t.get("id").or_else(|| t.get("name")).and_then(Value::as_str).map(String::from))
        .collect()
}

/// Resolve a ruleset's source units (largest `*.semantic_units.jsonl`) + duotext
/// sidecar from the configured data dir (`TRPG_DATA_DIR`, default `./data`).
/// Mirrors the CLI/orchestrator path; returns None when the dir is unavailable so
/// the caller can fall back to free-form materialization.
fn resolve_ruleset_units(_ruleset_id: &str) -> Option<(Vec<Unit>, Option<String>)> {
    let data_dir = std::env::var("TRPG_DATA_DIR").unwrap_or_else(|_| "./data".to_string());
    let units_dir = std::path::Path::new(&data_dir).join("parsed/source_units");
    let (units_path, source_id) = largest_units_file(&units_dir)?;
    let units = reader::load_units(&units_path).ok()?;
    if units.is_empty() {
        return None;
    }
    let sidecar = std::fs::read_to_string(
        std::path::Path::new(&data_dir).join(format!("markdown/rulebooks/{source_id}.md")),
    )
    .ok();
    Some((units, sidecar))
}

/// Largest `*.semantic_units.jsonl` under `dir`, with its `source_id` (filename
/// minus the suffix). Mirrors the CLI `largest_units_file`.
fn largest_units_file(dir: &std::path::Path) -> Option<(std::path::PathBuf, String)> {
    let mut best: Option<(std::path::PathBuf, String, u64)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        let source_id = match path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".semantic_units.jsonl")) {
            Some(id) => id.to_string(),
            None => continue,
        };
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        if best.as_ref().map(|(_, _, s)| size > *s).unwrap_or(true) {
            best = Some((path, source_id, size));
        }
    }
    best.map(|(p, id, _)| (p, id))
}

#[cfg(test)]
mod tests {
    use super::*;

    // The deterministic half of `ensure_category_compiled`: a `discovered` weapon
    // stub is detected as needing compile, and once a filled schema arrives it
    // REPLACES the stub by category_id (status flips discovered -> compiled,
    // schema_slots present). The live DB+LLM path is exercised by the ignored
    // `live_on_demand_compile` test below + `parse-staged`.
    #[test]
    fn stub_is_detected_then_replaced_by_compiled_schema() {
        let mut schemas = vec![
            json!({"category_id": "firearms", "kind": "weapon", "source_pages": "401-403", "status": "discovered"}),
            json!({"category_id": "spells", "kind": "spell", "source_pages": "230", "status": "discovered"}),
        ];
        // A weapon demand matches the firearms stub (object family), not the spell.
        assert!(category_in_family(&schemas[0], false), "weapon stub is in the object family");
        assert!(!category_in_family(&schemas[1], false), "spell stub is NOT in the object family");
        assert!(is_stub(&schemas[0]), "a discovered, slot-less category is a stub");

        // A compiled spell schema (status compiled, has slots) is no longer a stub.
        let compiled_spell = json!({"category_id": "spells", "kind": "spell", "status": "compiled", "schema_slots": [{"slot": "cost", "type": "number"}]});
        assert!(!is_stub(&compiled_spell), "a compiled category with slots is not a stub");

        // Materialize the firearms stub: replace it by category_id (no duplicate row).
        let filled = json!({"category_id": "firearms", "kind": "weapon", "status": "compiled",
            "schema_slots": [{"slot": "damage", "type": "dice", "hook": "core dice"}],
            "examples": [{"name": ".38 revolver", "slots": {"damage": "1D10"}}]});
        merge_compiled_schema(&mut schemas, filled);
        assert_eq!(schemas.len(), 2, "stub is replaced in place, not appended");
        let fired = schemas.iter().find(|s| s["category_id"] == "firearms").unwrap();
        assert_eq!(fired["status"].as_str(), Some("compiled"), "stub flipped to compiled");
        assert!(fired["schema_slots"].as_array().map(|a| !a.is_empty()).unwrap_or(false), "compiled schema carries schema_slots");
        assert!(!is_stub(fired), "the firearms entry is no longer a stub");
        // The untouched spell stub survives.
        assert_eq!(schemas.iter().find(|s| s["category_id"] == "spells").unwrap()["status"].as_str(), Some("discovered"));
    }

    // merge appends when no stub matches the filled schema's category_id.
    #[test]
    fn merge_appends_unmatched_category() {
        let mut schemas = vec![json!({"category_id": "armor", "kind": "armor", "status": "discovered"})];
        merge_compiled_schema(&mut schemas, json!({"category_id": "vehicles", "kind": "vehicle", "status": "compiled", "schema_slots": [{"slot": "speed", "type": "number"}]}));
        assert_eq!(schemas.len(), 2, "an unmatched category is appended");
    }
}
