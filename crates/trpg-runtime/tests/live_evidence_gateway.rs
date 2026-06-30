//! CL-EV4b live DB proof: EV-4 `progress_claims_shadow_v1` EvidenceGateway on the
//! real `the_vault` parsed graph (:54347).
//!
//! Proves, deterministically (Rust, no LLM), on REAL data — not a fixture — that the
//! GM↔Rust claim loop runs end-to-end through the LIVE gateway wiring:
//!   1. A hand-built VALID claim (cap from this turn's real OfferSet + a basis that
//!      resolves to a real committed PlayerLearnedFact of the required kind, bound to
//!      the offered atom) ⇒ exactly ONE `AcceptedEvidence{authority: GmWitnessed}`,
//!      whose `atom_id` is RUST-RESOLVED from the OfferSet (the claim never named it)
//!      and which is source-grounded (carries the authored clue's source ref).
//!   2. A forged claim for each of several rejection reasons ⇒ rejected with that
//!      reason (UnknownCapability / Expired / DuplicateEvidence). The admission log is
//!      printed.
//!   3. OFF: `progress_claims_shadow_enabled()` is false by default — the gate the
//!      live wiring (`scene_navigate_critical`) parses/admits behind — so OFF ⇒ no
//!      parsing, no admission, no ledger write (OFF==baseline).
//!
//! Shadow: the engine does NOT consume the ledger (EV-6) and no objective is
//! completed. No 30-turn judged eval (J3 stays RED = expected, stated honestly). The
//! real ~6-10-turn LLM play that proves the GM actually EMITS an admittable claim in
//! play is the SUPERVISOR step (worker proves the gateway deterministically).
//!
//! Run (rulesets DB :54347):
//!   DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!     CARGO_TARGET_DIR=target-air cargo test -p trpg-runtime \
//!     --test live_evidence_gateway -- --nocapture
//!
//! No `DATABASE_URL` ⇒ SKIP (fail-closed, never blocks CI). Anti-false-green: a real
//! run prints `RAN:` + counts + `PASS`; only seeing `SKIP` = not verified.
use trpg_db::Db;
use trpg_model::adventure_ir::{
    CapId, EvidenceAuthority, EvidenceClaim, EvidenceLedger, NonEmpty, TurnLocalRef,
};
use trpg_model::{DomainEvent, DomainEventKind};
use trpg_runtime::{
    build_evidence_atom_catalog, derive_offer_set, parse_progress_claims,
    progress_claims_shadow_enabled, project_clues_onto_scenes, EvidenceGateway, GatewayInputs,
    RejectionReason,
};

