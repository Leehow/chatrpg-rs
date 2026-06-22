//! Adventure IR — ObjectiveSpec / TrackerSpec / ProgressRule (P1-1). First-class
//! progress structures so "进度" is a real runtime dimension (the proven J3 root
//! cause is a missing progress dimension, not missing edges).
use super::{EffectExpr, EventPattern, PredicateExpr, ScoreEffect};
use crate::SourceRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Reference to a [`TrackerSpec`] by id (e.g. an objective deadline).
pub type TrackerRef = String;

/// A goal that defines "what counts as changing the world" — NOT how the player
/// must achieve it (anti-railroad: the guard defines success, the Director never
/// forces a path).
/// (No `Eq`: embeds `Vec<SourceRef>`, which is only `PartialEq`.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ObjectiveSpec {
    pub id: String,
    #[serde(default)]
    pub mission_id: Option<String>,
    #[serde(default)]
    pub mandatory: bool,
    pub success_when: PredicateExpr,
    #[serde(default)]
    pub failure_when: Option<PredicateExpr>,
    #[serde(default)]
    pub score_effects: Vec<ScoreEffect>,
    #[serde(default)]
    pub rewards: Vec<EffectExpr>,
    #[serde(default)]
    pub deadline: Option<TrackerRef>,
    #[serde(default)]
    pub source_evidence: Vec<SourceRef>,
}

/// Tracker kinds. This slice only needs Countdown/Timer; the enum is open for
/// P3+ (ProgressClock/ResourceTrack/...) without re-typing consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrackerKind {
    /// Counts down from a start to zero (e.g. escape clock).
    Countdown,
    /// World-time accumulator since an anchoring event (e.g. Foxwell 15min).
    Timer,
}

/// One rung of an escalating ability ladder (The Vault Chaos budget: each rung is
/// an Anomaly ability available once enough Chaos is spent). Capturing the rungs
/// is what keeps a Chaos tracker from being flattened to a bare integer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TrackerRung {
    /// Chaos/resource cost to use this rung's ability.
    pub cost: i64,
    /// The ability/effect name (e.g. "Manifest", "Expand", "Return").
    pub label: String,
    /// Verbatim authored description of what the ability does.
    pub detail: String,
}

/// A countdown/timer with an authored threshold. `at_threshold` effects fire when
/// the tracker reaches `threshold` (engine-applied in P1-3). `rungs` carries an
/// escalating ability ladder (Vault Chaos) when the tracker is a resource budget.
/// (No `Eq`: embeds `Vec<SourceRef>`, which is only `PartialEq`.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TrackerSpec {
    pub id: String,
    pub kind: TrackerKind,
    #[serde(default)]
    pub label: String,
    /// Countdown: starting value. Timer: 0.
    #[serde(default)]
    pub start: i64,
    /// Threshold that triggers `at_threshold`. Timer: minutes since anchor.
    pub threshold: i64,
    /// For a Timer, the event the elapsed window is measured from.
    #[serde(default)]
    pub anchor: Option<EventPattern>,
    #[serde(default)]
    pub at_threshold: Vec<EffectExpr>,
    #[serde(default)]
    pub visible_to_players: bool,
    #[serde(default)]
    pub source_evidence: Vec<SourceRef>,
    /// Escalating ability ladder (Vault Chaos budget). Empty for plain
    /// countdown/timer trackers → skipped on serialize (additive == baseline).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rungs: Vec<TrackerRung>,
}

/// Whether a [`ProgressRule`]'s guard/effects were fully normalized into
/// executable ASTs, partly opaque, or purely LLM-inferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NormalizationStatus {
    /// Guard + effects are all executable ASTs → Rust may auto-fire.
    Executable,
    /// Authored, with ≥1 opaque leaf → stored, shown to GM, never auto-fired.
    Opaque,
    /// LLM-inferred with no firm source anchor → retrieval/context only.
    Inferred,
}

/// Event–Condition–Effect rule: ON event, WHEN guard, THEN effects. The unified
/// dynamic-structure carrier (timers, objective completion, branch foreclosure).
/// (No `Eq`: embeds `Vec<SourceRef>`, which is only `PartialEq`.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProgressRule {
    pub id: String,
    pub on: EventPattern,
    pub when: PredicateExpr,
    pub then: Vec<EffectExpr>,
    #[serde(default)]
    pub source_evidence: Vec<SourceRef>,
    pub normalization: NormalizationStatus,
}

impl ProgressRule {
    /// A rule may auto-fire only when its guard is executable AND every effect is
    /// executable AND it is classified Executable. This is the single execution-
    /// layer fail-closed gate the engine consults.
    pub fn may_autofire(&self) -> bool {
        self.normalization == NormalizationStatus::Executable
            && self.when.is_executable()
            && self.then.iter().all(|e| e.is_executable())
    }

