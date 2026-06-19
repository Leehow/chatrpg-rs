//! DB-gated:验证 count_faces 骰池(Triangle 形态)对抗 check 经 contest 真出胜负——
//! 双方各掷自己的池、数 hits 多者胜,防御方骰子**真掷**(per-die 数组进结算)。
//! 对标 live_opposed.rs(roll_under)/ live_opposed_meet_or_beat.rs(meet_or_beat)的 pool 版。
//!
//! 自给自足:契约直接带具体 DicePoolCount target(对抗字段齐),resolve_outcome 经
//! infer_resolution_model 建 DicePoolOpposed——**不依赖库内 Triangle kernel**,任意 DATABASE_URL
//! 可跑(只用 DB 连接做 profile/resolution 落库)。跑完即弃(临时 session id)。
//!
//! Run:  DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
//!       cargo test -p trpg-contest --test live_pool_opposed -- --nocapture
use chrono::Utc;
use serde_json::json;
use trpg_contest::ContestService;
use trpg_db::Db;
use trpg_model::*;

const RULESET: &str = "triangle_agency";

/// 对抗骰池契约:target 直接给具体 DicePoolCount(face=3,阈值=1),并带 target_actor +
/// opponent_tested_parameter → contest 判为对抗 → 建 DicePoolOpposed。
fn opposed_pool_contract(session: &str) -> CheckContract {
    serde_json::from_value(json!({
        "check_id": format!("check_pool_{}", uuid::Uuid::new_v4().simple()),
        "session_id": session, "turn_id": "turn_test", "ruleset_id": RULESET, "module_id": null,
        "initiator": {"actor_id":"pc.current","actor_kind":"player_character","display_name":null},
        "target_actor": {"actor_id":"npc.rival","actor_kind":"npc","display_name":"Rival Agent"},
        "opposition": {"kind":"no_mechanical_opposition"},
        "action_summary": "对抗 Rival Agent", "intent_kind": "opposed_action",
        "check_label": "opposed competence pool", "dice_expression": "6d4", "modifiers": [],
        "target": {"kind":"dice_pool_count","target_face":3,"threshold":1,"label":"kernel core mechanic (dice pool)"},
        "tested_parameter": {"domain":null,"key":"Competence","label":"Competence"},
        "opponent_tested_parameter": {"domain":null,"key":"Competence","label":"Competence"},
        "actor_snapshot_ids": [], "source_refs": [], "learned_packet_ids": [],
        "roll_visibility": "public_gm_roll", "roll_authority": "system",
        "disclosure": {"show_roll_to_player":true,"show_formula_to_player":true,"show_dc_to_player":true,"show_success_failure_to_player":true,"reveal_after_scene":false,"reveal_after_session":false},
        "stakes": {"before_roll_public":"","success_public":"","failure_public":"","critical_public":null,"fumble_public":null,"success_patches_allowed":[],"failure_patches_allowed":[],"irreversible":false},
        "confidence": "medium", "ruling_status": "source_backed", "advice_refs": [], "expires_at_turn": null
    })).unwrap()
}

/// 一卷池骰:result 带 per-die `rolls` 数组(数 hits 的唯一来源),total 仅占位。
fn pool_roll(session: &str, roller: &str, kind: &str, dice: &[i64]) -> DiceRollRecord {
    let total: i64 = dice.iter().sum();
    serde_json::from_value(json!({
        "roll_id": format!("roll_{}", uuid::Uuid::new_v4().simple()), "session_id": session,
        "turn_id": "turn_test", "check_id": null, "roller_kind": kind, "roller_id": roller,
        "visibility": "public_gm_roll", "expression": "6d4", "result": {"total": total, "rolls": dice},
        "seed_commitment": "", "revealed_at": null, "created_at": Utc::now()
    })).unwrap()
}

