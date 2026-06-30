//! EV-3 `progress_offers_v1` — the **deterministic offer-derivation** half of the
//! Progression Evidence Layer (GPT Pro design §6 "回合开始" / §5.4 / §Q2).
//!
//! At turn start the frontier additionally emits a machine-readable
//! [`EvidenceOfferSet`]: the closed set of evidence atoms the GM is **allowed to
//! claim this turn**. EV-4 lets the GM return `progress_claims` against these
//! opaque handles; EV-3 only produces the offers + surfaces their human-readable
//! meanings in the GM prompt.
//!
//! Derivation is **conservative** (design §6 "不把整个词表给 GM"):
//! - the offered atoms = the EV-2 [`EvidenceAtomCatalog`] ∩ **what is surfaced this
//!   turn** = the *current scene's* `referenced_clue_ids` (the authored clues whose
//!   page lands inside the active scene). The whole catalog is never offered.
//! - **fail-closed**: an unknown current scene → no offers; a surfaced clue id with
//!   no catalog atom → no offer; a scene with no surfaced clues → empty set.
//! - **source-grounded**: every offer comes from a catalog atom that already
//!   carries ≥1 `SourceRef`; the meaning cites the authored page.
//! - **ZERO ruleset/module name branching** — structure only (clue id + page).
//!
//! Honest scope note: the EV-2 catalog currently holds only **clue** (`FactLearned`)
//! atoms, so EV-3 offers only surfaced-clue capabilities. The design's "legal
//! navigation targets (`LocationEntered`-kind)" offers require `LocationEntered`
//! atoms in the catalog (a later catalog extension); with none present, that
//! intersection is empty — fail-closed, not fabricated.
//!
//! **OFF == byte-identical baseline**: [`progress_offers_enabled`] is default OFF;
//! the live wiring (`scene_navigation::tiered`) computes nothing and mutates no
//! prompt when it is off. The prompt block is only appended when the derived set is
//! non-empty (see [`render_offer_prompt_block`]), so even an ON turn with no
//! surfaced clue leaves the prompt byte-identical.

use serde_json::Value;
use trpg_model::adventure_ir::{
    BasisKind, CapId, EvidenceAtomCatalog, EvidenceAtomSpec, EvidenceKind, EvidenceOffer,
    EvidenceOfferSet,
};
use trpg_model::ModuleGraph;

use crate::evidence_projection::progress_observable_leaf_catalog_enabled;

const PROGRESS_OFFERS_V1_ENV: &str = "TRPG_PROGRESS_OFFERS_V1";
const PROGRESS_EVIDENCE_V1_ENV: &str = "TRPG_PROGRESS_EVIDENCE_V1";

/// Pure flag parse (env-race-free; mirrors the EV-2 projector flag).
fn flag_on(raw: &str) -> bool {
    let v = raw.trim().to_ascii_lowercase();
    v == "1" || v == "true" || v == "on"
}

/// Whether the EV-3 offer derivation is active. Default OFF == byte-identical
/// baseline (no OfferSet, no prompt mutation). ON when EITHER the master
/// `progress_evidence_v1` OR the `progress_offers_v1` slice flag is truthy.
pub fn progress_offers_enabled() -> bool {
    std::env::var(PROGRESS_OFFERS_V1_ENV)
        .map(|v| flag_on(&v))
        .unwrap_or(false)
        || std::env::var(PROGRESS_EVIDENCE_V1_ENV)
            .map(|v| flag_on(&v))
            .unwrap_or(false)
}

/// The committed-event basis a later claim must cite for an offered atom kind. A
/// learned/revealed clue is admitted by a committed fact carrier this turn; the
/// other kinds map to their natural carrier. (Closed mapping, no name branching.)
fn required_basis_for(kind: EvidenceKind) -> Vec<BasisKind> {
    match kind {
        EvidenceKind::FactLearned | EvidenceKind::FactRevealed => vec![BasisKind::FactCommitted],
        EvidenceKind::LocationEntered => vec![BasisKind::LocationEntered],
        EvidenceKind::ChoiceCommitted => vec![BasisKind::ChoiceCommitted],
        EvidenceKind::ActionResolved
        | EvidenceKind::StateEstablished
        | EvidenceKind::EntityEncountered
        | EvidenceKind::RelationshipChanged
        | EvidenceKind::ResourceChanged => vec![BasisKind::OutcomeCommitted],
    }
}

