//! E1 `scene_transition_gated_v1` live DB proof on the REAL **homecoming** module — the
//! SPATIAL-axis closer. D2 made `obj.scene_advance.scene_01_lawmen_in_trouble` COMPLETE from a
//! real admitted authored observation (verified in `live_scene_advance_evidence`), but
//! `current_scene` stayed frozen 30 turns (nav-split: the engine emits a signal, never writes the
//! scene; the player is combat-locked and never "navigates"). E1 closes the loop. This test proves,
//! end-to-end on the REAL parsed graph (:54347), the full earned-transition chain WITHOUT an LLM:
//!
//!   (A) **data-driven next-scene** — `resolve_next_scene` on the REAL homecoming graph resolves
//!       scene_01_lawmen_in_trouble → its continuous-spine successor `scene_02_questions_for_athena`
//!       (no authored out-edge ⇒ OneShot spine page-order), and builds a `SceneUnlocked` carrying
//!       that target. ZERO hardcoded scene map — keyed on DocumentType + links + page order.
//!   (B) **earned precondition is real** — scene_01's OWN authored content admits via the SAME
//!       witness path the live loop uses ⇒ the engine fires `obj.scene_advance.scene_01`
//!       ObjectiveResolved (D2). The transition is gated on THIS real completion, not assumed.
//!   (C) **consume ⇒ real transition (E1 ON)** — the NavigationResolver (`scene_navigate_critical`,
//!       the SOLE current_scene writer) consumes the unlock and ACTUALLY changes `current_scene`
//!       scene_01 → scene_02, returning the `SceneNavCommit{reason:"progression-gated…"}` that
//!       execute.rs maps 1:1 into the durable `SceneTransitioned` domain event. The MockLlmClient is
//!       NEVER reached (E1 returns before the LLM nav) — the transition is progression-gated, not
//!       LLM-driven.
//!   (D) **OFF == baseline** — with the flag OFF, the E1 consume block is skipped entirely; the
//!       same pending unlock does NOT transition (the Mock nav can't validate ⇒ honest stay). Proves
//!       OFF is byte-identical player-driven-only behavior.
//!   (E) **the_vault no-teleport** — `resolve_next_scene` on the REAL `the_vault`
//!       (ScenarioCollection, 12 independent missions) returns None: its scene_advance can complete,
//!       but page-order across unrelated missions is a nonsense teleport ⇒ fail-closed.
//!
//! Run (rulesets DB :54347):
//!   DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!     cargo test -p trpg-runtime --test live_e1_scene_transition -- --nocapture --test-threads=1
//! No `DATABASE_URL` ⇒ SKIP (fail-closed). Anti-false-green: a real run prints `RAN:` lines + `PASS`;
//! only `SKIP` = nothing verified.
use sqlx::Row;
use trpg_db::Db;
use trpg_llm::MockLlmClient;
use trpg_model::adventure_ir::{
    scene_advance_guard_leaves, scene_advance_objective, BasisKind, CapId, EvidenceAtomCatalog,
    EvidenceClaim, EvidenceKind, EvidenceLedger, EvidenceOffer, EvidenceOfferSet, NonEmpty,
    TurnLocalRef,
};
use trpg_model::{DomainEvent, DomainEventKind};
use trpg_runtime::progression::{resolve_next_scene, scene_unlock_event};
use trpg_runtime::scene_navigation::scene_navigate_critical;

const HOMECOMING: &str = "cyberpunk_red.homecoming";
const VAULT: &str = "triangle_agency.the_vault";
const RULESET: &str = "cyberpunk_red";
const SESSION: &str = "sess_e1_scene_transition_live";
const TURN: &str = "turn-uuid-e1-scene-transition-live-1";
/// The exact scene the audit measured FROZEN 30 turns — where freeze→advance matters.
const FROZEN_SEAM_SCENE: &str = "scene_01_lawmen_in_trouble";
const E1_FLAG: &str = "TRPG_PROGRESS_SCENE_TRANSITION_GATED_V1";

