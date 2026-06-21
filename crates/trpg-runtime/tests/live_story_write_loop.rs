//! P6.8a — story_state WRITE-loop live acceptance (§二十四-#13 persist→selector segment + StoryThreadOpened
//! + OFF==baseline). The WRITE side closes the read/write asymmetry (P5⑤#3): P5 only LOADED
//! `rejected`; here the runtime PERSISTS it and the P5.3 selector then drops the thread.
//!
//! Segment (c) of the §二十四-#13 chain — persist → selector-drop (the WRITE side, proven against
//! a live DB). This is one of 3 segments that COMPOSE the chain (not a single tool-to-selector e2e;
//! the tool→nomination segment lives in trpg-gm). What this file proves directly:
//!   reject proposal → `commit_story_writes` (flag ON) → reload via `load_story_state`
//!   → `build_director_brief_packet` (the real P5.3 selector) → rejected thread NOT in
//!   primary/secondary.
//!
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!      cargo test -p trpg-runtime --test live_story_write_loop -- --nocapture --test-threads=1
//! No DATABASE_URL / unreachable ⇒ SKIP (fail-closed, never blocks CI).
//!
//! NOTE: `TRPG_STORY_WRITE_LOOP` is process-global; this file must run single-threaded
//! (`--test-threads=1`) so the OFF==baseline test never observes another test's ON flag.

use trpg_db::Db;
use trpg_director::{build_director_brief_packet, DirectorMode};
use trpg_model::{
    DomainEventKind, NpcActionIntent, NpcActionKind, StoryState, StoryThread, StoryThreadStatus,
    WorldReactionCandidate,
};
use trpg_runtime::director_brief::rejected_thread_ids;
use trpg_runtime::{commit_story_writes, rejection_proposal};

async fn connect_or_skip() -> Option<Db> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP: DATABASE_URL unset");
            return None;
        }
    };
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

