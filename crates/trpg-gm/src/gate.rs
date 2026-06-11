use crate::ledger::TurnLedger;
use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;
use trpg_agent::parse_roll_text;
use trpg_model::{CheckContract, CheckResultRecord, ContextRequest, StatePatch};
use trpg_runtime::RuntimeEngine;

/// 头部 gate 结算注入 seam（单测合成 open gate 绕开真 DB；生产装配恒 None）。
/// None ⇒ 无 open gate；Some(Ok) ⇒ 结算成功（契约+结果）；Some(Err) ⇒ 结算失败。
pub type GateResolverFn = Arc<dyn Fn(&str) -> Option<Result<(CheckContract, CheckResultRecord)>> + Send + Sync>;

/// 确定性头部第 4 步（spec §4）：玩家输入若是 open gate 的骰值/掷骰应答，
/// 在 LLM 看到输入前先结算，产物入账 + 折成 [roll] 事实注入 dynamic tail。
///
/// 两个 e2e must-fix 都收口在此：
/// 1. 裸 "roll"/系统代掷类回复 parse_roll_text 返回 None，但同样是 gate 应答——
///    复用 trpg-runtime 同源原语 wants_system_roll（resolve_roll_input 对此类
///    输入回退 contract.dice_expression），不自造关键词表。
/// 2. 结算 Err（如 reported totals 被禁）折成非致命事实（同 gate 事实通道），
///    绝不向上传播炸掉 play_cli_agent 的整个会话循环；回合继续由 agent 处理
///    （reprompt 玩家或叙事化）。
pub(crate) async fn resolve_pending_gate(
    engine: &RuntimeEngine,
    gate_resolver: Option<&GateResolverFn>,
    request: &ContextRequest,
    current_scene_id: Option<&str>,
    user_input: &str,
    ledger: &mut TurnLedger,
    resolved_gate_facts: &mut Vec<String>,
) {
    if parse_roll_text(user_input).is_none() && !trpg_runtime::wants_system_roll(user_input) {
        return;
    }
    let (session_id, turn_id) = (request.session_id.as_str(), request.turn_id.as_str());
    let resolution = match gate_resolver {
        Some(resolver) => resolver(user_input),
        None => match engine.db.get_open_pending_check(session_id).await.ok().flatten() {
            Some(pending) => {
                let contract = pending.contract.clone();
                Some(engine.resolve_check_with_input(session_id, turn_id, &pending.contract, user_input).await.map(|result| (contract, result)))
            }
            None => None,
        },
    };
    match resolution {
        Some(Ok((contract, result))) => {
            ledger.record_contract(&contract);
            ledger.record_result(&result);
            resolved_gate_facts.push(format!("[roll]Resolved pending check {}: {}[/roll]", result.check_id, serde_json::to_string(&result.outcome).unwrap_or_default()));
            // C7：盖章契约结算后 effect_policy 由 Rust 强制执行（spec §6"效果不留给
            // 叙事"在玩家亲掷路径同样成立）——失败折事实，绝不炸回合。
            run_policy_after_gate(engine, request, current_scene_id, &contract, &result.outcome, ledger, resolved_gate_facts).await;
        }
        Some(Err(err)) => {
            tracing::warn!(error = %err, session_id = %session_id, "pending check resolution failed; folded into turn context");
            resolved_gate_facts.push(format!("[roll]Pending check resolution failed: {err}. No roll was committed; the pending check is still open. Re-prompt the player for a valid roll reply or continue narratively without mechanical resolution.[/roll]"));
        }
        None => {}
    }
}

/// PURE：盖章解析——advice_refs 中 `scene_mechanic:<intent_id>`（stamp_scene_intent
/// 写入，此处恢复；盖章每契约至多一枚，取首个）。
pub(crate) fn scene_mechanic_ref(advice_refs: &[String]) -> Option<&str> {
    advice_refs.iter().find_map(|r| r.strip_prefix("scene_mechanic:"))
}