#[tokio::test]
async fn count_faces_opposed_resolves_by_hit_counts_with_real_defender_roll() {
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
            eprintln!("SKIP: connect failed: {e}");
            return;
        }
    };

    let session = format!("session_pool_test_{}", uuid::Uuid::new_v4().simple());
    let svc = ContestService::new(db.clone());

    // 攻击池 [3,3,1,2,4,3] = 3 个 hit(face 3);防御池 [3,1,2,2,4,1] = 1 个 hit。3>1 → 攻击方胜。
    // 防御骰是**真掷的** per-die 数组,直接进 resolve_pool_opposed 数 hits。
    let atk = pool_roll(
        &session,
        "pc.current",
        "player_character",
        &[3, 3, 1, 2, 4, 3],
    );
    let def = pool_roll(&session, "npc.rival", "npc", &[3, 1, 2, 2, 4, 1]);
    let out = svc
        .resolve_outcome(&opposed_pool_contract(&session), &atk, Some(&def))
        .await
        .expect("resolve_outcome must not error");
    println!("[pool-opposed] {}", serde_json::to_string(&out).unwrap());

    // 核心:resolution_model = DicePoolOpposed(非单方 DicePoolCount/Provisional)。
    let model_kind = out
        .get("resolution_model")
        .and_then(|m| m.get("kind"))
        .and_then(|v| v.as_str());
    assert_eq!(
        model_kind,
        Some("dice_pool_opposed"),
        "count_faces 对抗契约必须建 DicePoolOpposed,实际={:?}",
        model_kind
    );

    // 真胜负:success=true(非 null),winner=attacker,双方 hits 透出。
    assert_eq!(
        out.get("success").and_then(|v| v.as_bool()),
        Some(true),
        "攻击 3 hits > 防御 1 hit → 攻击方胜(消费层真结算,非 Provisional-null)"
    );
    let opp = out.get("opposed").expect("必须富化 opposed 块");
    assert_eq!(
        opp.get("attacker_hits").and_then(|v| v.as_i64()),
        Some(3),
        "攻击方 3 hits"
    );
    assert_eq!(
        opp.get("defender_hits").and_then(|v| v.as_i64()),
        Some(1),
        "防御方 1 hit(真掷池数出)"
    );
    assert_eq!(opp.get("winner").and_then(|v| v.as_str()), Some("attacker"));
    assert_eq!(
        opp.get("defender_dice")
            .and_then(|v| v.as_array())
            .map(|a| a.len()),
        Some(6),
        "防御方 per-die 数组进结算(defender 真掷,非省略)"
    );

    println!("PASS: count_faces 对抗 3v1 hits → attacker_wins,defender 真掷池");
}

/// fail-closed:防御方还没掷池(defender_roll=None)→ success 维持 null + 显式 awaiting_binding,
/// 而非静默 miss / 乱判命中。坐实 resolve_against_model 的待绑信号经 resolve_outcome 透出全链路。
#[tokio::test]
async fn count_faces_opposed_without_defender_roll_emits_awaiting_binding() {
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
            eprintln!("SKIP: connect failed: {e}");
            return;
        }
    };

    let session = format!("session_pool_unbound_{}", uuid::Uuid::new_v4().simple());
    let svc = ContestService::new(db.clone());
    let atk = pool_roll(
        &session,
        "pc.current",
        "player_character",
        &[3, 3, 3, 1, 2, 4],
    );
    let out = svc
        .resolve_outcome(&opposed_pool_contract(&session), &atk, None)
        .await
        .expect("resolve_outcome must not error");
    println!("[pool-unbound] {}", serde_json::to_string(&out).unwrap());

    // 仍是 DicePoolOpposed(建了对抗模型),但无防御骰 → success 维持 null(不乱判命中)。
    assert!(
        out.get("success").map(|v| v.is_null()).unwrap_or(true),
        "无防御骰 → success 必须 null,实际={:?}",
        out.get("success")
    );
    let awaiting = out.get("awaiting_binding").and_then(|v| v.as_str());
    assert!(
        awaiting.map(|s| !s.trim().is_empty()).unwrap_or(false),
        "无防御骰 → 必须带非空 awaiting_binding(显式待绑非静默 miss),keys={:?}",
        out.as_object().map(|o| o.keys().collect::<Vec<_>>())
    );

    println!(
        "PASS: 无防御骰 → success=null 且 awaiting_binding={:?}",
        awaiting
    );
}
