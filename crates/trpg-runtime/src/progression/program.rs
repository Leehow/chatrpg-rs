//! P1-3 program source: derive a runtime [`ProgressionProgram`] structurally from
//! the parsed [`ModuleGraph`]'s authored flow-link ridges. This repurposes the
//! flow-link producer's output (commit 580a9b1, `module_flow_links.rs`) into
//! [`ProgressRule`]s at runtime — fail-closed, source-anchored, ZERO ruleset/
//! module name branching (decisions are purely by link structure: anchored +
//! non-spatial = authored ridge, via the shared [`is_authored_flow_link`]).
//!
//! - A **Sequential** ridge "after X, go to Y" → an executable rule
//!   `ON Entered(X) WHEN true THEN Activate(Y)`: once the focus is in X, Y is a
//!   legal frontier candidate the Director may surface (SoftGravity, anti-railroad
//!   — surfaced, never teleported).
//! - A **Trigger/Branch/Timeline** ridge carries an authored CONDITION (the
//!   `source_anchor`: "Once they hack…", "If the PCs flee…") we cannot normalize
//!   into a finite guard → stored as [`PredicateExpr::OpaqueAuthoredText`]
//!   (classified Opaque) so Rust NEVER auto-fires it (fail-closed at the execution
//!   layer); kept for the GM/Director to read, never auto-driven.
//! - Spatial bridges (anchorless) and links to out-of-graph targets are skipped.
//!
//! The semantic GOLDEN objectives/timers (Neutralize Athena, Foxwell 15min) are
//! authored-text extraction (P1-2, LLM) and are NOT derived here; this structural
//! pass gives the engine a real, anchored flow program so the frontier is non-empty
//! on the live runtime path without inventing facts.
use super::engine::ProgressionProgram;
use crate::scene_navigation::is_authored_flow_link;
use trpg_model::adventure_ir::{
    EffectExpr, EventPattern, ObjectiveSpec, PredicateExpr, ProgressRule, TrackerSpec,
};
use trpg_model::{LinkType, ModuleGraph, ScenarioLink, SourceRef};

/// An owned [`ProgressionProgram`] (the engine borrows from this each turn).
#[derive(Debug, Clone, Default)]
pub struct OwnedProgram {
    pub rules: Vec<ProgressRule>,
    pub objectives: Vec<ObjectiveSpec>,
    pub trackers: Vec<TrackerSpec>,
}

impl OwnedProgram {
    /// Borrow as the engine's `ProgressionProgram<'_>`.
    pub fn borrow(&self) -> ProgressionProgram<'_> {
        ProgressionProgram {
            rules: &self.rules,
            objectives: &self.objectives,
            trackers: &self.trackers,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty() && self.objectives.is_empty() && self.trackers.is_empty()
    }
}

/// Derive the structural flow program from a parsed module graph. Pure,
/// deterministic; only authored, source-anchored, in-graph ridges become rules.
pub fn program_from_module_graph(graph: &ModuleGraph) -> OwnedProgram {
    let mut rules = Vec::new();
    for node in &graph.scenes {
        let from = node.node_id.trim();
        if from.is_empty() {
            continue;
        }
        for l in &node.links {
            if !is_authored_flow_link(l) {
                continue;
            }
            let to = l.to_node_id.trim();
            // fail-closed: never invent a target — must be a distinct in-graph node.
            if to.is_empty() || to == from || !graph.scenes.iter().any(|s| s.node_id == to) {
                continue;
            }
            if let Some(rule) = rule_from_ridge(from, to, l) {
                rules.push(rule);
            }
        }
    }
    OwnedProgram {
        rules,
        objectives: Vec::new(),
        trackers: Vec::new(),
    }
}

/// Turn one authored ridge into a [`ProgressRule`]. `None` only if the anchor is
/// (now) blank — `is_authored_flow_link` already guarantees it is non-empty.
fn rule_from_ridge(from: &str, to: &str, link: &ScenarioLink) -> Option<ProgressRule> {
    let anchor = link
        .source_anchor
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let on = EventPattern::Entered(from.to_string());
    let then = vec![EffectExpr::Activate(to.to_string())];
    let when = match link.link_type {
        // Sequential ridge: unconditional once entered → executable always-true guard.
        LinkType::Sequential => PredicateExpr::All(vec![]),
        // Trigger/Branch/Timeline: authored condition we cannot normalize → opaque
        // (classify → Opaque → never auto-fires; fail-closed at the execution layer).
        _ => PredicateExpr::OpaqueAuthoredText {
            raw_text: anchor.to_string(),
        },
    };
    let normalization = ProgressRule::classify(&when, &then);
    Some(ProgressRule {
        id: format!("rule.flow.{from}__{to}"),
        on,
        when,
        then,
        source_evidence: vec![anchor_evidence(anchor)],
        normalization,
    })
}

fn anchor_evidence(anchor: &str) -> SourceRef {
    let mut sr = SourceRef::default();
    sr.note = Some(format!("authored flow ridge: {anchor}"));
    sr
}

