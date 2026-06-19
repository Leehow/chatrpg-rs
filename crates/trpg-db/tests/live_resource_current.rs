//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
//!      cargo test -p trpg-db --test live_resource_current -- --nocapture
use serde_json::json;
use trpg_db::Db;
use trpg_model::*;

const SESSION: &str = "sess_slice_b_rc_test";
const ACTOR: &str = "pc.slice_b_rc";
const TOP_LEVEL_SESSION: &str = "sess_resource_top_level_seed";
const TOP_LEVEL_ACTOR: &str = "pc.resource_seed";

fn kernel() -> RuleKernel {
    RuleKernel {
        resource_tracks: vec![json!({"id":"sanity","kind":"track","initial":0,"max":99})],
        ..Default::default()
    }
}

#[tokio::test]
async fn write_then_load_round_trips_and_caps() {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP: DATABASE_URL unset");
            return;
        }
    };
    let db = match Db::connect(&url).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("SKIP: connect: {e}");
            return;
        }
    };
    sqlx::query("delete from generic_parameter_states where session_id=$1 and target_id=$2")
        .bind(SESSION)
        .bind(ACTOR)
        .execute(&db.pool)
        .await
        .unwrap();

    assert_eq!(
        db.load_resource_current(SESSION, ACTOR, "sanity", &kernel())
            .await,
        Some(0)
    );

    let stored = db
        .write_resource_current(
            SESSION,
            ACTOR,
            "sanity",
            65,
            Some(99),
            &[],
            Visibility::GmOnly,
            0,
        )
        .await
        .unwrap();
    assert_eq!(stored, 65);
    assert_eq!(
        db.load_resource_current(SESSION, ACTOR, "sanity", &kernel())
            .await,
        Some(65)
    );

    let capped = db
        .write_resource_current(
            SESSION,
            ACTOR,
            "sanity",
            250,
            Some(99),
            &[],
            Visibility::GmOnly,
            0,
        )
        .await
        .unwrap();
    assert_eq!(capped, 99);
    assert_eq!(
        db.load_resource_current(SESSION, ACTOR, "sanity", &kernel())
            .await,
        Some(99)
    );

    sqlx::query("delete from generic_parameter_states where session_id=$1 and target_id=$2")
        .bind(SESSION)
        .bind(ACTOR)
        .execute(&db.pool)
        .await
        .unwrap();
}

