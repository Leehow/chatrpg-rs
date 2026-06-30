//! CL-EV2b live DB proof: EV-2 `exact_evidence_projector_v1` on the real
//! `the_vault` parsed graph (:54347).
//!
//! Proves, deterministically (Rust, no LLM), on REAL data — not a fixture:
//!   1. The EvidenceAtomCatalog derives one source-grounded `fact:<clue_id>` atom
//!      per authored clue (each carries the clue's authored page as a SourceRef).
//!   2. A clue-reveal `PlayerLearnedFact` (the canonical `record_revealed_fact`
//!      carrier — `{fact_id, reason}`, no knowledge_state) over a REAL the_vault
//!      clue id projects to **exactly one** `AcceptedEvidence{ExactDomain}` whose
//!      atom_id + source_refs + basis are correct (the AcceptedEvidence is printed).
//!   3. A synthetic EV-1 `wf_chk_*` `WorldFactChanged` does **NOT** project
//!      (fail-closed: no authored atom).
//!   4. A non-true belief (`knowledge_state: suspects`) over the same authored clue
//!      does **NOT** project (the EV-1 forward-caveat fix).
//!   5. OFF run projects nothing: `exact_evidence_projector_enabled()` is false by
//!      default — the gate the live wiring (`scene_navigate_critical`) early-returns
//!      on, so the OFF path builds no catalog and emits no evidence.
//!
//! Engine does NOT consume the ledger (EV-6). No 30-turn judged eval here (J3 stays
//! RED = expected, stated honestly).
//!
//! Run (rulesets DB :54347):
//!   DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!     CARGO_TARGET_DIR=target-air cargo test -p trpg-runtime \
//!     --test live_exact_evidence_projector -- --nocapture
//!
//! No `DATABASE_URL` ⇒ SKIP (fail-closed, never blocks CI). Anti-false-green: a real
//! run prints `RAN:` + counts + `PASS`; only seeing `SKIP` = not verified.
use serde_json::json;
use trpg_db::Db;
use trpg_model::adventure_ir::EvidenceAuthority;
use trpg_model::{DomainEvent, DomainEventKind};
use trpg_runtime::{
    build_evidence_atom_catalog, exact_evidence_projector_enabled, project_exact_evidence,
};

const VAULT: &str = "triangle_agency.the_vault";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_vault_clue_reveal_projects_one_accepted_evidence() {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP: DATABASE_URL unset (need live rulesets DB :54347)");
            return;
        }
    };
    let db = Db::connect(&url).await.expect("connect live DB");
    let graph = db
        .load_module_graph(VAULT)
        .await
        .expect("query ok")
        .expect("the_vault module graph present in parsed_bundles");

    // ---- (1) catalog from authored clues ----
    let catalog = build_evidence_atom_catalog(&graph);
    // The first clue carrying BOTH an id and a page = the real authored ref we reveal.
    let (clue_id, clue_page) = graph
        .clues
        .iter()
        .find_map(|c| {
            let id = c
                .get("id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())?;
            let page = c.get("page").and_then(|v| v.as_u64())?;
            Some((id.to_string(), page as u32))
        })
        .expect("the_vault must carry at least one clue with an id + page");
    eprintln!(
        "RAN: the_vault clues={} catalog_atoms={} | sample authored ref = fact:{clue_id} (page {clue_page})",
        graph.clues.len(),
        catalog.len()
    );
    assert!(
        !catalog.is_empty(),
        "catalog derives ≥1 atom from authored clues"
    );
    let atom = catalog
        .resolve_fact(&clue_id)
        .expect("the sampled clue id resolves to a catalog atom");
    assert_eq!(
        atom.source_refs.first().and_then(|s| s.page),
        Some(clue_page),
        "atom carries the clue's authored page as a source span (source-grounded)"
    );

    // ---- (2) clue-reveal PlayerLearnedFact ⇒ exactly 1 AcceptedEvidence(ExactDomain) ----
    let reveal = DomainEvent::new(
        "de_clue_reveal_test",
        "sess_ev2",
        "turn_3",
        DomainEventKind::PlayerLearnedFact,
        json!({"fact_id": clue_id, "reason": "successful Investigation examined the clue"}),
    );
    let ledger = project_exact_evidence(std::slice::from_ref(&reveal), &catalog);
    assert_eq!(
        ledger.len(),
        1,
        "clue-reveal projects exactly one AcceptedEvidence"
    );
    let accepted = &ledger.entries()[0];
    eprintln!("RAN: projected AcceptedEvidence = {:#?}", accepted);
    assert_eq!(accepted.authority, EvidenceAuthority::ExactDomain);
    assert_eq!(accepted.atom_id, atom.atom_id, "correct atom_id");
    assert_eq!(
        accepted.basis_event_ids,
        vec!["de_clue_reveal_test".to_string()],
        "basis = the committed event"
    );
    assert_eq!(
        accepted.source_refs.first().and_then(|s| s.page),
        Some(clue_page),
        "AcceptedEvidence carries the atom's source ref"
    );

    // ---- (3) synthetic wf_chk_* WorldFactChanged ⇒ 0 (fail-closed) ----
    let synthetic = DomainEvent::new(
        "de_wf_chk_test",
        "sess_ev2",
        "turn_3",
        DomainEventKind::WorldFactChanged,
        json!({"fact_id": "wf_chk_check_2c160e4abc", "truth_status": "true"}),
    );
    let syn_ledger = project_exact_evidence(std::slice::from_ref(&synthetic), &catalog);
    assert!(
        syn_ledger.is_empty(),
        "synthetic wf_chk_* (no authored atom) is NOT projected (fail-closed)"
    );

    // ---- (4) non-true belief over an authored clue ⇒ 0 (EV-1 forward-caveat fix) ----
    let suspected = DomainEvent::new(
        "de_suspect_test",
        "sess_ev2",
        "turn_3",
        DomainEventKind::PlayerLearnedFact,
        json!({"fact_id": clue_id, "knowledge_state": "suspects"}),
    );
    let susp_ledger = project_exact_evidence(std::slice::from_ref(&suspected), &catalog);
    assert!(
        susp_ledger.is_empty(),
        "non-true belief over an authored ref is NOT projected (keys on payload knowledge_state)"
    );

    // ---- (5) OFF run projects nothing: the gate is false by default ----
    assert!(
        !exact_evidence_projector_enabled(),
        "default OFF: the projector gate is false ⇒ live wiring skips ⇒ zero projection"
    );

    eprintln!(
        "PASS: catalog_atoms={} | clue-reveal⇒1 AcceptedEvidence(ExactDomain) | wf_chk_⇒0 | suspects⇒0 | OFF gate=false",
        catalog.len()
    );
}
