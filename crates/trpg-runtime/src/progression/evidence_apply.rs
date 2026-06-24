//! EV-APPLY `witnessed_progression_apply_v1` (FINAL slice) — the ProgressionEngine
//! CONSUMES the AcceptedEvidence ledger.
//!
//! Everything before this slice was **shadow**: evidence flowed into an
//! [`EvidenceLedger`] (EV-2 exact projectors / EV-P4 capability binding / EV-P5
//! post-turn witness) but the engine never read it. EV-APPLY closes the loop —
//! WITHOUT touching the engine's pure evaluation core. The link is a single guard
//! predicate: an objective whose `success_when` is
//! [`trpg_model::adventure_ir::PredicateExpr::EvidencePresent`]`(atom)` completes
//! once that atom's [`trpg_model::adventure_ir::AcceptedEvidence`] is in the ledger.
//! [`apply_evidence_to_ctx`] projects the ledger into the guard-readable
//! [`EvalContext::present_evidence_atoms`] set the predicate consults; the existing
//! [`super::evaluate`] then fires `ObjectiveCompleted` exactly as it already does for
//! any other executable guard. **The engine produces the ProgressSignal — never the
//! LLM** (`LLM output ∩ ProgressSignal = ∅` holds: the LLM only ever proposed an
//! EvidenceClaim; Rust admitted it; Rust runs the guard).
//!
//! **nav-split**: this path emits `ObjectiveCompleted`/`SceneUnlocked` only and NEVER
//! mutates `current_scene` (`entered_locations`) — scene transitions stay the
//! NavigationResolver's player-driven job (design §七; no teleport/railroad).
//!
//! Flag-gated `witnessed_progression_apply_v1` (+ master `progress_evidence_v1`),
//! default OFF. When OFF the runtime never calls [`apply_evidence_to_ctx`], so
//! `present_evidence_atoms` stays empty and an EvidencePresent objective never
//! auto-completes — byte-identical to today's shadow behavior.
use std::env;
use trpg_model::adventure_ir::{EvalContext, EvidenceLedger};

const APPLY_ENV: &str = "TRPG_PROGRESS_WITNESSED_PROGRESSION_APPLY_V1";
const MASTER_ENV: &str = "TRPG_PROGRESS_EVIDENCE_V1";

fn flag_on(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "on" | "yes"
    )
}

/// Whether the engine consumes the EvidenceLedger this run. Default OFF → the engine
/// ignores the ledger exactly as today (shadow) == byte-identical baseline. Mirrors
/// the established `TRPG_PROGRESS_*_V1` flag pattern (own flag OR the master switch).
pub fn witnessed_progression_apply_enabled() -> bool {
    env::var(APPLY_ENV).map(|v| flag_on(&v)).unwrap_or(false)
        || env::var(MASTER_ENV).map(|v| flag_on(&v)).unwrap_or(false)
}

