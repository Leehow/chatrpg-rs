//! R5 T2: turns.pp_lifecycle 生命周期状态机 round-trip。
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!      cargo test -p trpg-db --test live_turn_pp_lifecycle -- --nocapture
//! 无 DATABASE_URL 时 SKIP（fail-closed，不卡 CI）。
use serde_json::json;
use trpg_db::Db;
use trpg_model::{PP_COMPLETE, PP_CRITICAL_DONE, PP_STREAMING};

// 两 test 并行跑，各用独立 session（共用同一 sessions 行会让 delete/insert 相互踩）。
const SESSION_ROUNDTRIP: &str = "sess_r5_pp_lifecycle_roundtrip";
const SESSION_LOAD_LAST: &str = "sess_r5_pp_lifecycle_load_last";

/// 0028 就地自施（幂等 add column if not exists）——不跑整条迁移链（链上有非幂等老迁移）。
/// 容忍并发自施竞态：`CREATE INDEX IF NOT EXISTS` 在两并发 test 下仍可能撞 pg_class
/// 唯一键（23505）；DDL 已是 if-not-exists 幂等，故忽略 "already exists" 类错误即可。
async fn ensure_column(db: &Db) {
    for stmt in include_str!("../../../migrations/0028_turn_pp_lifecycle_v120.sql").split(';') {
        let s = stmt.trim();
        if s.is_empty() {
            continue;
        }
        if let Err(e) = sqlx::query(s).execute(&db.pool).await {
            let msg = e.to_string();
            // 23505 = unique_violation（并发 create index 撞 pg_class）；42P07 = duplicate_table/index。
            let dup =
                msg.contains("already exists") || msg.contains("23505") || msg.contains("42P07");
            assert!(dup, "0028 statement must apply: {e}");
        }
    }
}

async fn seed_session(db: &Db, session: &str) {
    // turns.session_id 外键 → sessions(session_id)；先建会话再写 turn。
    db.create_session(session, "call_of_cthulhu_7e", None)
        .await
        .unwrap();
}

#[tokio::test]
async fn pp_lifecycle_streaming_critical_done_complete_roundtrip() {
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
    ensure_column(&db).await;
    seed_session(&db, SESSION_ROUNDTRIP).await;
    // turn_id 全局唯一化（set_turn_pp_lifecycle 按 turn_id 单键更新，避免跨 test 撞）。
    let turn = "turn_r5_pp_roundtrip";
    sqlx::query("delete from turns where session_id=$1")
        .bind(SESSION_ROUNDTRIP)
        .execute(&db.pool)
        .await
        .unwrap();

    // 无 turn 时 load_last 返 None（fail-closed：守卫据此立即放行）。
    assert_eq!(
        db.load_last_turn_pp_lifecycle(SESSION_ROUNDTRIP)
            .await
            .unwrap(),
        None
    );

    // save_turn 写入后默认 pp_lifecycle = streaming（迁移 default 值）。
    db.save_turn(
        SESSION_ROUNDTRIP,
        turn,
        "look around",
        "you see fog",
        json!({}),
        "ready",
    )
    .await
    .unwrap();
    assert_eq!(
        db.load_last_turn_pp_lifecycle(SESSION_ROUNDTRIP)
            .await
            .unwrap()
            .as_deref(),
        Some(PP_STREAMING)
    );

    // critical 末：set → critical_done。
    db.set_turn_pp_lifecycle(turn, PP_CRITICAL_DONE)
        .await
        .unwrap();
    assert_eq!(
        db.load_last_turn_pp_lifecycle(SESSION_ROUNDTRIP)
            .await
            .unwrap()
            .as_deref(),
        Some(PP_CRITICAL_DONE)
    );

    // heavy 末：set → complete。
    db.set_turn_pp_lifecycle(turn, PP_COMPLETE).await.unwrap();
    assert_eq!(
        db.load_last_turn_pp_lifecycle(SESSION_ROUNDTRIP)
            .await
            .unwrap()
            .as_deref(),
        Some(PP_COMPLETE)
    );

    // postprocess_status（ready）未被 pp_lifecycle 流转污染——两列正交。
    let st: (String,) =
        sqlx::query_as("select postprocess_status from turns where session_id=$1 and turn_id=$2")
            .bind(SESSION_ROUNDTRIP)
            .bind(turn)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        st.0, "ready",
        "pp_lifecycle 流转不得改写 postprocess_status"
    );

    sqlx::query("delete from turns where session_id=$1")
        .bind(SESSION_ROUNDTRIP)
        .execute(&db.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn load_last_returns_most_recent_turn_by_created_at() {
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
    ensure_column(&db).await;
    seed_session(&db, SESSION_LOAD_LAST).await;
    let turn_a = "turn_r5_pp_ll_a";
    let turn_b = "turn_r5_pp_ll_b";
    sqlx::query("delete from turns where session_id=$1")
        .bind(SESSION_LOAD_LAST)
        .execute(&db.pool)
        .await
        .unwrap();

    db.save_turn(SESSION_LOAD_LAST, turn_a, "i", "o", json!({}), "ready")
        .await
        .unwrap();
    db.set_turn_pp_lifecycle(turn_a, PP_COMPLETE).await.unwrap();
    // turn_b 后写 → created_at 更新 → load_last 必取 turn_b（仍 streaming）。
    db.save_turn(SESSION_LOAD_LAST, turn_b, "i2", "o2", json!({}), "ready")
        .await
        .unwrap();
    assert_eq!(
        db.load_last_turn_pp_lifecycle(SESSION_LOAD_LAST)
            .await
            .unwrap()
            .as_deref(),
        Some(PP_STREAMING),
        "load_last 必返最近一回合（turn_b）的 lifecycle，而非任意 turn"
    );

    sqlx::query("delete from turns where session_id=$1")
        .bind(SESSION_LOAD_LAST)
        .execute(&db.pool)
        .await
        .unwrap();
}
