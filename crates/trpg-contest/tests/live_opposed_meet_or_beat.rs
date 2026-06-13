//! DB-gated:验证 meet_or_beat 规则集(CPR)的对抗 check 经 contest 读对手 NPC 防御值出真胜负。
//! 对标 live_opposed.rs(roll_under)的 meet_or_beat 版,坐实 Phase 3 下游补读卡支路。
//!
//! 需 DATABASE_URL 指向含 `cyberpunk_red`(compare=meet_or_beat)kernel 的库(:54346);否则 SKIP。
//! 测试自给自足地 seed 一对临时 PC/NPC actor 参数(攻击方 Handgun 技能 + 防御方 defense 属性),
//! 不依赖外部夹具,跑完即弃(临时 session id)。
//!
//! Run:  DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54346/chatrpg \
//!       cargo test -p trpg-contest --test live_opposed_meet_or_beat -- --nocapture
use chrono::Utc;
use serde_json::json;
use trpg_contest::ContestService;
use trpg_db::Db;
use trpg_model::*;
use trpg_params::{RuntimeActorParameters, RuntimeParameterService};

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

fn opposed_contract(session: &str) -> CheckContract {
    serde_json::from_value(json!({
        "check_id": format!("check_mob_{}", uuid::Uuid::new_v4().simple()),
        "session_id": session, "turn_id": "turn_test", "ruleset_id": RULESET, "module_id": null,
        "initiator": {"actor_id":"pc.current","actor_kind":"player_character","display_name":null},
        "target_actor": {"actor_id":"npc.boss","actor_kind":"npc","display_name":"Scav Boss"},
        "opposition": {"kind":"no_mechanical_opposition"},
        "action_summary": "开火攻击 Scav Boss", "intent_kind": "attack",
        "check_label": "Handgun attack", "dice_expression": "1d10", "modifiers": [],
        "target": {"kind":"unknown_until_lookup"},
        "tested_parameter": {"domain":null,"key":"Handgun","label":"Handgun"},
        "opponent_tested_parameter": {"domain":null,"key":"defense","label":"defense"},
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
async fn meet_or_beat_opposed_reads_npc_defense_and_yields_verdict() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect failed: {e}"); return; } };
    // 闸门:库内必须有 cyberpunk_red(meet_or_beat)kernel,否则本期支路无从激活 → SKIP。
    let kernel = db.load_rule_kernel(RULESET).await.ok().flatten();
    let is_mob = kernel.as_ref().and_then(|k| k.dice_core.get("compare").and_then(|v| v.as_str())) == Some("meet_or_beat");
    if !is_mob { eprintln!("SKIP: no cyberpunk_red meet_or_beat kernel in this DB (need :54346)"); return; }

    // 自给自足 seed:临时 session,攻击方 Handgun=14、防御方 defense(DV)=12。跑完即弃。
    let session = format!("session_mob_test_{}", uuid::Uuid::new_v4().simple());
    let params = RuntimeParameterService::new(db.clone());
    params.upsert_actor_parameters(&seed_actor(&session, "pc.current", ActorKind::PlayerCharacter,
        json!({"stats": {"REF": 6}, "skills": {"Handgun": 14}}))).await.expect("seed pc");
    params.upsert_actor_parameters(&seed_actor(&session, "npc.boss", ActorKind::Npc,
        json!({"stats": {"defense": 12}, "skills": {}}))).await.expect("seed npc");

    let svc = ContestService::new(db.clone());
    // 攻击方 total 18(高,达成自家 14)、防御方 total 9(<12 未达成)→ 期望攻击方命中。
    let out = svc.resolve_outcome(&opposed_contract(&session), &roll(&session, 18), Some(&roll(&session, 9)))
        .await.expect("resolve_outcome must not error");
    println!("[mob-opposed] {}", serde_json::to_string(&out).unwrap());

    // 核心:resolution_model 必须是 OpposedRoll(非 StaticTargetNumber/Provisional)。
    let model_kind = out.get("resolution_model").and_then(|m| m.get("kind")).and_then(|v| v.as_str());
    assert_eq!(model_kind, Some("opposed_roll"),
        "meet_or_beat 对抗契约必须建 OpposedRoll(本期补的读卡支路),实际={:?}", model_kind);

    let success = out.get("success").and_then(|v| v.as_bool());
    assert!(success.is_some(), "对抗必须给出胜负(success != null),而非 Provisional-null(消费层失败)");

    let opposed = out.get("opposed");
    let dv = opposed.and_then(|o| o.get("defender_value")).and_then(|v| v.as_i64());
    assert_eq!(dv, Some(12), "outcome.opposed.defender_value 必须=12(消费了 NPC 卡的 defense)");
    let av = opposed.and_then(|o| o.get("attacker_value")).and_then(|v| v.as_i64());
    assert_eq!(av, Some(14), "attacker_value 应为 pc.current 的 Handgun=14");

    // 清场:删临时 session 的 actor 参数(留库干净)。
    sqlx::query("delete from runtime_actor_parameters where session_id=$1").bind(&session)
        .execute(&db.pool).await.ok();
    println!("PASS: meet_or_beat opposed 读到 NPC defense={:?},attacker(14) 命中,success={:?}", dv, success);
}

/// spec §4.3 / §6⑥:fail-closed 不静默 miss。防御方 NPC **没有** defense 值(现搓不成/
/// 查不到)→ OpposedRoll{defender_value:None} → resolve_opposed 返 None → success 仍 null。
/// 关键:此时 outcome 必须带**显式** `awaiting_binding` 信号(键存在且非空),GM agent 据此
/// 改道,而非看到 success=null 的沉默 miss。坐实 resolve_against_model 的待绑信号经
/// resolve_outcome 透出到 outcome JSON 这条全链路。
#[tokio::test]
async fn meet_or_beat_opposed_without_defense_emits_awaiting_binding() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect failed: {e}"); return; } };
    let kernel = db.load_rule_kernel(RULESET).await.ok().flatten();
    let is_mob = kernel.as_ref().and_then(|k| k.dice_core.get("compare").and_then(|v| v.as_str())) == Some("meet_or_beat");
    if !is_mob { eprintln!("SKIP: no cyberpunk_red meet_or_beat kernel in this DB (need :54346)"); return; }

    // 攻击方有 Handgun,但防御方 NPC **缺 defense**(stats 里没有 defense 键)→ 防御值查不到。
    let session = format!("session_mob_unbound_{}", uuid::Uuid::new_v4().simple());
    let params = RuntimeParameterService::new(db.clone());
    params.upsert_actor_parameters(&seed_actor(&session, "pc.current", ActorKind::PlayerCharacter,
        json!({"stats": {"REF": 6}, "skills": {"Handgun": 14}}))).await.expect("seed pc");
    params.upsert_actor_parameters(&seed_actor(&session, "npc.boss", ActorKind::Npc,
        json!({"stats": {"BODY": 5}, "skills": {}}))).await.expect("seed npc no-defense");

    let svc = ContestService::new(db.clone());
    let out = svc.resolve_outcome(&opposed_contract(&session), &roll(&session, 18), Some(&roll(&session, 9)))
        .await.expect("resolve_outcome must not error");
    println!("[mob-unbound] {}", serde_json::to_string(&out).unwrap());

    // 仍是 OpposedRoll(读卡支路建了对抗模型),但防御值缺 → success 维持 null(fail-closed)。
    let success = out.get("success");
    assert!(success.map(|v| v.is_null()).unwrap_or(true), "防御值缺 → success 必须 null(不乱判命中),实际={:?}", success);

    // 核心断言:outcome 出现非空 awaiting_binding 键(显式待绑,非静默 miss)。
    let awaiting = out.get("awaiting_binding").and_then(|v| v.as_str());
    assert!(awaiting.is_some(), "防御值缺 → outcome 必须带 awaiting_binding 信号(非静默 success=null),实际 keys={:?}",
        out.as_object().map(|o| o.keys().collect::<Vec<_>>()));
    assert!(!awaiting.unwrap().trim().is_empty(), "awaiting_binding 理由不得为空");

    sqlx::query("delete from runtime_actor_parameters where session_id=$1").bind(&session)
        .execute(&db.pool).await.ok();
    println!("PASS: 防御值缺 → success=null 且 awaiting_binding={:?}(显式待绑非静默 miss)", awaiting);
}
