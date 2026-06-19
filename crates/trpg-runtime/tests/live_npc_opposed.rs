//! GOLD live test:端到端验证"NPC 合成值消费层"——真调 ensure_npc_parameter 现搓
//! NPC 的对抗技能(LLM persona-judge,写 sheet + B1 投影 mechanical_profile),再经
//! ContestService::resolve_outcome 的对抗结算消费它,断言真出胜负(非 Provisional-null)。
//! 完全绕开 orchestrator 的 action_kind 分类(那是 LLM 路由,不可靠)。
//!
//! 需:DATABASE_URL 指向含 CoC 测试角色(pc.current 有 Spot Hidden)的库 + LLM env
//! (TRPG_LLM_*,codex-relay)。缺则 SKIP。
//! Run:
//!   set -a; source .env.example; source .env; set +a
//!   DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!   TRPG_NPC_PERSONA_SYNTHESIS=true \
//!   cargo test -p trpg-runtime --test live_npc_opposed -- --nocapture
use chrono::Utc;
use serde_json::json;
use trpg_contest::ContestService;
use trpg_db::Db;
use trpg_model::*;
use trpg_runtime::{npc_synth::NpcPersona, RuntimeEngine};

const SESSION: &str = "session_6def47593a094513a75b01b69a52c986";
const RULESET: &str = "call_of_cthulhu_7e";

fn opposed_contract() -> CheckContract {
    serde_json::from_value(json!({
        "check_id": format!("check_opp_{}", uuid::Uuid::new_v4().simple()),
        "session_id": SESSION, "turn_id": "turn_test", "ruleset_id": RULESET, "module_id": null,
        "initiator": {"actor_id":"pc.current","actor_kind":"player_character","display_name":null},
        // §3.3 盖章后的形态:target_actor=NPC + opponent_tested_parameter=防御方该测的键。
        "target_actor": {"actor_id":"npc.opposition","actor_kind":"npc","display_name":"拉斯"},
        "opposition": {"kind":"no_mechanical_opposition"},
        "action_summary": "悄悄行动避开拉斯", "intent_kind": "investigate_during_conflict",
        "check_label": "Spot Hidden check", "dice_expression": "1d100", "modifiers": [],
        "target": {"kind":"unknown_until_lookup"},
        "tested_parameter": {"domain":null,"key":"Spot Hidden","label":"Spot Hidden"},
        "opponent_tested_parameter": {"domain":null,"key":"perception","label":"perception"},
        "actor_snapshot_ids": [], "source_refs": [], "learned_packet_ids": [],
        "roll_visibility": "public_gm_roll", "roll_authority": "system",
        "disclosure": {"show_roll_to_player":true,"show_formula_to_player":true,"show_dc_to_player":true,"show_success_failure_to_player":true,"reveal_after_scene":false,"reveal_after_session":false},
        "stakes": {"before_roll_public":"","success_public":"","failure_public":"","critical_public":null,"fumble_public":null,"success_patches_allowed":[],"failure_patches_allowed":[],"irreversible":false},
        "confidence": "medium", "ruling_status": "source_backed", "advice_refs": [], "expires_at_turn": null
    })).unwrap()
}

fn roll(total: i64) -> DiceRollRecord {
    serde_json::from_value(json!({
        "roll_id": format!("roll_{}", uuid::Uuid::new_v4().simple()), "session_id": SESSION,
        "turn_id": "turn_test", "check_id": null, "roller_kind": "player_character", "roller_id": "pc.current",
        "visibility": "public_gm_roll", "expression": "1d100", "result": {"total": total, "rolls": [total]},
        "seed_commitment": "", "revealed_at": null, "created_at": Utc::now()
    })).unwrap()
}

