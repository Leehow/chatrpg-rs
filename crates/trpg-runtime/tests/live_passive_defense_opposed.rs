//! GOLD live e2e（Phase 3 §9 范围外遗留 gap）：**被动防御动作**（玩家选闪避/防御）
//! 的"对手该测键"方向。坐实 `check_param_need` 的修复——玩家被动防御时是 NPC 在
//! 攻击你，对手该测的不是它的**防御值**，而是它的**攻击技能**（命中你的能力）。
//!
//! 区别于 `live_opposed_meet_or_beat`（玩家主动攻击 → 对手测 defense）：本测走玩家
//! **闪避**（SituationActionKind::Dodge）→ check_param_need 应给 ("skills","attack")，
//! 再经 production 通路 `stamp_opposed_check` 盖章 + `ContestService` 对抗结算消费它。
//!
//! 判别式设计：NPC 卡同时 seed `skills.attack=15` 与 `stats.defense=8`（故意不同）。
//! - **修复后**（对手测 attack）→ defender_value=15；
//! - **旧 bug**（对手测 defense）→ defender_value=8。
//! defender_value 的取值直接、无歧义地证明方向对不对。
//!
//! 需 DATABASE_URL 指向含 `cyberpunk_red`(compare=meet_or_beat) kernel 的库(:54346)；
//! 否则 SKIP。完全确定性：自给自足 seed 一对临时 PC/NPC，跑完即弃，不依赖 LLM。
//!
//! Run:  DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54346/chatrpg \
//!       cargo test -p trpg-runtime --test live_passive_defense_opposed -- --nocapture
use chrono::Utc;
use serde_json::json;
use trpg_contest::ContestService;
use trpg_db::Db;
use trpg_model::*;
use trpg_params::{RuntimeActorParameters, RuntimeParameterService};
use trpg_runtime::{npc_synth::NpcPersona, stamp_opposed_check, RuntimeEngine};

const RULESET: &str = "cyberpunk_red";

fn seed_actor(session: &str, actor_id: &str, kind: ActorKind, mech: serde_json::Value) -> RuntimeActorParameters {
    RuntimeActorParameters {
        actor_param_id: format!("ap_{}", uuid::Uuid::new_v4().simple()),
        session_id: session.into(),
        actor_id: actor_id.into(),
        actor_kind: kind,
        ruleset_id: RULESET.into(),
        source_kind: "test_seed".into(),
        template_id: None,
        display_name: None,
        sheet_json: json!({}),
        mechanical_profile: mech,
        status_json: json!({}),
        visibility: Visibility::GmOnly,
        created_at_tick: Some(0),
        updated_at_tick: Some(0),
    }
}

/// 玩家闪避契约的"盖章前"形态：tested_parameter=玩家闪避技能 evasion；target_actor /
/// opponent_tested_parameter 留空，由 stamp_opposed_check 按 check_param_need 的结果填。
fn base_dodge_contract(session: &str) -> CheckContract {
    serde_json::from_value(json!({
        "check_id": format!("check_dodge_{}", uuid::Uuid::new_v4().simple()),
        "session_id": session, "turn_id": "turn_test", "ruleset_id": RULESET, "module_id": null,
        "initiator": {"actor_id":"pc.current","actor_kind":"player_character","display_name":null},
        "target_actor": null,
        "opposition": {"kind":"no_mechanical_opposition"},
        "action_summary": "侧身闪避清道夫的扫射", "intent_kind": "dodge",
        "check_label": "闪避来袭枪火", "dice_expression": "1d10", "modifiers": [],
        "target": {"kind":"unknown_until_lookup"},
        "tested_parameter": {"domain":null,"key":"evasion","label":"evasion"},
        "opponent_tested_parameter": null,
        "actor_snapshot_ids": [], "source_refs": [], "learned_packet_ids": [],
        "roll_visibility": "public_gm_roll", "roll_authority": "system",
        "disclosure": {"show_roll_to_player":true,"show_formula_to_player":true,"show_dc_to_player":true,"show_success_failure_to_player":true,"reveal_after_scene":false,"reveal_after_session":false},
        "stakes": {"before_roll_public":"","success_public":"","failure_public":"","critical_public":null,"fumble_public":null,"success_patches_allowed":[],"failure_patches_allowed":[],"irreversible":false},
        "confidence": "medium", "ruling_status": "source_backed", "advice_refs": [], "expires_at_turn": null
    })).unwrap()
}

fn roll(session: &str, total: i64) -> DiceRollRecord {
    serde_json::from_value(json!({
        "roll_id": format!("roll_{}", uuid::Uuid::new_v4().simple()), "session_id": session,
        "turn_id": "turn_test", "check_id": null, "roller_kind": "player_character", "roller_id": "pc.current",
        "visibility": "public_gm_roll", "expression": "1d10", "result": {"total": total, "rolls": [total]},
        "seed_commitment": "", "revealed_at": null, "created_at": Utc::now()
    })).unwrap()
}

