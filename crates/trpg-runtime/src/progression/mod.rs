//! Adventure IR runtime — Progression Engine (P1-3): the missing runtime PROGRESS
//! dimension that is the proven J3 stall root cause (producer-only补边 left J3
//! transitions 1→0). The engine consumes typed events, deterministically advances
//! objectives/trackers/units via authored ECA rules (Rust evaluates guards), emits
//! [`trpg_model::adventure_ir::ProgressSignal`]s, and computes the
//! [`AdvancementFrontier`] the Director selects focus from.
//!
//! Flag-gated by `TRPG_PROGRESSION_ENGINE` (default OFF). The engine module is
//! additive and not yet wired into the live navigator — the Director-consumer wiring
//! (evolving `scene_navigation/flow_links.rs`) and the real-runtime smoke land in a
//! relay-free pass; until then OFF==baseline is byte-identical (nothing calls it).

mod adapter;
mod engine;
mod frontier;
mod program;
mod spine;
mod state;

pub use adapter::{progress_events_from_domain, replay_domain_events};
pub use engine::{evaluate, ProgressionProgram};
pub use frontier::{compute_frontier, AdvancementFrontier};
pub use program::{
    derive_threat_objective, program_from_module_graph, ObjectiveDerivation, OwnedProgram,
};
pub use spine::augment_program_with_spine;
pub use state::{ProgressEvent, ProgressionState};

/// Whether the runtime ProgressionEngine is active. Default OFF → today's behavior
/// (byte-identical baseline). Mirrors the established TRPG_* flag pattern.
pub fn progression_engine_enabled() -> bool {
    std::env::var("TRPG_PROGRESSION_ENGINE")
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "on" | "yes"))
        .unwrap_or(false)
}

#[cfg(test)]
mod golden_tests {
    use super::*;
    use trpg_model::adventure_ir::{
        EffectExpr, EventPattern, IrValue, NormalizationStatus, ObjectiveSpec, ObjectiveStatus,
        PredicateExpr, ProgressRule, ProgressSignalKind, TrackerKind, TrackerSpec,
    };

    fn rule(id: &str, on: EventPattern, when: PredicateExpr, then: Vec<EffectExpr>) -> ProgressRule {
        let normalization = ProgressRule::classify(&when, &then);
        ProgressRule {
            id: id.into(),
            on,
            when,
            then,
            source_evidence: vec![],
            normalization,
        }
    }

    /// GOLDEN 1: 中和/控制/摧毁 Athena → Objective.NeutralizeAthena completed
    /// (ON WorldFactChanged → THEN Complete). And the frontier no longer lists it.
    #[test]
    fn neutralize_athena_completes_via_rule() {
        let mut state = ProgressionState::default();
        let athena_done = PredicateExpr::Any(vec![
            PredicateExpr::FactEquals {
                fact: "athena.status".into(),
                value: IrValue::Text("disabled".into()),
            },
            PredicateExpr::FactEquals {
                fact: "athena.status".into(),
                value: IrValue::Text("controlled".into()),
            },
            PredicateExpr::FactEquals {
                fact: "athena.status".into(),
                value: IrValue::Text("destroyed".into()),
            },
        ]);
        let rules = vec![rule(
            "rule.neutralize_athena",
            EventPattern::WorldFactChanged,
            athena_done.clone(),
            vec![
                EffectExpr::Complete("obj.neutralize_athena".into()),
                EffectExpr::Activate("beat.athena_message".into()),
            ],
        )];
        let objectives = vec![ObjectiveSpec {
            id: "obj.neutralize_athena".into(),
            mission_id: None,
            mandatory: true,
            success_when: athena_done,
            failure_when: None,
            score_effects: vec![],
            rewards: vec![],
            deadline: None,
            source_evidence: vec![],
        }];
        let program = ProgressionProgram {
            rules: &rules,
            objectives: &objectives,
            trackers: &[],
        };

        let events = vec![ProgressEvent::WorldFactChanged {
            fact: "athena.status".into(),
            value: IrValue::Text("disabled".into()),
        }];
        let signals = evaluate(&mut state, &events, &program);

        assert_eq!(
            state.objective_status("obj.neutralize_athena"),
            ObjectiveStatus::Completed
        );
        assert!(state.active_units.contains("beat.athena_message"));
        assert!(signals
            .iter()
            .any(|s| s.kind == ProgressSignalKind::ObjectiveCompleted
                && s.id == "obj.neutralize_athena"));
        // Frontier: completed objective drops out; the new beat is the legal next.
        let f = compute_frontier(&state, &objectives, &[]);
        assert!(f.active_units.contains(&"beat.athena_message".to_string()));
        assert!(!f.open_objectives.contains(&"obj.neutralize_athena".to_string()));
    }

