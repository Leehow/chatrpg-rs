//! CL-3(i) live DB proof: clue→scene **page-containment projection** on the real
//! `the_vault` parsed graph.
//!
//! Root cause it fixes (P2 Vault J3): `the_vault` scenes carry empty
//! `referenced_clue_ids` even though `module_graph.clues` has the clues (each with a
//! `page`) and scenes carry `page_start`/`page_end`. `clue_affordance` therefore
//! iterates an empty list → 0 `PlayerLearnedFact` → progression starves.
//!
//! This test proves, on REAL data (not a fixture):
//!   1. BASELINE (projection OFF): every scene's `referenced_clue_ids` is empty
//!      (byte-baseline — the persisted graph is untouched).
//!   2. PROJECTION ON: the first authored scene (smallest `page_start`, i.e. the
//!      `scene_001` equivalent — Springs Eternal at page 8) gets a NON-EMPTY
//!      `referenced_clue_ids`, and EVERY linked clue's `page` genuinely falls inside
//!      its containing scene's `[page_start, page_end]` (source-correct, never fabricated).
//!   3. Fail-closed accounting: the report's linked + skipped counts cover every clue
//!      that has an id (nothing silently dropped).
//!
//! Run (rulesets DB :54347):
//!   DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!     CARGO_TARGET_DIR=target-air cargo test -p trpg-runtime \
//!     --test live_clue_projection -- --nocapture
//!
//! No `DATABASE_URL` ⇒ SKIP (fail-closed, never blocks CI). Anti-false-green: a real
//! run prints `RAN:` + counts + `PASS`; only seeing `SKIP` = not verified.
use trpg_db::Db;
use trpg_model::MaterializationAffordanceMode::{Enforce, Off};
use trpg_runtime::progression::progress_events_from_domain;
use trpg_runtime::{
    clue_reveal_candidates, project_clues_onto_scenes, ClueProjectionReport, ResolvedCheck,
};

const VAULT: &str = "triangle_agency.the_vault";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_vault_clue_projection_populates_scene_referenced_clue_ids() {
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

    // ---- (1) BASELINE: persisted graph has empty referenced_clue_ids everywhere ----
    let baseline_nonempty = graph
        .scenes
        .iter()
        .filter(|s| !s.referenced_clue_ids.is_empty())
        .count();
    let clues_total = graph.clues.len();
    let clues_with_page = graph
        .clues
        .iter()
        .filter(|c| c.get("page").and_then(|p| p.as_u64()).is_some())
        .count();
    eprintln!(
        "RAN: the_vault scenes={} clues={} (with page={}) | BASELINE scenes-with-clue-refs={}",
        graph.scenes.len(),
        clues_total,
        clues_with_page,
        baseline_nonempty
    );
    assert_eq!(
        baseline_nonempty, 0,
        "BASELINE: the_vault scenes start with empty referenced_clue_ids (this IS the J3 break)"
    );
    assert!(
        clues_with_page > 0,
        "the_vault must carry clues with pages for the projection to have anything to do"
    );

    // ---- (2) PROJECTION ON: project clues onto containing scenes (in-memory) ----
    let mut projected = graph.clone();
    let report: ClueProjectionReport = project_clues_onto_scenes(&mut projected);
    eprintln!(
        "RAN: projection report = linked={} skipped(no_page={} out_of_range={} ambiguous={})",
        report.linked,
        report.skipped_no_page,
        report.skipped_out_of_range,
        report.skipped_ambiguous
    );
    assert!(
        report.linked > 0,
        "projection must link ≥1 clue onto a containing scene on real the_vault data"
    );

    // The first authored scene by page order = the `scene_001` equivalent (Springs
    // Eternal, page_start 8). It must now carry clues, all within its own page range.
    let mut ordered: Vec<_> = projected
        .scenes
        .iter()
        .filter(|s| s.page_start.is_some())
        .collect();
    ordered.sort_by_key(|s| s.page_start.unwrap());
    let first = ordered.first().expect("≥1 scene with a page_start");
    eprintln!(
        "RAN: first scene '{}' (id={}, pages={:?}..{:?}) referenced_clue_ids={:?}",
        first.title, first.node_id, first.page_start, first.page_end, first.referenced_clue_ids
    );
    assert!(
        !first.referenced_clue_ids.is_empty(),
        "first scene (scene_001 equiv) must get a NON-EMPTY referenced_clue_ids after projection"
    );

    // ---- (3) Source-correctness invariant: EVERY linked clue's page is genuinely
    //         inside its containing scene's [page_start, page_end] (never fabricated). ----
    let page_of = |cid: &str| -> Option<u64> {
        projected
            .clues
            .iter()
            .find(|c| c.get("id").and_then(|v| v.as_str()) == Some(cid))
            .and_then(|c| c.get("page").and_then(|p| p.as_u64()))
    };
    let mut verified_links = 0usize;
    for s in &projected.scenes {
        if s.referenced_clue_ids.is_empty() {
            continue;
        }
        let (Some(ps), Some(pe)) = (s.page_start, s.page_end) else {
            panic!(
                "scene {} got clue refs but has no full page range — impossible under page-containment",
                s.node_id
            );
        };
        for cid in &s.referenced_clue_ids {
            let page =
                page_of(cid).unwrap_or_else(|| panic!("linked clue {cid} must exist with a page"));
            assert!(
                (ps as u64) <= page && page <= (pe as u64),
                "clue {cid} page {page} must fall inside scene {} range [{ps},{pe}] (no fabricated link)",
                s.node_id
            );
            verified_links += 1;
        }
    }
    assert_eq!(
        verified_links, report.linked,
        "every reported link must be page-contained (counts must reconcile)"
    );

    // ---- OFF == baseline: the ORIGINAL graph object is still untouched ----
    assert_eq!(
        graph
            .scenes
            .iter()
            .filter(|s| !s.referenced_clue_ids.is_empty())
            .count(),
        0,
        "OFF==baseline: projecting onto a clone never mutates the originally-loaded graph"
    );

    eprintln!(
        "PASS: the_vault clue projection — {} source-correct page-contained links \
         (first scene '{}' now discoverable); baseline graph byte-untouched",
        verified_links, first.title
    );
}