#[tokio::test]
async fn passive_defense_opposes_npc_attack_skill_not_its_defense() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect failed: {e}"); return; } };
    // 闸门：库内必须有 cyberpunk_red(meet_or_beat) kernel，否则被动防御映射无从数据驱动 → SKIP。
    let kernel = db.load_rule_kernel(RULESET).await.ok().flatten();
    let is_mob = kernel.as_ref().and_then(|k| k.dice_core.get("compare").and_then(|v| v.as_str())) == Some("meet_or_beat");
    if !is_mob { eprintln!("SKIP: no cyberpunk_red meet_or_beat kernel in this DB (need :54346)"); return; }

    // 自给自足 seed（跑完即弃）：
    //   玩家（防御方）闪避技能 evasion=10；
    //   NPC（攻击方）attack=15（命中能力）+ defense=8（故意不同，作判别式）。
    let session = format!("session_passive_def_e2e_{}", uuid::Uuid::new_v4().simple());
    let npc_id = "npc.scav_gunner";
    let params = RuntimeParameterService::new(db.clone());
    params.upsert_actor_parameters(&seed_actor(&session, "pc.current", ActorKind::PlayerCharacter,
        json!({"stats": {"REF": 6}, "skills": {"evasion": 10}}))).await.expect("seed pc");
    params.upsert_actor_parameters(&seed_actor(&session, npc_id, ActorKind::Npc,
        json!({"stats": {"defense": 8}, "skills": {"attack": 15}}))).await.expect("seed npc");

    let engine = RuntimeEngine::new(db.clone());

    // —— ① THE FIX：被动防御动作（玩家选闪避 Dodge）→ check_param_need 取**对手该测键**。
    //    真 CPR kernel(meet_or_beat) 数据驱动，断言 ("skills","attack")——NPC 在攻击你，
    //    测它的攻击技能，而非它的防御值。零规则集硬编码（结果来自 kernel.compare 分支）。
    let (bucket, param) = engine.check_param_need(RULESET, &SituationActionKind::Dodge).await
        .expect("Dodge 必须需要对手参数（被动防御 → 对手攻击技能）");
    assert_eq!((bucket.as_str(), param.as_str()), ("skills", "attack"),
        "被动防御：对手该测键=攻击技能（NPC 在攻击你），不是它的防御值");
    println!("[fix] check_param_need(Dodge) = {bucket}.{param}");

    // —— ② production 盖章：玩家闪避契约 + 对手测键（与 GM 显式 opposed 同一通路）。
    let mut contract = base_dodge_contract(&session);
    let persona = NpcPersona { actor_id: npc_id.into(), name: "清道夫枪手".into(), prose: "举枪向你扫射的清道夫。".into() };
    stamp_opposed_check(&mut contract, &persona, &bucket, &param);
    assert_eq!(contract.target_actor.as_ref().map(|a| a.actor_id.as_str()), Some(npc_id), "盖章后 target_actor=攻击你的 NPC");
    assert_eq!(contract.opponent_tested_parameter.as_ref().map(|p| p.key.as_str()), Some("attack"), "opponent_tested_parameter=NPC 攻击技能");

    // —— ③ production 对抗结算。玩家闪避骰 vs NPC 攻击命中骰（meet_or_beat：total>=自家技能值）。
    //    玩家掷 9（<evasion 10 → 闪避未达成）；NPC 掷 16（>=attack 15 → 命中达成）
    //    → attacker(玩家闪避方)败、defender(NPC 攻击方)胜 → NPC 的枪火落在玩家身上。
    let svc = ContestService::new(db.clone());
    let out = svc.resolve_outcome(&contract, &roll(&session, 9), Some(&roll(&session, 16)))
        .await.expect("resolve_outcome must not error");
    println!("[passive-def] {}", serde_json::to_string(&out).unwrap());

    // 必须建 OpposedRoll（contract_is_opposed 触发），非 StaticTargetNumber/Provisional。
    let model_kind = out.pointer("/resolution_model/kind").and_then(|v| v.as_str());
    assert_eq!(model_kind, Some("opposed_roll"), "被动防御对抗必须建 OpposedRoll，实际={:?}", model_kind);

    // 对手测键正确：defender_value = NPC 的 attack=15（消费攻击技能），**不是** defense=8（旧 bug）。
    let dv = out.pointer("/opposed/defender_value").and_then(|v| v.as_i64());
    assert_eq!(dv, Some(15), "defender_value 必须=NPC attack=15（消费攻击技能）；若=8 说明仍误测 defense（方向反）");
    assert_ne!(dv, Some(8), "绝不能=defense=8（旧 bug 的方向）");
    // attacker_value = 玩家闪避技能 evasion=10。
    let av = out.pointer("/opposed/attacker_value").and_then(|v| v.as_i64());
    assert_eq!(av, Some(10), "attacker_value=玩家 evasion=10");

    // 伤害结算方向对：玩家闪避失败 + NPC 命中 → winner=defender（NPC 攻击方）→ 玩家挨枪。
    //   opposed-model 标签：attacker=发起 check 的玩家(闪避方)、defender=target_actor 的 NPC(攻击方)。
    let success = out.get("success").and_then(|v| v.as_bool());
    assert_eq!(success, Some(false), "玩家(attacker=闪避方)未胜 → NPC 攻击命中");
    let winner = out.pointer("/opposed/winner").and_then(|v| v.as_str());
    assert_eq!(winner, Some("defender"), "winner=defender(NPC 攻击方)→ 玩家承受攻击，方向正确");

    // 清场：删临时 session 的 actor 参数（留库干净）。
    sqlx::query("delete from runtime_actor_parameters where session_id=$1").bind(&session)
        .execute(&db.pool).await.ok();
    println!("PASS: 被动防御 → 对手测 attack=15(非 defense=8)，玩家闪避失败 → NPC 命中(winner=defender)，方向正确");
}
