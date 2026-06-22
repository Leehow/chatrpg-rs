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
}