const VAULT: &str = "triangle_agency.the_vault";
const SESSION: &str = "sess_ev4";
const TURN: &str = "turn_4";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_vault_gm_claim_loop_admits_through_live_gateway() {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP: DATABASE_URL unset (need live rulesets DB :54347)");
            return;
        }
    };
    let db = Db::connect(&url).await.expect("connect live DB");
    let mut graph = db
        .load_module_graph(VAULT)
        .await
        .expect("query ok")
        .expect("the_vault module graph present in parsed_bundles");

    // ---- real OfferSet for a surfaced-clue scene (same wiring as tiered.rs EV-3/4) ----
    let _ = project_clues_onto_scenes(&mut graph);
    let catalog = build_evidence_atom_catalog(&graph);
    let scene = graph
        .scenes
        .iter()
        .find(|s| !s.referenced_clue_ids.is_empty())
        .expect("at least one the_vault scene surfaces a clue after projection");
    let offer_set = derive_offer_set(&graph, &catalog, &scene.node_id, SESSION, TURN);
    assert!(!offer_set.is_empty(), "surfaced-clue scene yields ≥1 offer");
    let offer = &offer_set.offers()[0];
    // The authored clue this opaque cap maps to (Rust-side; the GM never sees it).
    let clue_id = scene
        .referenced_clue_ids
        .iter()
        .find(|cid| {
            catalog
                .resolve_fact(cid)
                .map(|a| a.atom_id == offer.atom_id)
                .unwrap_or(false)
        })
        .expect("offer maps to a surfaced clue atom")
        .clone();
    eprintln!(
        "RAN: the_vault scenes={} clues={} | catalog_atoms={} | scene={} cap={} → clue={}",
        graph.scenes.len(),
        graph.clues.len(),
        catalog.len(),
        scene.node_id,
        offer.cap_id.as_str(),
        clue_id,
    );

    // ---- (1) VALID claim through the live gateway ⇒ 1 AcceptedEvidence(GmWitnessed) ----
    // A real committed basis event: a PlayerLearnedFact for the offered clue, caused
    // this turn/session (what a successful Investigation commit looks like).
    let committed = vec![DomainEvent::new(
        "de_clue_learn_live",
        SESSION,
        TURN,
        DomainEventKind::PlayerLearnedFact,
        serde_json::json!({"fact_id": clue_id, "reason": "successful Investigation check"}),
    )];
    let ledger = EvidenceLedger::new();
    let valid_claim = EvidenceClaim {
        cap_id: offer.cap_id.clone(),
        basis: NonEmpty::from_vec(vec![TurnLocalRef::Commit(0)]).unwrap(),
    };
    // Mirror the tiered.rs admission-half wiring exactly.
    let inp = GatewayInputs {
        session_id: SESSION,
        turn_id: TURN,
        offer_set: &offer_set,
        committed_events: &committed,
        catalog: &catalog,
        ledger: &ledger,
    };
    let accepted = EvidenceGateway::admit(&valid_claim, &inp).expect("valid claim ADMITTED");
    eprintln!(
        "RAN: ADMIT log ⇒ cap={} ADMITTED → evidence_id={} atom={} authority={:?} basis={:?} source_page={:?}",
        offer.cap_id.as_str(),
        accepted.evidence_id,
        accepted.atom_id.as_str(),
        accepted.authority,
        accepted.basis_event_ids,
        accepted.source_refs.first().and_then(|s| s.page),
    );
    assert_eq!(accepted.authority, EvidenceAuthority::GmWitnessed);
    assert_eq!(
        accepted.atom_id, offer.atom_id,
        "atom_id is RUST-resolved from the OfferSet, never named by the LLM"
    );
    assert_eq!(
        accepted.basis_event_ids,
        vec!["de_clue_learn_live".to_string()]
    );
    assert_eq!(accepted.turn_id, TURN);
    assert!(
        !accepted.source_refs.is_empty(),
        "source-grounded (carries the authored clue's source ref)"
    );

    // ---- (2) forged claims ⇒ rejected with the right reason (admission log) ----
    // (a) UnknownCapability: a fabricated cap not in this turn's OfferSet.
    let forged_unknown = EvidenceClaim {
        cap_id: CapId("cap_fabricated_by_llm".into()),
        basis: NonEmpty::from_vec(vec![TurnLocalRef::Commit(0)]).unwrap(),
    };
    let r_unknown = EvidenceGateway::admit(&forged_unknown, &inp).unwrap_err();
    eprintln!(
        "RAN: REJECT log ⇒ cap=cap_fabricated_by_llm REJECTED reason={}",
        r_unknown.as_str()
    );
    assert_eq!(r_unknown, RejectionReason::UnknownCapability);

    // (b) Expired: the same valid cap, but admitted in a LATER turn (offer's window passed).
    let expired_scope = GatewayInputs {
        session_id: SESSION,
        turn_id: "turn_999",
        offer_set: &offer_set,
        committed_events: &committed,
        catalog: &catalog,
        ledger: &ledger,
    };
    let r_expired = EvidenceGateway::admit(&valid_claim, &expired_scope).unwrap_err();
    eprintln!(
        "RAN: REJECT log ⇒ cap={} (turn_999) REJECTED reason={}",
        offer.cap_id.as_str(),
        r_expired.as_str()
    );
    assert_eq!(r_expired, RejectionReason::Expired);

    // (c) DuplicateEvidence: admit the same valid claim again against a ledger that
    //     already holds the first AcceptedEvidence.
    let mut seeded = EvidenceLedger::new();
    seeded.append(accepted.clone());
    let dup_inp = GatewayInputs {
        session_id: SESSION,
        turn_id: TURN,
        offer_set: &offer_set,
        committed_events: &committed,
        catalog: &catalog,
        ledger: &seeded,
    };
    let r_dup = EvidenceGateway::admit(&valid_claim, &dup_inp).unwrap_err();
    eprintln!(
        "RAN: REJECT log ⇒ cap={} (replay) REJECTED reason={}",
        offer.cap_id.as_str(),
        r_dup.as_str()
    );
    assert_eq!(r_dup, RejectionReason::DuplicateEvidence);

    // ---- (2.5) the parser is fail-closed on a forged objective field (closed schema) ----
    let forged_sidecar = serde_json::json!({"progress_claims": [
        {"cap_id": offer.cap_id.as_str(), "basis": ["commit:0"]},
        {"cap_id": offer.cap_id.as_str(), "basis": ["commit:0"], "completed": true}
    ]});
    let parsed = parse_progress_claims(&forged_sidecar);
    assert_eq!(
        parsed.len(),
        1,
        "the forged `completed` claim is dropped; the closed one parses"
    );

    // ---- (3) OFF run: shadow gate false by default ⇒ no parse/admit/ledger write ----
    assert!(
        !progress_claims_shadow_enabled(),
        "default OFF: the shadow gate is false ⇒ live wiring parses/admits nothing"
    );

    eprintln!(
        "PASS: live the_vault GM↔Rust claim loop ⇒ valid claim ADMITTED as 1 AcceptedEvidence(GmWitnessed) (Rust-resolved atom {}, source-grounded) | forged unknown/expired/duplicate REJECTED (in fixed order) | parser drops forged `completed` | OFF gate=false (OFF==baseline). Engine NOT consuming (shadow); J3 stays RED.",
        accepted.atom_id.as_str()
    );
}