/// Project the turn's AcceptedEvidence ledger into the guard-readable set of admitted
/// atom ids the engine's `EvidencePresent`/`EvidenceAnyOf` predicates consult. Pure,
/// idempotent (a set). The runtime calls this ONLY when the apply flag is on, so OFF
/// leaves `ctx.present_evidence_atoms` empty == baseline.
pub fn apply_evidence_to_ctx(ctx: &mut EvalContext, ledger: &EvidenceLedger) {
    for ev in ledger.entries() {
        ctx.present_evidence_atoms
            .insert(ev.atom_id.as_str().to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progression::{compute_frontier, evaluate, ProgressionProgram, ProgressionState};
    use trpg_model::adventure_ir::{
        AcceptedEvidence, AtomId, EvidenceAuthority, EvidenceKind, ObjectiveSpec, ObjectiveStatus,
        PredicateExpr, ProgressSignalKind,
    };
    use trpg_model::SourceRef;

    /// A GuardLeaf "experiment" objective whose success hinges ONLY on its evidence
    /// atom (the EV-APPLY objective→atom link).
    fn experiment_objective(atom: &str) -> ObjectiveSpec {
        ObjectiveSpec {
            id: "obj.optional.0".into(),
            mission_id: None,
            mandatory: false,
            success_when: PredicateExpr::EvidencePresent {
                atom_id: atom.to_string(),
            },
            failure_when: None,
            score_effects: Vec::new(),
            rewards: Vec::new(),
            deadline: None,
            source_evidence: vec![SourceRef {
                source_id: "the_vault".into(),
                anchor_id: Some("packet".into()),
                note: Some("Conduct an experiment.".into()),
                ..Default::default()
            }],
        }
    }

    fn guard_leaf_evidence(atom: &str) -> AcceptedEvidence {
        AcceptedEvidence {
            evidence_id: "ev_1".into(),
            atom_id: AtomId(atom.to_string()),
            evidence_kind: EvidenceKind::ActionResolved,
            basis_event_ids: vec!["de_check_1".into()],
            source_refs: vec![SourceRef {
                source_id: "the_vault".into(),
                anchor_id: Some("packet".into()),
                ..Default::default()
            }],
            turn_id: "turn-1".into(),
            authority: EvidenceAuthority::GmWitnessed,
        }
    }

    fn program<'a>(objectives: &'a [ObjectiveSpec]) -> ProgressionProgram<'a> {
        ProgressionProgram {
            rules: &[],
            objectives,
            trackers: &[],
        }
    }

    #[test]
    fn evidence_in_ledger_fires_objective_completed() {
        // CL-APPLYa: an objective guarded by EvidencePresent(atom) + that atom's
        // AcceptedEvidence in the ledger ⇒ the engine fires ObjectiveCompleted.
        let atom = "atom:fbf98c1c";
        let objs = vec![experiment_objective(atom)];
        let mut ledger = EvidenceLedger::new();
        ledger.append(guard_leaf_evidence(atom));

        let mut state = ProgressionState::default();
        state.ctx.entered_locations.push("scene_001".into());
        apply_evidence_to_ctx(&mut state.ctx, &ledger);

        let signals = evaluate(&mut state, &[], &program(&objs));

        assert!(
            signals.iter().any(|s| s.kind == ProgressSignalKind::ObjectiveCompleted
                && s.id == "obj.optional.0"),
            "engine fires ObjectiveCompleted from admitted GuardLeaf evidence, got {signals:?}"
        );
        assert_eq!(
            state.objective_status("obj.optional.0"),
            ObjectiveStatus::Completed
        );
    }

    #[test]
    fn no_evidence_no_signal_fail_closed() {
        // CL-APPLYa: no AcceptedEvidence for the atom ⇒ no ObjectiveCompleted
        // (fail-closed: the objective stays open, never auto-completes on doubt).
        let objs = vec![experiment_objective("atom:fbf98c1c")];
        let mut state = ProgressionState::default();
        // ledger empty → apply nothing (this is also the OFF==baseline shape).
        apply_evidence_to_ctx(&mut state.ctx, &EvidenceLedger::new());

        let signals = evaluate(&mut state, &[], &program(&objs));
        assert!(
            !signals
                .iter()
                .any(|s| s.kind == ProgressSignalKind::ObjectiveCompleted),
            "no evidence ⇒ no ObjectiveCompleted, got {signals:?}"
        );
        assert_eq!(
            state.objective_status("obj.optional.0"),
            ObjectiveStatus::Inactive,
            "objective stays open"
        );
    }

    #[test]
    fn off_baseline_empty_atoms_never_completes_evidence_objective() {
        // OFF==baseline: when the apply flag is off the runtime never calls
        // apply_evidence_to_ctx, so present_evidence_atoms is empty EVEN IF a ledger
        // exists — an EvidencePresent objective behaves exactly like the shadow path
        // (never completes). This is the byte-identical baseline shape.
        let atom = "atom:fbf98c1c";
        let objs = vec![experiment_objective(atom)];
        let mut ledger = EvidenceLedger::new();
        ledger.append(guard_leaf_evidence(atom)); // ledger HAS the evidence …

        let mut state = ProgressionState::default();
        // … but OFF means we DO NOT apply it.
        let signals = evaluate(&mut state, &[], &program(&objs));
        assert!(
            signals.is_empty(),
            "OFF (ledger not applied) ⇒ no signals == baseline, got {signals:?}"
        );
        assert!(state.ctx.present_evidence_atoms.is_empty());
    }

    #[test]
    fn engine_never_mutates_current_scene_nav_split() {
        // CL-APPLYa nav-split: firing ObjectiveCompleted must NOT change the current
        // scene (entered_locations) — scene transitions stay the navigator's job.
        let atom = "atom:fbf98c1c";
        let objs = vec![experiment_objective(atom)];
        let mut ledger = EvidenceLedger::new();
        ledger.append(guard_leaf_evidence(atom));

        let mut state = ProgressionState::default();
        state.ctx.entered_locations.push("scene_001".into());
        apply_evidence_to_ctx(&mut state.ctx, &ledger);
        let before = state.ctx.entered_locations.clone();

        let signals = evaluate(&mut state, &[], &program(&objs));
        assert!(signals
            .iter()
            .any(|s| s.kind == ProgressSignalKind::ObjectiveCompleted));
        assert_eq!(
            state.ctx.entered_locations, before,
            "engine produced ObjectiveCompleted but never teleported the scene"
        );
    }

    #[test]
    fn completed_objective_leaves_the_open_frontier() {
        // CL-APPLYa/b: once completed via evidence, the objective is terminal and
        // leaves the open-objective frontier (the next mission beat can open).
        let atom = "atom:fbf98c1c";
        let objs = vec![experiment_objective(atom)];
        let mut ledger = EvidenceLedger::new();
        ledger.append(guard_leaf_evidence(atom));

        let mut state = ProgressionState::default();
        // before: open. (EvidencePresent is executable → the frontier lists it.)
        let open_before = compute_frontier(&state, &objs, &[]).open_objectives;
        assert_eq!(open_before, vec!["obj.optional.0".to_string()]);

        apply_evidence_to_ctx(&mut state.ctx, &ledger);
        evaluate(&mut state, &[], &program(&objs));

        let open_after = compute_frontier(&state, &objs, &[]).open_objectives;
        assert!(
            open_after.is_empty(),
            "completed objective leaves the open frontier, still: {open_after:?}"
        );
    }

    #[test]
    fn prep_packet_guard_leaf_evidence_completes_its_objective_end_to_end() {
        // CL-APPLYb (deterministic-live shape): the FULL Wall C chain on a the_vault-
        // shaped prep-packet — compile the authored objective leaf "Conduct an
        // experiment." into (a) its GuardLeaf atom and (b) the evidence-objective whose
        // `success_when = EvidencePresent(<that SAME atom>)`. Injecting the atom's
        // AcceptedEvidence into the ledger ⇒ the engine fires ObjectiveCompleted and the
        // objective leaves the open frontier. This proves the compiler's atom_id MATCHES
        // the ledger evidence (the integration the split unit tests don't cover) — the
        // OpaqueAuthoredText objective completes THROUGH typed evidence, never prose.
        use serde_json::json;
        use trpg_model::adventure_ir::{
            compile_prep_packet_guard_leaves, evidence_objectives_from_prep_packet,
        };
        use trpg_model::{ModuleGraph, ScenarioNode};

        let graph = ModuleGraph {
            module_id: "the_vault".into(),
            npcs: vec![json!({"id": "npc_anomaly", "name": "The Springs Eternal Anomaly"})],
            scenes: vec![ScenarioNode {
                node_id: "scene_001".into(),
                node_type: "scene".into(),
                title: "Springs Eternal".into(),
                page_start: Some(8),
                ..Default::default()
            }],
            ..Default::default()
        };
        let packet = json!({
            "mission_briefing": {
                "optional_objectives": [{"objective": "Conduct an experiment.", "reward": "+3 Commendations"}]
            },
            "anomaly_gm_only": {"name": "The Springs Eternal Anomaly"},
            "first_investigation_area": {"location": "Mercantile Avenue"}
        });

        // (a) the GuardLeaf atom the EV-P4 compiler emits for the authored leaf …
        let atom = compile_prep_packet_guard_leaves(&graph, &packet)
            .into_iter()
            .next()
            .expect("authored objective leaf compiles to a GuardLeaf atom");
        // (b) … and the objective wired to complete via THAT atom's evidence.
        let objs = evidence_objectives_from_prep_packet(&graph, &packet);
        assert_eq!(objs.len(), 1, "one evidence-backed objective from the leaf");
        let obj_id = objs[0].id.clone();

        // Inject the EV-P5-style admitted GuardLeaf evidence for the compiled atom id.
        let mut ledger = EvidenceLedger::new();
        ledger.append(guard_leaf_evidence(atom.atom_id.as_str()));

        let mut state = ProgressionState::default();
        state.ctx.entered_locations.push("scene_001".into());

        // before: the evidence-objective is open (EvidencePresent is executable).
        let open_before = compute_frontier(&state, &objs, &[]).open_objectives;
        assert_eq!(open_before, vec![obj_id.clone()]);

        apply_evidence_to_ctx(&mut state.ctx, &ledger);
        let before_scene = state.ctx.entered_locations.clone();
        let signals = evaluate(&mut state, &[], &program(&objs));

        assert!(
            signals
                .iter()
                .any(|s| s.kind == ProgressSignalKind::ObjectiveCompleted && s.id == obj_id),
            "the prep-packet objective completes through its compiled GuardLeaf atom, got {signals:?}"
        );
        assert_eq!(state.objective_status(&obj_id), ObjectiveStatus::Completed);
        // frontier advances: completed objective leaves the open set.
        assert!(compute_frontier(&state, &objs, &[])
            .open_objectives
            .is_empty());
        // nav-split: the engine never teleported the scene.
        assert_eq!(state.ctx.entered_locations, before_scene);
    }

    #[test]
    fn flag_defaults_off() {
        // The env may be set by a concurrent test; assert the parse, not the ambient
        // value: an unset/empty var is OFF.
        assert!(!flag_on(""));
        assert!(!flag_on("0"));
        assert!(flag_on("1"));
        assert!(flag_on("on"));
    }
}
