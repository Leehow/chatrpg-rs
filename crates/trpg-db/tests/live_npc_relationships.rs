//! NPC Relationship v1 live 验收（TC-NPC-02）：durable 结构化态度状态，store + update，
//! 不依赖 free-form memory summary。覆盖：
//!   - npc_relationship_roundtrip          upsert→load 往返，全通道与派生态保真
//!   - help_then_threat_store_and_update    load-default+apply+store 累积：help 升 trust/debt，
//!                                          threat 降 trust 升 fear；证据集累加去重
//!   - relationship_bounds_clamped_in_store 巨量 delta 经 apply 落库仍在边界内
//!   - unstable_target_id_fails_closed_at_write_boundary  serde 构造的非法 target id 写边界拒写
//!   - unstable_npc_id_fails_closed_no_row  非稳定 NPC id 拒写，库中无行
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!      cargo test -p trpg-db --test live_npc_relationships -- --nocapture
//! 无 DATABASE_URL 时 SKIP（fail-closed，不卡 CI）。
use trpg_db::Db;
use trpg_model::{NpcRelationship, NpcRelationshipDelta, NpcRelationshipTarget};

/// Durable load-or-default + bounded/evidence-gated apply + persist, expressed against
/// the public DB + model API only (the trpg-runtime wrapper mirrors this exact path but
/// lives above trpg-db; reproducing it here keeps this crate's tests dependency-clean).
async fn apply_delta(
    db: &Db,
    session_id: &str,
    npc_id: &str,
    target: NpcRelationshipTarget,
    delta: &NpcRelationshipDelta,
) -> anyhow::Result<NpcRelationship> {
    let current = db
        .load_npc_relationship(session_id, npc_id, target.kind_token(), target.target_id())
        .await?;
    let mut rel = match current {
        Some(rel) => rel,
        None => {
            NpcRelationship::new(session_id, npc_id, target).map_err(|e| anyhow::anyhow!("{e}"))?
        }
    };
    rel.apply_delta(delta).map_err(|e| anyhow::anyhow!("{e}"))?;
    db.upsert_npc_relationship(&rel).await?;
    Ok(rel)
}

async fn connect_or_skip() -> Option<Db> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP: DATABASE_URL unset");
            return None;
        }
    };
    let db = match Db::connect(&url).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("SKIP: connect failed: {e}");
            return None;
        }
    };
    db.migrate()
        .await
        .expect("migrate failed after successful connect");
    Some(db)
}

async fn purge(db: &Db, session: &str) {
    sqlx::query("delete from npc_relationships where session_id=$1")
        .bind(session)
        .execute(&db.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn npc_relationship_roundtrip() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_rel_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    // First interaction toward the player party.
    let rel = apply_delta(
        &db,
        &session,
        "npc_lars",
        NpcRelationshipTarget::PlayerParty,
        &NpcRelationshipDelta::help(vec!["evt_helped".into()]),
    )
    .await
    .unwrap();

    let loaded = db
        .load_npc_relationship(&session, "npc_lars", "player_party", "")
        .await
        .unwrap()
        .expect("relationship persisted");
    assert_eq!(
        loaded, rel,
        "load must reproduce the stored relationship exactly"
    );
    assert!(loaded.trust > 0 && loaded.debt > 0);
    assert_eq!(loaded.evidence_event_ids, vec!["evt_helped".to_string()]);

    purge(&db, &session).await;
}

#[tokio::test]
async fn help_then_threat_store_and_update() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_rel_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    let after_help = apply_delta(
        &db,
        &session,
        "npc_lars",
        NpcRelationshipTarget::PlayerParty,
        &NpcRelationshipDelta::help(vec!["evt_help".into()]),
    )
    .await
    .unwrap();
    let trust_after_help = after_help.trust;

    let after_threat = apply_delta(
        &db,
        &session,
        "npc_lars",
        NpcRelationshipTarget::PlayerParty,
        &NpcRelationshipDelta::threat(vec!["evt_threat".into()]),
    )
    .await
    .unwrap();

    assert!(after_threat.trust < trust_after_help, "threat lowers trust");
    assert!(after_threat.fear > 0, "threat raises fear");
    assert_eq!(
        after_threat.evidence_event_ids,
        vec!["evt_help".to_string(), "evt_threat".to_string()],
        "evidence accumulates across updates"
    );
    // Exactly one durable row (update, not insert).
    let count: i64 = sqlx::query_scalar(
        "select count(*) from npc_relationships where session_id=$1 and npc_id='npc_lars'",
    )
    .bind(&session)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "store+update keeps a single row per (npc,target)");

    purge(&db, &session).await;
}

#[tokio::test]
async fn relationship_bounds_clamped_in_store() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_rel_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    let rel = apply_delta(
        &db,
        &session,
        "npc_lars",
        NpcRelationshipTarget::Npc("npc_other".into()),
        &NpcRelationshipDelta {
            trust: 9_000,
            fear: 9_000,
            evidence_event_ids: vec!["evt".into()],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(rel.trust, 100);
    assert_eq!(rel.fear, 100);

    let loaded = db
        .load_npc_relationship(&session, "npc_lars", "npc", "npc_other")
        .await
        .unwrap()
        .expect("persisted");
    assert_eq!(loaded.trust, 100);
    assert_eq!(loaded.fear, 100);

    purge(&db, &session).await;
}

#[tokio::test]
async fn unstable_target_id_fails_closed_at_write_boundary() {
    // REV1 regression: NpcRelationship fields/variants are public and serde can build
    // one, so a manually constructed relationship with an invalid TARGET id must be
    // rejected by upsert_npc_relationship and write no row.
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_rel_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    // Bypass NpcRelationship::new (which now validates the target) via a serde value so
    // the invalid target reaches the DB write boundary directly.
    let raw = serde_json::json!({
        "session_id": session,
        "npc_id": "npc_lars",
        "target_kind": "npc",
        "target_id": "Goblin #2",
        "trust": 0, "respect": 0, "affection": 0, "debt": 0,
        "fear": 0, "suspicion": 0, "hostility": 0, "leverage": 0, "talkativeness": 50,
        "interaction_desire": 0, "stance": "neutral",
        "evidence_event_ids": ["evt"]
    });
    let rel: NpcRelationship =
        serde_json::from_value(raw).expect("deserialize into a public-field struct");

    let res = db.upsert_npc_relationship(&rel).await;
    assert!(
        res.is_err(),
        "invalid target id must fail closed at write boundary"
    );

    let count: i64 =
        sqlx::query_scalar("select count(*) from npc_relationships where session_id=$1")
            .bind(&session)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(count, 0, "no row written for an invalid target id");

    purge(&db, &session).await;
}

#[tokio::test]
async fn unstable_npc_id_fails_closed_no_row() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_rel_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    // A display-name-shaped NPC id is refused before any write.
    let res = apply_delta(
        &db,
        &session,
        "The Butler",
        NpcRelationshipTarget::PlayerParty,
        &NpcRelationshipDelta::help(vec!["evt".into()]),
    )
    .await;
    assert!(res.is_err(), "unstable NPC id must fail closed");

    let count: i64 =
        sqlx::query_scalar("select count(*) from npc_relationships where session_id=$1")
            .bind(&session)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(count, 0, "no row written for an unstable id");

    purge(&db, &session).await;
}
