//! StoryState 持久化 live 验收（P5.5 layered-runtime）。覆盖：
//!   - story_state_absent_is_none        新会话无行 ⇒ load 返回 None（fail-soft）
//!   - story_state_roundtrips            upsert 后 load 往返同一 StoryState
//!   - story_state_replay_is_idempotent  同 packet 重放 ⇒ 同一单行（on conflict(session_id)）
//!   - story_state_corrupt_blob_is_none  损坏 state_json ⇒ load 不 panic，降级 None
//! migration 0040 在 connect_or_skip 的 migrate() 里跑（与既有 live 测试同风格）。
//! story_state.session_id FK → sessions(session_id)：测试先 create_session，末尾连同 purge。
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!      cargo test -p trpg-db --test live_story_state -- --nocapture
//! 无 DATABASE_URL 时 SKIP（fail-closed，不卡 CI）。
use trpg_db::Db;
use trpg_model::{
    InterestSignal, PlayerInterestSignal, StoryState, StoryThread, StoryThreadStatus,
};

/// 连接 + migrate（含 0040）；无 DATABASE_URL 或连不上即 None（调用方 SKIP）。
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
    // DATABASE_URL 已设且连接成功后，migrate 失败必须 FAIL，绝不伪装成 SKIP。
    db.migrate()
        .await
        .expect("migrate failed after successful connect");
    Some(db)
}

/// story_state.session_id FK → sessions(session_id)：先建 session 行才能 upsert story_state。
async fn prepare(db: &Db, session: &str) {
    db.create_session(session, "call_of_cthulhu_7e", None)
        .await
        .expect("create_session failed");
}

async fn purge(db: &Db, session: &str) {
    for sql in [
        "delete from story_state where session_id=$1",
        "delete from sessions where session_id=$1",
    ] {
        sqlx::query(sql)
            .bind(session)
            .execute(&db.pool)
            .await
            .unwrap();
    }
}

fn sample_story() -> StoryState {
    StoryState {
        active_threads: vec![StoryThread {
            thread_id: "thr_main".into(),
            premise: "the relic must reach the temple".into(),
            status: StoryThreadStatus::Active,
            urgency: 0.7,
            ..Default::default()
        }],
        player_interests: vec![
            PlayerInterestSignal {
                thread_id: "thr_main".into(),
                signal: InterestSignal::Engaged,
                rejected: false,
                strength: 0.8,
                ..Default::default()
            },
            PlayerInterestSignal {
                thread_id: "thr_romance".into(),
                signal: InterestSignal::Avoidant,
                rejected: true,
                strength: 0.2,
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

/// 验收：新会话无 story_state 行 ⇒ load_story_state 返回 None（fail-soft，行缺失不报错）。
#[tokio::test]
async fn story_state_absent_is_none() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_story_absent_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;
    prepare(&db, &session).await;

    let got = db.load_story_state(&session).await.unwrap();
    assert!(got.is_none(), "fresh session ⇒ no story_state row ⇒ None");

    purge(&db, &session).await;
}

/// 验收：upsert_story_state 后 load_story_state 往返同一 StoryState（jsonb roundtrip）。
#[tokio::test]
async fn story_state_roundtrips() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_story_rt_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;
    prepare(&db, &session).await;

    let story = sample_story();
    db.upsert_story_state(&session, &story, "t1").await.unwrap();

    let got = db
        .load_story_state(&session)
        .await
        .unwrap()
        .expect("after upsert, load must return Some");
    assert_eq!(got, story, "StoryState round-trips through story_state jsonb");
    // rejected 标记确有持久化（§24-#13 selector 读得到）。
    let rejected: Vec<&str> = got
        .player_interests
        .iter()
        .filter(|s| s.rejected)
        .map(|s| s.thread_id.as_str())
        .collect();
    assert_eq!(rejected, vec!["thr_romance"], "rejected 标记持久化往返");

    purge(&db, &session).await;
}

/// 验收：同一 packet 重放 ⇒ 同一单行（on conflict(session_id) do update），不堆积。
/// 末次写入态覆盖（updated_turn 与 state_json 都被 excluded 更新）。
#[tokio::test]
async fn story_state_replay_is_idempotent() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_story_replay_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;
    prepare(&db, &session).await;

    let story = sample_story();
    for turn in ["t1", "t1", "t2"] {
        db.upsert_story_state(&session, &story, turn).await.unwrap();
    }
    let rows: i64 =
        sqlx::query_scalar("select count(*) from story_state where session_id=$1")
            .bind(&session)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(rows, 1, "重放只一行（按 session_id 幂等 upsert）");
    let updated_turn: Option<String> =
        sqlx::query_scalar("select updated_turn from story_state where session_id=$1")
            .bind(&session)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        updated_turn.as_deref(),
        Some("t2"),
        "末次写入的 updated_turn 覆盖（provenance 更新）"
    );
    // 仍能 load 回完整 StoryState。
    assert_eq!(
        db.load_story_state(&session).await.unwrap().unwrap(),
        story,
        "幂等重放后 load 仍往返同一 StoryState"
    );

    purge(&db, &session).await;
}

/// 验收：损坏 / 不兼容的 state_json ⇒ load_story_state 不 panic，降级 None（fail-soft）。
/// 直接写一块非 StoryState 形状的 jsonb（数组而非对象）模拟存量损坏 / schema 漂移。
#[tokio::test]
async fn story_state_corrupt_blob_is_none() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_story_corrupt_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;
    prepare(&db, &session).await;

    // StoryState 反序列化期望一个对象；写入一个数组 ⇒ from_value 失败 ⇒ 降级 None。
    sqlx::query(
        "insert into story_state (session_id, state_json, updated_turn) \
         values ($1, '[1,2,3]'::jsonb, 't_bad')",
    )
    .bind(&session)
    .execute(&db.pool)
    .await
    .unwrap();

    let got = db.load_story_state(&session).await;
    let got = got.expect("load_story_state must NOT error on a corrupt blob");
    assert!(
        got.is_none(),
        "corrupt state_json ⇒ fail-soft None (Director falls back to empty story)"
    );

    purge(&db, &session).await;
}
