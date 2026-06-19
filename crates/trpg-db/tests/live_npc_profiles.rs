//! NPC Profile Store v1 live 验收（TC-D3-01）：durable NpcProfile store + load，供后续
//! NPC mind/behavior production wiring 复用，而非只在测试里构造瞬时结构体。覆盖：
//!   - live_npc_profile_roundtrip               upsert→load 往返，完整 profile（含 GM-only
//!                                              secret）保真，重写幂等（单行）
//!   - live_npc_profile_rejects_unstable_actor_id  展示名形态 actor id 拒写，库中无行
//!   - loaded_npc_profile_safe_view_hides_secret   读回后 safe_view 不含 GM-only secret 文本
//!   - loaded_npc_profile_rejects_mismatched_actor_id  行 actor_id 与 profile_json 内 actor_id
//!                                              不一致时拒读（fail-closed，不返回身份漂移的 profile）
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!      cargo test -p trpg-db --test live_npc_profiles -- --nocapture
//! 无 DATABASE_URL 时 SKIP（fail-closed，不卡 CI）。
use trpg_db::Db;
use trpg_model::{NpcProfile, NpcSecret, SpeechStyle, Visibility};

const GM_ONLY_SENTINEL: &str = "GMONLY_SENTINEL_betrayer";

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
    sqlx::query("delete from npc_profiles where session_id=$1")
        .bind(session)
        .execute(&db.pool)
        .await
        .unwrap();
}

/// A full profile with persona fields and a GM-only secret, anchored to `actor_id`.
fn sample_profile(actor_id: &str) -> NpcProfile {
    NpcProfile {
        actor_id: actor_id.into(),
        name: "拉斯".into(),
        role: Some("加油站老板".into()),
        archetype: Some("taciturn_veteran".into()),
        personality_traits: vec!["寡言".into(), "warY".into()],
        values: vec!["守护小镇".into()],
        drives: vec!["保护女儿".into()],
        fears: vec!["旧日真相败露".into()],
        goals: vec!["撑过这一季".into()],
        secrets: vec![NpcSecret {
            secret_id: "sec".into(),
            content: GM_ONLY_SENTINEL.into(),
            visibility: Visibility::GmOnly,
            source_refs: vec![],
        }],
        speech_style: SpeechStyle {
            humor: Some("dry".into()),
            ..Default::default()
        },
        behavioral_boundaries: vec!["从不主动提起战争".into()],
        ..Default::default()
    }
}

#[tokio::test]
async fn live_npc_profile_roundtrip() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_prof_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    let profile = sample_profile("npc_lars");
    db.upsert_npc_profile(&session, &profile).await.unwrap();

    let loaded = db
        .load_npc_profile(&session, "npc_lars")
        .await
        .unwrap()
        .expect("profile persisted");
    assert_eq!(
        loaded, profile,
        "load must reproduce the stored profile exactly"
    );
    // Durable storage keeps the complete profile, including GM-only secret content.
    assert_eq!(loaded.secrets.len(), 1);
    assert_eq!(loaded.secrets[0].content, GM_ONLY_SENTINEL);

    // Re-upsert (mutated persona) keeps a single row and reflects the update.
    let mut updated = profile.clone();
    updated.role = Some("退役警长".into());
    db.upsert_npc_profile(&session, &updated).await.unwrap();
    let reloaded = db
        .load_npc_profile(&session, "npc_lars")
        .await
        .unwrap()
        .expect("profile persisted");
    assert_eq!(reloaded.role.as_deref(), Some("退役警长"));
    let count: i64 = sqlx::query_scalar(
        "select count(*) from npc_profiles where session_id=$1 and actor_id='npc_lars'",
    )
    .bind(&session)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        count, 1,
        "upsert keeps a single row per (session, actor_id)"
    );

    // load via a raw id with surrounding whitespace normalizes to the same row.
    let loaded_ws = db
        .load_npc_profile(&session, "  npc_lars  ")
        .await
        .unwrap()
        .expect("whitespace-padded actor id normalizes to same row");
    assert_eq!(loaded_ws.actor_id, "npc_lars");

    purge(&db, &session).await;
}

#[tokio::test]
async fn live_npc_profile_rejects_unstable_actor_id() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_prof_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    // A display-name-shaped actor id is refused before any write.
    let mut profile = sample_profile("npc_lars");
    profile.actor_id = "The Butler".into();
    let res = db.upsert_npc_profile(&session, &profile).await;
    assert!(res.is_err(), "unstable actor id must fail closed");

    let count: i64 = sqlx::query_scalar("select count(*) from npc_profiles where session_id=$1")
        .bind(&session)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "no row written for an unstable actor id");

    // load with an unstable actor id likewise fails closed (no silent None).
    let load_res = db.load_npc_profile(&session, "The Butler").await;
    assert!(
        load_res.is_err(),
        "load with unstable actor id must fail closed"
    );

    purge(&db, &session).await;
}

#[tokio::test]
async fn loaded_npc_profile_safe_view_hides_secret() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_prof_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    let profile = sample_profile("npc_lars");
    db.upsert_npc_profile(&session, &profile).await.unwrap();

    let loaded = db
        .load_npc_profile(&session, "npc_lars")
        .await
        .unwrap()
        .expect("profile persisted");

    // GM-only secret is present in the durable profile but absent from the safe view.
    assert!(
        loaded.secrets.iter().any(|s| s.content == GM_ONLY_SENTINEL),
        "durable profile retains GM-only secret"
    );
    let safe = loaded.safe_view();
    assert!(
        safe.known_secrets.is_empty(),
        "safe view must not surface GM-only secret content"
    );
    let safe_json = serde_json::to_string(&safe).unwrap();
    assert!(
        !safe_json.contains(GM_ONLY_SENTINEL),
        "GM-only secret leaked into safe-view serialization"
    );

    purge(&db, &session).await;
}

#[tokio::test]
async fn loaded_npc_profile_rejects_mismatched_actor_id() {
    // The card explicitly calls out mismatched profile actor ids: a row stored under
    // one stable actor_id whose profile_json carries a different actor_id must fail
    // closed on load rather than returning an identity-drifted profile.
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_prof_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    // Write a row directly so the row's actor_id ("npc_lars") and the embedded
    // profile_json.actor_id ("npc_someone_else") disagree.
    let mut inner = sample_profile("npc_someone_else");
    inner.name = "拉斯".into();
    let profile_json = serde_json::to_value(&inner).unwrap();
    sqlx::query(
        "insert into npc_profiles (session_id, actor_id, name, role, profile_json) \
         values ($1,$2,$3,$4,$5)",
    )
    .bind(&session)
    .bind("npc_lars")
    .bind("拉斯")
    .bind(Option::<&str>::None)
    .bind(&profile_json)
    .execute(&db.pool)
    .await
    .unwrap();

    let res = db.load_npc_profile(&session, "npc_lars").await;
    assert!(
        res.is_err(),
        "mismatched stored profile actor_id must fail closed on load"
    );

    purge(&db, &session).await;
}
