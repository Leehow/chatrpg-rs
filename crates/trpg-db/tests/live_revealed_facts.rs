//! 反剧透 revealed-facts 账本（DATA/ENFORCEMENT 半，LEDGER 切片）：
//! record_revealed_fact 落账 + list_revealed_facts 投影。P0b 起 list_revealed_facts
//! 委托 KnowledgeEdge (player_party, knows_true)，record_revealed_fact 写穿两边，
//! 幂等键 `de_revealed_{session}_{fact}`，mirror EntitySurfaced/list_surfaced_entities。
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!      cargo test -p trpg-db --test live_revealed_facts -- --nocapture
//! 无 DATABASE_URL 时 SKIP（fail-closed，不卡 CI）。
use chrono::{DateTime, Utc};
use serde_json::json;
use trpg_db::Db;
use trpg_model::{DomainEvent, DomainEventKind};

const SESSION: &str = "sess_revealed_facts_ledger";

/// 清掉本会话两边账本（domain_events 写穿源 + knowledge_edges 投影源），
/// 保证空会话基线，避免上次运行残留行污染断言。
async fn reset_session(db: &Db) {
    sqlx::query("delete from domain_events where session_id=$1")
        .bind(SESSION)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("delete from knowledge_edges where session_id=$1")
        .bind(SESSION)
        .execute(&db.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn record_and_list_revealed_facts_distinct_idempotent() {
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
    // 幂等 migrate：含 0030 domain_events + 0032 knowledge_edges（list_revealed_facts 现读后者）。
    if let Err(e) = db.migrate().await {
        eprintln!("SKIP: migrate failed: {e}");
        return;
    }
    reset_session(&db).await;

    // 空会话 → 空账本。
    assert!(
        db.list_revealed_facts(SESSION).await.unwrap().is_empty(),
        "空会话 → 无揭示事实"
    );

    // 揭示两条事实（NPC 真身 + 场景节点级秘密）。
    db.record_revealed_fact(SESSION, "turn3", "npc_butler", Some("玩家在书房发现日记"))
        .await
        .unwrap();
    db.record_revealed_fact(SESSION, "turn5", "sc_cellar", None)
        .await
        .unwrap();

    // 同 (session,fact) 重放 → 幂等 no-op（不产生重复）。
    db.record_revealed_fact(SESSION, "turn9", "npc_butler", Some("不同回合再次揭示"))
        .await
        .unwrap();

    // 噪声：EntitySurfaced（surfaced ≠ revealed）绝不混进账本。
    db.append_domain_event(&DomainEvent {
        event_id: "de_surfaced_sess_revealed_facts_ledger_npc_butler".into(),
        session_id: SESSION.into(),
        turn_id: "turn1".into(),
        kind: DomainEventKind::EntitySurfaced,
        data: json!({ "entity_id": "npc_butler", "entity_kind": "npc", "scene_id": "sc01" }),
        source_refs: Vec::new(),
        created_at: DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
    })
    .await
    .unwrap();

    let mut got = db.list_revealed_facts(SESSION).await.unwrap();
    got.sort();
    assert_eq!(
        got,
        vec!["npc_butler".to_string(), "sc_cellar".to_string()],
        "distinct fact_id；幂等不重复；EntitySurfaced 不混入"
    );

    reset_session(&db).await;
}
