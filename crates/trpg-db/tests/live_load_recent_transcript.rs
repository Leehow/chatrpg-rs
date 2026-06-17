//! P1-2: load_recent_transcript —— 从 DB 拼接会话最近 N 回合的对白成 transcript。
//! 这是 API ServerRecent 历史策略的数据源（前端只传 user_input 时第二回合不再丢上下文）。
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!      cargo test -p trpg-db --test live_load_recent_transcript -- --nocapture
//! 无 DATABASE_URL 时 SKIP（fail-closed，不卡 CI）。
use serde_json::json;
use trpg_db::Db;

// 各 test 用独立 session，避免并行跑互相踩 turns 行。
const SESSION_TWO_TURNS: &str = "sess_p12_recent_two_turns";
const SESSION_EMPTY: &str = "sess_p12_recent_empty";
const SESSION_LIMIT: &str = "sess_p12_recent_limit";

async fn connect() -> Option<Db> {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return None; } };
    match Db::connect(&url).await { Ok(d) => Some(d), Err(e) => { eprintln!("SKIP: connect: {e}"); None } }
}

async fn seed_session(db: &Db, session: &str) {
    // turns.session_id 外键 → sessions(session_id)；先建会话（幂等：已存在则忽略）。
    let _ = db.create_session(session, "call_of_cthulhu_7e", None).await;
    sqlx::query("delete from turns where session_id=$1").bind(session).execute(&db.pool).await.unwrap();
}

#[tokio::test]
async fn recent_transcript_contains_prior_turn_content_in_chronological_order() {
    let db = match connect().await { Some(d) => d, None => return };
    seed_session(&db, SESSION_TWO_TURNS).await;

    // 第一回合：玩家推门、GM 演浓雾。
    db.save_turn(SESSION_TWO_TURNS, "turn_p12_a", "我推开门", "门后是浓雾", json!({}), "ready").await.unwrap();
    // 第二回合：玩家点灯、GM 演火光。
    db.save_turn(SESSION_TWO_TURNS, "turn_p12_b", "我点亮提灯", "火光照出走廊", json!({}), "ready").await.unwrap();

    let transcript = db.load_recent_transcript(SESSION_TWO_TURNS, 12).await.unwrap()
        .expect("两回合后必有 transcript");

    // 第二回合提交（仅 user_input）时，载入的历史必含第一回合双方内容。
    assert!(transcript.contains("我推开门"), "transcript 必含第一回合玩家输入: {transcript}");
    assert!(transcript.contains("门后是浓雾"), "transcript 必含第一回合 GM 叙事: {transcript}");
    assert!(transcript.contains("我点亮提灯") && transcript.contains("火光照出走廊"), "transcript 必含第二回合: {transcript}");

    // 时间顺序：第一回合在第二回合之前（chronological，非倒序）。
    let pos_first = transcript.find("我推开门").unwrap();
    let pos_second = transcript.find("我点亮提灯").unwrap();
    assert!(pos_first < pos_second, "transcript 必按时间顺序（旧→新）: {transcript}");

    sqlx::query("delete from turns where session_id=$1").bind(SESSION_TWO_TURNS).execute(&db.pool).await.unwrap();
}

#[tokio::test]
async fn empty_session_yields_none() {
    let db = match connect().await { Some(d) => d, None => return };
    seed_session(&db, SESSION_EMPTY).await;
    // 无任何回合（新会话）→ None，调用方据此不注入空 transcript 块。
    assert_eq!(db.load_recent_transcript(SESSION_EMPTY, 12).await.unwrap(), None);
}

#[tokio::test]
async fn respects_turn_limit_keeping_most_recent() {
    let db = match connect().await { Some(d) => d, None => return };
    seed_session(&db, SESSION_LIMIT).await;
    // 写 3 回合，limit=2 → 只保留最近 2（第 1 回合被裁掉）。
    db.save_turn(SESSION_LIMIT, "turn_p12_l1", "最旧输入", "最旧叙事", json!({}), "ready").await.unwrap();
    db.save_turn(SESSION_LIMIT, "turn_p12_l2", "中间输入", "中间叙事", json!({}), "ready").await.unwrap();
    db.save_turn(SESSION_LIMIT, "turn_p12_l3", "最新输入", "最新叙事", json!({}), "ready").await.unwrap();

    let transcript = db.load_recent_transcript(SESSION_LIMIT, 2).await.unwrap().expect("必有 transcript");
    assert!(!transcript.contains("最旧输入"), "limit=2 必裁掉最旧回合: {transcript}");
    assert!(transcript.contains("中间输入") && transcript.contains("最新输入"), "必保留最近 2 回合: {transcript}");

    sqlx::query("delete from turns where session_id=$1").bind(SESSION_LIMIT).execute(&db.pool).await.unwrap();
}