/// PL-1 output: a source-grounded scene threat objective plus its single
/// outcome-gated rule. The objective is the OPEN frontier anchor that ends
/// scene_01 frontier starvation; the rule fires the gated outcome (reveal coords)
/// the moment the GM emits a neutralization-vector fact.
#[derive(Debug, Clone)]
pub struct ObjectiveDerivation {
    pub objective: ObjectiveSpec,
    pub outcome_rule: ProgressRule,
}

/// Derive a scene threat objective from a module's OWN matcher vocabulary
/// (`module_config` npc/tech tokens), aligning the GM's free-form `fact_id`s via
/// [`PredicateExpr::AnyFactMatches`] — the same case-insensitive-substring scheme
/// the module config already uses, so the LLM never evaluates the guard. ZERO
/// ruleset/module name branching (the caller passes structurally-extracted
/// tokens). Fail-closed: blank/empty vocab → `None` (never invent an objective).
pub fn derive_threat_objective(
    objective_id: &str,
    tokens: &[String],
    outcome_reveal_id: &str,
    evidence_note: &str,
) -> Option<ObjectiveDerivation> {
    let vocab: Vec<String> = tokens
        .iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    if vocab.is_empty() {
        return None;
    }
    let success_when = PredicateExpr::AnyFactMatches {
        tokens: vocab.clone(),
    };
    let mut obj_sr = SourceRef::default();
    obj_sr.note = Some(format!("threat objective (vocab-aligned): {evidence_note}"));
    let objective = ObjectiveSpec {
        id: objective_id.to_string(),
        mission_id: None,
        mandatory: false,
        success_when: success_when.clone(),
        failure_when: None,
        score_effects: vec![],
        rewards: vec![],
        deadline: None,
        source_evidence: vec![obj_sr],
    };
    let then = vec![
        EffectExpr::Complete(objective_id.to_string()),
        EffectExpr::Reveal(outcome_reveal_id.to_string()),
    ];
    let normalization = ProgressRule::classify(&success_when, &then);
    let mut rule_sr = SourceRef::default();
    rule_sr.note = Some(format!("outcome-gated on neutralization vector: {evidence_note}"));
    let outcome_rule = ProgressRule {
        id: format!("rule.outcome.{objective_id}"),
        on: EventPattern::WorldFactChanged,
        when: success_when,
        then,
        source_evidence: vec![rule_sr],
        normalization,
    };
    Some(ObjectiveDerivation {
        objective,
        outcome_rule,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::adventure_ir::NormalizationStatus;
    use trpg_model::{ScenarioNode};

    fn link(to: &str, lt: LinkType, anchor: Option<&str>) -> ScenarioLink {
        ScenarioLink {
            to_node_id: to.into(),
            reason: "r".into(),
            clue_id: None,
            link_type: lt,
            source_anchor: anchor.map(str::to_string),
        }
    }

    fn node(id: &str, links: Vec<ScenarioLink>) -> ScenarioNode {
        let mut n = ScenarioNode::default();
        n.node_id = id.into();
        n.title = id.into();
        n.links = links;
        n
    }

    fn graph(scenes: Vec<ScenarioNode>) -> ModuleGraph {
        let mut g = ModuleGraph::default();
        g.scenes = scenes;
        g
    }

    #[test]
    fn sequential_ridge_makes_executable_activate_rule() {
        let g = graph(vec![
            node("A", vec![link("B", LinkType::Sequential, Some("press deeper"))]),
            node("B", vec![]),
        ]);
        let p = program_from_module_graph(&g);
        assert_eq!(p.rules.len(), 1, "one authored sequential ridge → one rule");
        let r = &p.rules[0];
        assert_eq!(r.on, EventPattern::Entered("A".into()));
        assert_eq!(r.then, vec![EffectExpr::Activate("B".into())]);
        assert_eq!(r.normalization, NormalizationStatus::Executable);
        assert!(r.may_autofire(), "sequential ridge is executable");
        assert!(!r.source_evidence.is_empty(), "carries the anchor as evidence");
    }

    #[test]
    fn trigger_ridge_is_opaque_and_never_autofires() {
        let g = graph(vec![
            node("A", vec![link("C", LinkType::Trigger, Some("Once they hack the server"))]),
            node("C", vec![]),
        ]);
        let p = program_from_module_graph(&g);
        assert_eq!(p.rules.len(), 1);
        let r = &p.rules[0];
        assert_eq!(r.normalization, NormalizationStatus::Opaque);
        assert!(!r.may_autofire(), "un-normalizable authored condition must never auto-fire");
        assert!(matches!(r.when, PredicateExpr::OpaqueAuthoredText { .. }));
    }

    #[test]
    fn spatial_bridge_makes_no_rule() {
        let g = graph(vec![
            node("A", vec![link("D", LinkType::Spatial, None)]),
            node("D", vec![]),
        ]);
        assert!(program_from_module_graph(&g).rules.is_empty(), "spatial bridge is not a ridge");
    }

    #[test]
    fn out_of_graph_target_is_skipped_fail_closed() {
        let g = graph(vec![node(
            "A",
            vec![link("Z", LinkType::Sequential, Some("go to Z"))],
        )]);
        assert!(
            program_from_module_graph(&g).rules.is_empty(),
            "ridge to a non-existent node is dropped, never invented"
        );
    }

    #[test]
    fn engine_consumes_program_and_frontier_surfaces_next() {
        use crate::progression::{compute_frontier, evaluate, ProgressEvent, ProgressionState};
        let g = graph(vec![
            node("A", vec![link("B", LinkType::Sequential, Some("press deeper"))]),
            node("B", vec![]),
        ]);
        let p = program_from_module_graph(&g);
        let mut state = ProgressionState::default();
        // Focus is in A → seed Entered(A); the sequential ridge activates B.
        evaluate(&mut state, &[ProgressEvent::Entered("A".into())], &p.borrow());
        let f = compute_frontier(&state, &p.objectives, &p.trackers);
        assert!(
            f.active_units.contains(&"B".to_string()),
            "B is the legal next beat the Director may surface, frontier: {:?}",
            f.active_units
        );
    }

    #[test]
    fn empty_graph_yields_empty_program() {
        assert!(program_from_module_graph(&ModuleGraph::default()).is_empty());
    }

    // ---- PL-1: source-grounded threat objective (scene_01 starvation fix) ----

    #[test]
    fn threat_objective_derives_open_anchor_and_outcome_rule() {
        let tokens = vec![
            "drone".to_string(),
            "athena".to_string(),
            "hack".to_string(),
            "server".to_string(),
            "cable".to_string(),
            "切断".to_string(),
        ];
        let d = derive_threat_objective(
            "obj.neutralize.npc_athena_drone",
            &tokens,
            "rev.athena_coords",
            "module_config: npc.athena_drone + technical_option_table",
        )
        .expect("non-empty vocab derives an objective");

        // Objective is the open frontier anchor, success aligned by the vocab.
        assert_eq!(d.objective.id, "obj.neutralize.npc_athena_drone");
        assert!(matches!(
            d.objective.success_when,
            PredicateExpr::AnyFactMatches { .. }
        ));
        assert!(!d.objective.source_evidence.is_empty(), "carries real anchor");

        // Outcome-gated rule: on the neutralization fact, complete + reveal coords.
        let r = &d.outcome_rule;
        assert_eq!(r.on, EventPattern::WorldFactChanged);
        assert!(matches!(r.when, PredicateExpr::AnyFactMatches { .. }));
        assert_eq!(
            r.then,
            vec![
                EffectExpr::Complete("obj.neutralize.npc_athena_drone".into()),
                EffectExpr::Reveal("rev.athena_coords".into()),
            ]
        );
        assert_eq!(r.normalization, NormalizationStatus::Executable);
        assert!(r.may_autofire(), "executable guard + effects → auto-fires");
        assert!(!r.source_evidence.is_empty());
    }

    #[test]
    fn threat_objective_fail_closed_on_blank_vocab() {
        // No tokens, or only blank/whitespace tokens → never invent an objective.
        assert!(derive_threat_objective("obj.x", &[], "rev.x", "n").is_none());
        assert!(derive_threat_objective(
            "obj.x",
            &["".into(), "   ".into()],
            "rev.x",
            "n"
        )
        .is_none());
    }

    #[test]
    fn threat_objective_fixes_starvation_then_completes_on_live_fact() {
        use crate::progression::{compute_frontier, evaluate, ProgressEvent, ProgressionState};
        use trpg_model::adventure_ir::{IrValue, ProgressSignalKind};
        let tokens = vec!["drone".to_string(), "hack".to_string(), "server".to_string()];
        let d = derive_threat_objective("obj.threat", &tokens, "rev.coords", "cfg").unwrap();
        let mut program = OwnedProgram::default();
        program.objectives.push(d.objective);
        program.rules.push(d.outcome_rule);

        let mut state = ProgressionState::default();
        // BEFORE any fact: frontier is non-empty (the open objective ends starvation).
        let f0 = compute_frontier(&state, &program.objectives, &program.trackers);
        assert!(
            f0.open_objectives.contains(&"obj.threat".to_string()),
            "scene_01 frontier must be non-empty from entry (starvation fix)"
        );

        // The REAL live homecoming fact_id fires the outcome.
        let signals = evaluate(
            &mut state,
            &[ProgressEvent::WorldFactChanged {
                fact: "encounter.hacking_server".into(),
                value: IrValue::Bool(true),
            }],
            &program.borrow(),
        );
        assert!(signals
            .iter()
            .any(|s| s.kind == ProgressSignalKind::ObjectiveCompleted && s.id == "obj.threat"));
        assert!(signals
            .iter()
            .any(|s| s.kind == ProgressSignalKind::RevelationUnlocked && s.id == "rev.coords"));
    }
}
