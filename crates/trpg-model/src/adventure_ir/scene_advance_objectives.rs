//! D2 `progress_scene_advance_evidence_v1` (Part A: producer) — **scene-advance
//! evidence objectives from scene-salient authored observations**.
//!
//! Wall B (the J3 freeze on published modules): the durable progression signal
//! (`ObjectiveResolved`) is only produced for prep-packet mission objectives
//! ([`super::evidence_objectives_from_prep_packet`]). homecoming has none, so it
//! freezes — even though its scene-content observation atoms ARE compiled (EV-P3
//! [`super::compile_authored_observations`], scene-tagged), ARE offered to the LLM
//! PostTurnWitness when the player is in that scene, and ARE admitted to the
//! AcceptedEvidence ledger by the LLM-sensor route. The gap is purely consume-side:
//! **no objective references those admitted atom_ids**, so the engine completes
//! nothing.
//!
//! This module closes that gap WITHOUT a new matcher and WITHOUT a parallel
//! progression path: for the player's current scene it re-expresses that scene's
//! salient authored observations as **GuardLeaf** atoms ([D2-1]) and binds them to a
//! single scene-advance objective whose `success_when` is
//! [`PredicateExpr::EvidenceAnyOf`]`({those atom_ids})` ([D2-2]). The atom_ids are the
//! EXACT ids [`super::compile_authored_observations`] emits (role is excluded from
//! [`super::AtomId`]), so the ids this objective consults are precisely the ones the
//! live offer/witness pipeline admits — the engine then completes the objective via
//! the already-proven EV-APPLY consume ([`super::EvidencePresent`]/`EvidenceAnyOf`).
//!
//! Design law honored (codex-validated):
//! - **zero hardcoded semantic matcher** — the fact↔atom match is the LLM-sensor's
//!   (opaque cap → committed-check admission); Rust only does exact `atom_id` set
//!   membership. The EV-P3 verb lexicon is a compile-time prose→atom parser, not a
//!   runtime predicate matcher; this module does not touch it.
//! - **`LLM ∩ ProgressSignal = ∅`** — the LLM proposes only atoms; only the engine
//!   (via these objectives) produces a ProgressSignal.
//! - **fail-closed** — a scene with no salient observation atom yields NO objective;
//!   an empty atom set makes `EvidenceAnyOf` `False` (never match-all).
//! - **NO ruleset/module name branching** — purely scene-structural (`scene:<id>`).
//! - **scene-scoped** — the caller asks for ONE scene's objective at a time (the
//!   player's current scene); `mission_id` only filters frontier display, not engine
//!   completion, so we never resolve a scene the player is not engaging.

use crate::adventure_ir::authored_observation::tag_scene_prefix;
use crate::adventure_ir::{
    compile_authored_observations, EvidenceAtomSpec, ObjectiveSpec, PredicateExpr, ProgressRole,
};
use crate::{ModuleGraph, SourceRef};

/// The objective-id namespace for a scene's evidence-backed advance objective.
/// DISTINCT from the spine display objective `obj.advance.{scene}` (codex G): the
/// durable `ObjectiveResolved` is unambiguously a scene-advance progression *signal*,
/// never confused with a mission-objective resolution.
pub const SCENE_ADVANCE_PREFIX: &str = "obj.scene_advance.";

/// The salient authored observation atoms for `scene_id`, re-roled **GuardLeaf**
/// (D2-1). These are exactly [`compile_authored_observations`]'s scene-tagged atoms
/// (same `atom_id`; role is not in the hash) — the ones the live catalog offers and
/// the witness admits. Source-grounded by construction (EV-P3 drops source-less
/// atoms). Returns the atoms tagged `scene:<scene_id>`; empty if the scene has none.
pub fn scene_advance_guard_leaves(graph: &ModuleGraph, scene_id: &str) -> Vec<EvidenceAtomSpec> {
    let scene = scene_id.trim();
    if scene.is_empty() {
        return Vec::new();
    }
    let tag = format!("{}{scene}", tag_scene_prefix());
    compile_authored_observations(graph)
        .into_iter()
        .filter(|a| a.bindings.iter().any(|b| b == &tag))
        .map(|mut a| {
            a.progress_role = ProgressRole::GuardLeaf; // caller-added GuardLeaf (EV-P3 convention)
            a
        })
        .collect()
}

