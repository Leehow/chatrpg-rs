//! CL-EV4Rb deterministic live proof (in-crate so it can drive the MAIN-GM path through the
//! `pub(crate)` markup parser + admission helper): EV-4R `progress_claims_on_gm_v1` end-to-end on
//! the REAL `the_vault` parsed graph (:54347), no LLM.
//!
//! Proves the relocated GM↔Rust claim loop fires through the MAIN GM pipeline pieces:
//!   1. A main-GM-STYLE TurnDocument (real `[narration]…[/narration][progress_claims]…
//!      [/progress_claims]` markup) parses via the SAME `parse_turn_document` the live agent loop
//!      uses → one closed-schema `EvidenceClaim`; the cap/JSON NEVER reaches `player_text`.
//!   2. The claim cites a basis that resolves to a REAL committed `PlayerLearnedFact` — appended
//!      to the DB and read back via `list_domain_events` + the SAME turn-local filter the wiring
//!      applies — whose `turn_id` == the turn used to mint the cap + admit (the ALIGNMENT fix:
//!      a derived `turn_{count}` would mismatch and the gateway would reject with
//!      CausationMismatch). ⇒ exactly ONE `AcceptedEvidence{authority: GmWitnessed}`, atom
//!      RUST-resolved from the OfferSet (the GM never named it), source-grounded.
//!   3. A misaligned admission (cap minted for a different turn than the committed event carries)
//!      ⇒ rejected (CausationMismatch / Expired) — never admitted. Alignment is load-bearing.
//!
//! Shadow: the engine does NOT consume the ledger (EV-6); no objective completes. The real
//! ~8-turn LLM play that proves the MAIN GM actually EMITS an admittable claim is the SUPERVISOR
//! step. No `DATABASE_URL` ⇒ SKIP (fail-closed). Anti-false-green: a real run prints `RAN:` +
//! counts + `PASS`; only seeing `SKIP` = not verified.
//!
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!   CARGO_TARGET_DIR=target-air cargo test -p trpg-gm --lib evidence_claims_live -- --nocapture

use crate::evidence_claims::admit_gm_claims;
use crate::turn_markup_parser::parse_turn_document;
use trpg_model::adventure_ir::EvidenceAuthority;
use trpg_model::{DomainEvent, DomainEventKind};
use trpg_runtime::{
    build_evidence_atom_catalog, derive_offer_set, project_clues_onto_scenes, RejectionReason,
};

const VAULT: &str = "triangle_agency.the_vault";
const SESSION: &str = "sess_ev4r_live";
// A request-style turn_id (NOT `turn_{count}`): the SAME value stamped on the committed event.
const TURN: &str = "turn-ev4r-real-0001";

#[tokio::test]
async fn the_vault_main_gm_claim_admits_through_live_pipeline_with_turn_id_alignment() {
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
    let offer = offer_set.offers()[0].clone();
    // The clue the offered atom is bound to (Rust-side; the GM never sees the atom or clue id).
    let clue_id = catalog
        .resolve_atom_id(&offer.atom_id)
        .expect("offer atom is in the catalog")
        .bindings[0]
        .clone();

    // ---- a REAL committed PlayerLearnedFact (this turn, this clue): append + read back ----
    let event_id = format!("de_ev4r_live_{SESSION}_{TURN}_{clue_id}");
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
    // Read back through the SAME turn-local filter the live admission applies.
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
    // Alignment FACT: the committed event's turn_id == TURN (the cap-minting / admission turn).
    assert_eq!(
        turn_events[commit_idx].turn_id, TURN,
        "the real committed event's turn_id must equal the cap/admission turn (alignment)"
    );

    // ---- a MAIN-GM-style TurnDocument carrying the valid claim → parse → admit ----
    let gm_output = format!(
        "[narration]你接通终端，地下蓄水层的商业储运图谱在屏幕上铺开。[/narration]\
         [progress_claims][{{\"cap_id\":\"{}\",\"basis\":[\"commit:{}\"]}}][/progress_claims]",
        offer.cap_id.as_str(),
        commit_idx,
    );
    let doc = parse_turn_document(&gm_output);
    assert_eq!(doc.progress_claims.len(), 1, "main-GM markup parses one claim");
    // The cap handle / JSON never reaches the player.
    assert!(!doc.player_text().contains("progress_claims"));
    assert!(!doc.player_text().contains(offer.cap_id.as_str()));

    let (ledger, decisions) = admit_gm_claims(
        SESSION,
        TURN,
        &offer_set,
        &catalog,
        &turn_events,
        &doc.progress_claims,
        &trpg_model::adventure_ir::EvidenceLedger::new(),
    );
    assert_eq!(ledger.len(), 1, "valid main-GM claim ⇒ one AcceptedEvidence");
    let ev = &ledger.entries()[0];
    assert_eq!(ev.authority, EvidenceAuthority::GmWitnessed);
    assert_eq!(
        ev.atom_id, offer.atom_id,
        "atom is Rust-resolved from the OfferSet — the GM never named it"
    );
    assert_eq!(ev.basis_event_ids, vec![event_id.clone()]);
    assert_eq!(ev.turn_id, TURN);
    assert!(
        !ev.source_refs.is_empty(),
        "source-grounded (carries the authored clue's source ref)"
    );
    let page = ev.source_refs[0].page;
    assert!(decisions[0].result.is_ok());

    // ---- alignment is load-bearing: a cap minted for a DIFFERENT turn ⇒ rejected ----
    let misaligned = derive_offer_set(&graph, &catalog, &scene.node_id, SESSION, "turn_999_wrong");
    let mis_claim = format!(
        "[progress_claims][{{\"cap_id\":\"{}\",\"basis\":[\"commit:{}\"]}}][/progress_claims]",
        misaligned.offers()[0].cap_id.as_str(),
        commit_idx,
    );
    let mis_doc = parse_turn_document(&mis_claim);
    let (mis_ledger, mis_decisions) = admit_gm_claims(
        SESSION,
        "turn_999_wrong",
        &misaligned,
        &catalog,
        &turn_events, // events still carry the REAL turn TURN ≠ turn_999_wrong
        &mis_doc.progress_claims,
        &trpg_model::adventure_ir::EvidenceLedger::new(),
    );
    assert_eq!(mis_ledger.len(), 0, "misaligned turn ⇒ no admission");
    assert!(
        matches!(
            mis_decisions[0].result,
            Err(RejectionReason::CausationMismatch) | Err(RejectionReason::Expired)
        ),
        "misaligned turn ⇒ rejected (alignment load-bearing), got {:?}",
        mis_decisions[0].result
    );

    println!(
        "RAN: the_vault EV-4R main-GM claim loop — scene={} clue={} cap={} commit_idx={} \
         turn_events={} admitted={}(GmWitnessed atom={} page={:?}) misaligned_rejected={:?} PASS",
        scene.node_id,
        clue_id,
        offer.cap_id.as_str(),
        commit_idx,
        turn_events.len(),
        ledger.len(),
        ev.atom_id.as_str(),
        page,
        mis_decisions[0].result,
    );
}