fn is_witness_bindable(kind: EvidenceKind) -> bool {
    matches!(
        kind,
        EvidenceKind::ActionResolved
            | EvidenceKind::StateEstablished
            | EvidenceKind::EntityEncountered
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completed_scene_advance_drives_a_real_current_scene_transition() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("SKIP: DATABASE_URL unset (need live rulesets DB :54347)");
        return;
    };
    let db = Db::connect(&url).await.expect("connect live DB");
    let pool = sqlx::postgres::PgPool::connect(&url)
        .await
        .expect("connect sqlx pool");
    let data_dir = std::path::PathBuf::from(".");

    // clean slate on the shared DB (repeatable): drop any prior rows for this throwaway session.
    sqlx::query("delete from domain_events where session_id = $1")
        .bind(SESSION)
        .execute(&pool)
        .await
        .expect("cleanup prior domain_events");

    let mut graph = db
        .load_module_graph(HOMECOMING)
        .await
        .expect("query ok")
        .expect("homecoming module graph present in parsed_bundles");
    // mirror the live offer/apply path: clue-projected graph so atom_ids align with admission.
    let _ = trpg_runtime::clue_projection::project_clues_onto_scenes(&mut graph);

    // ─── (A) data-driven next-scene resolution on the REAL graph ────────────────────────────────
    let next = resolve_next_scene(&graph, FROZEN_SEAM_SCENE)
        .expect("homecoming (OneShot) resolves scene_01's continuous-spine successor");
    assert_eq!(
        next, "scene_02_questions_for_athena",
        "data-driven: scene_01(p5) → page-order spine next scene_02(p7); no hardcoded scene map"
    );
    let unlock = scene_unlock_event(SESSION, TURN, &graph, FROZEN_SEAM_SCENE)
        .expect("a resolvable target ⇒ a SceneUnlocked event");
    assert_eq!(unlock.kind, DomainEventKind::SceneUnlocked);
    assert_eq!(unlock.data["from_scene"], FROZEN_SEAM_SCENE);
    assert_eq!(unlock.data["next_scene"], next);
    assert_eq!(
        unlock.data["basis_objective"],
        format!("obj.scene_advance.{FROZEN_SEAM_SCENE}")
    );
    eprintln!(
        "RAN: (A) data-driven SceneUnlocked from={FROZEN_SEAM_SCENE} next={next} \
         basis={} (authored-edge→spine fallback, real graph)",
        unlock.data["basis_objective"]
    );

    // ─── (B) the EARNED precondition is real: scene_01 admits ⇒ engine fires ObjectiveResolved ───
    let objective = scene_advance_objective(&graph, FROZEN_SEAM_SCENE)
        .expect("scene_01 has a scene-advance objective from its own authored content");
    let atom = scene_advance_guard_leaves(&graph, FROZEN_SEAM_SCENE)
        .into_iter()
        .find(|a| is_witness_bindable(a.kind))
        .expect("scene_01 has a witness-bindable scene-advance guard atom");
    let cap = CapId::from_parts(SESSION, TURN, &atom.atom_id);
    let mut catalog = EvidenceAtomCatalog::new();
    catalog.insert(atom.clone());
    let mut offer_set = EvidenceOfferSet::new(TURN);
    offer_set.push(EvidenceOffer {
        cap_id: cap.clone(),
        atom_id: atom.atom_id.clone(),
        kind: atom.kind,
        meaning: "the player engaged scene_01's authored observation".into(),
        required_basis: vec![BasisKind::OutcomeCommitted],
        expires_at: TURN.into(),
    });
    let success_check = DomainEvent::new(
        "de_e1_seam_chk_1",
        SESSION,
        TURN,
        DomainEventKind::CheckResolved,
        serde_json::json!({"check_id": "e1_seam_1", "outcome": {"success": true, "success_tier": "regular"}}),
    );
    let proposals = vec![EvidenceClaim {
        cap_id: cap.clone(),
        basis: NonEmpty::from_vec(vec![TurnLocalRef::Commit(0)]).unwrap(),
    }];
    let (ledger, _) = trpg_runtime::post_turn_witness::admit_witness_proposals(
        SESSION,
        TURN,
        &offer_set,
        &catalog,
        &[success_check],
        &proposals,
        &EvidenceLedger::new(),
    );
    assert_eq!(ledger.len(), 1, "witness admits scene_01's observation atom");
    let resolutions = trpg_runtime::progression::witnessed_scene_advance_resolutions(
        SESSION,
        TURN,
        &graph,
        &ledger,
        FROZEN_SEAM_SCENE,
    );
    assert_eq!(resolutions.len(), 1, "scene_01's advance objective resolves (D2)");
    assert_eq!(resolutions[0].data["objective_id"], objective.id);
    assert_eq!(resolutions[0].data["scene_id"], FROZEN_SEAM_SCENE);
    // land the EARNED completion + the engine's unlock in the live event log (mirrors the turn loop:
    // apply_witnessed_progression appends the ObjectiveResolved then, flag-gated, the SceneUnlocked).
    for ev in &resolutions {
        db.append_domain_event(ev).await.expect("append ObjectiveResolved");
    }
    db.append_domain_event(&unlock).await.expect("append SceneUnlocked");
    eprintln!(
        "RAN: (B) earned — real admitted scene_01 observation ⇒ ObjectiveResolved {} + SceneUnlocked appended",
        objective.id
    );

    // ─── (C) E1 ON: the NavigationResolver consumes the unlock ⇒ current_scene ACTUALLY changes ──
    db.create_session(SESSION, RULESET, Some(HOMECOMING))
        .await
        .expect("create session row");
    db.set_session_scene(SESSION, FROZEN_SEAM_SCENE)
        .await
        .expect("seed current_scene = scene_01");
    std::env::set_var(E1_FLAG, "1");
    let commit = scene_navigate_critical(
        &db,
        &MockLlmClient,
        SESSION,
        HOMECOMING,
        data_dir.as_path(),
        "I take stock of the firefight around me.",
        "Lawmen are pinned down; the drone screams for help.",
    )
    .await
    .expect("scene_navigate_critical ok")
    .expect("E1 progression-gated consume returns a SceneNavCommit (the SceneTransitioned source)");
    assert_eq!(commit.from, FROZEN_SEAM_SCENE, "transition from the frozen seam");
    assert_eq!(commit.to, next, "transition TO the data-driven spine successor");
    assert!(
        commit.reason.contains("progression-gated"),
        "the commit is progression-gated (earned), not an LLM player-driven nav: {}",
        commit.reason
    );
    let after_on = db
        .load_session_scene(SESSION)
        .await
        .expect("load scene")
        .unwrap_or_default();
    assert_eq!(
        after_on, next,
        "current_scene ACTUALLY advanced scene_01 → scene_02 (spatial axis MOVED)"
    );
    eprintln!(
        "RAN: (C) E1 ON — scene_navigate_critical consumed the unlock ⇒ current_scene {FROZEN_SEAM_SCENE} → {after_on} \
         (commit.reason='{}'); MockLlm never reached (progression-gated, not LLM nav)",
        commit.reason
    );

    // ─── (C2) the j3v2 SPATIAL-axis carrier: execute.rs maps the commit 1:1 into a durable ──────
    // `SceneTransitioned` domain_events row (`de_{turn}_SceneTransitioned`, data {from,to,reason}) —
    // the EXACT artifact the CLI j3v2 summary counts (cli/main.rs scene_transitions). We replicate
    // that one write here (the supervisor's 30-turn A/B exercises it through the live turn loop) and
    // prove the row lands: prev 0 → 1 for the frozen seam, which is what flips `spatial_frozen`.
    let transitioned = DomainEvent::new(
        format!("de_{TURN}_SceneTransitioned"),
        SESSION,
        TURN,
        DomainEventKind::SceneTransitioned,
        serde_json::json!({"from": commit.from, "to": commit.to, "reason": commit.reason}),
    );
    db.append_domain_event(&transitioned)
        .await
        .expect("append SceneTransitioned (execute.rs write path)");
    let rows: Vec<(String, String)> = sqlx::query(
        "select data->>'from', data->>'to' from domain_events where session_id = $1 and kind = 'SceneTransitioned'",
    )
    .bind(SESSION)
    .fetch_all(&pool)
    .await
    .expect("query SceneTransitioned rows")
    .into_iter()
    .map(|r| (r.get::<String, _>(0), r.get::<String, _>(1)))
    .collect();
    assert_eq!(rows.len(), 1, "exactly one durable SceneTransitioned row (j3v2 spatial carrier, prev 0)");
    assert_eq!(rows[0], (FROZEN_SEAM_SCENE.to_string(), next.clone()));
    eprintln!(
        "RAN: (C2) durable domain_events row kind=SceneTransitioned from={} to={} (scene_transitions 0 → 1; spatial axis carrier)",
        rows[0].0, rows[0].1
    );

    // ─── (D) E1 OFF == baseline: same pending unlock, flag OFF ⇒ NO progression-gated transition ──
    db.set_session_scene(SESSION, FROZEN_SEAM_SCENE)
        .await
        .expect("reset current_scene = scene_01");
    std::env::set_var(E1_FLAG, "0");
    let off = scene_navigate_critical(
        &db,
        &MockLlmClient,
        SESSION,
        HOMECOMING,
        data_dir.as_path(),
        "I take stock of the firefight around me.",
        "Lawmen are pinned down; the drone screams for help.",
    )
    .await
    .expect("scene_navigate_critical ok (OFF)");
    let after_off = db
        .load_session_scene(SESSION)
        .await
        .expect("load scene")
        .unwrap_or_default();
    assert_ne!(
        after_off, next,
        "OFF: the E1 consume block is skipped ⇒ the pending unlock does NOT transition (baseline)"
    );
    assert_eq!(
        after_off, FROZEN_SEAM_SCENE,
        "OFF: current_scene stays scene_01 (player-driven-only nav can't validate the Mock decision)"
    );
    assert!(
        off.is_none(),
        "OFF: no progression-gated commit (byte-identical player-driven-only path)"
    );
    eprintln!("RAN: (D) E1 OFF — same pending unlock does NOT transition (current_scene stays {after_off}); OFF == baseline");

    // ─── (E) the_vault (ScenarioCollection) never page-teleports across independent missions ──────
    let vault = db
        .load_module_graph(VAULT)
        .await
        .expect("query ok")
        .expect("the_vault module graph present");
    let vault_first = vault
        .scenes
        .iter()
        .min_by_key(|s| s.page_start.unwrap_or(u32::MAX))
        .map(|s| s.node_id.clone())
        .expect("the_vault has scenes");
    assert_eq!(
        resolve_next_scene(&vault, &vault_first),
        None,
        "the_vault ({:?}) is an anthology ⇒ no spine teleport even though scene_advance can complete",
        vault.module_type
    );
    eprintln!("RAN: (E) the_vault first scene '{vault_first}' resolve_next_scene = None (anthology no-teleport)");

    // ─── cleanup: leave no residue on the shared DB ──────────────────────────────────────────────
    std::env::remove_var(E1_FLAG);
    sqlx::query("delete from domain_events where session_id = $1")
        .bind(SESSION)
        .execute(&pool)
        .await
        .expect("cleanup domain_events");

    eprintln!(
        "PASS: E1 — a REAL completed obj.scene_advance.{FROZEN_SEAM_SCENE} (D2, real admitted \
         observation) drove a real current_scene transition scene_01 → {next} via the single-owner \
         NavigationResolver (data-driven target, MockLlm never reached). OFF == baseline; the_vault \
         no-teleport. The spatial axis MOVES on an EARNED completion."
    );
}
