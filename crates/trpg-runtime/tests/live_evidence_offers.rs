//! CL-EV3b live DB proof: EV-3 `progress_offers_v1` on the real `the_vault` parsed
//! graph (:54347).
//!
//! Proves, deterministically (Rust, no LLM), on REAL data — not a fixture:
//!   1. After CL-1 clue projection, the active scene surfaces ≥1 authored clue; the
//!      EV-2 catalog has an atom for it.
//!   2. `derive_offer_set` for that scene yields an EvidenceOfferSet whose offers
//!      come ONLY from the scene's SURFACED clue atoms — each with an opaque,
//!      turn-scoped `cap_id` (≠ atom_id, ≠ an objective slug), the correct atom_id,
//!      and a `required_basis` (the OfferSet is printed).
//!   3. The OfferSet is CONSERVATIVE: it offers fewer atoms than the whole catalog
//!      (the whole vocabulary is never handed to the GM).
//!   4. The rendered GM prompt block shows the cap_id + human meaning + basis, and
//!      NEVER leaks the atom_id.
//!   5. OFF run: `progress_offers_enabled()` is false by default — the gate the live
//!      wiring (`scene_navigate_critical`) early-returns on — and an empty OfferSet
//!      renders an empty block, so the prompt stays byte-identical (OFF==baseline).
//!
//! No claims are parsed and the engine is unchanged (EV-4+). No 30-turn judged eval
//! here (J3 stays RED = expected, stated honestly).
//!
//! Run (rulesets DB :54347):
//!   DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!     CARGO_TARGET_DIR=target-air cargo test -p trpg-runtime \
//!     --test live_evidence_offers -- --nocapture
//!
//! No `DATABASE_URL` ⇒ SKIP (fail-closed, never blocks CI). Anti-false-green: a real
//! run prints `RAN:` + counts + `PASS`; only seeing `SKIP` = not verified.
use trpg_db::Db;
use trpg_runtime::{
    build_evidence_atom_catalog, derive_offer_set, progress_offers_enabled, project_clues_onto_scenes,
    render_offer_prompt_block,
};

const VAULT: &str = "triangle_agency.the_vault";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_vault_surfaced_clue_scene_produces_an_offer_set() {
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

    // ---- (1) project clues onto scenes (CL-1) so the active scene surfaces clues ----
    let report = project_clues_onto_scenes(&mut graph);
    let catalog = build_evidence_atom_catalog(&graph);
    // Pick the first scene that actually surfaces a clue after projection.
    let scene = graph
        .scenes
        .iter()
        .find(|s| !s.referenced_clue_ids.is_empty())
        .expect("at least one the_vault scene surfaces a clue after projection");
    eprintln!(
        "RAN: the_vault scenes={} clues={} clue_links={} | catalog_atoms={} | active scene = {} surfaces {:?}",
        graph.scenes.len(),
        graph.clues.len(),
        report.linked,
        catalog.len(),
        scene.node_id,
        scene.referenced_clue_ids,
    );

    // ---- (2) derive the OfferSet for that scene ----
    let offer_set = derive_offer_set(&graph, &catalog, &scene.node_id, "sess_ev3", "turn_4");
    eprintln!("RAN: offer_set = {:#?}", offer_set);
    assert!(!offer_set.is_empty(), "surfaced-clue scene yields ≥1 offer");
    assert_eq!(
        offer_set.len(),
        scene.referenced_clue_ids.len(),
        "one offer per surfaced clue atom (conservative, not the whole catalog)"
    );
    for o in offer_set.offers() {
        // opaque, turn-scoped cap_id ≠ atom_id ≠ objective slug
        assert!(o.cap_id.as_str().starts_with("cap_"), "opaque handle");
        assert_ne!(o.cap_id.as_str(), o.atom_id.as_str(), "cap_id ≠ atom_id");
        assert!(!o.cap_id.as_str().starts_with("atom:"), "cap_id is not an AtomId");
        assert!(!o.cap_id.as_str().contains('.'), "cap_id is not a dotted objective slug");
        assert_eq!(o.expires_at, "turn_4", "single-turn capability");
        assert!(!o.required_basis.is_empty(), "offer cites a required basis");
        // the offered atom is the catalog atom for a surfaced clue
        let surfaced = scene
            .referenced_clue_ids
            .iter()
            .any(|cid| catalog.resolve_fact(cid).map(|a| a.atom_id == o.atom_id).unwrap_or(false));
        assert!(surfaced, "every offered atom is a SURFACED clue atom");
    }

    // ---- (3) conservative: fewer offers than the whole catalog ----
    assert!(
        offer_set.len() <= catalog.len(),
        "never offers more than the catalog"
    );
    if catalog.len() > scene.referenced_clue_ids.len() {
        assert!(
            offer_set.len() < catalog.len(),
            "the whole vocabulary is NOT handed to the GM (offers < catalog atoms)"
        );
    }

    // ---- (4) rendered prompt block shows cap+meaning+basis, never the atom_id ----
    let block = render_offer_prompt_block(&offer_set);
    assert!(!block.is_empty(), "non-empty OfferSet renders a prompt block");
    for o in offer_set.offers() {
        assert!(block.contains(o.cap_id.as_str()), "block shows the opaque handle");
        assert!(
            !block.contains(o.atom_id.as_str()),
            "block NEVER leaks the atom_id (GM only sees the opaque handle)"
        );
    }
    assert!(block.contains("requires basis"), "block states the required basis");
    assert!(!block.contains("atom:"), "no atom id prefix anywhere in the prompt block");
    eprintln!("RAN: GM prompt block =\n{block}");

    // ---- (5) OFF run: gate false by default ⇒ empty block ⇒ prompt byte-identical ----
    assert!(
        !progress_offers_enabled(),
        "default OFF: the offers gate is false ⇒ live wiring skips ⇒ no OfferSet"
    );
    // An unknown current scene (or an OFF turn) ⇒ empty set ⇒ empty block ⇒ no prompt
    // mutation: mirror the tiered.rs append guard and assert byte-identity.
    let empty = derive_offer_set(&graph, &catalog, "scene_does_not_exist", "sess_ev3", "turn_4");
    assert!(empty.is_empty(), "fail-closed: unknown scene ⇒ empty OfferSet");
    let base = "BASE NAV PROMPT".to_string();
    let mut prompt = base.clone();
    let eb = render_offer_prompt_block(&empty);
    if !eb.is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(&eb);
    }
    assert_eq!(prompt, base, "empty OfferSet ⇒ prompt bytes identical (OFF==baseline)");

    eprintln!(
        "PASS: scene {} surfaces {} clue(s) ⇒ {} offer(s) (opaque cap_id, correct atom, basis) | catalog={} (conservative) | block hides atom_id | OFF gate=false ⇒ prompt unchanged",
        scene.node_id,
        scene.referenced_clue_ids.len(),
        offer_set.len(),
        catalog.len()
    );
}
