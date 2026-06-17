//! SemanticNeedClassifier (P0-3): the keyword-free replacement for the engine's
//! ad-hoc `looks_rule_or_module_sensitive` keyword scan. It projects a structured
//! {needs_rule, needs_material, needs_scene, confidence} signal DETERMINISTICALLY
//! off an already-computed `SemanticIntentResult` — no extra LLM call, no
//! per-ruleset/module keyword list. The bounded classifier already inferred
//! intent + materialization needs by MEANING (route_cues, materialization
//! requests, action kind); this just reads them off so downstream retrieval /
//! routing consume a semantic signal instead of scanning words.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use trpg_model::*;

/// What a turn needs, derived semantically from a `SemanticIntentResult`.
/// `confidence` is the semantic layer's confidence mapped to [0.0, 1.0]; it is
/// 0.0 when the classifier was unavailable so callers know to fall back to their
/// own audited lexical heuristic.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NeedSignal {
    pub needs_rule: bool,
    pub needs_material: bool,
    pub needs_scene: bool,
    pub confidence: f32,
}

impl NeedSignal {
    /// All-false / zero-confidence signal — the "semantic gave us nothing" value.
    pub const NONE: NeedSignal = NeedSignal { needs_rule: false, needs_material: false, needs_scene: false, confidence: 0.0 };

    /// Any kind of need was detected.
    pub fn any(&self) -> bool { self.needs_rule || self.needs_material || self.needs_scene }

    /// Whether the semantic signal is confident enough to be trusted over a
    /// caller's lexical fallback.
    pub fn is_confident(&self, threshold: f32) -> bool { self.confidence >= threshold }
}

/// A check-bearing action: resolving it requires a rules mechanic (contest,
/// reaction, technical/investigation check). Pure non-mechanical narration
/// actions (ask/move/observe) are excluded.
fn action_needs_rule(action: SituationActionKind) -> bool {
    matches!(action,
        SituationActionKind::Attack
      | SituationActionKind::UnderAttack
      | SituationActionKind::EnemyInitiatedConflict
      | SituationActionKind::SceneEntersConflict
      | SituationActionKind::Defend
      | SituationActionKind::Dodge
      | SituationActionKind::Counterattack
      | SituationActionKind::TakeCover
      | SituationActionKind::Hack
      | SituationActionKind::DisableDevice
      | SituationActionKind::CastOrUsePower
      | SituationActionKind::InvestigateDuringConflict)
}

