use crate::ledger::TurnLedger;
use anyhow::Result;
use std::sync::Arc;
use trpg_agent::parse_roll_text;
use trpg_model::{CheckContract, CheckResultRecord};
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
    session_id: &str,
    turn_id: &str,
    user_input: &str,
    ledger: &mut TurnLedger,
    resolved_gate_facts: &mut Vec<String>,
) {
    if parse_roll_text(user_input).is_none() && !trpg_runtime::wants_system_roll(user_input) {
        return;
    }
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
        }
        Some(Err(err)) => {
            tracing::warn!(error = %err, session_id = %session_id, "pending check resolution failed; folded into turn context");
            resolved_gate_facts.push(format!("[roll]Pending check resolution failed: {err}. No roll was committed; the pending check is still open. Re-prompt the player for a valid roll reply or continue narratively without mechanical resolution.[/roll]"));
        }
        None => {}
    }
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
    use super::fixtures::{gate_contract, gate_result};
    use super::*;
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
        resolve_pending_gate(&engine(), Some(&resolver), "s", "t", "roll", &mut ledger, &mut facts).await;
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
        resolve_pending_gate(&engine(), Some(&resolver), "s", "t", "我推开加油站的门走进去。", &mut ledger, &mut facts).await;
        assert!(!*called.lock().unwrap());
        assert!(facts.is_empty());
    }
}
