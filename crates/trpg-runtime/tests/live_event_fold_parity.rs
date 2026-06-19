//! P6.5 replay parity（LIVE）：把事件 seed 进真库（append_domain_event），再断言
//! 运行时事件折叠投影（trpg_runtime::event_fold）与 db 只读视图**集合相等**——
//!   PlayerExposureProjection::fold ↔ Db::list_surfaced_entities
//!   ContextSurfacedProjection::fold ↔ Db::list_context_surfaced_entities
//! 这把 FRAMEWORK §1 的「Projection 是事件折叠出的视图③，不是第二事实源」钉成可观测门。
//! 放在 trpg-runtime（已依赖 trpg-db）⇒ 零新 dep 边、无 crate cycle。
//!
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!      cargo test -p trpg-runtime --test live_event_fold_parity -- --nocapture
//! 无 DATABASE_URL 时 SKIP（fail-closed，不卡 CI）。
use chrono::{DateTime, Utc};
use serde_json::json;
use std::collections::BTreeSet;
use trpg_db::Db;
use trpg_model::{DomainEvent, DomainEventKind};
use trpg_runtime::event_fold::{ContextSurfacedProjection, PlayerExposureProjection, Projection};

const SESSION: &str = "sess_event_fold_parity";

async fn reset_session(db: &Db) {
    sqlx::query("delete from domain_events where session_id=$1")
        .bind(SESSION)
        .execute(&db.pool)
        .await
        .unwrap();
}

fn ev(id: &str, kind: DomainEventKind, entity_id: &str, entity_kind: &str) -> DomainEvent {
    DomainEvent {
        event_id: id.into(),
        session_id: SESSION.into(),
        turn_id: "turn1".into(),
        kind,
        data: json!({"entity_id": entity_id, "entity_kind": entity_kind}),
        source_refs: Vec::new(),
        created_at: DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
    }
}

#[tokio::test]
async fn fold_matches_db_views() {
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
    if let Err(e) = db.migrate().await {
        eprintln!("SKIP: migrate failed: {e}");
        return;
    }
    reset_session(&db).await;

    // 玩家暴露：PlayerExposed + 遗留 EntitySurfaced；ContextSurfaced 是噪声（不进暴露集）。
    let events = [
        ev(
            "de_ef_exp_npc_b",
            DomainEventKind::PlayerExposed,
            "npc_b",
            "npc",
        ),
        ev(
            "de_ef_ent_sc_a",
            DomainEventKind::EntitySurfaced,
            "sc_a",
            "scene",
        ),
        ev(
            "de_ef_ctx_hidden",
            DomainEventKind::ContextSurfaced,
            "npc_hidden",
            "npc",
        ),
        // 重复投递（同 event_id）：append on-conflict no-op；fold 也去重幂等。
        ev(
            "de_ef_exp_npc_b",
            DomainEventKind::PlayerExposed,
            "npc_b",
            "npc",
        ),
    ];
    for e in &events {
        db.append_domain_event(e).await.unwrap();
    }

    // ① 玩家暴露 parity：fold(list_domain_events) == list_surfaced_entities（集合相等）。
    let logged = db.list_domain_events(SESSION, 1000).await.unwrap();
    let folded: BTreeSet<(String, String)> = PlayerExposureProjection::fold(&logged).entities;
    let db_view: BTreeSet<(String, String)> = db
        .list_surfaced_entities(SESSION)
        .await
        .unwrap()
        .into_iter()
        .collect();
    assert_eq!(
        folded, db_view,
        "PlayerExposureProjection::fold 必与 list_surfaced_entities 集合相等"
    );
    assert!(
        !folded.iter().any(|(id, _)| id == "npc_hidden"),
        "ContextSurfaced(npc_hidden) 绝不进玩家暴露集"
    );

    // ② context parity：fold == list_context_surfaced_entities。
    let folded_ctx: BTreeSet<(String, String)> = ContextSurfacedProjection::fold(&logged).entities;
    let db_ctx: BTreeSet<(String, String)> = db
        .list_context_surfaced_entities(SESSION)
        .await
        .unwrap()
        .into_iter()
        .collect();
    assert_eq!(
        folded_ctx, db_ctx,
        "ContextSurfacedProjection::fold 必与 list_context_surfaced_entities 集合相等"
    );
    assert_eq!(
        folded_ctx,
        BTreeSet::from([("npc_hidden".to_string(), "npc".to_string())]),
        "context 视图只含 npc_hidden"
    );

    println!("PARITY OK: exposure={folded:?} context={folded_ctx:?}");
    reset_session(&db).await;
}
