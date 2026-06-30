//! CL-P2b deterministic live proof: EV-P2 `progress_exact_projectors_v1` exact
//! producers on REAL parsed graphs (:54347), no LLM.
//!
//! GPT Pro follow-up reframe: **most evidence must NOT depend on the GM's audit
//! compliance** (EV-P1 measured ~50%). These exact producers run pure Rust over THIS
//! turn's committed events (ContentDelivery consumes only a closed GM *reference*,
//! never a GM verdict). This test proves they FIRE on real authored data:
//!
//!   (i)  LocationEntered (pure Rust): on the REAL `homecoming` graph,
//!        `build_location_atom_catalog` derives `location:<id>` atoms from the
//!        scene/location nodes; a committed `SceneTransitioned` to a REAL scene node
//!        ⇒ exactly one `AcceptedEvidence(LocationEntered, ExactDomain)`, and a
//!        transition to a non-authored destination ⇒ none (fail-closed).
//!   (ii) ContentDelivery → FactLearned: on the REAL `the_vault` graph, a GM
//!        `ContentDelivery{delivery_cap, recipient=player, basis}` whose cap is in
//!        the turn's OfferSet (a surfaced clue) and whose basis resolves to a real
//!        committed `PlayerLearnedFact` ⇒ exactly one
//!        `AcceptedEvidence(FactLearned, ExactDomain)`, atom RUST-resolved from the
//!        OfferSet (the GM never names the fact/atom); an unknown cap ⇒ none.
//!
//! The committed events are constructed in-memory (the projectors take a committed
//! slice — same as the EV-2 live test); the AUTHORED graphs are real. Shadow: the
//! engine does NOT consume the ledger (EV-APPLY); J3 stays RED. The real ~8-turn LLM
//! play that proves exact evidence appears WITHOUT GM-audit reliance is the SUPERVISOR
//! step.
//!
//! Run (rulesets DB :54347):
//!   DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!     CARGO_TARGET_DIR=target-air cargo test -p trpg-runtime \
//!     --test live_exact_projectors -- --nocapture
//!
//! No `DATABASE_URL` ⇒ SKIP (fail-closed, never blocks CI). Anti-false-green: a real
//! run prints `RAN:` + counts + `PASS`; only seeing `SKIP` = not verified.
use serde_json::json;
use trpg_db::Db;
use trpg_model::adventure_ir::{
    ContentDelivery, DeliveryRecipient, EvidenceAuthority, EvidenceKind, EvidenceLedger, NonEmpty,
    TurnLocalRef,
};
use trpg_model::{DomainEvent, DomainEventKind};
use trpg_runtime::{
    build_evidence_atom_catalog, build_location_atom_catalog, derive_offer_set,
    progress_exact_projectors_enabled, project_clues_onto_scenes, project_content_delivery,
    project_location_entered,
};

const HOMECOMING: &str = "cyberpunk_red.homecoming";
const VAULT: &str = "triangle_agency.the_vault";
const SESSION: &str = "sess_evp2_live";
const TURN: &str = "turn-evp2-real-0001";

fn db_url() -> Option<String> {
    match std::env::var("DATABASE_URL") {
        Ok(u) => Some(u),
        Err(_) => {
            eprintln!("SKIP: DATABASE_URL unset (need live rulesets DB :54347)");
            None
        }
    }
}

/// (i) homecoming committed `SceneTransitioned` ⇒ AcceptedEvidence(LocationEntered).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn homecoming_committed_nav_projects_location_entered_exact() {
    let Some(url) = db_url() else { return };
    let db = Db::connect(&url).await.expect("connect live DB");
    let graph = db
        .load_module_graph(HOMECOMING)
        .await
        .expect("query ok")
        .expect("homecoming module graph present in parsed_bundles");

    // ---- (1) location catalog from REAL scene/location topology (pure Rust) ----
    let catalog = build_location_atom_catalog(&graph);
    assert!(
        !catalog.is_empty(),
        "homecoming scene/location nodes derive ≥1 LocationEntered atom"
    );
    // Pick a REAL scene node id as the nav destination (always an authored ref).
    let dest = graph
        .scenes
        .iter()
        .map(|s| s.node_id.trim())
        .find(|id| !id.is_empty())
        .expect("homecoming has at least one scene node with an id")
        .to_string();
    eprintln!(
        "RAN: homecoming scenes={} locations={} | location_atoms={} | nav destination = location:{dest}",
        graph.scenes.len(),
        graph.locations.len(),
        catalog.len(),
    );

    // ---- (2) a committed SceneTransitioned (player-driven move) to that real scene ----
    let nav = DomainEvent::new(
        "de_evp2_nav",
        SESSION,
        TURN,
        DomainEventKind::SceneTransitioned,
        json!({"from": "scene_origin", "to": dest}),
    );
    let led = project_location_entered(std::slice::from_ref(&nav), &catalog);
    assert_eq!(
        led.len(),
        1,
        "committed nav to an authored scene ⇒ 1 LocationEntered"
    );
    let ev = &led.entries()[0];
    eprintln!("RAN: LocationEntered ledger row = {ev:#?}");
    assert_eq!(ev.authority, EvidenceAuthority::ExactDomain);
    assert_eq!(ev.evidence_kind, EvidenceKind::LocationEntered);
    assert_eq!(
        ev.atom_id,
        catalog
            .resolve_grounding(&format!("location:{dest}"))
            .unwrap()
            .atom_id,
        "atom is the authored location node's atom (Rust-resolved by topology)"
    );
    assert_eq!(ev.basis_event_ids, vec!["de_evp2_nav".to_string()]);
    assert_eq!(ev.turn_id, TURN);
    assert!(!ev.source_refs.is_empty(), "source-grounded");

    // ---- (3) fail-closed: a transition to a non-authored destination projects nothing ----
    let bogus = DomainEvent::new(
        "de_evp2_bogus",
        SESSION,
        TURN,
        DomainEventKind::SceneTransitioned,
        json!({"to": "scene_not_in_this_module_xyz"}),
    );
    assert!(
        project_location_entered(std::slice::from_ref(&bogus), &catalog).is_empty(),
        "fail-closed: nav to a destination with no authored location atom ⇒ no evidence"
    );

    assert!(
        !progress_exact_projectors_enabled(),
        "default OFF: the exact-projectors gate is false ⇒ live wiring skips ⇒ no evidence"
    );
    eprintln!(
        "PASS: homecoming location_atoms={} | committed nav⇒1 AcceptedEvidence(LocationEntered,ExactDomain) | unauthored dest⇒0 | OFF gate=false",
        catalog.len()
    );
}