#[tokio::test]
async fn synthesized_npc_value_drives_opposed_verdict() {
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
    let params = trpg_params::RuntimeParameterService::new(db.clone());
    // 夹具门:pc.current 必须有 Spot Hidden(攻击方 tested value)。否则 SKIP。
    let pc = params
        .load_actor_parameters(SESSION, "pc.current")
        .await
        .ok()
        .flatten();
    if pc
        .as_ref()
        .and_then(|p| p.mechanical_profile.pointer("/skills/Spot Hidden"))
        .is_none()
    {
        eprintln!("SKIP: CoC fixture pc.current/Spot Hidden not found in this DB");
        return;
    }
    // npc.opposition 卡必须存在,ensure_npc_parameter 才能 load 后合成。确保它在。
    params
        .ensure_actor_parameters(SESSION, RULESET, "npc.opposition", ActorKind::Npc, 0)
        .await
        .expect("ensure npc.opposition card");

    let engine = RuntimeEngine::new(db.clone());
    // 真合成:现搓拉斯的对抗技能 perception(LLM persona-judge → 写 sheet + B1 投影 mech)。
    let persona = NpcPersona {
        actor_id: "npc.opposition".into(),
        name: "拉塞尔·威廉姆斯(拉斯)".into(),
        prose: "屠宰场里警觉而危险的人物,对周遭动静敏感。".into(),
    };
    let synth = engine
        .ensure_npc_parameter(
            SESSION,
            RULESET,
            &persona,
            "skills",
            "perception",
            "玩家试图在拉斯眼皮底下悄悄行动",
        )
        .await
        .expect("ensure_npc_parameter ok");
    let synth = match synth {
        Some(v) => v,
        None => {
            eprintln!(
                "SKIP: synthesis off / no LLM (set TRPG_NPC_PERSONA_SYNTHESIS=true + TRPG_LLM_*)"
            );
            return;
        }
    };
    println!("[synth] npc.opposition perception = {synth}");

    // B1 断言:合成值已投影进 mechanical_profile(contest 读的是 mech)。
    let npc = params
        .load_actor_parameters(SESSION, "npc.opposition")
        .await
        .ok()
        .flatten()
        .expect("npc card");
    let mech_perception = npc
        .mechanical_profile
        .pointer("/skills/perception")
        .cloned();
    assert!(
        mech_perception.is_some(),
        "B1: 合成的 perception 必须投影进 mechanical_profile,否则 contest 读不到"
    );
    println!(
        "[B1] mechanical_profile.skills.perception = {:?}",
        mech_perception
    );

    // 对抗结算:攻击方掷低(roll_under 易成功),防御方掷高(易失败)→ 期望攻击方胜。
    let svc = ContestService::new(db.clone());
    let def_roll = roll(96); // 防御方现掷(runtime 在生产里预掷;此处手传模拟)
    let out = svc
        .resolve_outcome(&opposed_contract(), &roll(5), Some(&def_roll))
        .await
        .expect("resolve");
    println!("[opposed] {}", serde_json::to_string(&out).unwrap());

    let success = out.get("success").and_then(|v| v.as_bool());
    let opposed = out.get("opposed");
    // 核心断言:真出胜负(非 Provisional-null)+ 消费了 NPC 卡的 defender_value。
    assert!(
        success.is_some(),
        "对抗必须给出胜负,而非 Provisional-null(消费层失败)"
    );
    let dv = opposed
        .and_then(|o| o.get("defender_value"))
        .and_then(|v| v.as_i64());
    assert!(
        dv.is_some(),
        "outcome.opposed.defender_value 必须非空(= 消费了 NPC 现搓的 perception)"
    );
    let av = opposed
        .and_then(|o| o.get("attacker_value"))
        .and_then(|v| v.as_i64());
    assert_eq!(
        av,
        Some(75),
        "attacker_value 应为 pc.current 的 Spot Hidden=75"
    );
    println!(
        "PASS: NPC 现搓值 perception={:?} 驱动对抗结算,attacker(75) vs defender, success={:?}",
        dv, success
    );
}