/// The authored display name for a clue id (source-grounded human label), if the
/// module carries one. Falls back to the id at the call site.
fn clue_name(graph: &ModuleGraph, clue_id: &str) -> Option<String> {
    graph.clues.iter().find_map(|c| {
        let id = c.get("id").and_then(Value::as_str)?;
        if id != clue_id {
            return None;
        }
        c.get("name")
            .or_else(|| c.get("title"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    })
}

/// Human-readable meaning of a clue-learned capability (shown to the GM). Cites the
/// authored label + page; never the (GM-hidden) atom id.
fn clue_meaning(label: &str, page: Option<u32>) -> String {
    match page {
        Some(p) => format!("the player learned the authored clue \"{label}\" (source p{p})"),
        None => format!("the player learned the authored clue \"{label}\""),
    }
}

/// Whether an atom kind is an EV-P3 observable-action capability (NOT a Fact*
/// kind — those are surfaced via the clue path). Only these are offered from the
/// observable-leaf catalog so the clue offers stay the EV-3 path.
fn is_observable_action_kind(kind: EvidenceKind) -> bool {
    matches!(
        kind,
        EvidenceKind::ActionResolved
            | EvidenceKind::StateEstablished
            | EvidenceKind::LocationEntered
            | EvidenceKind::EntityEncountered
    )
}

/// Human-readable, source-grounded meaning of an observable-action capability. Built
/// from the atom's grounding verb-kind + its authored source note (the authored
/// affordance/mechanic text). NEVER leaks the atom_id or any objective id.
fn action_meaning(atom: &EvidenceAtomSpec) -> String {
    let verb = match atom.kind {
        EvidenceKind::ActionResolved => "resolved an authored action",
        EvidenceKind::StateEstablished => "established an authored world state",
        EvidenceKind::LocationEntered => "entered an authored location",
        EvidenceKind::EntityEncountered => "encountered an authored entity",
        _ => "did an authored thing",
    };
    let note = atom
        .source_refs
        .first()
        .and_then(|s| s.note.clone())
        .filter(|n| !n.trim().is_empty());
    match note {
        Some(n) => format!("the player {verb}: \"{}\"", n.trim()),
        None => format!("the player {verb} ({})", atom.grounding),
    }
}

/// Derive the turn-local [`EvidenceOfferSet`] = the EV-2 catalog ∩ the current
/// scene's surfaced clue atoms. Deterministic, fail-closed, source-grounded (see
/// module docs). `turn_id` scopes the opaque [`CapId`]s and the `expires_at`.
pub fn derive_offer_set(
    graph: &ModuleGraph,
    catalog: &EvidenceAtomCatalog,
    current_scene: &str,
    session_id: &str,
    turn_id: &str,
) -> EvidenceOfferSet {
    let mut set = EvidenceOfferSet::new(turn_id);
    let Some(scene) = graph.scenes.iter().find(|s| s.node_id == current_scene) else {
        return set; // fail-closed: unknown current scene → no offers
    };
    for clue_id in &scene.referenced_clue_ids {
        let Some(atom) = catalog.resolve_fact(clue_id) else {
            continue; // fail-closed: surfaced clue with no catalog atom → no offer
        };
        let page = atom.source_refs.first().and_then(|s| s.page);
        let label = clue_name(graph, clue_id).unwrap_or_else(|| clue_id.clone());
        set.push(EvidenceOffer {
            cap_id: CapId::from_parts(session_id, turn_id, &atom.atom_id),
            atom_id: atom.atom_id.clone(),
            kind: atom.kind,
            meaning: clue_meaning(&label, page),
            required_basis: required_basis_for(atom.kind),
            expires_at: turn_id.to_string(),
        });
    }
    // EV-P3: additionally offer observable-action atoms scoped to THIS scene. Flag-
    // guarded so OFF ⇒ no extra offers ⇒ prompt byte-identical. Deduped by cap_id.
    if progress_observable_leaf_catalog_enabled() {
        let scene_tag = format!("scene:{current_scene}");
        for atom in catalog.atoms() {
            if !is_observable_action_kind(atom.kind) {
                continue; // Fact* atoms stay on the surfaced-clue path
            }
            if !atom.bindings.iter().any(|b| b == &scene_tag) {
                continue; // not surfaced in the current scene
            }
            let cap_id = CapId::from_parts(session_id, turn_id, &atom.atom_id);
            if set.offers().iter().any(|o| o.cap_id == cap_id) {
                continue; // dedup by cap_id
            }
            set.push(EvidenceOffer {
                cap_id,
                atom_id: atom.atom_id.clone(),
                kind: atom.kind,
                meaning: action_meaning(atom),
                required_basis: required_basis_for(atom.kind),
                expires_at: turn_id.to_string(),
            });
        }
    }
    set
}

/// Render the GM-facing prompt block for an offer set. The GM is shown ONLY the
/// opaque `cap_id`, the human meaning, and the required basis — never the atom id
/// or any objective id. Returns an **empty string** for an empty set so the caller
/// can keep the prompt byte-identical (the append is guarded on non-empty).
pub fn render_offer_prompt_block(set: &EvidenceOfferSet) -> String {
    if set.is_empty() {
        return String::new();
    }
    let mut s = String::from(
        "PROGRESSION CAPABILITIES (this turn only): if the player's action this turn \
         actually caused one of these, you MAY note it via its handle. You are NOT \
         required to force any of them; ignore those that did not happen.",
    );
    for o in set.offers() {
        let basis = o
            .required_basis
            .iter()
            .map(|b| b.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        s.push_str(&format!(
            "\n- {} = {} [requires basis: {}]",
            o.cap_id.as_str(),
            o.meaning,
            basis
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use trpg_model::ScenarioNode;

    /// Minimal graph: one clue (id+page) + one scene whose page range contains it.
    /// `surfaced` controls whether the scene references the clue (surfaced) or not.
    fn graph(clue_id: &str, page: u32, surfaced: bool) -> ModuleGraph {
        let mut scene = ScenarioNode {
            node_id: "scene_001".into(),
            node_type: "scene".into(),
            title: "Springs Eternal".into(),
            ..Default::default()
        };
        if surfaced {
            scene.referenced_clue_ids = vec![clue_id.to_string()];
        }
        ModuleGraph {
            module_id: "the_vault".into(),
            clues: vec![json!({"id": clue_id, "name": "Aquifer storyboard", "page": page})],
            scenes: vec![scene],
            ..Default::default()
        }
    }

    fn catalog(graph: &ModuleGraph) -> EvidenceAtomCatalog {
        crate::evidence_projection::build_evidence_atom_catalog(graph)
    }

    #[test]
    fn surfaced_clue_yields_one_offer_with_opaque_cap_and_correct_atom_and_basis() {
        let g = graph("clue_aquifer_commercial", 10, true);
        let cat = catalog(&g);
        let set = derive_offer_set(&g, &cat, "scene_001", "sess_a", "turn_5");
        assert_eq!(set.len(), 1, "exactly one offer for the surfaced clue");
        let o = &set.offers()[0];
        // opaque, turn-scoped cap_id ≠ atom_id
        assert!(o.cap_id.as_str().starts_with("cap_"), "opaque handle");
        assert_ne!(o.cap_id.as_str(), o.atom_id.as_str(), "cap_id ≠ atom_id");
        // correct atom
        assert_eq!(
            o.atom_id,
            cat.resolve_fact("clue_aquifer_commercial").unwrap().atom_id,
            "offer maps to the catalog atom"
        );
        assert_eq!(o.kind, EvidenceKind::FactLearned);
        assert_eq!(
            o.required_basis,
            vec![BasisKind::FactCommitted],
            "clue basis = committed fact"
        );
        assert_eq!(o.expires_at, "turn_5", "single-turn capability");
        assert!(
            o.meaning.contains("Aquifer storyboard"),
            "meaning cites the authored label"
        );
        assert!(
            o.meaning.contains("p10"),
            "meaning cites the authored source page"
        );
    }

    #[test]
    fn unsurfaced_atom_yields_no_offer() {
        // The clue IS in the catalog, but the current scene does not surface it.
        let g = graph("clue_aquifer_commercial", 10, false);
        let cat = catalog(&g);
        assert_eq!(cat.len(), 1, "catalog still has the clue atom");
        let set = derive_offer_set(&g, &cat, "scene_001", "sess_a", "turn_5");
        assert!(
            set.is_empty(),
            "conservative: only surfaced atoms are offered"
        );
    }

    #[test]
    fn surfaced_clue_without_catalog_atom_yields_no_offer() {
        // Scene surfaces a clue id that the catalog has no atom for (fail-closed).
        let mut g = graph("clue_aquifer_commercial", 10, true);
        g.scenes[0].referenced_clue_ids = vec!["clue_not_in_catalog".into()];
        let cat = catalog(&g);
        let set = derive_offer_set(&g, &cat, "scene_001", "sess_a", "turn_5");
        assert!(
            set.is_empty(),
            "fail-closed: surfaced ref with no atom → no offer"
        );
    }

    #[test]
    fn unknown_current_scene_yields_empty_set() {
        let g = graph("clue_a", 10, true);
        let cat = catalog(&g);
        let set = derive_offer_set(&g, &cat, "scene_does_not_exist", "sess_a", "turn_5");
        assert!(
            set.is_empty(),
            "fail-closed: unknown current scene → no offers"
        );
    }

    #[test]
    fn cap_id_never_equals_atom_id_or_objective_id() {
        let g = graph("clue_a", 10, true);
        let cat = catalog(&g);
        let set = derive_offer_set(&g, &cat, "scene_001", "sess_a", "turn_5");
        let o = &set.offers()[0];
        assert_ne!(o.cap_id.as_str(), o.atom_id.as_str());
        assert!(!o.cap_id.as_str().starts_with("atom:"));
        // an objective id is a dotted slug; the opaque handle is not.
        assert_ne!(o.cap_id.as_str(), "obj.advance.scene_001");
        assert!(
            !o.cap_id.as_str().contains('.'),
            "opaque handle is not a dotted objective slug"
        );
    }

    #[test]
    fn render_block_empty_for_empty_set_keeps_prompt_byte_identical() {
        let base = "BASE NAV PROMPT\nline2".to_string();
        let empty = EvidenceOfferSet::new("turn_5");
        let block = render_offer_prompt_block(&empty);
        assert!(block.is_empty(), "empty set ⇒ empty block");
        // mirror the tiered.rs append guard exactly
        let mut prompt = base.clone();
        if !block.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(&block);
        }
        assert_eq!(
            prompt, base,
            "empty offer set ⇒ prompt bytes identical (OFF==baseline)"
        );
    }

    #[test]
    fn render_block_shows_cap_and_meaning_but_never_atom_id() {
        let g = graph("clue_aquifer_commercial", 10, true);
        let cat = catalog(&g);
        let set = derive_offer_set(&g, &cat, "scene_001", "sess_a", "turn_5");
        let block = render_offer_prompt_block(&set);
        let o = &set.offers()[0];
        assert!(
            block.contains(o.cap_id.as_str()),
            "block shows the opaque handle"
        );
        assert!(
            block.contains("Aquifer storyboard"),
            "block shows the human meaning"
        );
        assert!(
            block.contains("fact_committed"),
            "block shows the required basis"
        );
        assert!(
            !block.contains(o.atom_id.as_str()),
            "block NEVER leaks the atom_id (GM only sees the opaque handle)"
        );
        assert!(
            !block.contains("atom:"),
            "no atom id prefix anywhere in the prompt block"
        );
    }

    #[test]
    fn flag_defaults_off() {
        assert!(
            !flag_on("0") && !flag_on("false") && !flag_on("off") && !flag_on(""),
            "only 1/true/on arm the offers"
        );
        assert!(flag_on("1") && flag_on("true") && flag_on("on"));
    }

    // ───────────────────────── EV-P3 observable-action offers ─────────────────

    use std::sync::Mutex;
    /// Serializes the env-mutating EV-P3 tests so the process-global flag never
    /// races a parallel test.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// A graph with one surfaced clue AND a director affordance whose target
    /// (the clue name "Aquifer") resolves — so the catalog gains an action atom
    /// tagged to the entry scene.
    fn graph_with_affordance() -> ModuleGraph {
        let mut scene = ScenarioNode {
            node_id: "scene_001".into(),
            node_type: "scene".into(),
            title: "Springs Eternal".into(),
            page_start: Some(8),
            ..Default::default()
        };
        scene.referenced_clue_ids = vec!["clue_aquifer_commercial".into()];
        ModuleGraph {
            module_id: "the_vault".into(),
            clues: vec![json!({"id": "clue_aquifer_commercial", "name": "Aquifer", "page": 8})],
            scenes: vec![scene],
            director_facilitation: Some(trpg_model::DirectorModuleConfig {
                affordance_items: vec![trpg_model::DirectorAffordanceItem {
                    description: "Investigate the Aquifer commercial.".into(),
                    implies_vectors: vec!["Aquifer".into()],
                }],
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn off_offers_only_surfaced_clue_and_prompt_byte_identical() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("TRPG_PROGRESS_OBSERVABLE_LEAF_CATALOG_V1");
        std::env::remove_var("TRPG_PROGRESS_EVIDENCE_V1");
        let graph = graph_with_affordance();
        // catalog built OFF ⇒ clue-only (no action atoms)
        let cat = crate::evidence_projection::build_evidence_atom_catalog(&graph);
        assert_eq!(cat.len(), 1, "OFF ⇒ clue-only catalog (EV-2 baseline)");
        let set = derive_offer_set(&graph, &cat, "scene_001", "sess_a", "turn_5");
        assert_eq!(set.len(), 1, "OFF ⇒ only the surfaced clue offer");
        assert_eq!(set.offers()[0].kind, EvidenceKind::FactLearned);
    }

    #[test]
    fn on_offers_include_action_atom_for_scene() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("TRPG_PROGRESS_OBSERVABLE_LEAF_CATALOG_V1", "1");
        let graph = graph_with_affordance();
        let cat = crate::evidence_projection::build_evidence_atom_catalog(&graph);
        assert!(
            cat.len() >= 2,
            "ON ⇒ clue + ≥1 action atom, got {}",
            cat.len()
        );
        let set = derive_offer_set(&graph, &cat, "scene_001", "sess_a", "turn_5");
        std::env::remove_var("TRPG_PROGRESS_OBSERVABLE_LEAF_CATALOG_V1");
        let has_action = set
            .offers()
            .iter()
            .any(|o| o.kind == EvidenceKind::ActionResolved);
        assert!(
            has_action,
            "ON ⇒ scene_001 gains an action offer (not just the clue)"
        );
        // opaque + source-grounded meaning, never an atom id leaked
        for o in set.offers() {
            assert!(o.cap_id.as_str().starts_with("cap_"));
            assert!(!o.meaning.contains("atom:"), "meaning never leaks atom id");
        }
        let action = set
            .offers()
            .iter()
            .find(|o| o.kind == EvidenceKind::ActionResolved)
            .unwrap();
        assert_eq!(
            action.required_basis,
            vec![BasisKind::OutcomeCommitted],
            "action capability requires a committed outcome"
        );
        assert!(
            action
                .meaning
                .contains("Investigate the Aquifer commercial"),
            "meaning cites the authored affordance text: {}",
            action.meaning
        );
    }
}