/// (ii) the_vault GM ContentDelivery ref ⇒ AcceptedEvidence(FactLearned).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_vault_content_delivery_projects_fact_learned_exact() {
    let Some(url) = db_url() else { return };
    let db = Db::connect(&url).await.expect("connect live DB");
    let mut graph = db
        .load_module_graph(VAULT)
        .await
        .expect("query ok")
        .expect("the_vault module graph present in parsed_bundles");

    // ---- (1) real OfferSet for a surfaced-clue scene (SAME wiring as derive_evidence_offers) ----
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
    // The clue the offer is bound to (Rust-side; the GM never sees the atom or clue id).
    let clue_id = catalog
        .resolve_atom_id(&offer0.atom_id)
        .expect("offer atom is in the catalog")
        .bindings[0]
        .clone();
    eprintln!(
        "RAN: the_vault scenes={} clues={} | offers={} | active scene = {} surfaces clue {clue_id}",
        graph.scenes.len(),
        graph.clues.len(),
        offer_set.len(),
        scene.node_id,
    );

    // ---- (2) a REAL committed PlayerLearnedFact (this turn, the offered clue) = the basis ----
    let committed = vec![DomainEvent::new(
        "de_evp2_reveal",
        SESSION,
        TURN,
        DomainEventKind::PlayerLearnedFact,
        json!({"fact_id": clue_id, "reason": "presented the authored storyboard to the player"}),
    )];

    // ---- (3) the GM's closed ContentDelivery ref (cap + recipient=player + basis ONLY) ----
    let delivery = ContentDelivery {
        delivery_cap: offer0.cap_id.clone(),
        recipient: DeliveryRecipient::Player,
        basis: NonEmpty::from_vec(vec![TurnLocalRef::Commit(0)]).unwrap(),
    };
    let led = project_content_delivery(
        std::slice::from_ref(&delivery),
        SESSION,
        TURN,
        &offer_set,
        &catalog,
        &committed,
        &EvidenceLedger::new(),
    );
    assert_eq!(led.len(), 1, "verified content delivery ⇒ 1 FactLearned");
    let ev = &led.entries()[0];
    eprintln!("RAN: FactLearned ledger row = {ev:#?}");
    assert_eq!(
        ev.authority,
        EvidenceAuthority::ExactDomain,
        "an explicit authored-content materialization is an EXACT producer (not a GM guess)"
    );
    assert_eq!(ev.evidence_kind, EvidenceKind::FactLearned);
    assert_eq!(
        ev.atom_id, offer0.atom_id,
        "atom is Rust-resolved from the OfferSet cap→atom map; the GM never named it"
    );
    assert_eq!(ev.basis_event_ids, vec!["de_evp2_reveal".to_string()]);
    assert!(
        !ev.source_refs.is_empty(),
        "source-grounded (carries the clue's authored page)"
    );

    // ---- (4) fail-closed: a delivery citing a cap NOT in this turn's OfferSet ⇒ none ----
    let forged = ContentDelivery {
        delivery_cap: trpg_model::adventure_ir::CapId("cap_fabricated00".into()),
        recipient: DeliveryRecipient::Player,
        basis: NonEmpty::from_vec(vec![TurnLocalRef::Commit(0)]).unwrap(),
    };
    assert!(
        project_content_delivery(
            std::slice::from_ref(&forged),
            SESSION,
            TURN,
            &offer_set,
            &catalog,
            &committed,
            &EvidenceLedger::new(),
        )
        .is_empty(),
        "fail-closed: a cap not in this turn's OfferSet projects nothing"
    );

    assert!(
        !progress_exact_projectors_enabled(),
        "default OFF: the exact-projectors gate is false ⇒ live wiring skips ⇒ no evidence"
    );
    eprintln!(
        "PASS: the_vault offers={} | verified content-delivery⇒1 AcceptedEvidence(FactLearned,ExactDomain) atom=Rust-resolved | unknown cap⇒0 | OFF gate=false",
        offer_set.len()
    );
}
