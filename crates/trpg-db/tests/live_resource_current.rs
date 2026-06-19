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
