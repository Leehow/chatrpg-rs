//! L4.2 — scene-plan trigger live acceptance: crossing a scene boundary produces exactly ONE
//! ScenePlanCreated event when `TRPG_DIRECTOR_SCENE_PLAN` is ON; OFF writes nothing (byte-identical
//! baseline). Mirrors `live_story_write_loop.rs` (DB-backed runtime acceptance against :54347).
//!
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!      cargo test -p trpg-runtime --test live_scene_plan -- --nocapture --test-threads=1
//! No DATABASE_URL / unreachable ⇒ SKIP (fail-closed, never blocks CI).
//!
//! NOTE: `TRPG_DIRECTOR_SCENE_PLAN` is process-global; this file must run single-threaded
//! (`--test-threads=1`) so the OFF==baseline test never observes another test's ON flag.

use trpg_db::Db;
use trpg_model::{DomainEventKind, StoryState, StoryThread, StoryThreadStatus};
use trpg_runtime::{emit_scene_plan_on_change, scene_forbidden_reveals};

async fn connect_or_skip() -> Option<Db> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = match Db::connect(&url).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("SKIP: connect failed: {e}");
            return None;
        }
    };
    db.migrate().await.expect("migrate after connect");
    Some(db)
}

async fn prepare(db: &Db, session: &str) {
    db.create_session(session, "call_of_cthulhu_7e", None)
        .await
        .expect("create_session");
}

async fn purge(db: &Db, session: &str) {
    for sql in [
        "delete from domain_events where session_id=$1",
        "delete from story_state where session_id=$1",
        "delete from sessions where session_id=$1",
    ] {
        sqlx::query(sql)
            .bind(session)
            .execute(&db.pool)
            .await
            .unwrap();
    }
}

fn seed_story() -> StoryState {
    StoryState {
        active_threads: vec![
            StoryThread {
                thread_id: "thr_a".into(),
                status: StoryThreadStatus::Active,
                ..Default::default()
            },
            StoryThread {
                thread_id: "thr_b".into(),
                status: StoryThreadStatus::Escalating,
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

async fn scene_plan_events(db: &Db, session: &str) -> Vec<trpg_model::DomainEvent> {
    db.list_domain_events(session, 100)
        .await
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == DomainEventKind::ScenePlanCreated)
        .collect()
}

/// OFF (default): a scene change emits NO ScenePlanCreated event — byte-identical baseline.
#[tokio::test]
async fn off_scene_change_emits_no_plan_event() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_sp_off_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;
    prepare(&db, &session).await;
    db.upsert_story_state(&session, &seed_story(), "t0")
        .await
        .unwrap();

    std::env::remove_var("TRPG_DIRECTOR_SCENE_PLAN");
    let emitted =
        emit_scene_plan_on_change(&db, &session, "mod_missing", "t1", "scene_market").await;
    assert!(!emitted, "OFF ⇒ no emit");
    assert!(
        scene_plan_events(&db, &session).await.is_empty(),
        "OFF ⇒ zero ScenePlanCreated rows (byte-identical baseline)"
    );
    purge(&db, &session).await;
}

/// ON: a scene change emits exactly ONE ScenePlanCreated event; a replayed crossing into the same
/// scene folds to the SAME idempotent row (on conflict do nothing) ⇒ still exactly one.
#[tokio::test]
async fn on_scene_change_emits_exactly_one_idempotent_plan_event() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_sp_on_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;
    prepare(&db, &session).await;
    db.upsert_story_state(&session, &seed_story(), "t0")
        .await
        .unwrap();

    std::env::set_var("TRPG_DIRECTOR_SCENE_PLAN", "1");
    let first = emit_scene_plan_on_change(&db, &session, "mod_missing", "t1", "scene_market").await;
    // Replay the SAME scene crossing on a later turn — must not double-count.
    let second = emit_scene_plan_on_change(&db, &session, "mod_missing", "t2", "scene_market").await;
    std::env::remove_var("TRPG_DIRECTOR_SCENE_PLAN");

    assert!(first, "ON ⇒ first crossing emits");
    assert!(second, "ON ⇒ replay still appends (on conflict do nothing, no error)");

    let events = scene_plan_events(&db, &session).await;
    assert_eq!(
        events.len(),
        1,
        "exactly ONE ScenePlanCreated row for the scene (idempotent key)"
    );
    assert_eq!(events[0].event_id, format!("de_scene_{session}_scene_market_created"));
    assert_eq!(events[0].data["scene_id"], "scene_market");
    purge(&db, &session).await;
}

/// L4.3 — the PROACTIVE forbidden-reveal set is derived from REAL persisted state: a building
/// thread's secret fact is forbidden; OFF ⇒ empty (the reactive gate stays in full charge). The
/// behavioral "the LLM does not reveal it" assertion is LV.1 condition #4; here we prove the set
/// fed to the Narrator is correctly computed from live story + the session's current scene.
#[tokio::test]
async fn forbidden_reveals_derived_from_live_building_thread() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_sp_fbd_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;
    prepare(&db, &session).await;

    // A still-building (Active, not ReadyForPayoff) thread carrying a secret fact, plus a ripe
    // thread whose payoff fact must NOT be forbidden.
    let story = StoryState {
        active_threads: vec![
            StoryThread {
                thread_id: "thr_build".into(),
                status: StoryThreadStatus::Active,
                related_fact_ids: vec!["secret_cult_meeting".into()],
                ..Default::default()
            },
            StoryThread {
                thread_id: "thr_ripe".into(),
                status: StoryThreadStatus::ReadyForPayoff,
                related_fact_ids: vec!["payoff_confront".into()],
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    db.upsert_story_state(&session, &story, "t0").await.unwrap();
    db.set_session_scene(&session, "scene_temple").await.unwrap();

    // OFF: empty proactive set — the reactive gate stays fully in charge (byte-identical baseline).
    std::env::remove_var("TRPG_DIRECTOR_SCENE_PLAN");
    assert!(
        scene_forbidden_reveals(&db, &session, None).await.is_empty(),
        "OFF ⇒ no proactive forbidden set"
    );

    // ON: the building thread's secret is forbidden; the ripe thread's payoff fact is NOT.
    std::env::set_var("TRPG_DIRECTOR_SCENE_PLAN", "1");
    let forbidden = scene_forbidden_reveals(&db, &session, None).await;
    std::env::remove_var("TRPG_DIRECTOR_SCENE_PLAN");
    assert_eq!(
        forbidden,
        vec!["secret_cult_meeting".to_string()],
        "ON ⇒ building thread's secret forbidden; ripe payoff fact excluded"
    );
    purge(&db, &session).await;
}
