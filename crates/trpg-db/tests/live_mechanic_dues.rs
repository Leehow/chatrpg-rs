//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
//!      cargo test -p trpg-db --test live_mechanic_dues -- --nocapture
use chrono::Utc;
use serde_json::json;
use sqlx::Row;
use trpg_db::Db;
use trpg_model::{DueSource, DueStatus, MechanicDue};

const SESSION: &str = "sess_b4_mechanic_dues_test";
const REOPEN_SESSION: &str = "sess_b6_reopen_scene_waived_test";

fn sample_due() -> MechanicDue {
    MechanicDue {
        due_id: format!("due_{}", uuid::Uuid::new_v4().simple()),
        session_id: SESSION.to_string(),
        turn_id: "turn_b4_test".to_string(),
        source: DueSource::Threshold,
        source_track: Some("sanity".to_string()),
        hook_event: None,
        mechanic_id: None,
        threshold_desc: "may trigger temporary insanity".to_string(),
        followup_procedure_id: Some("coc.temporary_insanity".to_string()),
        owner_kind: "actor".to_string(),
        owner_id: "pc.current".to_string(),
        evidence: json!({"before": 38, "after": 32, "delta": -6}),
        status: DueStatus::Open,
        created_at: Utc::now(),
    }
}

/// 0027 就地自施（幂等 create if not exists）——不跑整条迁移链（链上有
/// 非幂等老迁移），与 live db 实际状态解耦。
async fn ensure_table(db: &Db) {
    for stmt in include_str!("../../../migrations/0027_mechanic_dues_v120.sql").split(';') {
        let s = stmt.trim();
        if s.is_empty() { continue; }
        sqlx::query(s).execute(&db.pool).await.expect("0027 statement must apply");
    }
}

#[tokio::test]
async fn mechanic_due_roundtrip_and_status_update() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect: {e}"); return; } };
    ensure_table(&db).await;
    sqlx::query("delete from mechanic_dues where session_id=$1").bind(SESSION).execute(&db.pool).await.unwrap();

    let due = sample_due();
    db.insert_mechanic_due(&due).await.unwrap();

    let open = db.list_open_mechanic_dues(SESSION).await.unwrap();
    let got = open.iter().find(|d| d.due_id == due.due_id).expect("list_open must contain the inserted due");
    assert_eq!(got.source, DueSource::Threshold);
    assert_eq!(got.status, DueStatus::Open);
    assert_eq!(got.turn_id, "turn_b4_test");
    assert_eq!(got.source_track.as_deref(), Some("sanity"));
    assert_eq!(got.followup_procedure_id.as_deref(), Some("coc.temporary_insanity"));
    assert_eq!(got.threshold_desc, "may trigger temporary insanity");
    assert_eq!(got.owner_kind, "actor");
    assert_eq!(got.owner_id, "pc.current");
    assert_eq!(got.evidence.get("delta").and_then(|v| v.as_i64()), Some(-6));

    db.update_mechanic_due_status(&due.due_id, "waived", Some("reason"), Some("scene")).await.unwrap();
    let open_after = db.list_open_mechanic_dues(SESSION).await.unwrap();
    assert!(!open_after.iter().any(|d| d.due_id == due.due_id), "waived due must leave list_open");
    let waived = db.list_mechanic_dues_with_status(SESSION, "waived").await.unwrap();
    let w = waived.iter().find(|d| d.due_id == due.due_id).expect("list_mechanic_dues_with_status(waived) must contain it");
    assert_eq!(w.status, DueStatus::Waived);
    let row = sqlx::query("select waive_reason, waive_scope from mechanic_dues where due_id=$1")
        .bind(&due.due_id).fetch_one(&db.pool).await.unwrap();
    assert_eq!(row.get::<Option<String>, _>("waive_reason").as_deref(), Some("reason"), "waive_reason must land in the row");
    assert_eq!(row.get::<Option<String>, _>("waive_scope").as_deref(), Some("scene"), "waive_scope must land in the row");

    sqlx::query("delete from mechanic_dues where session_id=$1").bind(SESSION).execute(&db.pool).await.unwrap();
}

/// B6 场景切换重开：scope=scene 的 waived 行回到 open（waive 记账列清空），
/// scope=turn 的 waived 行不受影响——watcher 抑制规则①（同 key 已有 open due
/// 跳过）随之接管，scene 豁免不再是会话级永久豁免。
#[tokio::test]
async fn reopen_scene_waived_dues_reopens_only_scene_scope() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect: {e}"); return; } };
    ensure_table(&db).await;
    sqlx::query("delete from mechanic_dues where session_id=$1").bind(REOPEN_SESSION).execute(&db.pool).await.unwrap();

    let mut scene_due = sample_due();
    scene_due.session_id = REOPEN_SESSION.to_string();
    let mut turn_due = sample_due();
    turn_due.session_id = REOPEN_SESSION.to_string();
    db.insert_mechanic_due(&scene_due).await.unwrap();
    db.insert_mechanic_due(&turn_due).await.unwrap();
    db.update_mechanic_due_status(&scene_due.due_id, "waived", Some("calm scene"), Some("scene")).await.unwrap();
    db.update_mechanic_due_status(&turn_due.due_id, "waived", Some("narrative"), Some("turn")).await.unwrap();
    assert!(db.list_open_mechanic_dues(REOPEN_SESSION).await.unwrap().is_empty(), "both waived → no open dues");

    let reopened = db.reopen_scene_waived_dues(REOPEN_SESSION).await.unwrap();
    assert_eq!(reopened, 1, "exactly the scene-scoped row must reopen");

    let open = db.list_open_mechanic_dues(REOPEN_SESSION).await.unwrap();
    assert!(open.iter().any(|d| d.due_id == scene_due.due_id), "scene-waived due must be open again");
    assert!(!open.iter().any(|d| d.due_id == turn_due.due_id), "turn-waived due must stay waived");
    let waived = db.list_mechanic_dues_with_status(REOPEN_SESSION, "waived").await.unwrap();
    assert!(waived.iter().any(|d| d.due_id == turn_due.due_id), "turn-waived due must remain in waived list");
    // 重开后 waive 记账列清空（open 行不残留 scene 标记，避免未来查询误抑制）。
    let row = sqlx::query("select waive_reason, waive_scope from mechanic_dues where due_id=$1")
        .bind(&scene_due.due_id).fetch_one(&db.pool).await.unwrap();
    assert_eq!(row.get::<Option<String>, _>("waive_reason"), None, "reopened row must clear waive_reason");
    assert_eq!(row.get::<Option<String>, _>("waive_scope"), None, "reopened row must clear waive_scope");

    sqlx::query("delete from mechanic_dues where session_id=$1").bind(REOPEN_SESSION).execute(&db.pool).await.unwrap();
}
