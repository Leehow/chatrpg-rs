//! TruthGraph T1（优化2 #5）：list_surfaced_entities distinct 投影。
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!      cargo test -p trpg-db --test live_surfaced_entities -- --nocapture
//! 无 DATABASE_URL 时 SKIP（fail-closed，不卡 CI）。
use chrono::{DateTime, Utc};
use serde_json::json;
use trpg_db::Db;
use trpg_model::{DomainEvent, DomainEventKind};

const SESSION: &str = "sess_truthgraph_t1_surfaced";

/// 0030 就地自施（幂等 create table / index if not exists）。
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

fn surfaced(event_id: &str, entity_id: &str, kind: &str) -> DomainEvent {
    DomainEvent {
        event_id: event_id.into(),
        session_id: SESSION.into(),
        turn_id: "turn1".into(),
        kind: DomainEventKind::EntitySurfaced,
        data: json!({ "entity_id": entity_id, "entity_kind": kind, "scene_id": "sc01" }),
        source_refs: Vec::new(),
        created_at: DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
    }
}

#[tokio::test]
async fn list_surfaced_entities_returns_distinct_pairs() {
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

    sqlx::query("delete from domain_events where session_id=$1")
        .bind(SESSION)
        .execute(&db.pool)
        .await
        .unwrap();

    // 空会话 → 空投影。
    assert!(
        db.list_surfaced_entities(SESSION).await.unwrap().is_empty(),
        "空会话投影返空"
    );

    // append 2 个 EntitySurfaced（1 clue + 1 npc）。
    db.append_domain_event(&surfaced(
        "de_surfaced_sess_truthgraph_t1_surfaced_clue_letter",
        "clue_letter",
        "clue",
    ))
    .await
    .unwrap();
    db.append_domain_event(&surfaced(
        "de_surfaced_sess_truthgraph_t1_surfaced_npc_ras",
        "npc_ras",
        "npc",
    ))
    .await
    .unwrap();

    // 同 event_id 重放（再 surface）→ 幂等 no-op。
    db.append_domain_event(&surfaced(
        "de_surfaced_sess_truthgraph_t1_surfaced_clue_letter",
        "clue_letter",
        "clue",
    ))
    .await
    .unwrap();

    // 噪声：另一种 kind 不该混进投影。
    db.append_domain_event(&DomainEvent {
        event_id: "de_noise_TurnStarted".into(),
        session_id: SESSION.into(),
        turn_id: "turn1".into(),
        kind: DomainEventKind::TurnStarted,
        data: json!({ "entity_id": "should_not_appear" }),
        source_refs: Vec::new(),
        created_at: DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
    })
    .await
    .unwrap();

    let mut got = db.list_surfaced_entities(SESSION).await.unwrap();
    got.sort();
    assert_eq!(
        got,
        vec![
            ("clue_letter".to_string(), "clue".to_string()),
            ("npc_ras".to_string(), "npc".to_string())
        ],
        "distinct (entity_id, entity_kind)，幂等不重复、非 EntitySurfaced 不混入"
    );

    sqlx::query("delete from domain_events where session_id=$1")
        .bind(SESSION)
        .execute(&db.pool)
        .await
        .unwrap();
}