/// CL-3(ii) deterministic live-DB end-to-end chain proof on the real `the_vault`:
/// projection ON → a successful investigative check on scene_001 yields a clue-reveal
/// candidate (= the source-backed FACT that the commit boundary turns into a
/// `PlayerLearnedFact`), and feeding that `PlayerLearnedFact` through the REAL
/// progression adapter produces a `WorldFactChanged` ProgressEvent. The OFF run
/// (projection disabled → empty `referenced_clue_ids`) yields ZERO candidates → zero
/// carrier facts → zero `WorldFactChanged` (baseline).
///
/// This is the worker-runnable, **deterministic** mechanism proof (Rust-evaluated,
/// LLM-never, zero fabrication). The full LLM ≤6-turn play that emits the actual
/// `domain_events` rows + J3 grading is the supervisor's 30-turn engine-ON eval
/// (see READY_FOR_FULL.sentinel) — same worker/supervisor split as P2-3.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_vault_successful_investigation_feeds_progression_adapter() {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP: DATABASE_URL unset (need live rulesets DB :54347)");
            return;
        }
    };
    let db = Db::connect(&url).await.expect("connect live DB");
    let base_graph = db
        .load_module_graph(VAULT)
        .await
        .expect("query ok")
        .expect("the_vault module graph present");

    // ---- PROJECTION ON: project clues onto containing scenes ----
    let mut graph = base_graph.clone();
    let _ = project_clues_onto_scenes(&mut graph);
    // The scene_001 equivalent (smallest page_start) now carries ≥1 clue.
    let mut ordered: Vec<usize> = (0..graph.scenes.len())
        .filter(|&i| graph.scenes[i].page_start.is_some())
        .collect();
    ordered.sort_by_key(|&i| graph.scenes[i].page_start.unwrap());
    let s_idx = *ordered.first().expect("≥1 paged scene");
    let scene = graph.scenes[s_idx].clone();
    let clue_id = scene
        .referenced_clue_ids
        .first()
        .expect("scene_001 must have a projected clue")
        .clone();
    // The clue's authored name (for a realistic investigative check text).
    let clue_name = graph
        .clues
        .iter()
        .find(|c| c.get("id").and_then(|v| v.as_str()) == Some(clue_id.as_str()))
        .and_then(|c| c.get("name").and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_else(|| clue_id.clone());
    eprintln!(
        "RAN: scene_001 id={} projected clue id={} name={:?}",
        scene.node_id, clue_id, clue_name
    );

    // ---- A successful investigative check whose text examines that clue ----
    // (mirrors what the GM turn-loop joins from CheckResultRecord + CheckContract.)
    let label = format!("Investigate {clue_name}");
    let check = ResolvedCheck {
        success: true,
        tested_parameter: Some("Investigation"),
        check_label: &label,
        action_summary: "The agent searches the scene, examining the clue.",
    };

    // ---- Both gates ON (Enforce ∧ gating_on) → ≥1 source-backed candidate ----
    let cands = clue_reveal_candidates(&check, &scene, &graph, Enforce, true);
    eprintln!(
        "RAN: clue_reveal_candidates (Enforce ∧ gating ON) → {} candidate(s)",
        cands.len()
    );
    assert!(
        !cands.is_empty(),
        "successful investigation on a projected scene_001 clue must yield ≥1 reveal candidate"
    );
    assert_eq!(
        cands[0].fact_id, clue_id,
        "candidate fact = source-backed clue id"
    );

    // ---- The candidate → PlayerLearnedFact (what the commit boundary persists) ----
    // → feed through the REAL progression adapter → WorldFactChanged ProgressEvent.
    let learned: trpg_model::DomainEvent = serde_json::from_value(serde_json::json!({
        "event_id": "e_cl3_learned",
        "session_id": "cl3_smoke",
        "turn_id": "t1",
        "kind": "PlayerLearnedFact",
        "data": { "fact_id": cands[0].fact_id },
        "source_refs": [],
        "created_at": "1970-01-01T00:00:00Z"
    }))
    .expect("build PlayerLearnedFact domain event");
    let progress = progress_events_from_domain(std::slice::from_ref(&learned));
    let world_fact_changed = progress
        .iter()
        .filter(|e| {
            matches!(
                e,
                trpg_runtime::progression::ProgressEvent::WorldFactChanged { fact, .. } if fact == &clue_id
            )
        })
        .count();
    eprintln!(
        "RAN: adapter(PlayerLearnedFact{{{}}}) → WorldFactChanged ProgressEvent count={}",
        clue_id, world_fact_changed
    );
    assert_eq!(
        world_fact_changed, 1,
        "adapter must map the clue PlayerLearnedFact → exactly one WorldFactChanged"
    );

    // ---- OFF baseline: no projection → empty referenced_clue_ids → 0 carriers ----
    let off_scene = base_graph
        .scenes
        .iter()
        .find(|s| s.node_id == scene.node_id)
        .expect("scene present in baseline")
        .clone();
    assert!(
        off_scene.referenced_clue_ids.is_empty(),
        "OFF baseline: scene_001 referenced_clue_ids stays empty (byte-baseline)"
    );
    let off_cands = clue_reveal_candidates(&check, &off_scene, &base_graph, Enforce, true);
    eprintln!(
        "RAN: OFF (no projection) → {} candidate(s) (expect 0)",
        off_cands.len()
    );
    assert!(
        off_cands.is_empty(),
        "OFF: with empty referenced_clue_ids, successful investigation reveals nothing → 0 carriers"
    );

    // ---- Double-gate proof: Off mode OR gating-off → 0 (even with projection ON) ----
    assert!(
        clue_reveal_candidates(&check, &scene, &graph, Off, true).is_empty(),
        "mode Off ⇒ no candidate (gate 1)"
    );
    assert!(
        clue_reveal_candidates(&check, &scene, &graph, Enforce, false).is_empty(),
        "gating OFF ⇒ no candidate (gate 2)"
    );

    eprintln!(
        "PASS: the_vault investigation chain — projection ON: 1 candidate → PlayerLearnedFact → \
         1 WorldFactChanged; OFF: 0 candidates → 0 carriers (baseline); double-gate holds"
    );
}
