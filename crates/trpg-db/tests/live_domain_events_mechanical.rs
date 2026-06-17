//! eventlog 深化：DiceRolled / CheckResolved 机械事件 write-through。
//! insert_dice_roll / insert_check_result 主插成功后 fail-soft 追加 domain_event；
//! 幂等键 de_roll_{roll_id} / de_check_{check_id}（重放不增行）。
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!      cargo test -p trpg-db --test live_domain_events_mechanical -- --nocapture
//! 无 DATABASE_URL 时 SKIP（fail-closed，不卡 CI）。
use chrono::{DateTime, Utc};
use serde_json::json;
use trpg_db::Db;
use trpg_model::{ActorKind, CheckResultRecord, DiceRollRecord, DomainEventKind, RollVisibility};

const SESSION: &str = "sess_eventlog_mech_dice_check";
const TURN: &str = "turn_eventlog_mech";
const ROLL_ID: &str = "roll_eventlog_mech_1";
const CHECK_ID: &str = "check_eventlog_mech_1";

/// 0030 就地自施（幂等 create table / index if not exists）——主库表 dice_rolls /
/// check_results / domain_events 已由迁移链建好，这里只补 domain_events 防裸库。
async fn ensure_schema(db: &Db) {
    for stmt in include_str!("../../../migrations/0030_domain_events.sql").split(';') {
        let s = stmt.trim();
        if s.is_empty() { continue; }
        if let Err(e) = sqlx::query(s).execute(&db.pool).await {
            let msg = e.to_string();
            let dup = msg.contains("already exists") || msg.contains("23505") || msg.contains("42P07");
            assert!(dup, "0030 statement must apply: {e}");
        }
    }
}

fn fixed_ts() -> DateTime<Utc> { DateTime::<Utc>::from_timestamp(0, 0).unwrap() }

fn sample_roll() -> DiceRollRecord {
    DiceRollRecord {
        roll_id: ROLL_ID.into(),
        session_id: SESSION.into(),
        turn_id: TURN.into(),
        check_id: Some(CHECK_ID.into()),
        roller_kind: ActorKind::PlayerCharacter,
        roller_id: Some("actor.investigator".into()),
        visibility: RollVisibility::PublicGmRoll,
        expression: "1d100".into(),
        result: json!({"total": 42, "dice": [42]}),
        seed_commitment: "seed_commit_mech".into(),
        revealed_at: None,
        created_at: fixed_ts(),
    }
}

#[tokio::test]
async fn dice_roll_and_check_result_emit_domain_events_idempotent() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect: {e}"); return; } };
    ensure_schema(&db).await;

    // 清场：删 domain_events + 本测涉及的 dice_rolls / check_results 行（避免上一次残留）。
    sqlx::query("delete from domain_events where session_id=$1").bind(SESSION).execute(&db.pool).await.unwrap();
    sqlx::query("delete from dice_rolls where roll_id=$1").bind(ROLL_ID).execute(&db.pool).await.unwrap();
    sqlx::query("delete from check_results where check_id=$1").bind(CHECK_ID).execute(&db.pool).await.unwrap();

    // 1) insert_dice_roll → 主插成功 + write-through 一条 DiceRolled。
    let roll = sample_roll();
    db.insert_dice_roll(&roll).await.unwrap();

    // 2) insert_check_result → 主插成功 + write-through 一条 CheckResolved；
    //    session/turn 取自内嵌 result.roll。
    let check = CheckResultRecord {
        check_id: CHECK_ID.into(),
        roll: sample_roll(),
        outcome: json!({"success": true, "total": 42, "band": "regular_success"}),
        committed_patches: Vec::new(),
        created_at: fixed_ts(),
    };
    db.insert_check_result(&check).await.unwrap();

    // 按回合取，应含一条 DiceRolled(de_roll_*) + 一条 CheckResolved(de_check_*)。
    let evs = db.list_domain_events_for_turn(TURN).await.unwrap();
    let dice: Vec<_> = evs.iter().filter(|e| e.kind == DomainEventKind::DiceRolled).collect();
    let checks: Vec<_> = evs.iter().filter(|e| e.kind == DomainEventKind::CheckResolved).collect();
    assert_eq!(dice.len(), 1, "应有 1 条 DiceRolled");
    assert_eq!(checks.len(), 1, "应有 1 条 CheckResolved");

    let d = dice[0];
    assert_eq!(d.event_id, format!("de_roll_{ROLL_ID}"), "DiceRolled 幂等键");
    assert_eq!(d.session_id, SESSION);
    assert_eq!(d.turn_id, TURN);
    assert_eq!(d.data["check_id"], json!(CHECK_ID), "data 带 check_id");
    assert_eq!(d.data["expression"], json!("1d100"), "data 带 expression");
    assert_eq!(d.data["visibility"], json!("public_gm_roll"), "data 带 visibility token");

    let c = checks[0];
    assert_eq!(c.event_id, format!("de_check_{CHECK_ID}"), "CheckResolved 幂等键");
    assert_eq!(c.session_id, SESSION, "session 取自 result.roll");
    assert_eq!(c.turn_id, TURN, "turn 取自 result.roll");
    assert_eq!(c.data["check_id"], json!(CHECK_ID));
    assert_eq!(c.data["outcome"]["success"], json!(true), "outcome json 内嵌");

    // 3) 幂等：重放同 roll_id / check_id → domain_events 不新增（on conflict do nothing）。
    db.insert_dice_roll(&sample_roll()).await.unwrap();
    db.insert_check_result(&CheckResultRecord {
        check_id: CHECK_ID.into(),
        roll: sample_roll(),
        outcome: json!({"success": true}),
        committed_patches: Vec::new(),
        created_at: fixed_ts(),
    }).await.unwrap();
    let re = db.list_domain_events_for_turn(TURN).await.unwrap();
    let re_dice = re.iter().filter(|e| e.kind == DomainEventKind::DiceRolled).count();
    let re_checks = re.iter().filter(|e| e.kind == DomainEventKind::CheckResolved).count();
    assert_eq!(re_dice, 1, "重放同 roll_id 不得新增 DiceRolled（幂等）");
    assert_eq!(re_checks, 1, "重放同 check_id 不得新增 CheckResolved（幂等）");

    // 清场。
    sqlx::query("delete from domain_events where session_id=$1").bind(SESSION).execute(&db.pool).await.unwrap();
    sqlx::query("delete from dice_rolls where roll_id=$1").bind(ROLL_ID).execute(&db.pool).await.unwrap();
    sqlx::query("delete from check_results where check_id=$1").bind(CHECK_ID).execute(&db.pool).await.unwrap();
}