/// gate 结算后 effect_policy 强制执行：契约盖章引用 → module graph 找回 intent →
/// scene_policy::apply_effect_policy（按 outcome success 选分支、落 world_event
/// effect_applied 单点证据）。执行结果（成功摘要 / 失败原因 / fail-closed 跳过）
/// 一律折成 [scene_policy] gate 事实——可观测不静默，绝不向上传播炸回合
/// （对齐 resolve_pending_gate 的 Err 折叠惯例）。
async fn run_policy_after_gate(
    engine: &RuntimeEngine,
    request: &ContextRequest,
    current_scene_id: Option<&str>,
    contract: &CheckContract,
    outcome: &Value,
    ledger: &mut TurnLedger,
    facts: &mut Vec<String>,
) {
    let Some(intent_id) = scene_mechanic_ref(&contract.advice_refs) else { return };
    // fail-closed：outcome 无 success 布尔不执行（对齐 run_policy_after_settlement）。
    let Some(success) = outcome.get("success").and_then(Value::as_bool) else {
        facts.push(format!("[scene_policy]Scene mechanic {intent_id} not enforced: outcome has no success field.[/scene_policy]"));
        return;
    };
    match locate_and_apply(engine, request, current_scene_id, contract, intent_id, success, ledger).await {
        Ok(patches) => {
            let branch = if success { "success" } else { "failure" };
            facts.push(format!("[scene_policy]Scene mechanic {intent_id} consequences enforced by the engine ({branch}): {}. Narrate these effects as already applied; do not re-apply or invent others.[/scene_policy]", crate::scene_policy::patches_summary(&patches)));
        }
        Err(err) => {
            tracing::warn!(error = %err, intent_id, "gate-path effect_policy enforcement failed; folded into turn context");
            facts.push(format!("[scene_policy]Scene mechanic {intent_id} consequences could NOT be enforced: {err}. Narrate without inventing mechanical effects.[/scene_policy]"));
        }
    }
}

/// 找回盖章绑定并执行。先按当前场景语义（与盖章时一致）；找不到再全场景精确
/// id 扫——绑定在开 gate 时已经过 resolve_scene_intent 校验，等待期间场景漂移
/// 不应让模组既定后果蒸发（这是恢复已验证的绑定，非 tool-call 时的跨场景错绑）。
async fn locate_and_apply(
    engine: &RuntimeEngine,
    request: &ContextRequest,
    current_scene_id: Option<&str>,
    contract: &CheckContract,
    intent_id: &str,
    success: bool,
    ledger: &mut TurnLedger,
) -> Result<Vec<StatePatch>> {
    let mid = request.module_id.as_deref().or(contract.module_id.as_deref())
        .ok_or_else(|| anyhow::anyhow!("no module bound to session or contract"))?;
    let graph = engine.db.load_module_graph(mid).await?
        .ok_or_else(|| anyhow::anyhow!("module graph not loaded: {mid}"))?;
    let intent = crate::scene_policy::find_scene_intent(&graph, current_scene_id, intent_id)
        .or_else(|| graph.scenes.iter().flat_map(|s| &s.scene_mechanics).find(|i| i.intent_id == intent_id))
        .ok_or_else(|| anyhow::anyhow!("scene mechanic not found in module graph: {intent_id}"))?
        .clone();
    crate::scene_policy::apply_effect_policy(engine, request, &intent, success, ledger).await
}

/// 合成 gate 测试夹具（gate/turn_loop 两处单测共用；对齐 trpg-agent
/// gm_loop.rs sample_check 的字段全集）。
#[cfg(test)]
pub(crate) mod fixtures {
    use chrono::Utc;
    use serde_json::json;
    use trpg_model::{ActorKind, ActorRef, CheckContract, CheckResultRecord, CheckStakes, CheckTargetModel, DiceRollRecord, OppositionModel, RollAuthority, RollDisclosurePolicy, RollVisibility, RulingConfidence, RulingStatus};

