//! D2 `progress_scene_advance_evidence_v1` (Part B: consume) — the
//! ProgressionEngine consumes the AcceptedEvidence ledger against the player's
//! CURRENT scene's evidence-backed advance objective and emits a durable
//! `ObjectiveResolved` when a real admitted authored observation of that scene lands.
//!
//! This is the scene-content analogue of EV-APPLY's
//! [`super::witnessed_objective_resolutions`] (the prep-packet path). It shares the
//! SAME engine ([`super::evaluate`]) and the SAME ledger projection
//! ([`super::apply_evidence_to_ctx`]) — NO parallel progression machinery — but:
//! - it derives its objective from scene-salient authored observations
//!   ([`trpg_model::adventure_ir::scene_advance_objective`]) instead of prep-packet
//!   mission objectives, so it fires on published modules (homecoming) that have no
//!   prep-packet objectives; and
//! - it labels the durable event `"signal":"scene_advance"` and uses the distinct
//!   `obj.scene_advance.*` id namespace, so a thin scene-engagement advance is never
//!   confused with a mission-objective resolution (codex G).
//!
//! Discipline (codex-validated):
//! - **standalone flag** `TRPG_PROGRESS_SCENE_ADVANCE_EVIDENCE_V1` (NOT ORed into the
//!   master) → with it OFF the runtime never calls this, byte-identical to baseline
//!   even when the evidence master is on.
//! - **scene-scoped** — builds ONLY the current scene's advance objective (a blank
//!   current scene ⇒ nothing), so we never resolve a scene the player is not in
//!   (`mission_id` alone does NOT gate engine completion — codex H1).
//! - **fail-closed** — no current scene / no salient atom / no admitted evidence ⇒
//!   ZERO events (never fabricate an advance).
//! - **nav-split** — emits `ObjectiveResolved` only; never mutates `current_scene`.
//! - **`LLM ∩ ProgressSignal = ∅`** — the LLM only proposed atoms; the engine here
//!   runs the guard and produces the signal.

use std::env;
use trpg_model::adventure_ir::{scene_advance_objective, EvidenceLedger, PredicateExpr, ProgressSignalKind};
use trpg_model::{DomainEvent, DomainEventKind, ModuleGraph};

const SCENE_ADV_ENV: &str = "TRPG_PROGRESS_SCENE_ADVANCE_EVIDENCE_V1";

fn flag_on(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "on" | "yes"
    )
}

/// Whether the engine consumes the ledger for scene-advance objectives this run.
/// Default OFF and **standalone** (no master OR) so OFF == byte-identical to 3f92013
/// even when the evidence master (`TRPG_PROGRESS_EVIDENCE_V1`) is on.
pub fn scene_advance_evidence_enabled() -> bool {
    env::var(SCENE_ADV_ENV).map(|v| flag_on(&v)).unwrap_or(false)
}

