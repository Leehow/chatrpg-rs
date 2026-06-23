//! CL-P1b deterministic live proof (in-crate so it drives the MAIN-GM path through the
//! `pub(crate)` markup parser + audit evaluator): EV-P1 `progress_evidence_audit_required_v1`
//! end-to-end on the REAL `the_vault` parsed graph (:54347), no LLM.
//!
//! Proves the mandatory-audit producer protocol fires through the MAIN GM pipeline pieces:
//!   1. A main-GM-style TurnDocument carrying a COMPLETE `[evidence_audit]` (a decision for
//!      EVERY offered capability, echoing the OfferSet id) parses via the SAME
//!      `parse_turn_document` the live agent loop uses → one closed-schema `EvidenceAudit`; the
//!      cap/JSON NEVER reaches `player_text`. The one Observed decision cites a basis that
//!      resolves to a REAL committed `PlayerLearnedFact` (this turn) ⇒ exactly ONE
//!      `AcceptedEvidence{authority: GmWitnessed}`, atom RUST-resolved from the OfferSet, and
//!      `audit_completeness == 1.0`.
//!   2. An INCOMPLETE audit (one offered cap omitted) ⇒ `ProducerProtocolFailure`
//!      (`IncompleteDecisions`), NO admission — never a silent "none" (the EV-P1 fix).
//!
//! Shadow: the engine does NOT consume the ledger (EV-APPLY); no objective completes. The real
//! ~8-turn LLM play that proves the MAIN GM actually EMITS a complete audit is the SUPERVISOR
//! step. No `DATABASE_URL` ⇒ SKIP (fail-closed). Anti-false-green: a real run prints `RAN:` +
//! counts + `PASS`; only seeing `SKIP` = not verified.
//!
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!   CARGO_TARGET_DIR=target-air cargo test -p trpg-gm --lib evidence_audit_live -- --nocapture

use crate::evidence_audit::{evaluate_gm_audit, ProtocolFailureKind};
use crate::turn_markup_parser::parse_turn_document;
use std::collections::BTreeMap;
use trpg_model::adventure_ir::{
    EvidenceAudit, EvidenceAuthority, NonEmpty, NotObservedReason, TurnLocalRef, WitnessDecision,
};
use trpg_model::{DomainEvent, DomainEventKind};
use trpg_runtime::{build_evidence_atom_catalog, derive_offer_set, project_clues_onto_scenes};

const VAULT: &str = "triangle_agency.the_vault";
const SESSION: &str = "sess_evp1_live";
// A request-style turn_id (NOT `turn_{count}`): the SAME value stamped on the committed event.
const TURN: &str = "turn-evp1-real-0001";

