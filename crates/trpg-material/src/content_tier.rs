//! M5 / DP-1 = 4b: provisional discoverable-content tier.
//! Part A: source-present content → VerifiedExact (unconditional).
//! Part B: persona-judge synth admitted as ProvisionalNeedsAudit ONLY when all hold:
//!   (i) facts_can_reveal only, (ii) no gm_truth contradiction, (iii) tagged provisional.
//! Gate: TRPG_MATERIALIZATION_AFFORDANCE=enforce. Off → byte-equal baseline.

use serde_json::{json, Value};
use trpg_model::{BindingVerificationStatus, MaterializationAffordanceMode};

/// Outcome of a 4b provisional content evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentTierDecision {
    /// All three invariants hold; admit as provisional content.
    AdmitProvisional,
    /// Invariant (i) violated: payload references a withheld fact.
    BlockedWithheldFact { withheld_field: String },
    /// Invariant (ii) violated: payload contradicts gm_truth.
    BlockedContradictsTruth { contradicted_key: String },
    /// Invariant (iii) violated: payload is not tagged `provisional`.
    BlockedNotTaggedProvisional,
    /// Flag is Off → not evaluated.
    SkippedFlagOff,
}

impl ContentTierDecision {
    #[allow(dead_code)]
    pub fn is_admitted(&self) -> bool {
        matches!(self, Self::AdmitProvisional)
    }
    pub fn is_blocked(&self) -> bool {
        matches!(
            self,
            Self::BlockedWithheldFact { .. }
                | Self::BlockedContradictsTruth { .. }
                | Self::BlockedNotTaggedProvisional
        )
    }
    pub fn block_reason(&self) -> &'static str {
        match self {
            Self::BlockedWithheldFact { .. } => "withheld_fact_referenced",
            Self::BlockedContradictsTruth { .. } => "contradicts_gm_truth",
            Self::BlockedNotTaggedProvisional => "not_tagged_provisional",
            Self::AdmitProvisional => "admitted",
            Self::SkippedFlagOff => "flag_off",
        }
    }
}

/// Required fields for source-present discoverable content.
/// All must be present and non-null for `VerifiedExact`.
pub const CONTENT_REQUIRED_FIELDS: &[&str] = &[
    "content_kind",  // "clue_body" | "npc_persona_prose" | "scene_read_aloud"
    "content_text",  // the actual text payload
    "source_refs",   // must cite at least one source document
];

/// Evaluate the three 4b invariants for a persona-judge synthesized content payload.
///
/// `payload`  — the extracted JSON for the content demand.
/// `gm_truth` — optional ground-truth JSON for the NPC/scene (from module bundle).
///              Pass `None` when unavailable; invariant (ii) passes trivially.
///
/// Gate is read from `TRPG_MATERIALIZATION_AFFORDANCE`; Off → `SkippedFlagOff`.
pub fn evaluate_4b_invariants(payload: &Value, gm_truth: Option<&Value>) -> ContentTierDecision {
    evaluate_4b_invariants_with_mode(payload, gm_truth, MaterializationAffordanceMode::from_env())
}

/// Inner version that accepts an explicit mode — used by tests to avoid env-var races.
pub(crate) fn evaluate_4b_invariants_with_mode(
    payload: &Value,
    gm_truth: Option<&Value>,
    mode: MaterializationAffordanceMode,
) -> ContentTierDecision {
    if mode.is_off() {
        return ContentTierDecision::SkippedFlagOff;
    }

    // Invariant (iii): tagged `provisional`.
    let tagged = payload
        .get("provisional")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !tagged {
        return ContentTierDecision::BlockedNotTaggedProvisional;
    }

    // Invariant (i): `facts_will_withhold` must not appear or must be empty.
    // The payload must NOT reference any key listed under `facts_will_withhold`.
    if let Some(withheld_arr) = payload.get("facts_will_withhold").and_then(Value::as_array) {
        if !withheld_arr.is_empty() {
            let withheld_field = withheld_arr
                .first()
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            return ContentTierDecision::BlockedWithheldFact { withheld_field };
        }
    }
    // Belt-and-suspenders: if the payload carries a `withhold_keys` list, verify
    // none appear verbatim in content_text.
    if let (Some(text), Some(keys)) = (
        payload.get("content_text").and_then(Value::as_str),
        payload.get("withhold_keys").and_then(Value::as_array),
    ) {
        for key in keys {
            if let Some(k) = key.as_str() {
                if !k.trim().is_empty() && text.contains(k) {
                    return ContentTierDecision::BlockedWithheldFact {
                        withheld_field: k.to_string(),
                    };
                }
            }
        }
    }

    // Invariant (ii): content_text must not contradict gm_truth.
    if let Some(truth) = gm_truth {
        if let Some(contradiction_key) = find_contradiction(payload, truth) {
            return ContentTierDecision::BlockedContradictsTruth {
                contradicted_key: contradiction_key,
            };
        }
    }

    ContentTierDecision::AdmitProvisional
}

