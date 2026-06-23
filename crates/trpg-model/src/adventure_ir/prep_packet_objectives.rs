//! EV-P4 `progress_capability_binding_v1` (Part B) — **GuardLeaf-from-prep-packet**.
//!
//! The live the_vault `module_graph` has **no objectives field** (the agentic reader
//! flattened the 12 missions to bare scenes — EV-P3 found every catalog atom stays
//! `CarrierOnly`). But the mission's authored objectives DO survive in the
//! `module_prep_packets.packet_json` `current_session_packet` (owner default: read
//! them from the prep-packet — lighter than persisting `induct_missions`).
//!
//! This module reads `mission_briefing.optional_objectives[].objective` (authored
//! objective leaves) and compiles them into **GuardLeaf** evidence atoms via the
//! SAME validator as EV-P3 ([`parse_action_phrase`], [`ProgressRole::GuardLeaf`]):
//! real source span, target resolves to the module graph, fail-closed, anti-
//! tautology. The objective's target is bound using the SAME packet's authored
//! mission context (the anomaly name + the first investigation location) as
//! candidate bindings — so "Conduct an experiment." binds to the mission's authored
//! Anomaly entity (design `GPTpro-producer-firing-followup.md` §A: "conduct an
//! experiment" → ActionResolved atom). **No ruleset/module name branch**; a hint
//! that resolves to no graph ref produces no atom (anti-fabrication).

use crate::adventure_ir::authored_observation::{entry_scene_id, tag_scene};
use crate::adventure_ir::{
    parse_action_phrase, EvidenceAtomSpec, GraphRefIndex, ObjectiveSpec, PredicateExpr, ProgressRole,
};
use crate::{ModuleGraph, SourceRef};
use serde_json::Value;

/// The packet anchor recorded as the source span of a prep-packet objective.
const PACKET_ANCHOR: &str = "current_session_packet.mission_briefing.optional_objectives";