    /// Derive the honest [`NormalizationStatus`] from guard/effect executability.
    /// Producers call this so storage is fail-closed at the execution layer.
    pub fn classify(when: &PredicateExpr, then: &[EffectExpr]) -> NormalizationStatus {
        if when.is_executable() && then.iter().all(|e| e.is_executable()) {
            NormalizationStatus::Executable
        } else {
            NormalizationStatus::Opaque
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adventure_ir::IrValue;

    #[test]
    fn executable_rule_may_autofire() {
        let when = PredicateExpr::FactEquals {
            fact: "athena.status".into(),
            value: IrValue::Text("disabled".into()),
        };
        let then = vec![EffectExpr::Complete("obj.neutralize_athena".into())];
        let norm = ProgressRule::classify(&when, &then);
        assert_eq!(norm, NormalizationStatus::Executable);
        let rule = ProgressRule {
            id: "rule.athena".into(),
            on: EventPattern::WorldFactChanged,
            when,
            then,
            source_evidence: vec![],
            normalization: norm,
        };
        assert!(rule.may_autofire());
    }

    #[test]
    fn opaque_guard_never_autofires() {
        let when = PredicateExpr::OpaqueAuthoredText {
            raw_text: "若说服守卫".into(),
        };
        let then = vec![EffectExpr::Activate("beat.x".into())];
        let norm = ProgressRule::classify(&when, &then);
        assert_eq!(norm, NormalizationStatus::Opaque);
        let rule = ProgressRule {
            id: "rule.x".into(),
            on: EventPattern::ChoiceRecorded,
            when,
            then,
            source_evidence: vec![],
            normalization: norm,
        };
        assert!(!rule.may_autofire(), "opaque guard must never auto-fire");
    }

    #[test]
    fn opaque_effect_blocks_autofire_even_if_marked_executable() {
        // Adversarial: a mislabeled rule must still be caught by may_autofire.
        let rule = ProgressRule {
            id: "rule.y".into(),
            on: EventPattern::TimeAdvanced,
            when: PredicateExpr::ClockAtLeast {
                id: "c".into(),
                value: 1,
            },
            then: vec![EffectExpr::OpaqueAuthoredText {
                raw_text: "GM 裁定".into(),
            }],
            source_evidence: vec![],
            normalization: NormalizationStatus::Executable, // lie
        };
        assert!(!rule.may_autofire());
    }

    #[test]
    fn objective_roundtrips_with_defaults() {
        let o = ObjectiveSpec {
            id: "obj.neutralize_athena".into(),
            mission_id: None,
            mandatory: true,
            success_when: PredicateExpr::ObjectiveStatus {
                id: "x".into(),
                status: super::super::ObjectiveStatus::Completed,
            },
            failure_when: None,
            score_effects: vec![],
            rewards: vec![],
            deadline: None,
            source_evidence: vec![],
        };
        let s = serde_json::to_string(&o).unwrap();
        let back: ObjectiveSpec = serde_json::from_str(&s).unwrap();
        assert_eq!(o, back);
    }

    #[test]
    fn chaos_tracker_carries_ability_ladder_rungs() {
        // The Vault Chaos budget is an escalating ability ladder, not a flat int.
        // Faithfully representing it means the tracker carries its rungs.
        let t = TrackerSpec {
            id: "tracker.chaos.springs_eternal".into(),
            kind: TrackerKind::Countdown,
            label: "Chaos".into(),
            start: 0,
            threshold: 0,
            anchor: None,
            at_threshold: vec![],
            visible_to_players: false,
            source_evidence: vec![],
            rungs: vec![
                TrackerRung {
                    cost: 2,
                    label: "Refresh".into(),
                    detail: "A mundane target's appearance is altered to look years younger".into(),
                },
                TrackerRung {
                    cost: 12,
                    label: "Return".into(),
                    detail: "A mundane target is fully under the Anomaly's influence".into(),
                },
            ],
        };
        assert_eq!(t.rungs.len(), 2);
        assert_eq!(t.rungs[0].cost, 2);
        assert_eq!(t.rungs[1].label, "Return");
    }

    #[test]
    fn empty_rungs_omitted_off_is_byte_identical() {
        // Additive field: empty rungs must be skipped on serialize so existing
        // trackers stay byte-identical to baseline.
        let t = TrackerSpec {
            id: "t".into(),
            kind: TrackerKind::Timer,
            label: String::new(),
            start: 0,
            threshold: 15,
            anchor: None,
            at_threshold: vec![],
            visible_to_players: false,
            source_evidence: vec![],
            rungs: vec![],
        };
        let json = serde_json::to_value(&t).unwrap();
        assert!(
            json.get("rungs").is_none(),
            "empty rungs must be skipped on serialize (additive == baseline)"
        );
        let back: TrackerSpec = serde_json::from_value(json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn timer_tracker_models_foxwell() {
        let t = TrackerSpec {
            id: "tracker.scavvs_timer".into(),
            kind: TrackerKind::Timer,
            label: "Foxwell 滞留".into(),
            start: 0,
            threshold: 15,
            anchor: Some(EventPattern::Entered("unit.foxwell_services".into())),
            at_threshold: vec![EffectExpr::Activate("encounter.scavvs".into())],
            visible_to_players: false,
            source_evidence: vec![],
            rungs: vec![],
        };
        assert_eq!(t.kind, TrackerKind::Timer);
        assert_eq!(t.threshold, 15);
    }
}