/// Build the `result_json` blob for a blocked 4b event (used in audit insert).
pub fn blocked_content_event_json(
    demand_id: &str,
    binding_id: &str,
    decision: &ContentTierDecision,
    payload: &Value,
) -> Value {
    json!({
        "policy": "4b_provisional_content_tier",
        "demand_id": demand_id,
        "binding_id": binding_id,
        "block_reason": decision.block_reason(),
        "decision": format!("{:?}", decision),
        "payload_kind": payload.get("content_kind").and_then(Value::as_str).unwrap_or("unknown"),
        "note": "Provisional content blocked; no discoverable-content writeback applied."
    })
}

/// Build the `verifier_json` blob for an admitted provisional content row.
pub fn admitted_content_verifier_json(
    demand_id: &str,
    extraction_run_id: &str,
    payload: &Value,
) -> Value {
    json!({
        "demand_id": demand_id,
        "extraction_run_id": extraction_run_id,
        "required_fields": CONTENT_REQUIRED_FIELDS,
        "content_kind": payload.get("content_kind").and_then(Value::as_str).unwrap_or("unknown"),
        "tier": "4b_provisional_content",
        "invariants_checked": ["facts_can_reveal_only", "no_gm_truth_contradiction", "tagged_provisional"],
        "writeback_policy": "provisional content admitted; tagged ProvisionalNeedsAudit; not a ground-truth fact; not a mechanical parameter"
    })
}

/// Determine the `BindingVerificationStatus` for a content target based on
/// source-present field completeness (Part A, unconditional).
///
/// `missing_required` is computed by the caller from `CONTENT_REQUIRED_FIELDS`.
#[allow(dead_code)]
pub fn content_verification_status(missing_required: &[String]) -> BindingVerificationStatus {
    if missing_required.is_empty() {
        BindingVerificationStatus::VerifiedExact
    } else {
        BindingVerificationStatus::ProvisionalNeedsAudit
    }
}