    /// GOLDEN 2: 进入 Foxwell 后经过 15min(world time) → ScavvsEncounter active
    /// (Timer anchored on Entered, ON TimeAdvanced + threshold). Before 15min:
    /// frontier has no scavvs; after: it does.
    #[test]
    fn foxwell_timer_activates_scavvs_after_15min() {
        let mut state = ProgressionState::default();
        let tracker = TrackerSpec {
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
        let trackers = vec![tracker];
        let program = ProgressionProgram {
            rules: &[],
            objectives: &[],
            trackers: &trackers,
        };

        // Enter Foxwell, then only 10 minutes pass → not yet.
        evaluate(
            &mut state,
            &[ProgressEvent::Entered("unit.foxwell_services".into())],
            &program,
        );
        evaluate(
            &mut state,
            &[ProgressEvent::TimeAdvanced { minutes: 10 }],
            &program,
        );
        assert!(!state.active_units.contains("encounter.scavvs"), "10<15");
        let f_early = compute_frontier(&state, &[], &trackers);
        assert!(!f_early.active_units.contains(&"encounter.scavvs".to_string()));

        // 6 more minutes → 16 >= 15 → Scavvs fires exactly once.
        let signals = evaluate(
            &mut state,
            &[ProgressEvent::TimeAdvanced { minutes: 6 }],
            &program,
        );
        assert!(state.active_units.contains("encounter.scavvs"));
        assert!(signals
            .iter()
            .any(|s| s.kind == ProgressSignalKind::BeatActivated && s.id == "encounter.scavvs"));
        let f_late = compute_frontier(&state, &[], &trackers);
        assert!(f_late.active_units.contains(&"encounter.scavvs".to_string()));
        assert!(f_late.due_trackers.contains(&"tracker.scavvs_timer".to_string()));

        // Idempotent: another tick does not re-fire.
        let again = evaluate(
            &mut state,
            &[ProgressEvent::TimeAdvanced { minutes: 5 }],
            &program,
        );
        assert!(!again
            .iter()
            .any(|s| s.kind == ProgressSignalKind::BeatActivated && s.id == "encounter.scavvs"));
    }

    /// GOLDEN 3: 选择"帮 Quil" → Beat.EscapeCondo active 且 Outcome.AcceptHisako
    /// disabled (ON ChoiceRecorded + Branch).
    #[test]
    fn side_with_quil_branches_correctly() {
        let mut state = ProgressionState::default();
        // Pre-existing outcome that the branch must foreclose.
        state.active_units.insert("outcome.accept_hisako".into());
        let rules = vec![rule(
            "rule.quil_branch",
            EventPattern::ChoiceRecorded,
            PredicateExpr::ChoiceMade {
                key: "quil_vs_hisako".into(),
                value: "side_with_quil".into(),
            },
            vec![
                EffectExpr::Activate("beat.escape_condo".into()),
                EffectExpr::Disable("outcome.accept_hisako".into()),
            ],
        )];
        let program = ProgressionProgram {
            rules: &rules,
            objectives: &[],
            trackers: &[],
        };
        evaluate(
            &mut state,
            &[ProgressEvent::ChoiceRecorded {
                key: "quil_vs_hisako".into(),
                value: "side_with_quil".into(),
            }],
            &program,
        );
        assert!(state.active_units.contains("beat.escape_condo"));
        assert!(state.disabled_units.contains("outcome.accept_hisako"));
        // Frontier must NOT offer the foreclosed branch.
        let f = compute_frontier(&state, &[], &[]);
        assert!(!f.active_units.contains(&"outcome.accept_hisako".to_string()));
        assert!(f.active_units.contains(&"beat.escape_condo".to_string()));
    }

    /// Fail-closed: an opaque-guarded rule never auto-fires even on event match.
    #[test]
    fn opaque_rule_never_fires() {
        let mut state = ProgressionState::default();
        let r = ProgressRule {
            id: "rule.opaque".into(),
            on: EventPattern::ChoiceRecorded,
            when: PredicateExpr::OpaqueAuthoredText {
                raw_text: "若玩家用巧妙方式".into(),
            },
            then: vec![EffectExpr::Activate("beat.secret".into())],
            source_evidence: vec![],
            normalization: NormalizationStatus::Opaque,
        };
        let rules = vec![r];
        let program = ProgressionProgram {
            rules: &rules,
            objectives: &[],
            trackers: &[],
        };
        let signals = evaluate(
            &mut state,
            &[ProgressEvent::ChoiceRecorded {
                key: "k".into(),
                value: "v".into(),
            }],
            &program,
        );
        assert!(!state.active_units.contains("beat.secret"));
        assert!(signals.is_empty());
    }

    /// J3-fix invariant: an objective completed in place (no LocationChanged) still
    /// produces a semantic ProgressSignal — the exact case J3 v1 misjudged.
    #[test]
    fn in_place_objective_progress_is_semantic() {
        let mut state = ProgressionState::default();
        let objectives = vec![ObjectiveSpec {
            id: "obj.charge_athena".into(),
            mission_id: None,
            mandatory: false,
            success_when: PredicateExpr::FactEquals {
                fact: "athena.charged".into(),
                value: IrValue::Bool(true),
            },
            failure_when: None,
            score_effects: vec![],
            rewards: vec![],
            deadline: None,
            source_evidence: vec![],
        }];
        let program = ProgressionProgram {
            rules: &[],
            objectives: &objectives,
            trackers: &[],
        };
        let signals = evaluate(
            &mut state,
            &[ProgressEvent::WorldFactChanged {
                fact: "athena.charged".into(),
                value: IrValue::Bool(true),
            }],
            &program,
        );
        // No location changed, but semantic progress IS recorded.
        assert!(!signals
            .iter()
            .any(|s| s.kind == ProgressSignalKind::LocationChanged));
        let sem = signals
            .iter()
            .find(|s| s.kind == ProgressSignalKind::ObjectiveCompleted)
            .expect("semantic objective progress emitted");
        assert_eq!(sem.kind.j3_axis(), trpg_model::adventure_ir::J3Axis::Semantic);
    }

    #[test]
    fn flag_defaults_off() {
        // Without the env var, the engine is gated off (baseline behavior).
        std::env::remove_var("TRPG_PROGRESSION_ENGINE");
        assert!(!progression_engine_enabled());
    }
}