/// The evidence-backed advance objective for `scene_id` (D2-2), or `None`
/// (fail-closed) if the scene has no salient authored observation atom. The
/// objective's `success_when = EvidenceAnyOf({the scene's GuardLeaf atom_ids})`, so a
/// single admitted authored observation of THIS scene completes it via the EV-APPLY
/// engine consume. Scene-scoped (`mission_id = Some(scene)`), source-grounded.
pub fn scene_advance_objective(graph: &ModuleGraph, scene_id: &str) -> Option<ObjectiveSpec> {
    let scene = scene_id.trim();
    let leaves = scene_advance_guard_leaves(graph, scene);
    if leaves.is_empty() {
        return None; // fail-closed: never invent a goal without source-grounded atoms
    }
    let mut atom_ids: Vec<String> = leaves
        .iter()
        .map(|a| a.atom_id.as_str().to_string())
        .collect();
    atom_ids.sort();
    atom_ids.dedup();
    let source = SourceRef {
        source_id: graph.module_id.clone(),
        anchor_id: Some(format!("scene_advance:{scene}")),
        note: Some(format!(
            "scene-advance evidence objective (any salient authored observation of {scene})"
        )),
        ..Default::default()
    };
    Some(ObjectiveSpec {
        id: format!("{SCENE_ADVANCE_PREFIX}{scene}"),
        mission_id: Some(scene.to_string()),
        mandatory: false,
        success_when: PredicateExpr::EvidenceAnyOf { atom_ids },
        failure_when: None,
        score_effects: Vec::new(),
        rewards: Vec::new(),
        deadline: None,
        source_evidence: vec![source],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adventure_ir::EvidenceKind;
    use crate::{DirectorAffordanceItem, DirectorModuleConfig, ScenarioNode};
    use serde_json::json;

    /// A graph whose entry scene carries one resolvable affordance observation
    /// ("Investigate the Aquifer commercial." → action atom bound to clue_aquifer,
    /// tagged scene:scene_001).
    fn graph_with_scene_affordance() -> ModuleGraph {
        ModuleGraph {
            module_id: "the_vault".into(),
            clues: vec![json!({"id": "clue_aquifer_commercial", "name": "Aquifer"})],
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

    #[test]
    fn guard_leaves_reuse_compile_authored_observations_atom_ids_as_guard_leaf() {
        // D2-1: the scene's guard-leaf atoms are EXACTLY EV-P3's scene-tagged atoms
        // (same atom_id — role is not in the hash) but re-roled GuardLeaf.
        let g = graph_with_scene_affordance();
        let ev_p3: Vec<_> = compile_authored_observations(&g)
            .into_iter()
            .filter(|a| a.bindings.iter().any(|b| b == "scene:scene_001"))
            .collect();
        assert!(!ev_p3.is_empty(), "EV-P3 produces a scene_001-tagged atom");

        let leaves = scene_advance_guard_leaves(&g, "scene_001");
        assert_eq!(leaves.len(), ev_p3.len(), "same scene atoms, re-roled");
        for l in &leaves {
            assert_eq!(
                l.progress_role,
                ProgressRole::GuardLeaf,
                "re-roled GuardLeaf"
            );
            assert!(
                ev_p3.iter().any(|a| a.atom_id == l.atom_id),
                "atom_id matches the live-admitted EV-P3 atom: {}",
                l.atom_id.as_str()
            );
            assert!(!l.source_refs.is_empty(), "source-grounded");
        }
    }

    #[test]
    fn scene_advance_objective_anyof_contains_the_scene_atom_ids() {
        // D2-2: the objective's EvidenceAnyOf consults the SAME atom_ids the live
        // pipeline admits, so an admitted scene atom completes it via EV-APPLY.
        let g = graph_with_scene_affordance();
        let leaves = scene_advance_guard_leaves(&g, "scene_001");
        let obj = scene_advance_objective(&g, "scene_001").expect("scene has an atom ⇒ objective");

        assert_eq!(
            obj.id, "obj.scene_advance.scene_001",
            "distinct id namespace"
        );
        assert_eq!(obj.mission_id.as_deref(), Some("scene_001"), "scene-scoped");
        assert!(!obj.source_evidence.is_empty(), "source-grounded");
        match &obj.success_when {
            PredicateExpr::EvidenceAnyOf { atom_ids } => {
                for l in &leaves {
                    assert!(
                        atom_ids.iter().any(|a| a == l.atom_id.as_str()),
                        "AnyOf includes the scene's admitted atom id"
                    );
                }
                assert!(!atom_ids.is_empty(), "non-empty (else match-all-False)");
            }
            other => panic!("expected EvidenceAnyOf, got {other:?}"),
        }
        assert!(
            obj.success_when.is_executable(),
            "evidence guard is executable"
        );
    }

    #[test]
    fn scene_with_no_salient_observation_yields_no_objective_fail_closed() {
        // A scene with no affordance/mechanic/gm_notes atom ⇒ no objective (never invent).
        let g = ModuleGraph {
            module_id: "the_vault".into(),
            scenes: vec![ScenarioNode {
                node_id: "scene_barren".into(),
                node_type: "scene".into(),
                title: "Empty Room".into(),
                page_start: Some(2),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(scene_advance_guard_leaves(&g, "scene_barren").is_empty());
        assert!(
            scene_advance_objective(&g, "scene_barren").is_none(),
            "fail-closed: no source-grounded atom ⇒ no advance objective"
        );
    }

    #[test]
    fn blank_or_unknown_scene_yields_nothing() {
        let g = graph_with_scene_affordance();
        assert!(scene_advance_guard_leaves(&g, "   ").is_empty());
        assert!(scene_advance_objective(&g, "").is_none());
        // a scene id with no tagged atoms (the affordance tagged scene_001, not this).
        assert!(scene_advance_objective(&g, "scene_999").is_none());
    }

    #[test]
    fn anyof_atom_kind_is_an_observable_action_kind() {
        // sanity: the aquifer affordance compiles to an ActionResolved atom (the
        // admissible kind the witness/exact path can land in the ledger).
        let g = graph_with_scene_affordance();
        let leaves = scene_advance_guard_leaves(&g, "scene_001");
        assert!(leaves
            .iter()
            .any(|a| a.kind == EvidenceKind::ActionResolved));
    }

    #[test]
    fn empty_graph_yields_nothing() {
        let g = ModuleGraph::default();
        assert!(scene_advance_guard_leaves(&g, "scene_001").is_empty());
        assert!(scene_advance_objective(&g, "scene_001").is_none());
    }
}