    pub(crate) fn gate_contract(check_id: &str) -> CheckContract {
        CheckContract {
            check_id: check_id.into(), session_id: "s".into(), turn_id: "t".into(), ruleset_id: "rs".into(), module_id: None,
            initiator: ActorRef { actor_id: "pc.current".into(), actor_kind: ActorKind::PlayerCharacter, display_name: Some("PC".into()) },
            target_actor: None, opposition: OppositionModel::NoMechanicalOpposition,
            action_summary: "climb".into(), intent_kind: "athletics".into(), check_label: "Climb".into(),
            dice_expression: "1d100".into(), modifiers: vec![],
            target: CheckTargetModel::StaticNumber { value: 50, label: "skill".into() },
            tested_parameter: None, opponent_tested_parameter: None, actor_snapshot_ids: vec![], source_refs: vec![], learned_packet_ids: vec![],
            roll_visibility: RollVisibility::PublicGmRoll, roll_authority: RollAuthority::System,
            disclosure: RollDisclosurePolicy::for_visibility(RollVisibility::PublicGmRoll),
            stakes: CheckStakes { before_roll_public: "risky".into(), success_public: "ok".into(), failure_public: "bad".into(), critical_public: None, fumble_public: None, success_patches_allowed: vec![], failure_patches_allowed: vec![], irreversible: false },
            confidence: RulingConfidence::High, ruling_status: RulingStatus::SourceBacked, advice_refs: vec![], expires_at_turn: Some("t".into()),
        }
    }

    pub(crate) fn gate_result(check_id: &str) -> CheckResultRecord {
        let roll = DiceRollRecord { roll_id: format!("roll_{check_id}"), session_id: "s".to_string(), turn_id: "t".to_string(), check_id: Some(check_id.to_string()), roller_kind: ActorKind::PlayerCharacter, roller_id: Some("pc.current".to_string()), visibility: RollVisibility::PublicGmRoll, expression: "1d100".to_string(), result: json!({"total": 27}), seed_commitment: "seed".to_string(), revealed_at: None, created_at: Utc::now() };
        CheckResultRecord { check_id: check_id.to_string(), roll, outcome: json!({"success": true}), committed_patches: vec![], created_at: Utc::now() }
    }