/// P6.2 codex#2 guard: two genuinely-different resource changes to the same actor/track in
/// one session MUST produce two ResourceChanged rows (not one swallowed by event_id collision),
/// while a true replay of the SAME transition stays idempotent. Also asserts OFF==baseline:
/// the write_resource_current return value + the stored gps row are unaffected by the append.
#[tokio::test]
async fn resource_changed_two_different_changes_two_rows_and_replay_idempotent() {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP: DATABASE_URL unset");
            return;
        }
    };
    let db = match Db::connect(&url).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("SKIP: connect: {e}");
            return;
        }
    };
    const RC_SESSION: &str = "sess_p62_resource_changed_guard";
    const RC_ACTOR: &str = "pc.p62_rc_guard";
    // Clean both the gps row and any prior ResourceChanged events for this session.
    sqlx::query("delete from generic_parameter_states where session_id=$1 and target_id=$2")
        .bind(RC_SESSION)
        .bind(RC_ACTOR)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("delete from domain_events where session_id=$1")
        .bind(RC_SESSION)
        .execute(&db.pool)
        .await
        .unwrap();

    let count_rc = |db: Db| async move {
        sqlx::query_scalar::<_, i64>(
            "select count(*) from domain_events where session_id=$1 and kind='ResourceChanged'",
        )
        .bind(RC_SESSION)
        .fetch_one(&db.pool)
        .await
        .unwrap()
    };
    let load_gps = |db: Db| async move {
        sqlx::query_scalar::<_, serde_json::Value>(
            "select value_json from generic_parameter_states where session_id=$1 and target_kind='actor' and target_id=$2 and parameter_path='resources.sanity.current'",
        )
        .bind(RC_SESSION)
        .bind(RC_ACTOR)
        .fetch_one(&db.pool)
        .await
        .unwrap()
    };

    // Change 1: prior=seed(0) -> 30.
    let r1 = db
        .write_resource_current(
            RC_SESSION,
            RC_ACTOR,
            "sanity",
            30,
            Some(99),
            &[],
            Visibility::GmOnly,
            0,
        )
        .await
        .unwrap();
    assert_eq!(r1, 30);
    assert_eq!(load_gps(db.clone()).await, json!(30));
    assert_eq!(count_rc(db.clone()).await, 1, "first change => 1 ResourceChanged row");

    // Change 2: a DIFFERENT transition prior=30 -> 70. world_tick still 0 — must NOT collide.
    let r2 = db
        .write_resource_current(
            RC_SESSION,
            RC_ACTOR,
            "sanity",
            70,
            Some(99),
            &[],
            Visibility::GmOnly,
            0,
        )
        .await
        .unwrap();
    assert_eq!(r2, 70);
    assert_eq!(load_gps(db.clone()).await, json!(70));
    assert_eq!(
        count_rc(db.clone()).await,
        2,
        "codex#2: two DIFFERENT changes must yield TWO rows, not one swallowed"
    );

    // Replay-idempotency: a "no-op" write where current already equals the target (prior=70 ->
    // capped=70) is itself a distinct transition (70->70, a 3rd row). Issuing that IDENTICAL
    // write AGAIN reproduces the same hash inputs => identical event_id => `on conflict (event_id)
    // do nothing` folds it. So row 1 of 70->70 adds a row; row 2 of 70->70 must NOT.
    let r_noop = db
        .write_resource_current(
            RC_SESSION, RC_ACTOR, "sanity", 70, Some(99), &[], Visibility::GmOnly, 0,
        )
        .await
        .unwrap();
    assert_eq!(r_noop, 70);
    let after_first_noop = count_rc(db.clone()).await; // 70->70 is a new distinct transition
    assert_eq!(after_first_noop, 3, "70->70 is a distinct transition => 3rd row");

    // Now REPLAY the identical 70->70 write: same (prior,capped,cap,visibility) => same event_id.
    // OFF==baseline: the return value + stored gps row are unchanged by the (folded) append.
    let r_replay = db
        .write_resource_current(
            RC_SESSION, RC_ACTOR, "sanity", 70, Some(99), &[], Visibility::GmOnly, 0,
        )
        .await
        .unwrap();
    assert_eq!(r_replay, 70, "OFF==baseline: replay return value unchanged");
    assert_eq!(load_gps(db.clone()).await, json!(70));
    assert_eq!(
        count_rc(db.clone()).await,
        3,
        "replaying the identical prior=70->70 transition stays idempotent (still 3 rows)"
    );

    sqlx::query("delete from generic_parameter_states where session_id=$1 and target_id=$2")
        .bind(RC_SESSION)
        .bind(RC_ACTOR)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("delete from domain_events where session_id=$1")
        .bind(RC_SESSION)
        .execute(&db.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn top_level_sheet_field_seeds_resource_track_current() {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP: DATABASE_URL unset");
            return;
        }
    };
    let db = match Db::connect(&url).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("SKIP: connect: {e}");
            return;
        }
    };
    sqlx::query("delete from generic_parameter_states where session_id=$1 and target_id=$2")
        .bind(TOP_LEVEL_SESSION)
        .bind(TOP_LEVEL_ACTOR)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query(r#"insert into runtime_actor_parameters
        (id, actor_param_id, session_id, actor_id, actor_kind, ruleset_id, source_kind, template_id, display_name, sheet_json, mechanical_profile, status_json, visibility, created_at_tick, updated_at_tick)
        values (gen_random_uuid(), $1, $2, $3, 'player_character', 'call_of_cthulhu_7e', 'test_seed', null, $3, $4, '{}'::jsonb, '{}'::jsonb, 'gm_only', 0, 0)
        on conflict (session_id, actor_id) do update set sheet_json=excluded.sheet_json, ruleset_id=excluded.ruleset_id, actor_kind=excluded.actor_kind"#)
        .bind(format!("ap_{TOP_LEVEL_ACTOR}")).bind(TOP_LEVEL_SESSION).bind(TOP_LEVEL_ACTOR)
        .bind(json!({"luck": 55, "resources": {}}))
        .execute(&db.pool).await.unwrap();

    let kernel = RuleKernel {
        resource_tracks: vec![json!({"id":"luck","kind":"points","max":99})],
        ..Default::default()
    };
    assert_eq!(
        db.load_resource_current(TOP_LEVEL_SESSION, TOP_LEVEL_ACTOR, "luck", &kernel)
            .await,
        Some(55)
    );

    sqlx::query("delete from generic_parameter_states where session_id=$1 and target_id=$2")
        .bind(TOP_LEVEL_SESSION)
        .bind(TOP_LEVEL_ACTOR)
        .execute(&db.pool)
        .await
        .unwrap();
}
