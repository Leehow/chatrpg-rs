//! PL-5 (PHASE 2 ROUND-2): generalize the per-scene open-objective + frontier
//! derivation from scene_01 (PL-1's `derive_threat_objective`) to the WHOLE spine.
//!
//! Proven gap (ROUND-2 diagnosis): after PL-1+PL-2 scene_01's starvation was
//! fixed, but the run still went 1-hop — the player reached scene_02 and froze,
//! because scene_02+ had no objective and no out-edge to surface. The producer
//! finds no anchored "go-to" cue in the combat/exploration prose of those scenes,
//! so `program_from_module_graph` produces no ridge rule for them → frontier empty
//! → frozen. This pass fills that gap from STRUCTURE, source-grounded, fail-closed.
//!
//! Two additive products per spine scene (ordered by page sequence — authored
//! structure, NOT scene-name/ruleset branching):
//!   1. A **spine-order activation rule** `ON Entered(from) THEN Activate(to)`
//!      where `to` is the next spine scene. This makes the frontier non-empty the
//!      moment the focus enters ANY spine scene (the robust starvation fix), reusing
//!      the exact same Activate-surfacing mechanism as an authored Sequential ridge
//!      (SoftGravity — surfaced, never teleported; the navigator's departure-commit
//!      gate still decides if the player actually moves). Skipped when an authored
//!      ridge already covers `from→to` (no duplicate).
//!   2. A **per-scene open advance objective** `obj.advance.{from}` gated on the
//!      scene's OWN structural vocabulary via [`PredicateExpr::AnyFactMatches`] (the
//!      same deterministic, Rust-evaluated, LLM-never scheme PL-2 proved aligns the
//!      GM's free-form `fact_id`s). It is scene-scoped (`mission_id = from`) so only
//!      the current scene's goal surfaces (see `compute_frontier`), and its outcome
//!      rule completes it when a matching fact lands. Fail-closed: a scene whose
//!      structural vocab is empty gets NO objective (never invents one).
//!
//! ZERO ruleset/module/scene-name branching: everything derives from the parsed
//! graph's page order + each scene's own tokens. OFF==baseline: this is only called
//! inside the `TRPG_PROGRESSION_ENGINE` block; nothing runs when the engine is OFF.
use super::program::OwnedProgram;
use trpg_model::adventure_ir::{
    EffectExpr, EventPattern, ObjectiveSpec, PredicateExpr, ProgressRule,
};
use trpg_model::{ModuleGraph, ScenarioNode, SourceRef};

/// Augment an existing program with spine-wide activation rules + per-scene open
/// objectives. Pure, deterministic, additive. Empty/degenerate graphs → no-op.
pub fn augment_program_with_spine(program: &mut OwnedProgram, graph: &ModuleGraph) {
    let order = spine_order(graph);
    for win in order.windows(2) {
        let from_node = win[0];
        let to_node = win[1];
        let from = from_node.node_id.trim();
        let to = to_node.node_id.trim();
        if from.is_empty() || to.is_empty() || from == to {
            continue;
        }
        push_spine_activation(program, from, to);
        push_scene_objective(program, from_node);
    }
    // The terminal spine scene has no successor → still give it an open objective so
    // its frontier is non-empty (it has no out-edge to surface, but a goal can be).
    if let Some(last) = order.last() {
        push_scene_objective(program, last);
    }
}

/// Spine order = scenes with a non-empty id, ordered by authored page sequence
/// (`page_start`), then by their stable position in the parsed list. Pure structure.
fn spine_order(graph: &ModuleGraph) -> Vec<&ScenarioNode> {
    let mut idx: Vec<(usize, &ScenarioNode)> = graph
        .scenes
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.node_id.trim().is_empty())
        .collect();
    idx.sort_by_key(|(i, s)| (s.page_start.unwrap_or(u32::MAX), *i));
    idx.into_iter().map(|(_, s)| s).collect()
}

/// Push `ON Entered(from) THEN Activate(to)` unless an authored ridge rule already
/// activates `to` from entering `from` (avoid duplicate surfacing).
fn push_spine_activation(program: &mut OwnedProgram, from: &str, to: &str) {
    let already = program.rules.iter().any(|r| {
        matches!(&r.on, EventPattern::Entered(f) if f == from)
            && r.then
                .iter()
                .any(|e| matches!(e, EffectExpr::Activate(t) if t == to))
    });
    if already {
        return;
    }
    let on = EventPattern::Entered(from.to_string());
    let when = PredicateExpr::All(vec![]); // unconditional once entered (like a Sequential ridge)
    let then = vec![EffectExpr::Activate(to.to_string())];
    let normalization = ProgressRule::classify(&when, &then);
    let mut sr = SourceRef::default();
    sr.note = Some(format!("spine order (page sequence): {from} → {to}"));
    program.rules.push(ProgressRule {
        id: format!("rule.spine.{from}__{to}"),
        on,
        when,
        then,
        source_evidence: vec![sr],
        normalization,
    });
}