/// Two live threads — the player engaged with `thr_a` and explicitly refused `thr_b`. The
/// thread we will reject (`thr_b`) is otherwise MORE attractive (higher urgency/interest), so
/// the only thing that can keep it out of the spotlight is a persisted rejection.
fn seed_story() -> StoryState {
    StoryState {
        active_threads: vec![
            StoryThread {
                thread_id: "thr_a".into(),
                status: StoryThreadStatus::Active,
                urgency: 0.3,
                player_interest: 0.4,
                participant_ids: vec!["npc_x".into()],
                ..Default::default()
            },
            StoryThread {
                thread_id: "thr_b".into(),
                status: StoryThreadStatus::Escalating,
                urgency: 0.9,
                player_interest: 0.9,
                participant_ids: vec!["npc_x".into()],
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

fn candidate_pool() -> Vec<WorldReactionCandidate> {
    vec![WorldReactionCandidate {
        npc_id: "npc_x".into(),
        stance: String::new(),
        emotional_state: String::new(),
        urgency: 0.0,
        feasibility: 0.0,
        risk: 0.0,
        action_intent: Some(NpcActionIntent {
            kind: NpcActionKind::Speak,
            target_ref: None,
            description: String::new(),
        }),
        knowledge_basis: vec![],
        source_event_ids: vec!["e1".into()],
    }]
}

/// §二十四-#13 FULL CHAIN (flag ON): persist a rejection → reload → run the real P5.3 selector
/// → the rejected thread is NOT primary/secondary, even though it scores higher pre-rejection.
#[tokio::test]
async fn full_chain_rejection_persist_then_selector_drops() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_sw_reject_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;
    prepare(&db, &session).await;

    // Seed the story (thr_b is the stronger thread).
    db.upsert_story_state(&session, &seed_story(), "t0")
        .await
        .unwrap();

    // BEFORE rejection: the selector picks thr_b as primary (it scores higher).
    let before = db.load_story_state(&session).await.unwrap().unwrap();
    let plan_before = build_director_brief_packet(
        DirectorMode::OnDemand,
        &candidate_pool(),
        &before,
        None,
        None,
        &[],
        &rejected_thread_ids(&before),
        "pc_1",
    );
    assert_eq!(
        plan_before.primary_thread_id.as_deref(),
        Some("thr_b"),
        "pre-rejection: stronger thr_b is primary"
    );

    // WRITE LOOP ON: persist a structured rejection proposal for thr_b (§宪法④ commit half).
    std::env::set_var("TRPG_STORY_WRITE_LOOP", "1");
    commit_story_writes(&db, &session, &[rejection_proposal("thr_b")], &[], "t1")
        .await
        .expect("commit_story_writes (ON) persists the rejection");
    std::env::remove_var("TRPG_STORY_WRITE_LOOP");

    // Reload: the persisted rejection is present.
    let after = db.load_story_state(&session).await.unwrap().unwrap();
    let rejected = rejected_thread_ids(&after);
    assert_eq!(
        rejected,
        vec!["thr_b".to_string()],
        "persisted player_interests now flags thr_b rejected"
    );

    // AFTER rejection: the real P5.3 selector drops thr_b from primary AND secondary.
    let plan_after = build_director_brief_packet(
        DirectorMode::OnDemand,
        &candidate_pool(),
        &after,
        None,
        None,
        &[],
        &rejected,
        "pc_1",
    );
    assert_eq!(
        plan_after.primary_thread_id.as_deref(),
        Some("thr_a"),
        "post-rejection: primary falls back to the non-rejected thr_a"
    );
    assert!(
        !plan_after
            .secondary_thread_ids
            .contains(&"thr_b".to_string()),
        "rejected thr_b must NOT appear in secondary either"
    );
    assert_ne!(
        plan_after.primary_thread_id.as_deref(),
        Some("thr_b"),
        "FULL CHAIN: rejected thread is never spotlighted"
    );

    purge(&db, &session).await;
}

/// StoryThreadOpened (flag ON): a Dormant thread whose related fact becomes player-known floors
/// to Introduced and persists. An already-open thread on the same fact is NOT advanced.
#[tokio::test]
async fn thread_opened_floors_dormant_to_introduced() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_sw_open_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;
    prepare(&db, &session).await;

    let story = StoryState {
        active_threads: vec![
            StoryThread {
                thread_id: "thr_dormant".into(),
                status: StoryThreadStatus::Dormant,
                related_fact_ids: vec!["fact_relic".into()],
                ..Default::default()
            },
            StoryThread {
                thread_id: "thr_already".into(),
                status: StoryThreadStatus::Active,
                related_fact_ids: vec!["fact_relic".into()],
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    db.upsert_story_state(&session, &story, "t0").await.unwrap();

    // The relic fact just became player-known (a PlayerLearnedFact this turn).
    std::env::set_var("TRPG_STORY_WRITE_LOOP", "1");
    commit_story_writes(&db, &session, &[], &["fact_relic".to_string()], "t1")
        .await
        .expect("commit_story_writes (ON) floors the dormant thread");
    std::env::remove_var("TRPG_STORY_WRITE_LOOP");

    let after = db.load_story_state(&session).await.unwrap().unwrap();
    let dormant = after
        .active_threads
        .iter()
        .find(|t| t.thread_id == "thr_dormant")
        .unwrap();
    let already = after
        .active_threads
        .iter()
        .find(|t| t.thread_id == "thr_already")
        .unwrap();
    assert_eq!(
        dormant.status,
        StoryThreadStatus::Introduced,
        "StoryThreadOpened: Dormant thread floored to Introduced"
    );
    assert_eq!(
        already.status,
        StoryThreadStatus::Active,
        "already-open thread is NOT advanced (P6.8b OutOfScope)"
    );

    // L3.1: the floor wrote through a StoryThreadOpened domain event keyed on the resulting
    // status. Exactly one (for the dormant thread that floored); the already-open thread is
    // untouched ⇒ no event for it.
    let events = db.list_domain_events(&session, 100).await.unwrap();
    let opened: Vec<_> = events
        .iter()
        .filter(|e| e.kind == DomainEventKind::StoryThreadOpened)
        .collect();
    assert_eq!(
        opened.len(),
        1,
        "exactly one StoryThreadOpened event in the ledger (only the dormant thread floored)"
    );
    assert_eq!(
        opened[0].event_id,
        format!("de_thread_{}_thr_dormant_Introduced", session),
        "idempotent key on the resulting status"
    );
    assert_eq!(opened[0].data["thread_id"], "thr_dormant");
    assert_eq!(opened[0].data["new_status"], "Introduced");

    purge(&db, &session).await;
}

/// L3.1 OFF==baseline for the ledger: with the flag unset, the floor write-loop is a no-op, so
/// NO StoryThread domain events are appended (the append lives strictly inside the ON path).
#[tokio::test]
async fn off_writes_no_thread_events() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    std::env::remove_var("TRPG_STORY_WRITE_LOOP");

    let session = format!("sess_sw_noev_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;
    prepare(&db, &session).await;

    let story = StoryState {
        active_threads: vec![StoryThread {
            thread_id: "thr_dormant".into(),
            status: StoryThreadStatus::Dormant,
            related_fact_ids: vec!["fact_relic".into()],
            ..Default::default()
        }],
        ..Default::default()
    };
    db.upsert_story_state(&session, &story, "t0").await.unwrap();

    // Flag OFF ⇒ commit is a no-op ⇒ no thread events written.
    commit_story_writes(&db, &session, &[], &["fact_relic".to_string()], "t1")
        .await
        .expect("commit_story_writes (OFF) is a no-op Ok");

    let events = db.list_domain_events(&session, 100).await.unwrap();
    assert!(
        events
            .iter()
            .all(|e| e.kind != DomainEventKind::StoryThreadOpened),
        "OFF: no StoryThread ledger events"
    );

    purge(&db, &session).await;
}

/// OFF == baseline: with the flag unset, `commit_story_writes` performs NO story_state write —
/// the persisted row is byte-identical to what was seeded (the P5 load side is unaffected).
#[tokio::test]
async fn off_is_baseline_no_write() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    // Ensure OFF regardless of test ordering.
    std::env::remove_var("TRPG_STORY_WRITE_LOOP");

    let session = format!("sess_sw_off_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;
    prepare(&db, &session).await;

    let seeded = seed_story();
    db.upsert_story_state(&session, &seeded, "t0")
        .await
        .unwrap();

    // Flag OFF ⇒ this must be a complete no-op (no read-modify-write of story_state).
    commit_story_writes(
        &db,
        &session,
        &[rejection_proposal("thr_b")],
        &["fact_relic".to_string()],
        "t1",
    )
    .await
    .expect("commit_story_writes (OFF) is a no-op Ok");

    let after = db.load_story_state(&session).await.unwrap().unwrap();
    assert_eq!(
        after, seeded,
        "OFF: story_state is byte-identical to the seed (no rejection, no thread floor)"
    );
    assert!(
        rejected_thread_ids(&after).is_empty(),
        "OFF: no rejection was persisted"
    );

    purge(&db, &session).await;
}