/// Read a boolean `route_cues.<key>` the LLM emitted. These are semantic
/// evidence flags set by the classifier, not engine keyword scans.
fn route_cue(sem: &SemanticIntentResult, key: &str) -> bool {
    sem.raw_json
        .get("route_cues")
        .and_then(|v| v.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Stateless projector from a semantic classification result to a `NeedSignal`.
pub struct SemanticNeedClassifier;

impl SemanticNeedClassifier {
    /// Derive the need signal from a semantic result. When the classifier was
    /// unavailable (`semantic_unavailable_no_route`) returns [`NeedSignal::NONE`].
    pub fn classify(sem: &SemanticIntentResult) -> NeedSignal {
        if sem.classifier == "semantic_unavailable_no_route" {
            return NeedSignal::NONE;
        }
        let confidence = match sem.confidence {
            RulingConfidence::High => 0.9,
            RulingConfidence::Medium => 0.6,
            RulingConfidence::Low => 0.3,
        };
        let mats = &sem.materialization_requests;
        // needs_rule: the turn invokes a resolvable mechanic — a check/contest/
        // effect/condition must be materialized, OR the action itself is a
        // check-bearing (combat / contested / investigation) action, OR the LLM
        // raised a combat/assessment route cue.
        let needs_rule = mats.iter().any(|m| matches!(m.target_kind,
                RuleBindingTargetKind::CheckContract
              | RuleBindingTargetKind::EffectContract
              | RuleBindingTargetKind::ContestProfile
              | RuleBindingTargetKind::ConditionDefinition))
            || action_needs_rule(sem.primary_action_kind)
            || route_cue(sem, "combat_action")
            || route_cue(sem, "assessment_check");
        // needs_material: a concrete object / ability / actor parameter must be
        // looked up, OR the LLM named object/ability refs or an object-use cue.
        let needs_material = mats.iter().any(|m| matches!(m.target_kind,
                RuleBindingTargetKind::ObjectDefinition
              | RuleBindingTargetKind::AbilityDefinition
              | RuleBindingTargetKind::ActorParameter))
            || !sem.object_refs.is_empty()
            || !sem.ability_refs.is_empty()
            || route_cue(sem, "object_interaction")
            || route_cue(sem, "source_object_use");
        // needs_scene: the player is asking about / moving through / observing the
        // scene rather than resolving a mechanic against an entity.
        let needs_scene = matches!(sem.primary_action_kind,
                SituationActionKind::AskQuestion | SituationActionKind::Move)
            || matches!(sem.frame_relation, FrameRelation::PauseAndObserve);
        NeedSignal { needs_rule, needs_material, needs_scene, confidence }
    }

    /// Convenience for the common `Option<&SemanticIntentResult>` call site:
    /// `None` (no semantic computed at all) → [`NeedSignal::NONE`].
    pub fn classify_opt(sem: Option<&SemanticIntentResult>) -> NeedSignal {
        sem.map(Self::classify).unwrap_or(NeedSignal::NONE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;

    fn result(action: SituationActionKind, confidence: RulingConfidence, classifier: &str, raw: Value) -> SemanticIntentResult {
        SemanticIntentResult {
            semantic_id: "s".into(), session_id: "sess".into(), turn_id: "t".into(), ruleset_id: "rs".into(),
            primary_action_kind: action, gate_relation: "no_active_gate".into(),
            frame_relation: FrameRelation::OutsideFrameAction, target_refs: vec![], object_refs: vec![], ability_refs: vec![],
            materialization_requests: vec![], secrecy_policy: "gm_only".into(), confidence,
            classifier: classifier.into(), rationale_brief: None, raw_json: raw, created_at: Utc::now(),
        }
    }
    fn mat(kind: RuleBindingTargetKind) -> MaterializationRequest {
        MaterializationRequest { request_id: "m".into(), target_kind: kind, target_id: None, target_label: "x".into(), target_description: String::new(), evidence_span: String::new(), requested_fields: vec![], urgency: RuntimeUrgency::Soon, visibility: Visibility::GmOnly, metadata: json!({}) }
    }

    // needs_rule: a combat action implies a rules check is needed.
    #[test]
    fn combat_action_needs_rule() {
        let s = result(SituationActionKind::Attack, RulingConfidence::High, "llm", json!({}));
        assert!(SemanticNeedClassifier::classify(&s).needs_rule, "attack should need a rule");
    }

    // needs_rule: a check_contract materialization request implies needs_rule.
    #[test]
    fn check_contract_request_needs_rule() {
        let mut s = result(SituationActionKind::Unknown, RulingConfidence::Medium, "llm", json!({}));
        s.materialization_requests = vec![mat(RuleBindingTargetKind::CheckContract)];
        assert!(SemanticNeedClassifier::classify(&s).needs_rule);
    }

    // needs_material: an object_definition materialization request implies needs_material.
    #[test]
    fn object_definition_request_needs_material() {
        let mut s = result(SituationActionKind::UseItem, RulingConfidence::Medium, "llm", json!({}));
        s.materialization_requests = vec![mat(RuleBindingTargetKind::ObjectDefinition)];
        assert!(SemanticNeedClassifier::classify(&s).needs_material, "object definition should need material");
    }

    // needs_material: object_interaction route_cue alone implies needs_material.
    #[test]
    fn object_interaction_cue_needs_material() {
        let s = result(SituationActionKind::Unknown, RulingConfidence::Medium, "llm", json!({"route_cues":{"object_interaction":true}}));
        assert!(SemanticNeedClassifier::classify(&s).needs_material);
    }

    // needs_scene: asking a question / observation is a scene-information need.
    #[test]
    fn ask_question_needs_scene() {
        let s = result(SituationActionKind::AskQuestion, RulingConfidence::Medium, "llm", json!({}));
        let n = SemanticNeedClassifier::classify(&s);
        assert!(n.needs_scene, "ask_question should need scene info");
        assert!(!n.needs_rule, "a plain question should not need a rule");
    }

    // confidence thresholds: High/Medium/Low map onto a monotone scale, and the
    // unavailable classifier yields NONE (0.0) so callers fall back to lexical.
    #[test]
    fn confidence_maps_from_semantic_confidence() {
        let high = SemanticNeedClassifier::classify(&result(SituationActionKind::Attack, RulingConfidence::High, "llm", json!({})));
        let medium = SemanticNeedClassifier::classify(&result(SituationActionKind::Attack, RulingConfidence::Medium, "llm", json!({})));
        let low = SemanticNeedClassifier::classify(&result(SituationActionKind::Attack, RulingConfidence::Low, "llm", json!({})));
        assert!(high.confidence > medium.confidence && medium.confidence > low.confidence, "monotone High>Medium>Low");
        assert!(high.is_confident(0.7) && !medium.is_confident(0.7), "0.7 threshold splits High from Medium");
        assert!(medium.is_confident(0.5) && !low.is_confident(0.5), "0.5 threshold splits Medium from Low");
    }

    #[test]
    fn unavailable_classifier_is_none() {
        let s = result(SituationActionKind::Attack, RulingConfidence::High, "semantic_unavailable_no_route", json!({}));
        assert_eq!(SemanticNeedClassifier::classify(&s), NeedSignal::NONE);
        assert_eq!(SemanticNeedClassifier::classify_opt(None), NeedSignal::NONE);
        assert!(!NeedSignal::NONE.any());
    }
}