/// Push a scene-scoped open advance objective + its outcome rule, gated on the
/// scene's own structural vocab. Fail-closed: empty vocab ⇒ nothing. Idempotent.
fn push_scene_objective(program: &mut OwnedProgram, node: &ScenarioNode) {
    let scene = node.node_id.trim();
    if scene.is_empty() {
        return;
    }
    let obj_id = format!("obj.advance.{scene}");
    if program.objectives.iter().any(|o| o.id == obj_id) {
        return; // already derived (terminal-scene re-call, or a prior window)
    }
    let vocab = scene_vocab(node);
    if vocab.is_empty() {
        return; // fail-closed: never invent a goal without source-grounded tokens
    }
    let success_when = PredicateExpr::AnyFactMatches {
        tokens: vocab.clone(),
    };
    let mut obj_sr = SourceRef::default();
    obj_sr.note = Some(format!("spine advance objective (scene-vocab aligned): {scene}"));
    program.objectives.push(ObjectiveSpec {
        id: obj_id.clone(),
        mission_id: Some(scene.to_string()), // scene-scoped: only surfaces in this scene
        mandatory: false,
        success_when: success_when.clone(),
        failure_when: None,
        score_effects: vec![],
        rewards: vec![],
        deadline: None,
        source_evidence: vec![obj_sr],
    });
    let then = vec![EffectExpr::Complete(obj_id.clone())];
    let normalization = ProgressRule::classify(&success_when, &then);
    let mut rule_sr = SourceRef::default();
    rule_sr.note = Some(format!("scene advance outcome (vocab match completes goal): {scene}"));
    program.rules.push(ProgressRule {
        id: format!("rule.advance.{scene}"),
        on: EventPattern::WorldFactChanged,
        when: success_when,
        then,
        source_evidence: vec![rule_sr],
        normalization,
    });
}

