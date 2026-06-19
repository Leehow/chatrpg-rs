//! coverage: list_turn_traces 取一个会话全部 trace（created_at 升序）。
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!      cargo test -p trpg-db --test live_list_turn_traces -- --nocapture
//! 无 DATABASE_URL 时 SKIP（fail-closed，不卡 CI）。
use serde_json::json;
use trpg_db::Db;
use trpg_model::TurnTrace;

const SESSION: &str = "sess_coverage_list_traces";

/// 0029 就地自施（幂等 add column / create if not exists）——不跑整条迁移链。
async fn ensure_schema(db: &Db) {
    for stmt in include_str!("../../../migrations/0029_turn_trace_failure_v120.sql").split(';') {
        let s = stmt.trim();
        if s.is_empty() {
            continue;
        }
        if let Err(e) = sqlx::query(s).execute(&db.pool).await {
            let msg = e.to_string();
            let dup =
                msg.contains("already exists") || msg.contains("23505") || msg.contains("42P07");
            assert!(dup, "0029 statement must apply: {e}");
        }
    }
}

#[tokio::test]
async fn list_turn_traces_returns_session_traces_in_order() {
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
    ensure_schema(&db).await;
    db.create_session(SESSION, "call_of_cthulhu_7e", None)
        .await
        .unwrap();
    sqlx::query("delete from turn_traces where session_id=$1")
        .bind(SESSION)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("delete from turns where session_id=$1")
        .bind(SESSION)
        .execute(&db.pool)
        .await
        .unwrap();

    // 空会话 → 空 Vec。
    assert!(
        db.list_turn_traces(SESSION, 100).await.unwrap().is_empty(),
        "无 trace 时返回空 Vec"
    );

    // 写两个回合的 trace（turns 行先建满足外键）。
    for (turn, signal, life) in [
        ("t_cov_1", "turn_complete", "complete"),
        ("t_cov_2", "turn_failed", "failed"),
    ] {
        db.save_turn(SESSION, turn, "act", "narr", json!({}), "ready")
            .await
            .unwrap();
        let mut tr = TurnTrace::new(turn, SESSION);
        tr.signal = signal.into();
        tr.pp_lifecycle = life.into();
        db.upsert_turn_trace(&tr).await.unwrap();
    }

    let traces = db.list_turn_traces(SESSION, 100).await.unwrap();
    assert_eq!(traces.len(), 2, "应取到两条 trace");
    assert_eq!(traces[0].turn_id, "t_cov_1", "created_at 升序：先写的在前");
    assert_eq!(traces[1].turn_id, "t_cov_2");
    assert_eq!(traces[1].pp_lifecycle, "failed", "trace 字段完整反序列化");

    sqlx::query("delete from turn_traces where session_id=$1")
        .bind(SESSION)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("delete from turns where session_id=$1")
        .bind(SESSION)
        .execute(&db.pool)
        .await
        .unwrap();
}