#[tokio::test]
async fn the_vault_main_gm_audit_admits_through_live_pipeline_and_flags_incomplete() {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP: DATABASE_URL unset (need live rulesets DB :54347)");
            return;
        }
    };
    let db = trpg_db::Db::connect(&url).await.expect("connect live DB");
    let mut graph = db
        .load_module_graph(VAULT)
        .await
        .expect("query ok")
        .expect("the_vault module graph present in parsed_bundles");

    // ---- real OfferSet for a surfaced-clue scene (SAME wiring as derive_evidence_offers) ----
    let _ = project_clues_onto_scenes(&mut graph);
    let catalog = build_evidence_atom_catalog(&graph);
    let scene = graph
        .scenes
        .iter()
        .find(|s| !s.referenced_clue_ids.is_empty())
        .expect("at least one the_vault scene surfaces a clue after projection")
        .clone();
    let offer_set = derive_offer_set(&graph, &catalog, &scene.node_id, SESSION, TURN);
    assert!(!offer_set.is_empty(), "surfaced-clue scene yields ≥1 offer");
    let offer0 = offer_set.offers()[0].clone();
    // The clue offer0's atom is bound to (Rust-side; the GM never sees the atom or clue id).
    let clue_id = catalog
        .resolve_atom_id(&offer0.atom_id)
        .expect("offer atom is in the catalog")
        .bindings[0]
        .clone();

    // ---- a REAL committed PlayerLearnedFact (this turn, offer0's clue): append + read back ----
    let event_id = format!("de_evp1_live_{SESSION}_{TURN}_{clue_id}");
    let committed = DomainEvent::new(
        &event_id,
        SESSION,
        TURN,
        DomainEventKind::PlayerLearnedFact,
        serde_json::json!({"fact_id": clue_id, "reason": "successful Investigation check"}),
    );
    db.append_domain_event(&committed)
        .await
        .expect("append committed PlayerLearnedFact");
    let turn_events: Vec<DomainEvent> = db
        .list_domain_events(SESSION, 5000)
        .await
        .expect("list domain events")
        .into_iter()
        .filter(|e| e.turn_id == TURN)
        .collect();
    let commit_idx = turn_events
        .iter()
        .position(|e| e.event_id == event_id)
        .expect("the just-committed event is in this turn's events");

    // ---- build a COMPLETE audit: offer0 observed (cites the committed event); any further
    //      offered caps not_observed. Echo the OfferSet id (Rust checks it). ----
    let mut decisions: BTreeMap<_, _> = BTreeMap::new();
    decisions.insert(
        offer0.cap_id.clone(),
        WitnessDecision::Observed {
            basis: NonEmpty::from_vec(vec![TurnLocalRef::Commit(commit_idx)]).unwrap(),
        },
    );
    for o in offer_set.offers().iter().skip(1) {
        decisions.insert(
            o.cap_id.clone(),
            WitnessDecision::NotObserved {
                reason: NotObservedReason::MerelyImplied,
            },
        );
    }
    let complete = EvidenceAudit {
        offer_set_id: offer_set.id(),
        decisions,
    };

    // ---- prove the live MARKUP path: serialize → `[evidence_audit]` tag → parse → evaluate ----
    let gm_output = format!(
        "[narration]你接通终端，地下蓄水层的商业储运图谱在屏幕上铺开。[/narration]\
         [evidence_audit]{}[/evidence_audit]",
        serde_json::to_string(&complete).unwrap(),
    );
    let doc = parse_turn_document(&gm_output);
    assert!(doc.evidence_audit.is_some(), "main-GM markup parses one EvidenceAudit");
    assert!(!doc.player_text().contains("evidence_audit"));
    assert!(!doc.player_text().contains(offer0.cap_id.as_str()));

    let out = evaluate_gm_audit(
        SESSION,
        TURN,
        &offer_set,
        &catalog,
        &turn_events,
        doc.evidence_audit.as_ref(),
    );
    assert_eq!(out.telemetry.completeness, 1.0, "every offered cap had exactly one decision");
    assert_eq!(out.telemetry.protocol_failure, None, "complete audit ⇒ no protocol failure");
    assert_eq!(out.telemetry.observed, 1, "exactly one observed decision");
    assert_eq!(out.ledger.len(), 1, "the observed decision ⇒ one AcceptedEvidence");
    let ev = &out.ledger.entries()[0];
    assert_eq!(ev.authority, EvidenceAuthority::GmWitnessed);
    assert_eq!(ev.atom_id, offer0.atom_id, "atom Rust-resolved from the OfferSet (GM never named it)");
    assert_eq!(ev.basis_event_ids, vec![event_id.clone()]);
    assert_eq!(ev.turn_id, TURN);
    assert!(!ev.source_refs.is_empty(), "source-grounded");

    // ---- an INCOMPLETE audit (omit offer0's decision) ⇒ ProducerProtocolFailure, no admit ----
    let mut incomplete_decisions: BTreeMap<_, _> = BTreeMap::new();
    for o in offer_set.offers().iter().skip(1) {
        incomplete_decisions.insert(
            o.cap_id.clone(),
            WitnessDecision::NotObserved {
                reason: NotObservedReason::Failed,
            },
        );
    }
    let incomplete = EvidenceAudit {
        offer_set_id: offer_set.id(),
        decisions: incomplete_decisions,
    };
    let inc_out = evaluate_gm_audit(
        SESSION,
        TURN,
        &offer_set,
        &catalog,
        &turn_events,
        Some(&incomplete),
    );
    assert_eq!(inc_out.ledger.len(), 0, "incomplete audit ⇒ no admission (fail-closed)");
    assert_eq!(
        inc_out.telemetry.protocol_failure,
        Some(ProtocolFailureKind::IncompleteDecisions),
        "missing a decision ⇒ ProducerProtocolFailure, never a silent none"
    );

    // ---- and a MISSING audit (None) ⇒ MissingAudit failure (the EV-4R silent-omit, now measured) ----
    let none_out = evaluate_gm_audit(SESSION, TURN, &offer_set, &catalog, &turn_events, None);
    assert_eq!(
        none_out.telemetry.protocol_failure,
        Some(ProtocolFailureKind::MissingAudit),
        "no audit at all ⇒ MissingAudit (not none)"
    );

    println!(
        "RAN: the_vault EV-P1 mandatory audit — scene={} clue={} offers={} commit_idx={} \
         complete(completeness={} observed={} admitted={} GmWitnessed atom={} page={:?}) \
         incomplete_failure={:?} missing_failure={:?} PASS",
        scene.node_id,
        clue_id,
        offer_set.len(),
        commit_idx,
        out.telemetry.completeness,
        out.telemetry.observed,
        out.ledger.len(),
        ev.atom_id.as_str(),
        ev.source_refs[0].page,
        inc_out.telemetry.protocol_failure,
        none_out.telemetry.protocol_failure,
    );
}
