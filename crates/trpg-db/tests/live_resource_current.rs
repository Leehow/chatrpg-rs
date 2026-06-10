//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
//!      cargo test -p trpg-db --test live_resource_current -- --nocapture
use serde_json::json;
use trpg_db::Db;
use trpg_model::*;

const SESSION: &str = "sess_slice_b_rc_test";
const ACTOR: &str = "pc.slice_b_rc";

fn kernel() -> RuleKernel {
    RuleKernel {
        resource_tracks: vec![json!({"id":"sanity","kind":"track","initial":0,"max":99})],
        ..Default::default()
    }
}

#[tokio::test]
async fn write_then_load_round_trips_and_caps() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect: {e}"); return; } };
    sqlx::query("delete from generic_parameter_states where session_id=$1 and target_id=$2").bind(SESSION).bind(ACTOR).execute(&db.pool).await.unwrap();

    assert_eq!(db.load_resource_current(SESSION, ACTOR, "sanity", &kernel()).await, Some(0));

    let stored = db.write_resource_current(SESSION, ACTOR, "sanity", 65, Some(99), &[], Visibility::GmOnly, 0).await.unwrap();
    assert_eq!(stored, 65);
    assert_eq!(db.load_resource_current(SESSION, ACTOR, "sanity", &kernel()).await, Some(65));

    let capped = db.write_resource_current(SESSION, ACTOR, "sanity", 250, Some(99), &[], Visibility::GmOnly, 0).await.unwrap();
    assert_eq!(capped, 99);
    assert_eq!(db.load_resource_current(SESSION, ACTOR, "sanity", &kernel()).await, Some(99));

    sqlx::query("delete from generic_parameter_states where session_id=$1 and target_id=$2").bind(SESSION).bind(ACTOR).execute(&db.pool).await.unwrap();
}
