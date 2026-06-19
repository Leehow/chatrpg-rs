//! R5 T2: turn 高水位守卫三分支（fail-closed 放行，从不硬拒玩家）。
//! Run: TRPG_TEST_DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!      cargo test -p trpg-runtime --test turn_high_water_guard -- --nocapture
//! 无 TRPG_TEST_DATABASE_URL 时 SKIP（fail-closed，不卡 CI）。
use serde_json::json;
use std::time::Instant;
use trpg_db::Db;
use trpg_model::PP_CRITICAL_DONE;
use trpg_runtime::await_prev_turn_critical;

/// 0028 就地自施（幂等）；容忍并发自施撞 pg_class 唯一键（23505/42P07）。
async fn apply_0028(db: &Db) {
    for stmt in include_str!("../../../migrations/0028_turn_pp_lifecycle_v120.sql").split(';') {
        let s = stmt.trim();
        if s.is_empty() {
            continue;
        }
        if let Err(e) = sqlx::query(s).execute(&db.pool).await {
            let msg = e.to_string();
            let dup =
                msg.contains("already exists") || msg.contains("23505") || msg.contains("42P07");
            assert!(dup, "0028 statement must apply: {e}");
        }
    }
}

async fn setup(url: &str, session: &str) -> Db {
    let db = Db::connect(url).await.expect("connect");
    apply_0028(&db).await;
    db.create_session(session, "call_of_cthulhu_7e", None)
        .await
        .unwrap();
    sqlx::query("delete from turns where session_id=$1")
        .bind(session)
        .execute(&db.pool)
        .await
        .unwrap();
    db
}

fn skip() -> Option<String> {
    match std::env::var("TRPG_TEST_DATABASE_URL") {
        Ok(u) => Some(u),
        Err(_) => {
            eprintln!("SKIP: set TRPG_TEST_DATABASE_URL to CoC DB on :54347");
            None
        }
    }
}

#[tokio::test]
async fn no_prev_turn_returns_immediately() {
    let Some(url) = skip() else {
        return;
    };
    let session = "sess_r5_hw_no_prev";
    let db = setup(&url, session).await;
    let t = Instant::now();
    await_prev_turn_critical(&db, session, 2000, 100).await; // 无 turn → 立即放行
    assert!(t.elapsed().as_millis() < 200, "无上一回合必须立即放行");
}

#[tokio::test]
async fn critical_already_reached_returns_immediately() {
    let Some(url) = skip() else {
        return;
    };
    let session = "sess_r5_hw_critical_ready";
    let db = setup(&url, session).await;
    let turn = "turn_r5_hw_cr";
    db.save_turn(session, turn, "i", "o", json!({}), "ready")
        .await
        .unwrap();
    db.set_turn_pp_lifecycle(turn, PP_CRITICAL_DONE)
        .await
        .unwrap();
    let t = Instant::now();
    await_prev_turn_critical(&db, session, 2000, 100).await; // >= critical → 立即
    assert!(t.elapsed().as_millis() < 200, "critical_done 必须立即放行");
}

#[tokio::test]
async fn waits_then_proceeds_when_critical_arrives_late() {
    let Some(url) = skip() else {
        return;
    };
    let session = "sess_r5_hw_late";
    let db = setup(&url, session).await;
    let turn = "turn_r5_hw_late";
    db.save_turn(session, turn, "i", "o", json!({}), "ready")
        .await
        .unwrap(); // streaming
                   // 后台 ~300ms 后翻 critical_done（模拟前一回合 critical 组延迟落账）。
    let db2 = db.clone();
    let turn_owned = turn.to_string();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let _ = db2
            .set_turn_pp_lifecycle(&turn_owned, PP_CRITICAL_DONE)
            .await;
    });
    let t = Instant::now();
    await_prev_turn_critical(&db, session, 2000, 100).await;
    let ms = t.elapsed().as_millis();
    assert!(ms >= 250, "应等到 critical 到达（≥~300ms），实测 {ms}ms");
    assert!(
        ms < 1500,
        "到达后必须立刻继续，远早于 2000ms 超时，实测 {ms}ms"
    );
}

#[tokio::test]
async fn timeout_proceeds_fail_closed_when_critical_never_arrives() {
    let Some(url) = skip() else {
        return;
    };
    let session = "sess_r5_hw_timeout";
    let db = setup(&url, session).await;
    let turn = "turn_r5_hw_timeout";
    db.save_turn(session, turn, "i", "o", json!({}), "ready")
        .await
        .unwrap(); // 恒 streaming
    let t = Instant::now();
    await_prev_turn_critical(&db, session, 600, 100).await; // 永不到达 → 超时放行
    let ms = t.elapsed().as_millis();
    assert!(ms >= 600, "超时前必须 bounded 等满 timeout，实测 {ms}ms");
    assert!(
        ms < 1200,
        "超时后必须 fail-closed 放行（不死等），实测 {ms}ms"
    );
}
