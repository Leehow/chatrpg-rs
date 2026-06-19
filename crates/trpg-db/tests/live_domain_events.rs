//! eventlog T1：domain_events append/list 幂等 + seq 时间线 + 按回合过滤。
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!      cargo test -p trpg-db --test live_domain_events -- --nocapture
//! 无 DATABASE_URL 时 SKIP（fail-closed，不卡 CI）。
use chrono::{DateTime, Utc};
use serde_json::json;
use trpg_db::Db;
use trpg_model::{DomainEvent, DomainEventKind};

const SESSION: &str = "sess_eventlog_t1_domain_events";

/// 0030 就地自施（幂等 create table / index if not exists）——不跑整条迁移链
/// （链上有非幂等老迁移）。容忍并发自施竞态：CREATE INDEX IF NOT EXISTS 在两并发
/// test 下仍可能撞 pg_class 唯一键（23505）；DDL 已幂等，忽略 "already exists" 类即可。
async fn ensure_schema(db: &Db) {
    for stmt in include_str!("../../../migrations/0030_domain_events.sql").split(';') {
        let s = stmt.trim();
        if s.is_empty() {
            continue;
        }
        if let Err(e) = sqlx::query(s).execute(&db.pool).await {
            let msg = e.to_string();
            let dup =
                msg.contains("already exists") || msg.contains("23505") || msg.contains("42P07");
            assert!(dup, "0030 statement must apply: {e}");
        }
    }
}

fn ev(
    event_id: &str,
    turn_id: &str,
    kind: DomainEventKind,
    data: serde_json::Value,
) -> DomainEvent {
    DomainEvent {
        event_id: event_id.into(),
        session_id: SESSION.into(),
        turn_id: turn_id.into(),
        kind,
        data,
        source_refs: Vec::new(),
        created_at: DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
    }
}

#[tokio::test]
async fn domain_events_append_list_idempotent_and_turn_filter() {
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

    // 清场（domain_events 无外键，直接按 session 删）。
    sqlx::query("delete from domain_events where session_id=$1")
        .bind(SESSION)
        .execute(&db.pool)
        .await
        .unwrap();

    // 无记录时 list 返空。
    assert!(
        db.list_domain_events(SESSION, 100)
            .await
            .unwrap()
            .is_empty(),
        "空会话 list 返空"
    );

    let turn_a = "turn_eventlog_a";
    let turn_b = "turn_eventlog_b";
    let started = ev(
        "de_turn_a_TurnStarted",
        turn_a,
        DomainEventKind::TurnStarted,
        json!({"input": "look"}),
    );
    let finalized = ev(
        "de_turn_a_TurnFinalized",
        turn_a,
        DomainEventKind::TurnFinalized,
        json!({"signal": "turn_complete"}),
    );

    // append 2 条（同 turn_a）。
    db.append_domain_event(&started).await.unwrap();
    db.append_domain_event(&finalized).await.unwrap();

    // list_domain_events 返 2 条，seq 升序（先 Started 后 Finalized）。
    let listed = db.list_domain_events(SESSION, 100).await.unwrap();
    assert_eq!(listed.len(), 2, "应有 2 条");
    assert_eq!(listed[0], started, "seq 升序首条 = TurnStarted（字节等价）");
    assert_eq!(
        listed[1], finalized,
        "seq 升序次条 = TurnFinalized（字节等价）"
    );

    // 重放同 event_id → 仍 2 条（on conflict do nothing 幂等）。
    db.append_domain_event(&started).await.unwrap();
    db.append_domain_event(&finalized).await.unwrap();
    let relisted = db.list_domain_events(SESSION, 100).await.unwrap();
    assert_eq!(relisted.len(), 2, "重放同 event_id 不得新增行（幂等）");

    // 另一回合 + SceneTransitioned。
    let scene = ev(
        "de_turn_b_SceneTransitioned",
        turn_b,
        DomainEventKind::SceneTransitioned,
        json!({"from": "sc01", "to": "sc02"}),
    );
    db.append_domain_event(&scene).await.unwrap();

    // list_domain_events_for_turn 按回合过滤。
    let for_a = db.list_domain_events_for_turn(turn_a).await.unwrap();
    assert_eq!(for_a.len(), 2, "turn_a 有 2 条");
    assert!(for_a.iter().all(|e| e.turn_id == turn_a));
    let for_b = db.list_domain_events_for_turn(turn_b).await.unwrap();
    assert_eq!(for_b.len(), 1, "turn_b 有 1 条");
    assert_eq!(for_b[0].kind, DomainEventKind::SceneTransitioned);

    // 全会话现 3 条。
    assert_eq!(db.list_domain_events(SESSION, 100).await.unwrap().len(), 3);

    sqlx::query("delete from domain_events where session_id=$1")
        .bind(SESSION)
        .execute(&db.pool)
        .await
        .unwrap();
}
