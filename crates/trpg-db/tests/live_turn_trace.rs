//! obs T4: turn_traces upsert/load round-trip + turns.failure_kind 落库。
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!      cargo test -p trpg-db --test live_turn_trace -- --nocapture
//! 无 DATABASE_URL 时 SKIP（fail-closed，不卡 CI）。
use serde_json::json;
use trpg_db::Db;
use trpg_model::{TurnFailureRecord, TurnTrace};

const SESSION: &str = "sess_obs_t4_turn_trace";

/// 0029 就地自施（幂等 add column / create if not exists）——不跑整条迁移链
/// （链上有非幂等老迁移）。容忍并发自施竞态：`CREATE INDEX IF NOT EXISTS` 在
/// 两并发 test 下仍可能撞 pg_class 唯一键（23505）；DDL 已幂等，忽略 "already
/// exists" 类错误即可。
async fn ensure_schema(db: &Db) {
    for stmt in include_str!("../../../migrations/0029_turn_trace_failure_v120.sql").split(';') {
        let s = stmt.trim();
        if s.is_empty() {
            continue;
        }
        if let Err(e) = sqlx::query(s).execute(&db.pool).await {
            let msg = e.to_string();
            // 23505 = unique_violation（并发 create index 撞 pg_class）；42P07 = duplicate_table/index。
            let dup =
                msg.contains("already exists") || msg.contains("23505") || msg.contains("42P07");
            assert!(dup, "0029 statement must apply: {e}");
        }
    }
}

async fn seed_session(db: &Db) {
    // turns.session_id 外键 → sessions(session_id)；先建会话再写 turn。
    db.create_session(SESSION, "call_of_cthulhu_7e", None)
        .await
        .unwrap();
}

#[tokio::test]
async fn turn_trace_upsert_load_roundtrip_and_failure_kind() {
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
    seed_session(&db).await;
    let turn = "turn_obs_t4_roundtrip";
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

    // 无记录时 load 返 None。
    assert_eq!(
        db.load_turn_trace(turn).await.unwrap(),
        None,
        "无 trace 时 load 返 None"
    );

    // 写 turn 行（failure_kind 默认 NULL=成功）。
    db.save_turn(
        SESSION,
        turn,
        "look around",
        "you see fog",
        json!({}),
        "ready",
    )
    .await
    .unwrap();
    let fk: (Option<String>,) = sqlx::query_as("select failure_kind from turns where turn_id=$1")
        .bind(turn)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(fk.0, None, "新回合 failure_kind 默认 NULL（成功）");

    // upsert + load round-trip：写一份带各字段的 trace。
    let mut trace = TurnTrace::new(turn, SESSION);
    trace.phases_run = vec!["context_assembly".into(), "finalize".into()];
    trace.bp1_hash = Some("sha256:bp1".into());
    trace.bp2_hash = Some("sha256:bp2".into());
    trace.bp3_hash = Some("sha256:bp3".into());
    trace.signal = "turn_complete".into();
    trace.warnings = vec!["audit lag".into()];
    trace.pp_lifecycle = "complete".into();
    trace.narration_hash = Some("sha256:narr".into());
    db.upsert_turn_trace(&trace).await.unwrap();
    let loaded = db
        .load_turn_trace(turn)
        .await
        .unwrap()
        .expect("trace 应已落库");
    assert_eq!(loaded, trace, "load 回来的 trace 必与写入字节等价");

    // upsert 幂等覆盖：同 turn_id 改 signal 再写 → load 取到新值（on conflict do update）。
    let mut trace2 = trace.clone();
    trace2.signal = "turn_failed".into();
    trace2.failure = Some(TurnFailureRecord {
        phase: "finalize".into(),
        message: "save_turn timeout".into(),
        failure_kind: "failed_finalize".into(),
    });
    db.upsert_turn_trace(&trace2).await.unwrap();
    let loaded2 = db.load_turn_trace(turn).await.unwrap().expect("trace 仍在");
    assert_eq!(loaded2, trace2, "同 turn_id upsert 必覆盖为最新 trace");
    let cnt: (i64,) = sqlx::query_as("select count(*) from turn_traces where turn_id=$1")
        .bind(turn)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(cnt.0, 1, "upsert 不得产生重复行");

    // set_turn_failure_kind：失败时落 failure_kind（与 trace 正交）。
    db.set_turn_failure_kind(turn, "failed_finalize")
        .await
        .unwrap();
    let fk2: (Option<String>,) = sqlx::query_as("select failure_kind from turns where turn_id=$1")
        .bind(turn)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        fk2.0.as_deref(),
        Some("failed_finalize"),
        "failure_kind 必落库"
    );
    // failure_kind 不污染 postprocess_status（两列正交）。
    let st: (String,) = sqlx::query_as("select postprocess_status from turns where turn_id=$1")
        .bind(turn)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        st.0, "ready",
        "set_turn_failure_kind 不得改写 postprocess_status"
    );

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
