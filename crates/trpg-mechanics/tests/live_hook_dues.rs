//! B5 测试 5：open due 抑制重发 + turn waive 不抑制 + scene waive 抑制。
//! 对标 trpg-db/tests/live_mechanic_dues.rs 真库样板（无 DATABASE_URL 即 SKIP）。
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
//!      cargo test -p trpg-mechanics --test live_hook_dues -- --nocapture
use serde_json::json;
use trpg_db::Db;
use trpg_mechanics::watcher::HookEvent;
use trpg_mechanics::RefereeCombatService;
use trpg_model::RuleKernel;

const SESSION: &str = "sess_b5_hook_dues_test";
const RULESET: &str = "rs_b5_hook_dues_test";

/// 0027 就地自施（幂等 create if not exists）——不跑整条迁移链（链上有
/// 非幂等老迁移），与 live db 实际状态解耦（B4 同款）。
async fn ensure_table(db: &Db) {
    for stmt in include_str!("../../../migrations/0027_mechanic_dues_v120.sql").split(';') {
        let s = stmt.trim();
        if s.is_empty() {
            continue;
        }
        sqlx::query(s)
            .execute(&db.pool)
            .await
            .expect("0027 statement must apply");
    }
}

/// 最小 kernel：mechanics_catalog 单条 TurnStart 钩子条目（throwaway ruleset id，
/// fixture 词汇不属于逻辑硬编码）。
fn test_kernel() -> RuleKernel {
    serde_json::from_value(json!({
        "kernel_id": "kernel_b5_hook_test",
        "ruleset_id": RULESET,
        "version": "b5-test",
        "mechanics_catalog": [{
            "id": "test.upkeep",
            "name": "Upkeep",
            "when_to_use": "at the start of every turn",
            "hooks": [{"event": "turn_start"}]
        }]
    }))
    .expect("minimal kernel json must deserialize")
}

async fn cleanup(db: &Db) {
    sqlx::query("delete from mechanic_dues where session_id=$1")
        .bind(SESSION)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("delete from rule_kernels where ruleset_id=$1")
        .bind(RULESET)
        .execute(&db.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn open_due_suppresses_refire_and_turn_waive_does_not() {
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
    ensure_table(&db).await;
    cleanup(&db).await;
    db.upsert_rule_kernel(&test_kernel())
        .await
        .expect("kernel upsert");
    let svc = RefereeCombatService::new(db.clone());

    // 首发：TurnStart 钩子条目 → 1 条 due 落库并返回。
    let first = svc
        .dues_for_hook(SESSION, "turn_1", RULESET, &HookEvent::TurnStart)
        .await
        .unwrap();
    assert_eq!(first.len(), 1, "first fire must produce one due: {first:?}");
    assert_eq!(first[0].mechanic_id.as_deref(), Some("test.upkeep"));
    assert_eq!(first[0].hook_event.as_deref(), Some("turn_start"));

    // 抑制规则 ①：同 (mechanic_id, hook_event) 已有 open due → 不再产。
    let second = svc
        .dues_for_hook(SESSION, "turn_2", RULESET, &HookEvent::TurnStart)
        .await
        .unwrap();
    assert!(
        second.is_empty(),
        "open due must suppress refire: {second:?}"
    );

    // waive(scope=turn) 后 → 再产（turn 豁免下回合仍提醒，spec §5.3）。
    db.update_mechanic_due_status(&first[0].due_id, "waived", Some("test"), Some("turn"))
        .await
        .unwrap();
    let third = svc
        .dues_for_hook(SESSION, "turn_3", RULESET, &HookEvent::TurnStart)
        .await
        .unwrap();
    assert_eq!(
        third.len(),
        1,
        "turn-scoped waiver must NOT suppress: {third:?}"
    );

    // waive(scope=scene) 后 → 不产（场景内豁免有效）。
    db.update_mechanic_due_status(&third[0].due_id, "waived", Some("test"), Some("scene"))
        .await
        .unwrap();
    let fourth = svc
        .dues_for_hook(SESSION, "turn_4", RULESET, &HookEvent::TurnStart)
        .await
        .unwrap();
    assert!(
        fourth.is_empty(),
        "scene-scoped waiver must suppress: {fourth:?}"
    );

    cleanup(&db).await;
}
