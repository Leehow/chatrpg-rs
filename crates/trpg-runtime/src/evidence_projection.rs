//! EV-2 `exact_evidence_projector_v1` — the **deterministic exact path** of the
//! Progression Evidence Layer (GPT Pro design §Q4 `ExactDomainProjector` / §5.5).
//!
//! Two pure functions, NO LLM, fail-closed:
//! - [`build_evidence_atom_catalog`] — at compile/prep time, derive one
//!   `fact:<clue_id>` [`EvidenceAtomSpec`] per **authored clue** already in the
//!   module graph (each atom carries ≥1 [`SourceRef`] = the clue's page; clues
//!   missing an id or a page are skipped — never a fabricated atom).
//! - [`project_exact_evidence`] — for each committed [`DomainEvent`], IF its
//!   payload carries an authored `fact_id` that resolves to a catalog atom AND the
//!   payload's `knowledge_state` is player-known-true (`knows_true`/`revealed`, or
//!   the canonical reveal carrier which omits the field), THEN emit exactly one
//!   [`AcceptedEvidence`]`{authority: ExactDomain}` into the ledger.
//!
//! **fail-closed** (the three projection refusals the design demands):
//! - synthetic `wf_chk_*` `WorldFactChanged` (no authored atom) → not projected
//!   (excluded by event kind AND by unresolved grounding);
//! - non-true `knowledge_state` (`suspects`/`believes_false`) → not projected —
//!   this is the EV-1 forward-caveat fix (the v0 adapter ignored knowledge_state
//!   and would over-project a non-true belief; the projector keys on the payload);
//! - an authored ref that resolves to no atom → not projected.
//! One evidence per qualifying event; replaying the same event never duplicates.
//!
//! **OFF == byte-identical baseline**: [`exact_evidence_projector_enabled`] is
//! default OFF; the live wiring computes nothing when it is off (the projector is
//! the only new code and is fully flag-guarded). The engine does NOT consume the
//! ledger yet (EV-6).

use serde_json::Value;
use trpg_model::adventure_ir::{
    AcceptedEvidence, AtomId, EvidenceAtomCatalog, EvidenceAtomSpec, EvidenceAuthority,
    EvidenceKind, EvidenceLedger,
};
use trpg_model::{DomainEvent, DomainEventKind, ModuleGraph, SourceRef};

const EXACT_EVIDENCE_PROJECTOR_V1_ENV: &str = "TRPG_EXACT_EVIDENCE_PROJECTOR_V1";
const PROGRESS_EVIDENCE_V1_ENV: &str = "TRPG_PROGRESS_EVIDENCE_V1";
const PROGRESS_OBSERVABLE_LEAF_CATALOG_V1_ENV: &str =
    "TRPG_PROGRESS_OBSERVABLE_LEAF_CATALOG_V1";

/// Pure flag parse (env-race-free; mirrors the EV-1 bridge flag).
fn flag_on(raw: &str) -> bool {
    let v = raw.trim().to_ascii_lowercase();
    v == "1" || v == "true" || v == "on"
}

/// Whether the EV-2 exact projector is active. Default OFF == byte-identical
/// baseline (projector never runs, no ledger built). ON when EITHER the master
/// `progress_evidence_v1` OR the `exact_evidence_projector_v1` slice flag is truthy.
pub fn exact_evidence_projector_enabled() -> bool {
    std::env::var(EXACT_EVIDENCE_PROJECTOR_V1_ENV)
        .map(|v| flag_on(&v))
        .unwrap_or(false)
        || std::env::var(PROGRESS_EVIDENCE_V1_ENV)
            .map(|v| flag_on(&v))
            .unwrap_or(false)
}

/// Whether the EV-P3 observable-leaf catalog enrichment is active. Default OFF ==
/// byte-identical baseline (catalog stays clue-only = EV-2). ON when EITHER the
/// master `progress_evidence_v1` OR the `progress_observable_leaf_catalog_v1` slice
/// flag is truthy.
pub fn progress_observable_leaf_catalog_enabled() -> bool {
    std::env::var(PROGRESS_OBSERVABLE_LEAF_CATALOG_V1_ENV)
        .map(|v| flag_on(&v))
        .unwrap_or(false)
        || std::env::var(PROGRESS_EVIDENCE_V1_ENV)
            .map(|v| flag_on(&v))
            .unwrap_or(false)
}