/// Derive a scene's matcher vocabulary from its OWN structure: id, title, and the
/// ids of entities it references. Tokens are lowercased alphanumeric words ≥4 chars,
/// minus generic structural noise; deduped, order-preserving. Source-grounded; no
/// hardcoded scene/ruleset names.
fn scene_vocab(node: &ScenarioNode) -> Vec<String> {
    const NOISE: &[&str] = &[
        "scene", "node", "beat", "chapter", "mission", "phase", "location", "zone",
        "encounter", "with", "from", "into", "that", "this", "they", "them", "your",
        "have", "will", "the", "and", "for", "npc", "clue", "loc",
    ];
    let mut sources: Vec<&str> = vec![node.node_id.as_str(), node.title.as_str()];
    for v in [
        &node.referenced_npc_ids,
        &node.referenced_clue_ids,
        &node.referenced_location_ids,
    ] {
        sources.extend(v.iter().map(String::as_str));
    }
    let mut out: Vec<String> = Vec::new();
    for s in sources {
        for raw in s.split(|c: char| !c.is_ascii_alphanumeric()) {
            let tok = raw.to_ascii_lowercase();
            if tok.len() < 4 || tok.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            if NOISE.contains(&tok.as_str()) {
                continue;
            }
            if !out.contains(&tok) {
                out.push(tok);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::adventure_ir::{EvalContext, IrValue, ObjectiveStatus, PredicateValue};

    fn scene(id: &str, title: &str, page: Option<u32>) -> ScenarioNode {
        ScenarioNode {
            node_id: id.to_string(),
            title: title.to_string(),
            page_start: page,
            ..Default::default()
        }
    }

    fn graph(scenes: Vec<ScenarioNode>) -> ModuleGraph {
        ModuleGraph {
            scenes,
            ..Default::default()
        }
    }

    #[test]
    fn spine_gives_each_scene_an_activation_to_its_successor() {
        let g = graph(vec![
            scene("scene_01_lawmen", "Lawmen in Trouble", Some(2)),
            scene("scene_02_athena", "Questions for Athena", Some(5)),
            scene("scene_03_foxwell", "Foxwell Services", Some(9)),
        ]);
        let mut p = OwnedProgram::default();
        augment_program_with_spine(&mut p, &g);
        // two adjacent pairs ⇒ two spine activation rules.
        let spine_rules: Vec<_> = p.rules.iter().filter(|r| r.id.starts_with("rule.spine.")).collect();
        assert_eq!(spine_rules.len(), 2, "one activation per adjacent spine pair");
        assert!(p
            .rules
            .iter()
            .any(|r| r.id == "rule.spine.scene_01_lawmen__scene_02_athena"));
        assert!(p
            .rules
            .iter()
            .any(|r| r.id == "rule.spine.scene_02_athena__scene_03_foxwell"));
    }

    #[test]
    fn every_scene_gets_a_scene_scoped_open_objective() {
        let g = graph(vec![
            scene("scene_01_lawmen", "Lawmen in Trouble", Some(2)),
            scene("scene_02_athena", "Questions for Athena", Some(5)),
        ]);
        let mut p = OwnedProgram::default();
        augment_program_with_spine(&mut p, &g);
        // both scenes (incl. the terminal one) get an advance objective, scene-scoped.
        assert!(p.objectives.iter().any(|o| o.id == "obj.advance.scene_01_lawmen"
            && o.mission_id.as_deref() == Some("scene_01_lawmen")));
        assert!(p.objectives.iter().any(|o| o.id == "obj.advance.scene_02_athena"
            && o.mission_id.as_deref() == Some("scene_02_athena")));
    }

    #[test]
    fn scene_objective_completes_on_matching_structural_fact() {
        // scene title "Questions for Athena" → vocab includes "athena","questions".
        let g = graph(vec![scene("scene_02_athena", "Questions for Athena", Some(5))]);
        let mut p = OwnedProgram::default();
        augment_program_with_spine(&mut p, &g);
        let obj = p
            .objectives
            .iter()
            .find(|o| o.id == "obj.advance.scene_02_athena")
            .expect("objective derived");
        // a GM free-form fact_id containing a scene token satisfies the guard.
        let mut ctx = EvalContext::default();
        ctx.facts
            .insert("clue.athena_gave_coords".to_string(), IrValue::Bool(true));
        assert_eq!(obj.success_when.eval(&ctx), PredicateValue::True);
        // an unrelated fact does not.
        let mut ctx2 = EvalContext::default();
        ctx2.facts
            .insert("npc.butler_greeted".to_string(), IrValue::Bool(true));
        assert_eq!(obj.success_when.eval(&ctx2), PredicateValue::False);
    }

    #[test]
    fn fail_closed_when_scene_has_no_structural_tokens() {
        // numeric-only id and blank title ⇒ no vocab ⇒ no objective (never invent).
        let g = graph(vec![scene("0001", "", Some(1)), scene("0002", "", Some(2))]);
        let mut p = OwnedProgram::default();
        augment_program_with_spine(&mut p, &g);
        assert!(
            p.objectives.is_empty(),
            "blank/numeric scenes get no objective (fail-closed)"
        );
        // spine activations still derive (structural ordering doesn't need vocab).
        assert_eq!(p.rules.iter().filter(|r| r.id.starts_with("rule.spine.")).count(), 1);
    }

    #[test]
    fn does_not_duplicate_an_authored_ridge_activation() {
        use trpg_model::adventure_ir::ProgressRule as PR;
        let g = graph(vec![
            scene("scene_01", "Opening Scene", Some(1)),
            scene("scene_02", "Second Scene", Some(2)),
        ]);
        let mut p = OwnedProgram::default();
        // pre-seed an authored ridge that already activates scene_02 from scene_01.
        let when = PredicateExpr::All(vec![]);
        let then = vec![EffectExpr::Activate("scene_02".to_string())];
        let normalization = PR::classify(&when, &then);
        p.rules.push(PR {
            id: "rule.flow.scene_01__scene_02".to_string(),
            on: EventPattern::Entered("scene_01".to_string()),
            when,
            then,
            source_evidence: vec![],
            normalization,
        });
        augment_program_with_spine(&mut p, &g);
        assert!(
            !p.rules.iter().any(|r| r.id == "rule.spine.scene_01__scene_02"),
            "must not duplicate an existing authored ridge activation"
        );
    }

    #[test]
    fn empty_graph_is_noop() {
        let mut p = OwnedProgram::default();
        augment_program_with_spine(&mut p, &graph(vec![]));
        assert!(p.is_empty());
    }

    #[test]
    fn page_order_drives_spine_not_list_order() {
        // list order is reversed vs page order; spine must follow page_start.
        let g = graph(vec![
            scene("late", "Final Confrontation", Some(20)),
            scene("early", "Opening Hook", Some(1)),
        ]);
        let mut p = OwnedProgram::default();
        augment_program_with_spine(&mut p, &g);
        assert!(p.rules.iter().any(|r| r.id == "rule.spine.early__late"));
        assert!(!p.rules.iter().any(|r| r.id == "rule.spine.late__early"));
        let _ = ObjectiveStatus::Inactive; // keep import used across cfgs
    }
}