/// Lightweight contradiction check: for each scalar key in `gm_truth`, if the same
/// key exists in `payload` with a different non-null value, it's a contradiction.
/// Returns the first contradicted key found, or `None`.
fn find_contradiction(payload: &Value, gm_truth: &Value) -> Option<String> {
    let truth_obj = gm_truth.as_object()?;
    let pay_obj = payload.as_object()?;
    for (key, truth_val) in truth_obj {
        // Only compare scalar leaf values (strings, numbers, bools); skip arrays/objects.
        if !truth_val.is_string() && !truth_val.is_number() && !truth_val.is_boolean() {
            continue;
        }
        if let Some(pay_val) = pay_obj.get(key) {
            if pay_val.is_null() {
                continue; // null payload field → no contradiction
            }
            if pay_val != truth_val {
                return Some(key.clone());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Unit tests (TDD red→green)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Tests use evaluate_4b_invariants_with_mode directly to avoid env-var races
    // (Rust tests run in parallel; global env var mutation is unreliable).

    const ENFORCE: MaterializationAffordanceMode = MaterializationAffordanceMode::Enforce;
    const OFF: MaterializationAffordanceMode = MaterializationAffordanceMode::Off;

    // ------------------------------------------------------------------
    // Part A: content_verification_status
    // ------------------------------------------------------------------

    #[test]
    fn source_present_all_fields_gives_verified_exact() {
        // All required fields present → VerifiedExact (strict gate passes).
        let missing: Vec<String> = vec![];
        assert_eq!(
            content_verification_status(&missing),
            BindingVerificationStatus::VerifiedExact,
            "source-present content with all fields must be VerifiedExact"
        );
    }

    #[test]
    fn source_present_missing_field_gives_provisional() {
        let missing = vec!["source_refs".to_string()];
        assert_eq!(
            content_verification_status(&missing),
            BindingVerificationStatus::ProvisionalNeedsAudit
        );
    }

    // ------------------------------------------------------------------
    // TDD #2: 4b invariants all satisfied → AdmitProvisional
    // ------------------------------------------------------------------

    #[test]
    fn all_three_invariants_satisfied_admits_provisional() {
        let payload = json!({
            "content_kind": "npc_persona_prose",
            "content_text": "张警长是个老实人，平日话不多。",
            "source_refs": ["module://npc/zhang"],
            "provisional": true,
            "facts_can_reveal": ["occupation", "personality"],
            "facts_will_withhold": []
        });
        let d = evaluate_4b_invariants_with_mode(&payload, None, ENFORCE);
        assert_eq!(d, ContentTierDecision::AdmitProvisional,
            "all invariants satisfied → admitted: {:?}", d);
    }

    // ------------------------------------------------------------------
    // TDD #3a: invariant (i) violated — withheld fact
    // ------------------------------------------------------------------

    #[test]
    fn withheld_fact_in_facts_will_withhold_blocks() {
        let payload = json!({
            "content_kind": "npc_persona_prose",
            "content_text": "张警长是无辜的。",
            "source_refs": ["module://npc/zhang"],
            "provisional": true,
            "facts_will_withhold": ["real_murderer_identity"]
        });
        let d = evaluate_4b_invariants_with_mode(&payload, None, ENFORCE);
        assert!(d.is_blocked(), "withheld fact must block: {:?}", d);
        assert_eq!(d.block_reason(), "withheld_fact_referenced");
    }

    // ------------------------------------------------------------------
    // TDD #3b: invariant (ii) violated — contradicts gm_truth
    // ------------------------------------------------------------------

    #[test]
    fn contradiction_of_gm_truth_blocks() {
        let payload = json!({
            "content_kind": "npc_persona_prose",
            "content_text": "张警长是凶手。",
            "source_refs": ["module://npc/zhang"],
            "provisional": true,
            "facts_will_withhold": [],
            "is_murderer": true   // contradicts gm_truth below
        });
        let gm_truth = json!({ "is_murderer": false });
        let d = evaluate_4b_invariants_with_mode(&payload, Some(&gm_truth), ENFORCE);
        assert!(d.is_blocked(), "gm_truth contradiction must block: {:?}", d);
        assert_eq!(d.block_reason(), "contradicts_gm_truth");
    }

    // ------------------------------------------------------------------
    // TDD #3c: invariant (iii) violated — not tagged provisional
    // ------------------------------------------------------------------

    #[test]
    fn untagged_payload_blocks() {
        let payload = json!({
            "content_kind": "npc_persona_prose",
            "content_text": "张警长看起来很紧张。",
            "source_refs": ["module://npc/zhang"],
            "provisional": false,   // NOT tagged
            "facts_will_withhold": []
        });
        let d = evaluate_4b_invariants_with_mode(&payload, None, ENFORCE);
        assert!(d.is_blocked(), "untagged payload must block: {:?}", d);
        assert_eq!(d.block_reason(), "not_tagged_provisional");
    }

    #[test]
    fn missing_provisional_tag_blocks() {
        let payload = json!({
            "content_kind": "npc_persona_prose",
            "content_text": "张警长看起来很紧张。",
            "source_refs": ["module://npc/zhang"],
            // `provisional` key entirely absent → treated as false
            "facts_will_withhold": []
        });
        let d = evaluate_4b_invariants_with_mode(&payload, None, ENFORCE);
        assert_eq!(d, ContentTierDecision::BlockedNotTaggedProvisional);
    }

    // ------------------------------------------------------------------
    // TDD #4: flag Off → SkippedFlagOff (byte-equal baseline)
    // ------------------------------------------------------------------

    #[test]
    fn flag_off_returns_skipped() {
        let payload = json!({
            "content_kind": "npc_persona_prose",
            "content_text": "任何内容。",
            "source_refs": ["module://npc/x"],
            "provisional": true,
            "facts_will_withhold": []
        });
        let d = evaluate_4b_invariants_with_mode(&payload, None, OFF);
        assert_eq!(d, ContentTierDecision::SkippedFlagOff,
            "flag Off must skip all evaluation");
    }

    // ------------------------------------------------------------------
    // CONTENT_REQUIRED_FIELDS completeness guard
    // ------------------------------------------------------------------

    #[test]
    fn required_fields_list_has_three_entries() {
        assert_eq!(CONTENT_REQUIRED_FIELDS.len(), 3,
            "content required fields must be content_kind, content_text, source_refs");
    }

    // ------------------------------------------------------------------
    // find_contradiction scalar match / mismatch
    // ------------------------------------------------------------------

    #[test]
    fn find_contradiction_returns_none_when_consistent() {
        let payload = json!({"is_murderer": false, "name": "张警长"});
        let truth = json!({"is_murderer": false});
        assert!(find_contradiction(&payload, &truth).is_none());
    }

    #[test]
    fn find_contradiction_skips_null_payload_values() {
        let payload = json!({"is_murderer": null});
        let truth = json!({"is_murderer": false});
        // null payload → no contradiction (don't penalise absent info)
        assert!(find_contradiction(&payload, &truth).is_none());
    }

    #[test]
    fn withhold_keys_in_content_text_blocks() {
        // Belt-and-suspenders: if the synthesised text contains a withheld key term.
        let payload = json!({
            "content_kind": "npc_persona_prose",
            "content_text": "他知道那个real_murderer_identity。",
            "source_refs": ["module://npc/zhang"],
            "provisional": true,
            "facts_will_withhold": [],
            "withhold_keys": ["real_murderer_identity"]
        });
        let d = evaluate_4b_invariants_with_mode(&payload, None, ENFORCE);
        assert!(d.is_blocked(), "withhold_keys in text must block: {:?}", d);
        assert_eq!(d.block_reason(), "withheld_fact_referenced");
    }
}