/// Run the engine over the player's CURRENT scene's evidence-backed advance objective
/// and return the `ObjectiveResolved` events for it (≤1). The objective's guard is
/// `EvidenceAnyOf({the scene's authored observation atom_ids})`; it completes when any
/// of those atoms is in the admitted ledger — the SAME atom ids the live offer/witness
/// pipeline admits (both derive from `compile_authored_observations`). Mirrors
/// [`super::witnessed_objective_resolutions`] (shared engine core) but is scene-derived
/// and distinctly labeled. Performs no IO; the caller persists + dedups on `event_id`.
pub fn witnessed_scene_advance_resolutions(
    session_id: &str,
    turn_id: &str,
    graph: &ModuleGraph,
    ledger: &EvidenceLedger,
    current_scene: &str,
) -> Vec<DomainEvent> {
    let scene = current_scene.trim();
    if scene.is_empty() {
        return Vec::new(); // fail-closed: no current scene ⇒ no scene-advance
    }
    // scene-scoped: ONLY the current scene's objective (mission_id does not gate
    // engine completion). Fail-closed when the scene has no salient observation atom.
    let Some(objective) = scene_advance_objective(graph, scene) else {
        return Vec::new();
    };
    let objectives = [objective];

    let mut state = super::ProgressionState::default();
    // seed the current scene ONLY so the scene-scoped objective surfaces; the engine
    // reads it, never writes it (nav-split).
    state.ctx.entered_locations.push(scene.to_string());
    super::apply_evidence_to_ctx(&mut state.ctx, ledger);

    let program = super::ProgressionProgram {
        rules: &[],
        objectives: &objectives,
        trackers: &[],
    };
    let signals = super::evaluate(&mut state, &[], &program);

    signals
        .iter()
        .filter(|s| s.kind == ProgressSignalKind::ObjectiveCompleted)
        .map(|s| {
            // the matched atom: the first guard atom actually in the ledger (audit /
            // provenance — codex H2). The guard is only True because an atom is present.
            let matched_atom = objectives
                .iter()
                .find(|o| o.id == s.id)
                .and_then(|o| match &o.success_when {
                    PredicateExpr::EvidenceAnyOf { atom_ids } => atom_ids
                        .iter()
                        .find(|a| state.ctx.present_evidence_atoms.contains(*a))
                        .cloned(),
                    PredicateExpr::EvidencePresent { atom_id } => Some(atom_id.clone()),
                    _ => None,
                });
            let data = serde_json::json!({
                "objective_id": s.id,
                "atom_id": matched_atom,
                "scene_id": scene,
                "signal": "scene_advance",
            });
            DomainEvent::new(
                format!("de_objresolved_{session_id}_{}", s.id),
                session_id.to_string(),
                turn_id.to_string(),
                DomainEventKind::ObjectiveResolved,
                data,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::adventure_ir::{
        scene_advance_guard_leaves, AcceptedEvidence, AtomId, EvidenceAuthority, EvidenceKind,
    };
    use trpg_model::{DirectorAffordanceItem, DirectorModuleConfig, ScenarioNode, SourceRef};

    /// A graph whose entry scene carries one resolvable affordance observation,
    /// tagged scene:scene_001 (same fixture shape as the producer's tests).
    fn graph_with_scene_affordance() -> ModuleGraph {
        ModuleGraph {
            module_id: "the_vault".into(),
            clues: vec![serde_json::json!({"id": "clue_aquifer_commercial", "name": "Aquifer"})],
            director_facilitation: Some(DirectorModuleConfig {
                affordance_items: vec![DirectorAffordanceItem {
                    description: "Investigate the Aquifer commercial.".into(),
                    implies_vectors: vec!["Aquifer".into()],
                }],
                ..Default::default()
            }),
            scenes: vec![ScenarioNode {
                node_id: "scene_001".into(),
                node_type: "scene".into(),
                title: "Springs Eternal".into(),
                page_start: Some(8),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    /// AcceptedEvidence admitting `atom` (as the witness/exact path would).
    fn admitted(atom: &str) -> AcceptedEvidence {
        AcceptedEvidence {
            evidence_id: format!("ev_{atom}"),
            atom_id: AtomId(atom.to_string()),
            evidence_kind: EvidenceKind::ActionResolved,
            basis_event_ids: vec!["de_check_1".into()],
            source_refs: vec![SourceRef {
                source_id: "the_vault".into(),
                ..Default::default()
            }],
            turn_id: "turn-1".into(),
            authority: EvidenceAuthority::GmWitnessed,
        }
    }

    #[test]
    fn flag_defaults_off_and_is_standalone() {
        assert!(!flag_on(""));
        assert!(!flag_on("0"));
        assert!(flag_on("1"));
        assert!(flag_on("on"));
    }

    #[test]
    fn off_baseline_gate_is_false_when_env_unset() {
        // OFF==baseline (mirror evidence_apply.rs `off_baseline_*`): with the standalone
        // flag unset, the gate the turn loop checks (`if scene_adv_on`) is false, so the
        // runtime NEVER builds/evaluates a scene-advance objective ⇒ no event ⇒
        // byte-identical to 3f92013. No test in this binary sets the var (it reads it
        // directly), and the flag is NOT ORed into the evidence master, so even master-ON
        // leaves this off. The system-level OFF arm is the supervisor's 30-turn A/B.
        assert!(
            !scene_advance_evidence_enabled(),
            "default OFF + standalone ⇒ runtime never calls the D2 consume ⇒ baseline"
        );
    }

    #[test]
    fn admitted_scene_atom_fires_one_scene_advance_objectiveresolved() {
        let g = graph_with_scene_affordance();
        let atom = scene_advance_guard_leaves(&g, "scene_001")[0]
            .atom_id
            .as_str()
            .to_string();
        let mut ledger = EvidenceLedger::new();
        ledger.append(admitted(&atom));

        let events =
            witnessed_scene_advance_resolutions("sess-1", "turn-7", &g, &ledger, "scene_001");
        assert_eq!(events.len(), 1, "one ObjectiveResolved for the current scene");
        let ev = &events[0];
        assert_eq!(ev.kind, DomainEventKind::ObjectiveResolved);
        assert_eq!(ev.event_id, "de_objresolved_sess-1_obj.scene_advance.scene_001");
        assert_eq!(ev.data["objective_id"], "obj.scene_advance.scene_001");
        assert_eq!(ev.data["atom_id"], atom, "matched atom recorded (audit)");
        assert_eq!(ev.data["scene_id"], "scene_001");
        assert_eq!(ev.data["signal"], "scene_advance", "distinctly labeled");
    }

    #[test]
    fn empty_ledger_fails_closed_no_events() {
        let g = graph_with_scene_affordance();
        let events = witnessed_scene_advance_resolutions(
            "sess-1",
            "turn-7",
            &g,
            &EvidenceLedger::new(),
            "scene_001",
        );
        assert!(events.is_empty(), "no admitted atom ⇒ no advance, got {events:?}");
    }

    #[test]
    fn scene_with_no_atom_is_noop() {
        let g = ModuleGraph {
            module_id: "the_vault".into(),
            scenes: vec![ScenarioNode {
                node_id: "scene_barren".into(),
                node_type: "scene".into(),
                title: "Empty".into(),
                page_start: Some(2),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut ledger = EvidenceLedger::new();
        ledger.append(admitted("atom:whatever"));
        assert!(
            witnessed_scene_advance_resolutions("s", "t", &g, &ledger, "scene_barren").is_empty()
        );
    }

    #[test]
    fn blank_current_scene_is_noop() {
        let g = graph_with_scene_affordance();
        let atom = scene_advance_guard_leaves(&g, "scene_001")[0]
            .atom_id
            .as_str()
            .to_string();
        let mut ledger = EvidenceLedger::new();
        ledger.append(admitted(&atom));
        assert!(witnessed_scene_advance_resolutions("s", "t", &g, &ledger, "   ").is_empty());
    }

    #[test]
    fn only_the_current_scene_resolves_not_other_scenes() {
        // codex H1: even if another scene's atom is admitted, only the CURRENT scene's
        // advance objective is built/evaluated — we never resolve a scene the player
        // is not in. Here scene_001's atom is admitted but current_scene is a DIFFERENT
        // scene with no admitted atom of its own ⇒ no resolution.
        let g = graph_with_scene_affordance();
        let scene1_atom = scene_advance_guard_leaves(&g, "scene_001")[0]
            .atom_id
            .as_str()
            .to_string();
        let mut ledger = EvidenceLedger::new();
        ledger.append(admitted(&scene1_atom));
        // ask for a scene the player is NOT in (scene_001's atom is admitted, but we
        // pass scene_999 which has no objective) ⇒ nothing.
        assert!(
            witnessed_scene_advance_resolutions("s", "t", &g, &ledger, "scene_999").is_empty(),
            "only the current scene's objective is considered"
        );
    }

    #[test]
    fn event_id_is_idempotent_turn_independent() {
        let g = graph_with_scene_affordance();
        let atom = scene_advance_guard_leaves(&g, "scene_001")[0]
            .atom_id
            .as_str()
            .to_string();
        let mut ledger = EvidenceLedger::new();
        ledger.append(admitted(&atom));
        let a = witnessed_scene_advance_resolutions("sess-1", "turn-3", &g, &ledger, "scene_001");
        let b = witnessed_scene_advance_resolutions("sess-1", "turn-9", &g, &ledger, "scene_001");
        assert_eq!(a[0].event_id, b[0].event_id, "turn-independent id ⇒ dedups across turns");
    }
}