/// A non-empty trimmed string at `value[key]`, if present.
fn str_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Read the mission's authored optional objectives from the prep-packet's
/// `current_session_packet`. Each `{ "objective": "<text>", ... }` becomes one
/// [`ObjectiveSpec`] whose `success_when` is an un-normalized
/// [`PredicateExpr::OpaqueAuthoredText`] (Rust never auto-fires it; it is an
/// observable leaf the GuardLeaf compiler turns into an atom). The closed object
/// CANNOT carry an objective/atom id — only the authored text. Fail-closed: a
/// missing briefing / empty objective string yields nothing (never a fabricated
/// objective).
pub fn objectives_from_prep_packet(packet_csp: &Value, module_digest: &str) -> Vec<ObjectiveSpec> {
    let Some(arr) = packet_csp
        .get("mission_briefing")
        .and_then(|b| b.get("optional_objectives"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in arr {
        let Some(text) = str_field(entry, "objective") else {
            continue; // fail-closed: no authored objective text → no spec
        };
        out.push(ObjectiveSpec {
            id: format!("obj.optional.{}", out.len()),
            mission_id: None,
            mandatory: false,
            success_when: PredicateExpr::OpaqueAuthoredText {
                raw_text: text.clone(),
            },
            failure_when: None,
            score_effects: Vec::new(),
            rewards: Vec::new(),
            deadline: None,
            source_evidence: vec![SourceRef {
                source_id: module_digest.to_string(),
                anchor_id: Some(PACKET_ANCHOR.to_string()),
                note: Some(text),
                ..Default::default()
            }],
        });
    }
    out
}

/// The authored mission-context targets an objective leaf may bind to — taken ONLY
/// from authored packet fields (the GM-only Anomaly `name` + the first investigation
/// `location`). Each source string is offered whole AND tokenized to its
/// significant (≥4-char) words, so a phrase like "Conduct an experiment." can bind
/// to the mission's authored Anomaly even though the objective text itself names no
/// entity. Resolution to a REAL graph ref is enforced downstream (anti-fabrication).
pub fn prep_packet_binding_hints(packet_csp: &Value) -> Vec<String> {
    let mut hints: Vec<String> = Vec::new();
    let mut push = |s: Option<String>| {
        if let Some(s) = s {
            for tok in s.split(|c: char| !c.is_alphanumeric()) {
                if tok.chars().count() >= 4 {
                    hints.push(tok.to_string());
                }
            }
            hints.push(s);
        }
    };
    if let Some(anomaly) = packet_csp.get("anomaly_gm_only") {
        push(str_field(anomaly, "name"));
    }
    if let Some(area) = packet_csp.get("first_investigation_area") {
        push(str_field(area, "location"));
    }
    let mut seen = std::collections::HashSet::new();
    hints.retain(|h| seen.insert(h.to_ascii_lowercase()));
    hints
}

/// Compile the prep-packet's authored objective leaves into **GuardLeaf** evidence
/// atoms. Reuses the EV-P3 validator unchanged ([`parse_action_phrase`]): a leaf
/// yields an atom ONLY if it carries a lexicon verb AND its target resolves to a
/// real module-graph ref; otherwise nothing (fail-closed, anti-tautology).
///
/// **Anti-fabrication**: the bound target must be an entity the SAME packet's
/// authored mission context (anomaly name / investigation location) explicitly
/// resolves to — NOT a target the objective text reached via an incidental
/// stop-word substring (e.g. "an" ⊂ "anomaly"). The `authored` set is computed only
/// from the authored hints; an empty set (no resolvable authored context) ⇒ no
/// GuardLeaf, ever. Atoms are tagged to the mission's entry scene so the offer
/// frontier can surface them. Deduped by grounding (first wins).
pub fn compile_prep_packet_guard_leaves(
    graph: &ModuleGraph,
    packet_csp: &Value,
) -> Vec<EvidenceAtomSpec> {
    let module_digest = graph.module_id.as_str();
    let objectives = objectives_from_prep_packet(packet_csp, module_digest);
    let hints = prep_packet_binding_hints(packet_csp);
    let index = GraphRefIndex::build(graph);
    let entry = entry_scene_id(graph);

    // Anti-fabrication: the ONLY graph refs an objective leaf may bind to are those
    // the authored mission context resolves to. No authored context resolves ⇒ no atom.
    let authored: std::collections::HashSet<String> =
        hints.iter().filter_map(|h| index.resolve(h)).collect();

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for obj in &objectives {
        let PredicateExpr::OpaqueAuthoredText { raw_text } = &obj.success_when else {
            continue;
        };
        let source = obj
            .source_evidence
            .first()
            .cloned()
            .unwrap_or_else(|| SourceRef {
                source_id: module_digest.to_string(),
                anchor_id: Some(PACKET_ANCHOR.to_string()),
                note: Some(raw_text.clone()),
                ..Default::default()
            });
        let Some(atom) = parse_action_phrase(
            raw_text,
            source,
            &hints,
            &index,
            ProgressRole::GuardLeaf,
            module_digest,
        ) else {
            continue; // fail-closed: no verb / no resolvable target → no GuardLeaf
        };
        // The grounding target (first binding) must be an authored-context entity.
        if !atom.bindings.iter().any(|b| authored.contains(b)) {
            continue; // anti-fabrication: incidental/stop-word binding → drop
        }
        if !seen.insert(atom.grounding.clone()) {
            continue;
        }
        out.push(match &entry {
            Some(s) => tag_scene(atom, s),
            None => atom,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adventure_ir::EvidenceKind;
    use crate::ScenarioNode;
    use serde_json::json;

    /// A graph whose only NPC is the mission's authored Anomaly (so "anomaly"
    /// resolves) plus one paged scene (entry scene).
    fn graph_with_anomaly() -> ModuleGraph {
        ModuleGraph {
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
        }
    }

    /// A packet with one objective leaf + the authored anomaly name as context.
    fn packet(objective: &str) -> Value {
        json!({
            "mission_briefing": {
                "optional_objectives": [
                    {"objective": objective, "reward": "+3 Commendations"}
                ]
            },
            "anomaly_gm_only": {"name": "The Springs Eternal Anomaly"},
            "first_investigation_area": {"location": "Mercantile Avenue"}
        })
    }

    #[test]
    fn objectives_from_packet_are_opaque_leaves_with_no_id_leak() {
        let p = packet("Conduct an experiment.");
        let objs = objectives_from_prep_packet(&p, "the_vault");
        assert_eq!(objs.len(), 1, "one authored objective leaf");
        let o = &objs[0];
        match &o.success_when {
            PredicateExpr::OpaqueAuthoredText { raw_text } => {
                assert_eq!(raw_text, "Conduct an experiment.")
            }
            other => panic!("expected OpaqueAuthoredText, got {other:?}"),
        }
        assert!(o.mission_id.is_none(), "the closed objective carries no mission/objective id binding");
        assert!(!o.source_evidence.is_empty(), "source-grounded (carries the packet anchor)");
        assert_eq!(o.source_evidence[0].anchor_id.as_deref(), Some(PACKET_ANCHOR));
    }

    #[test]
    fn prep_packet_objective_binds_to_authored_anomaly_as_guard_leaf() {
        let g = graph_with_anomaly();
        let p = packet("Conduct an experiment.");
        let atoms = compile_prep_packet_guard_leaves(&g, &p);
        assert!(!atoms.is_empty(), "the authored objective compiles to ≥1 GuardLeaf atom");
        let a = &atoms[0];
        assert_eq!(a.progress_role, ProgressRole::GuardLeaf, "objective leaf ⇒ GuardLeaf (guard-checkable)");
        assert_eq!(a.kind, EvidenceKind::ActionResolved, "'conduct' ⇒ ActionResolved");
        assert!(a.grounding.contains("npc_anomaly"), "bound to the authored mission Anomaly: {}", a.grounding);
        assert!(!a.source_refs.is_empty(), "source-grounded");
        assert!(a.bindings.iter().any(|b| b == "scene:scene_001"), "tagged to the mission entry scene for offering");
    }

    #[test]
    fn objective_with_no_resolvable_target_yields_no_atom() {
        // Same objective, but the graph has NO entity the authored hints resolve to.
        let g = ModuleGraph {
            module_id: "the_vault".into(),
            npcs: vec![json!({"id": "npc_butler", "name": "Reginald the Butler"})],
            scenes: vec![ScenarioNode {
                node_id: "scene_001".into(),
                node_type: "scene".into(),
                page_start: Some(8),
                ..Default::default()
            }],
            ..Default::default()
        };
        let atoms = compile_prep_packet_guard_leaves(&g, &packet("Conduct an experiment."));
        assert!(atoms.is_empty(), "fail-closed: no authored hint resolves ⇒ no fabricated GuardLeaf");
    }

    #[test]
    fn objective_binding_via_stopword_to_unauthored_entity_is_dropped() {
        // The graph NPC "Daniel" contains the stop-word "an" (Dani-an-... actually
        // "an" ⊂ "daniel"), so the phrase token "an" in "Conduct an experiment."
        // would resolve to npc_dan via incidental substring match. But the authored
        // mission context (anomaly name "The Hidden Anomaly") resolves to NO graph
        // entity → the atom must be dropped (anti-fabrication), not bound to npc_dan.
        let g = ModuleGraph {
            module_id: "the_vault".into(),
            npcs: vec![json!({"id": "npc_dan", "name": "Daniel"})],
            scenes: vec![ScenarioNode {
                node_id: "scene_001".into(),
                node_type: "scene".into(),
                page_start: Some(8),
                ..Default::default()
            }],
            ..Default::default()
        };
        let p = json!({
            "mission_briefing": {"optional_objectives": [{"objective": "Conduct an experiment."}]},
            "anomaly_gm_only": {"name": "The Hidden Anomaly"}
        });
        let atoms = compile_prep_packet_guard_leaves(&g, &p);
        assert!(
            atoms.is_empty(),
            "anti-fabrication: a stop-word binding to an entity no authored hint names is dropped, got {:?}",
            atoms.iter().map(|a| a.grounding.clone()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn objective_without_lexicon_verb_yields_no_atom() {
        // "Wear a flower crown." — 'wear' is not in the action lexicon ⇒ no atom even
        // though the anomaly hint resolves.
        let g = graph_with_anomaly();
        let atoms = compile_prep_packet_guard_leaves(&g, &packet("Wear a flower crown."));
        assert!(atoms.is_empty(), "fail-closed: no lexicon verb ⇒ no action atom");
    }

    #[test]
    fn missing_mission_briefing_yields_nothing() {
        let g = graph_with_anomaly();
        let empty = json!({"npcs": []});
        assert!(objectives_from_prep_packet(&empty, "the_vault").is_empty());
        assert!(compile_prep_packet_guard_leaves(&g, &empty).is_empty());
    }
}