    pub(crate) fn gate_request(module_id: Option<&str>) -> trpg_model::ContextRequest {
        trpg_model::ContextRequest { ruleset_id: "rs".to_string(), module_id: module_id.map(str::to_string), session_id: "s".to_string(), turn_id: "t".to_string(), viewer: trpg_model::VisibilityProfile::gm(), token_budget: trpg_model::TokenBudget::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{gate_contract, gate_request, gate_result};
    use super::*;
    use serde_json::json;
    use sqlx::postgres::PgPoolOptions;
    use std::sync::Mutex;
    use trpg_db::Db;

    fn engine() -> RuntimeEngine {
        let pool = PgPoolOptions::new().connect_lazy("postgres://chatrpg:chatrpg@localhost:54347/chatrpg").expect("lazy pool");
        RuntimeEngine::new(Db { pool })
    }

    #[tokio::test]
    async fn bare_roll_reply_enters_gate_resolution() {
        // fix 1 单元面：裸 "roll"（parse_roll_text → None）必须进结算路径，
        // 产物入账 + gate 事实入通道。
        let called = Arc::new(Mutex::new(false));
        let flag = called.clone();
        let resolver: GateResolverFn = Arc::new(move |input| { assert_eq!(input, "roll"); *flag.lock().unwrap() = true; Some(Ok((gate_contract("check_gate"), gate_result("check_gate")))) });
        let (mut ledger, mut facts) = (TurnLedger::new(), Vec::new());
        resolve_pending_gate(&engine(), Some(&resolver), &gate_request(None), None, "roll", &mut ledger, &mut facts).await;
        assert!(*called.lock().unwrap(), "gate resolution path was not taken for bare roll reply");
        assert_eq!(ledger.snapshot().check_results.len(), 1);
        assert!(facts.iter().any(|f| f.contains("Resolved pending check check_gate")));
    }

    #[tokio::test]
    async fn non_roll_reply_never_touches_gate_resolution() {
        // 守卫另一侧：普通叙事输入既非骰值也非代掷请求 ⇒ 结算路径不进入。
        let called = Arc::new(Mutex::new(false));
        let flag = called.clone();
        let resolver: GateResolverFn = Arc::new(move |_| { *flag.lock().unwrap() = true; None });
        let (mut ledger, mut facts) = (TurnLedger::new(), Vec::new());
        resolve_pending_gate(&engine(), Some(&resolver), &gate_request(None), None, "我推开加油站的门走进去。", &mut ledger, &mut facts).await;
        assert!(!*called.lock().unwrap());
        assert!(facts.is_empty());
    }

    // C7 纯函数面：盖章解析是 stamp_scene_intent 的逆操作。
    #[test]
    fn scene_mechanic_ref_recovers_stamp() {
        let refs = vec!["gm_agent.roll_check".to_string(), "scene_mechanic:cut_cable".to_string()];
        assert_eq!(scene_mechanic_ref(&refs), Some("cut_cable"));
        assert_eq!(scene_mechanic_ref(&["gm_agent.roll_check".to_string()]), None);
    }

    fn stamped_resolver(outcome: serde_json::Value) -> GateResolverFn {
        Arc::new(move |_| {
            let mut contract = gate_contract("check_gate");
            contract.advice_refs.push("scene_mechanic:cut_cable".to_string());
            let mut result = gate_result("check_gate");
            result.outcome = outcome.clone();
            Some(Ok((contract, result)))
        })
    }

    #[tokio::test]
    async fn gate_settlement_attempts_effect_policy_for_stamped_contract() {
        // C7 红线：盖章契约结算后必须尝试 effect_policy。此处 request 无模组 →
        // 执行失败折成可观测 [scene_policy] 事实（fail-closed 不静默、不炸回合）。
        let resolver = stamped_resolver(json!({"success": true}));
        let (mut ledger, mut facts) = (TurnLedger::new(), Vec::new());
        resolve_pending_gate(&engine(), Some(&resolver), &gate_request(None), None, "roll", &mut ledger, &mut facts).await;
        let fact = facts.iter().find(|f| f.contains("[scene_policy]")).expect("stamped contract must surface a scene_policy fact after settlement");
        assert!(fact.contains("cut_cable"), "fact must name the intent: {fact}");
    }

    #[tokio::test]
    async fn gate_settlement_without_stamp_adds_no_scene_policy_fact() {
        // 无盖章的普通 gate：结算照旧，绝不多出 scene_policy 动作。
        let resolver: GateResolverFn = Arc::new(|_| Some(Ok((gate_contract("check_gate"), gate_result("check_gate")))));
        let (mut ledger, mut facts) = (TurnLedger::new(), Vec::new());
        resolve_pending_gate(&engine(), Some(&resolver), &gate_request(None), None, "roll", &mut ledger, &mut facts).await;
        assert!(facts.iter().any(|f| f.contains("Resolved pending check")));
        assert!(!facts.iter().any(|f| f.contains("[scene_policy]")), "unstamped gate must not trigger scene policy: {facts:?}");
    }

    #[tokio::test]
    async fn gate_settlement_without_success_field_skips_policy() {
        // fail-closed：outcome 无 success 布尔 → 不执行 effect_policy，折 skip 事实
        //（对齐 run_policy_after_settlement 的同款守卫）。
        let resolver = stamped_resolver(json!({}));
        let (mut ledger, mut facts) = (TurnLedger::new(), Vec::new());
        resolve_pending_gate(&engine(), Some(&resolver), &gate_request(None), None, "roll", &mut ledger, &mut facts).await;
        let fact = facts.iter().find(|f| f.contains("[scene_policy]")).expect("skip must still be observable");
        assert!(fact.contains("no success"), "skip fact must state the reason: {fact}");
    }
}
