//! GOLD 等价 live e2e：binding takeover 竖切的**逐字段等价铁证**。
//!
//! 走 runtime 公共入口 `resolve_check_with_input` →（私有）`resolve_outcome_with_opposition`
//! → `resolve_check_dispatch`（本竖切的钩子）。同一 CoC roll_under 检定（Spot Hidden=75）、
//! 同一确定性掷骰总值（player-reported total=30），分别在 `TRPG_BINDING_TAKEOVER` **关 / 开**
//! 下各结算一次，断言机械结果**逐字段相等**——证明 binding 接管路径与原路径零行为变更。
//! 再断言 ON 路径确实产出真实结算（target=75、success=true），坐实"takeover 真的驱动了一次结算"。
//!
//! 需 DATABASE_URL 指向含 CoC 测试角色（session_6def…/pc.current/Spot Hidden）+ roll_under
//! kernel 的库（:54347）；否则 SKIP。完全确定性、不依赖 LLM、跑完清理自建 check_result。
//!
//! Run:  DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
//!       cargo test -p trpg-runtime --test live_binding_takeover_equiv -- --nocapture
use chrono::Utc;
use serde_json::Value;
use trpg_db::Db;
use trpg_model::*;
use trpg_runtime::RuntimeEngine;

const SESSION: &str = "session_6def47593a094513a75b01b69a52c986";
const RULESET: &str = "call_of_cthulhu_7e";
const ACTOR: &str = "pc.current";

fn spot_hidden_contract(check_id: &str) -> CheckContract {
    CheckContract {
        check_id: check_id.into(),
        session_id: SESSION.into(),
        turn_id: "turn_test".into(),
        ruleset_id: RULESET.into(),
        module_id: None,
        initiator: ActorRef { actor_id: ACTOR.into(), actor_kind: ActorKind::PlayerCharacter, display_name: None },
        target_actor: None,
        opposition: OppositionModel::NoMechanicalOpposition,
        action_summary: "Spot Hidden".into(),
        intent_kind: "ability:skill_use".into(),
        check_label: "Spot Hidden (core mechanic)".into(),
        dice_expression: "1d100".into(),
        modifiers: vec![],
        target: CheckTargetModel::UnknownUntilLookup,
        tested_parameter: Some(TestedParameter { domain: None, key: "Spot Hidden".into(), label: "Spot Hidden".into() }),
        opponent_tested_parameter: None,
        actor_snapshot_ids: vec![],
        source_refs: vec![],
        learned_packet_ids: vec![],
        roll_visibility: RollVisibility::PublicGmRoll,
        roll_authority: RollAuthority::System,
        disclosure: RollDisclosurePolicy::for_visibility(RollVisibility::PublicGmRoll),
        stakes: CheckStakes::default(),
        confidence: RulingConfidence::Medium,
        ruling_status: RulingStatus::SourceBacked,
        advice_refs: vec![],
        expires_at_turn: None,
    }
}

/// 抽取参与等价比较的机械字段（与 check_id/contest_id 等非确定 id 无关）。
fn mech_fields(outcome: &Value) -> Value {
    serde_json::json!({
        "total": outcome.get("total").cloned(),
        "target": outcome.get("target").cloned(),
        "success": outcome.get("success").cloned(),
        "degree": outcome.get("degree").cloned(),
        "success_tier": outcome.get("success_tier").cloned(),
        "success_tier_rank": outcome.get("success_tier_rank").cloned(),
        "resolution_model_kind": outcome.pointer("/resolution_model/kind").cloned(),
    })
}

#[tokio::test]
async fn binding_takeover_check_resolution_is_byte_equivalent() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect failed: {e}"); return; } };
    // 闸门：库内须有 CoC roll_under kernel + Spot Hidden 测试角色，否则 SKIP。
    let kernel = db.load_rule_kernel(RULESET).await.ok().flatten();
    let is_roll_under = kernel.as_ref().and_then(|k| k.dice_core.get("compare").and_then(|v| v.as_str())) == Some("roll_under");
    if !is_roll_under { eprintln!("SKIP: 本库无 call_of_cthulhu_7e roll_under kernel（需 :54347）"); return; }
    let has_fixture = trpg_params::RuntimeParameterService::new(db.clone())
        .load_actor_parameters(SESSION, ACTOR).await.ok().flatten()
        .and_then(|p| p.mechanical_profile.get("skills").and_then(|s| s.get("Spot Hidden")).cloned())
        .is_some();
    if !has_fixture { eprintln!("SKIP: 本库无 CoC 测试角色 Spot Hidden fixture"); return; }

    // 确定性掷骰：player-reported total，输入 "30" → ReportedTotal(30)（见 parse_roll_text）。
    std::env::set_var("TRPG_PLAYER_REPORTED_ROLL_TOTALS", "1");
    let engine = RuntimeEngine::new(db.clone());

    // —— ① 关（显式 0：直调权威 resolve_outcome 原路径。默认已 ON，故此处必须显式关）。
    std::env::set_var("TRPG_BINDING_TAKEOVER", "0");
    let c_off = spot_hidden_contract("check_takeover_off");
    let res_off = engine.resolve_check_with_input(SESSION, "turn_off", &c_off, "30").await.expect("resolve OFF");
    let off = mech_fields(&res_off.outcome);
    println!("[OFF] {}", serde_json::to_string(&off).unwrap());

    // —— ② 开（经 BindingResolver → ExecutionTier 路由 → capability executor）。
    std::env::set_var("TRPG_BINDING_TAKEOVER", "1");
    let c_on = spot_hidden_contract("check_takeover_on");
    let res_on = engine.resolve_check_with_input(SESSION, "turn_on", &c_on, "30").await.expect("resolve ON");
    let on = mech_fields(&res_on.outcome);
    println!("[ON ] {}", serde_json::to_string(&on).unwrap());

    // —— ③ 逐字段等价：takeover 路径与原路径零行为变更。
    assert_eq!(off, on, "binding takeover ON 的机械结果必须与 OFF 逐字段相等（零行为变更）");
    // —— ④ ON 路径确实驱动了真实结算（非回退/provisional）：Spot Hidden=75，roll 30 → 成功。
    assert_eq!(on.get("target").and_then(|v| v.as_i64()), Some(75), "ON 路径须读到真实 Spot Hidden=75（证明 takeover 真结算）");
    assert_eq!(on.get("success").and_then(|v| v.as_bool()), Some(true), "30 <= 75 必成功");

    // 清理自建 check_result（仅删本测两条，不动 fixture 其他数据）。
    for cid in ["check_takeover_off", "check_takeover_on"] {
        sqlx::query("delete from check_results where check_id = $1").bind(cid).execute(&db.pool).await.ok();
    }
    std::env::remove_var("TRPG_BINDING_TAKEOVER");
    std::env::remove_var("TRPG_PLAYER_REPORTED_ROLL_TOTALS");
    let _ = Utc::now();
    println!("PASS: binding takeover ON==OFF 逐字段等价，且 ON 真实结算 Spot Hidden(75) roll30=success。");
}