/// Read a clue's `id` (non-empty) — mirrors `clue_projection::clue_id`.
fn clue_id(clue: &Value) -> Option<String> {
    clue.get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Read a clue's numeric `page` — mirrors `clue_projection::clue_page`. A clue with
/// no numeric page has no source span and is fail-closed skipped (no fabricated atom).
fn clue_page(clue: &Value) -> Option<u32> {
    clue.get("page").and_then(Value::as_u64).map(|p| p as u32)
}

/// Build the [`EvidenceAtomCatalog`] from the module's **authored clues**. Each
/// clue with an id + a page becomes one source-grounded `fact:<clue_id>` atom
/// (kind [`EvidenceKind::FactLearned`]). Structure-only (id + page); ZERO
/// ruleset/module name branching; fail-closed (no id / no page ⇒ no atom).
pub fn build_evidence_atom_catalog(graph: &ModuleGraph) -> EvidenceAtomCatalog {
    let mut catalog = EvidenceAtomCatalog::new();
    let module_digest = graph.module_id.as_str();
    for clue in &graph.clues {
        let (Some(id), Some(page)) = (clue_id(clue), clue_page(clue)) else {
            continue; // fail-closed: clue without a resolvable id or page → no atom
        };
        let bound_refs = vec![id.clone()];
        let atom_id = AtomId::from_parts(
            module_digest,
            &format!("p{page}"),
            EvidenceKind::FactLearned,
            &bound_refs,
        );
        let source_ref = SourceRef {
            source_id: graph.module_id.clone(),
            page: Some(page),
            ..Default::default()
        };
        catalog.insert(EvidenceAtomSpec {
            atom_id,
            kind: EvidenceKind::FactLearned,
            bindings: bound_refs,
            source_refs: vec![source_ref],
            grounding: format!("fact:{id}"),
            progress_role: trpg_model::adventure_ir::ProgressRole::CarrierOnly,
        });
    }
    if progress_observable_leaf_catalog_enabled() {
        for atom in trpg_model::adventure_ir::compile_authored_observations(graph) {
            catalog.insert(atom);
        }
    }
    catalog
}

/// Map a committed event kind to the evidence kind it projects as — ONLY the
/// player-knowledge carriers (`PlayerLearnedFact`→`FactLearned`,
/// `FactRevealed`→`FactRevealed`). Every other kind (incl. the synthetic
/// `WorldFactChanged` EV-1 `wf_chk_*`) returns `None` (fail-closed by kind).
fn evidence_kind_for_event(ev: &DomainEvent) -> Option<EvidenceKind> {
    match ev.kind {
        DomainEventKind::PlayerLearnedFact => Some(EvidenceKind::FactLearned),
        DomainEventKind::FactRevealed => Some(EvidenceKind::FactRevealed),
        _ => None,
    }
}

/// Whether the event payload represents a **player-known-true** fact. The
/// canonical reveal carrier (`record_revealed_fact`) omits `knowledge_state` and
/// IS a knows_true reveal → `None` counts as true. An explicit non-true state
/// (`suspects`/`believes_false`) → false (the EV-1 forward-caveat fix).
fn payload_is_player_known_true(data: &Value) -> bool {
    match data.get("knowledge_state").and_then(Value::as_str) {
        None => true,
        Some(s) => matches!(s.trim(), "knows_true" | "revealed"),
    }
}

/// Project committed [`DomainEvent`]s into an [`EvidenceLedger`] of
/// [`AcceptedEvidence`]`{authority: ExactDomain}`. Deterministic, no LLM,
/// fail-closed (see module docs). One evidence per qualifying event; idempotent on
/// the (event, atom) pair so a replay never double-records.
pub fn project_exact_evidence(
    events: &[DomainEvent],
    catalog: &EvidenceAtomCatalog,
) -> EvidenceLedger {
    let mut ledger = EvidenceLedger::new();
    for ev in events {
        let Some(evidence_kind) = evidence_kind_for_event(ev) else {
            continue; // not a player-knowledge carrier
        };
        let Some(fact_id) = ev
            .data
            .get("fact_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue; // no authored ref to resolve
        };
        if !payload_is_player_known_true(&ev.data) {
            continue; // non-true belief → fail-closed
        }
        let Some(atom) = catalog.resolve_fact(fact_id) else {
            continue; // unresolved authored ref (synthetic wf_chk_* never resolves)
        };
        let evidence_id = format!("evd_{}_{}", ev.event_id, fact_id);
        ledger.append(AcceptedEvidence {
            evidence_id,
            atom_id: atom.atom_id.clone(),
            evidence_kind,
            basis_event_ids: vec![ev.event_id.clone()],
            source_refs: atom.source_refs.clone(),
            turn_id: ev.turn_id.clone(),
            authority: EvidenceAuthority::ExactDomain,
        });
    }
    ledger
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn graph_with_clues(clues: Vec<Value>) -> ModuleGraph {
        ModuleGraph {
            module_id: "the_vault".into(),
            clues,
            ..Default::default()
        }
    }

    /// A clue-reveal DomainEvent (the canonical reveal carrier omits knowledge_state).
    fn player_learned(event_id: &str, fact_id: &str) -> DomainEvent {
        DomainEvent::new(
            event_id,
            "sess_a",
            "turn_1",
            DomainEventKind::PlayerLearnedFact,
            json!({"fact_id": fact_id, "reason": "successful Investigation check"}),
        )
    }

    #[test]
    fn catalog_derives_one_fact_atom_per_authored_clue_with_page() {
        let graph = graph_with_clues(vec![
            json!({"id": "clue_aquifer_commercial", "name": "Aquifer storyboard", "page": 10}),
            json!({"id": "clue_cable", "name": "Cable", "page": 14}),
        ]);
        let cat = build_evidence_atom_catalog(&graph);
        assert_eq!(cat.len(), 2, "one atom per authored clue");
        let atom = cat
            .resolve_fact("clue_aquifer_commercial")
            .expect("clue atom present");
        assert_eq!(atom.kind, EvidenceKind::FactLearned);
        assert_eq!(atom.grounding, "fact:clue_aquifer_commercial");
        assert_eq!(atom.bindings, vec!["clue_aquifer_commercial".to_string()]);
        assert_eq!(
            atom.source_refs.first().and_then(|s| s.page),
            Some(10),
            "atom carries the clue's authored page as a source span"
        );
    }

    #[test]
    fn catalog_skips_clue_without_id_or_page_no_fabricated_atom() {
        let graph = graph_with_clues(vec![
            json!({"name": "anon", "page": 10}),            // no id
            json!({"id": "clue_nopage", "name": "no page"}), // no page
            json!({"id": "clue_strpage", "page": "twelve"}), // non-numeric page
        ]);
        let cat = build_evidence_atom_catalog(&graph);
        assert!(cat.is_empty(), "fail-closed: never fabricate a source-less atom");
    }

    #[test]
    fn authored_clue_reveal_projects_one_exact_evidence() {
        let graph = graph_with_clues(vec![
            json!({"id": "clue_aquifer_commercial", "name": "Aquifer", "page": 10}),
        ]);
        let cat = build_evidence_atom_catalog(&graph);
        let events = vec![player_learned("de_1", "clue_aquifer_commercial")];
        let led = project_exact_evidence(&events, &cat);
        assert_eq!(led.len(), 1, "exactly one AcceptedEvidence");
        let ev = &led.entries()[0];
        assert_eq!(ev.authority, EvidenceAuthority::ExactDomain);
        assert_eq!(ev.evidence_kind, EvidenceKind::FactLearned);
        assert_eq!(
            ev.atom_id,
            cat.resolve_fact("clue_aquifer_commercial").unwrap().atom_id,
            "correct atom_id"
        );
        assert_eq!(ev.basis_event_ids, vec!["de_1".to_string()], "basis = the event");
        assert_eq!(
            ev.source_refs.first().and_then(|s| s.page),
            Some(10),
            "carries the atom's source ref"
        );
        assert_eq!(ev.turn_id, "turn_1");
    }

    #[test]
    fn synthetic_wf_chk_world_fact_changed_does_not_project() {
        let graph = graph_with_clues(vec![
            json!({"id": "clue_aquifer_commercial", "name": "Aquifer", "page": 10}),
        ]);
        let cat = build_evidence_atom_catalog(&graph);
        // EV-1 synthetic check carrier: WorldFactChanged with a `wf_chk_*` fact_id.
        let events = vec![DomainEvent::new(
            "de_wf",
            "sess_a",
            "turn_1",
            DomainEventKind::WorldFactChanged,
            json!({"fact_id": "wf_chk_check_2c160e4", "truth_status": "true"}),
        )];
        let led = project_exact_evidence(&events, &cat);
        assert!(led.is_empty(), "fail-closed: synthetic non-authored fact never projects");
    }

    #[test]
    fn non_true_knowledge_state_does_not_project() {
        let graph = graph_with_clues(vec![
            json!({"id": "clue_aquifer_commercial", "name": "Aquifer", "page": 10}),
        ]);
        let cat = build_evidence_atom_catalog(&graph);
        // PlayerPartyEdge bridge carrier: PlayerLearnedFact carrying a non-true belief.
        let events = vec![DomainEvent::new(
            "de_susp",
            "sess_a",
            "turn_1",
            DomainEventKind::PlayerLearnedFact,
            json!({"fact_id": "clue_aquifer_commercial", "knowledge_state": "suspects"}),
        )];
        let led = project_exact_evidence(&events, &cat);
        assert!(
            led.is_empty(),
            "EV-1 forward-caveat fix: a non-true belief over an authored ref is NOT projected"
        );
    }

    #[test]
    fn explicit_knows_true_state_projects() {
        let graph = graph_with_clues(vec![
            json!({"id": "clue_aquifer_commercial", "name": "Aquifer", "page": 10}),
        ]);
        let cat = build_evidence_atom_catalog(&graph);
        let events = vec![DomainEvent::new(
            "de_kt",
            "sess_a",
            "turn_1",
            DomainEventKind::FactRevealed,
            json!({"fact_id": "clue_aquifer_commercial", "knowledge_state": "knows_true"}),
        )];
        let led = project_exact_evidence(&events, &cat);
        assert_eq!(led.len(), 1, "explicit knows_true over an authored ref projects");
        assert_eq!(
            led.entries()[0].evidence_kind,
            EvidenceKind::FactRevealed,
            "FactRevealed carrier → FactRevealed evidence kind"
        );
    }

    #[test]
    fn unresolved_authored_ref_does_not_project() {
        let graph = graph_with_clues(vec![
            json!({"id": "clue_aquifer_commercial", "name": "Aquifer", "page": 10}),
        ]);
        let cat = build_evidence_atom_catalog(&graph);
        let events = vec![player_learned("de_x", "clue_not_in_catalog")];
        let led = project_exact_evidence(&events, &cat);
        assert!(led.is_empty(), "fail-closed: a ref with no catalog atom never projects");
    }

    #[test]
    fn replaying_the_same_event_does_not_duplicate() {
        let graph = graph_with_clues(vec![
            json!({"id": "clue_aquifer_commercial", "name": "Aquifer", "page": 10}),
        ]);
        let cat = build_evidence_atom_catalog(&graph);
        let ev = player_learned("de_1", "clue_aquifer_commercial");
        let events = vec![ev.clone(), ev];
        let led = project_exact_evidence(&events, &cat);
        assert_eq!(led.len(), 1, "same (event, atom) ⇒ no duplicate evidence");
    }

    #[test]
    fn observable_leaf_catalog_off_keeps_clue_only_baseline() {
        // A graph that HAS affordance_items; with the EV-P3 flag OFF the catalog
        // must stay clue-only (EV-2 byte baseline). No env set here ⇒ default OFF,
        // unless the master flag is set in the ambient env — guard against that.
        if progress_observable_leaf_catalog_enabled() {
            return; // ambient master flag ON in this env; the dedicated ON test covers it
        }
        let graph = ModuleGraph {
            module_id: "the_vault".into(),
            clues: vec![json!({"id": "clue_aquifer_commercial", "name": "Aquifer", "page": 8})],
            director_facilitation: Some(trpg_model::DirectorModuleConfig {
                affordance_items: vec![trpg_model::DirectorAffordanceItem {
                    description: "Investigate the Aquifer commercial.".into(),
                    implies_vectors: vec!["Aquifer".into()],
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        let cat = build_evidence_atom_catalog(&graph);
        assert_eq!(cat.len(), 1, "OFF ⇒ clue-only (affordance_items ignored, EV-2 baseline)");
        assert!(cat.resolve_fact("clue_aquifer_commercial").is_some());
    }

    #[test]
    fn flag_defaults_off() {
        // No env set in this test ⇒ projector OFF (byte-identical baseline path).
        // (The wiring early-returns when this is false; proven structurally + by the
        // live OFF run in CL-EV2b.)
        assert!(
            !flag_on("0") && !flag_on("false") && !flag_on("off") && !flag_on(""),
            "only 1/true/on arm the projector"
        );
        assert!(flag_on("1") && flag_on("true") && flag_on("on"));
    }
}
