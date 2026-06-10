use crate::ledger::TurnLedger;
use crate::tools::{AwaitingPlayerRoll, GmTool, ToolCtx, ToolError, ToolOutput, ToolSpec};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use trpg_agent::make_pending_check;
use trpg_model::*;
use uuid::Uuid;

#[derive(Debug, Clone, Deserialize)]
pub struct OpposedArgs {
    pub npc_id: String,
    pub opponent_parameter: String,
    /// 防御参数所在桶（schema default "skills"——默认值放 schema/serde 层，不在代码里硬编码语义）。
    #[serde(default = "default_skills_bucket")]
    pub bucket: String,
}

fn default_skills_bucket() -> String { "skills".to_string() }

#[derive(Debug, Clone, Deserialize)]
pub struct RollCheckArgs {
    pub check_label: String,
    pub tested_parameter: String,
    pub actor_id: Option<String>,
    pub opposed: Option<OpposedArgs>,
    #[serde(default = "default_public")]
    pub visibility: String,
    pub intent_kind: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StakesArgs { pub before: String, pub success: String, pub failure: String }

#[derive(Debug, Clone, Deserialize)]
pub struct RequestPlayerRollArgs {
    pub check_label: String,
    pub tested_parameter: String,
    pub stakes: StakesArgs,
    #[serde(default = "default_public")]
    pub visibility: String,
}

fn default_public() -> String { "public".to_string() }

pub fn parse_roll_check_args(value: Value) -> Result<RollCheckArgs> {
    let args: RollCheckArgs = serde_json::from_value(value)
        .map_err(|e| ToolError::recoverable("invalid_arguments", format!("roll_check arguments invalid: {e}"), Some("Provide check_label and tested_parameter.".to_string())))?;
    if args.tested_parameter.trim().is_empty() {
        return Err(ToolError::recoverable("invalid_arguments", "tested_parameter is required", Some("Bind the check to the real actor parameter being tested.".to_string())));
    }
    Ok(args)
}

fn parse_player_args(value: Value) -> Result<RequestPlayerRollArgs> {
    let args: RequestPlayerRollArgs = serde_json::from_value(value)
        .map_err(|e| ToolError::recoverable("invalid_arguments", format!("request_player_roll arguments invalid: {e}"), Some("Provide check_label, tested_parameter, stakes, and visibility.".to_string())))?;
    if args.tested_parameter.trim().is_empty() {
        return Err(ToolError::recoverable("invalid_arguments", "tested_parameter is required", Some("Bind the pending roll to the actor parameter.".to_string())));
    }
    Ok(args)
}

fn visibility_for_system(s: &str) -> Result<RollVisibility> {
    match s.trim().to_ascii_lowercase().as_str() {
        "public" => Ok(RollVisibility::PublicGmRoll),
        "secret" => Ok(RollVisibility::PrivateGmRoll),
        other => Err(ToolError::recoverable("invalid_arguments", format!("invalid visibility: {other}"), Some("Use public or secret.".to_string()))),
    }
}

pub fn build_check_contract_for_args(session_id: &str, turn_id: &str, ruleset_id: &str, module_id: Option<&str>, args: &RollCheckArgs, dice: &str) -> Result<CheckContract> {
    let visibility = visibility_for_system(&args.visibility)?;
    let actor_id = args.actor_id.clone().unwrap_or_else(|| "pc.current".to_string());
    Ok(CheckContract {
        check_id: format!("check_{}", Uuid::new_v4().simple()),
        session_id: session_id.to_string(),
        turn_id: turn_id.to_string(),
        ruleset_id: ruleset_id.to_string(),
        module_id: module_id.map(str::to_string),
        initiator: ActorRef { actor_id, actor_kind: ActorKind::PlayerCharacter, display_name: Some("current PC".to_string()) },
        target_actor: None,
        opposition: OppositionModel::NoMechanicalOpposition,
        action_summary: args.check_label.chars().take(500).collect(),
        intent_kind: args.intent_kind.clone().unwrap_or_else(|| "agent_selected_check".to_string()),
        check_label: args.check_label.clone(),
        dice_expression: dice.to_string(),
        modifiers: vec![],
        target: CheckTargetModel::UnknownUntilLookup,
        tested_parameter: Some(TestedParameter { domain: None, key: args.tested_parameter.clone(), label: args.tested_parameter.clone() }),
        opponent_tested_parameter: None,
        actor_snapshot_ids: vec![],
        source_refs: vec![],
        learned_packet_ids: vec![],
        roll_visibility: visibility,
        roll_authority: RollAuthority::System,
        disclosure: RollDisclosurePolicy::for_visibility(visibility),
        stakes: CheckStakes {
            before_roll_public: format!("A {} check is required; its result determines the immediate consequence.", args.check_label),
            success_public: format!("The {} check succeeds.", args.check_label),
            failure_public: format!("The {} check fails and a cost follows.", args.check_label),
            critical_public: None,
            fumble_public: None,
            success_patches_allowed: vec![],
            failure_patches_allowed: vec![],
            irreversible: false,
        },
        confidence: RulingConfidence::Medium,
        ruling_status: RulingStatus::Provisional,
        advice_refs: vec!["gm_agent.roll_check".to_string()],
        expires_at_turn: Some(turn_id.to_string()),
    })
}

pub fn build_player_contract_for_args(session_id: &str, turn_id: &str, ruleset_id: &str, module_id: Option<&str>, args: &RequestPlayerRollArgs, dice: &str) -> Result<CheckContract> {
    let mut sys_args = RollCheckArgs { check_label: args.check_label.clone(), tested_parameter: args.tested_parameter.clone(), actor_id: Some("pc.current".to_string()), opposed: None, visibility: args.visibility.clone(), intent_kind: Some("player_roll_requested".to_string()) };
    sys_args.visibility = "public".to_string();
    let mut c = build_check_contract_for_args(session_id, turn_id, ruleset_id, module_id, &sys_args, dice)?;
    c.roll_visibility = RollVisibility::PlayerRollRequired;
    c.roll_authority = RollAuthority::Player;
    c.disclosure = RollDisclosurePolicy::for_visibility(RollVisibility::PlayerRollRequired);
    c.stakes.before_roll_public = args.stakes.before.clone();
    c.stakes.success_public = args.stakes.success.clone();
    c.stakes.failure_public = args.stakes.failure.clone();
    Ok(c)
}

async fn kernel_dice(ctx: &ToolCtx<'_>) -> Result<String> {
    let kernel = ctx.engine.db.load_rule_kernel(&ctx.request.ruleset_id).await?;
    let dice = kernel
        .and_then(|k| k.dice_core.get("dice").and_then(Value::as_str).map(str::to_string))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ToolError::recoverable("missing_kernel_dice", "rule kernel has no dice_core.dice", Some("Call retrieve_rules for a source-backed procedure, ask for a player roll, or narrate without a mechanical roll.".to_string())))?;
    Ok(dice)
}

pub struct RollCheckTool;

#[async_trait]
impl GmTool for RollCheckTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec { name: "roll_check", schema: json!({"type":"function","function":{"name":"roll_check","description":"Execute a source-backed system roll. tested_parameter is mandatory.","parameters":{"type":"object","properties":{"check_label":{"type":"string"},"tested_parameter":{"type":"string"},"actor_id":{"type":"string"},"opposed":{"type":"object","properties":{"npc_id":{"type":"string"},"opponent_parameter":{"type":"string"},"bucket":{"type":"string","default":"skills"}},"required":["npc_id","opponent_parameter"]},"visibility":{"type":"string","enum":["public","secret"]},"intent_kind":{"type":"string"}},"required":["check_label","tested_parameter"]}}}) }
    }

    async fn call(&self, ctx: &ToolCtx<'_>, ledger: &mut TurnLedger, args: Value) -> Result<ToolOutput> {
        let args = parse_roll_check_args(args)?;
        let dice = kernel_dice(ctx).await?;
        let mut contract = build_check_contract_for_args(&ctx.request.session_id, &ctx.request.turn_id, &ctx.request.ruleset_id, ctx.request.module_id.as_deref(), &args, &dice)?;
        if let Some(opposed) = &args.opposed {
            let persona = trpg_runtime::npc_synth::NpcPersona { actor_id: opposed.npc_id.clone(), name: opposed.npc_id.clone(), prose: "opposition selected by GM agent".to_string() };
            // fail-closed 不静默：防御参数合成失败 ⇒ 结构化错误让 agent 改道
            // （request_player_roll / 非对抗 roll_check / 纯叙事），绝不带着未
            // 物化的防御值继续 stamp。注意 ensure_npc_parameter 返回
            // Result<Option<Value>>：合成门关（TRPG_NPC_PERSONA_SYNTHESIS=0）
            // 或 NPC 无参数卡时是 Ok(None) 而非 Err——两种形态都必须拦。
            let synthesized = ctx.engine.ensure_npc_parameter(&ctx.request.session_id, &ctx.request.ruleset_id, &persona, &opposed.bucket, &opposed.opponent_parameter, &args.check_label).await;
            match synthesized {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return Err(ToolError::recoverable(
                        "npc_synthesis_unavailable",
                        format!("opponent parameter {}.{} for {} not materialized (synthesis gated off or NPC has no parameter card)", opposed.bucket, opposed.opponent_parameter, opposed.npc_id),
                        Some("Retry without `opposed`, use request_player_roll, or narrate without a contested roll.".to_string()),
                    ));
                }
                Err(err) => {
                    return Err(ToolError::recoverable(
                        "npc_synthesis_unavailable",
                        format!("could not materialize opponent parameter {}.{} for {}: {err}", opposed.bucket, opposed.opponent_parameter, opposed.npc_id),
                        Some("Retry without `opposed`, use request_player_roll, or narrate without a contested roll.".to_string()),
                    ));
                }
            }
            trpg_runtime::stamp_opposed_check(&mut contract, &persona, &opposed.bucket, &opposed.opponent_parameter);
        }
        // 契约落 check_contracts 表（对齐旧路径 persist_agent_plan 的 db.insert_check_contract；
        // 只入内存 ledger 的话 Task 11 验收 SQL 对 agent 路径恒为空）。
        ctx.engine.db.insert_check_contract(&contract, "created").await?;
        ledger.record_contract(&contract);
        let exec = ctx.engine.execute_system_roll_bundle(&ctx.request.session_id, &ctx.request.turn_id, &contract).await?;
        ledger.record_execution(&exec);
        Ok(ToolOutput::ok(json!({
            "check_id": contract.check_id,
            "check_label": contract.check_label,
            "roll_policy": exec.roll_policy,
            "outcome": exec.primary.outcome,
            "primary_roll": exec.primary.roll.result,
            "committed_patch_count": exec.primary.committed_patches.len(),
            "followup_count": exec.followups.len()
        })))
    }
}

pub struct RequestPlayerRollTool;

#[async_trait]
impl GmTool for RequestPlayerRollTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec { name: "request_player_roll", schema: json!({"type":"function","function":{"name":"request_player_roll","description":"Open an InteractionGate and wait for the player to roll personally.","parameters":{"type":"object","properties":{"check_label":{"type":"string"},"tested_parameter":{"type":"string"},"stakes":{"type":"object","properties":{"before":{"type":"string"},"success":{"type":"string"},"failure":{"type":"string"}},"required":["before","success","failure"]},"visibility":{"type":"string","enum":["public","secret"]}},"required":["check_label","tested_parameter","stakes"]}}}) }
    }

    async fn call(&self, ctx: &ToolCtx<'_>, ledger: &mut TurnLedger, args: Value) -> Result<ToolOutput> {
        let args = parse_player_args(args)?;
        let dice = kernel_dice(ctx).await?;
        let contract = build_player_contract_for_args(&ctx.request.session_id, &ctx.request.turn_id, &ctx.request.ruleset_id, ctx.request.module_id.as_deref(), &args, &dice)?;
        // 对齐 persist_agent_plan 既有行为：插新 pending 前先作废旧 open gate，
        // 防同会话累积多个 open pending check。
        ctx.engine.db.cancel_open_pending_checks_for_session(&ctx.request.session_id, PendingCheckStatus::Superseded).await?;
        ctx.engine.db.insert_check_contract(&contract, "created").await?;
        let pending = make_pending_check(&contract);
        let gate = InteractionGate::from_pending_check(&pending);
        ctx.engine.db.insert_pending_check(&pending).await?;
        ctx.engine.db.insert_interaction_gate(&gate).await?;
        ledger.record_contract(&contract);
        ledger.record_gate(&gate);
        Ok(ToolOutput::awaiting(json!({"check_id": contract.check_id, "prompt_public": pending.prompt_public}), AwaitingPlayerRoll { check_id: contract.check_id, prompt_public: pending.prompt_public }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use trpg_model::{CheckTargetModel, RollAuthority, RollVisibility};

    #[test]
    fn roll_args_require_tested_parameter() {
        let err = parse_roll_check_args(json!({"check_label":"Scan","visibility":"public"})).unwrap_err();
        assert!(err.to_string().contains("invalid_arguments"));
    }

    #[test]
    fn build_contract_binds_tested_parameter_and_visibility() {
        let args = RollCheckArgs { check_label: "Stealth".to_string(), tested_parameter: "Stealth".to_string(), actor_id: Some("pc.current".to_string()), opposed: None, visibility: "secret".to_string(), intent_kind: Some("stealth_or_dangerous_movement".to_string()) };
        let contract = build_check_contract_for_args("s", "t", "cyberpunk_red", Some("homecoming"), &args, "1d10").unwrap();
        assert_eq!(contract.tested_parameter.as_ref().unwrap().key, "Stealth");
        assert_eq!(contract.roll_visibility, RollVisibility::PrivateGmRoll);
        assert_eq!(contract.roll_authority, RollAuthority::System);
        // CheckTargetModel 是带数据枚举，没有 as_str()，必须用 matches! 断言。
        assert!(matches!(contract.target, CheckTargetModel::UnknownUntilLookup));
    }

    #[test]
    fn request_player_roll_contract_uses_player_authority() {
        let args = RequestPlayerRollArgs { check_label: "Athletics".to_string(), tested_parameter: "Athletics".to_string(), stakes: StakesArgs { before: "The jump is risky.".to_string(), success: "You land cleanly.".to_string(), failure: "You fall short.".to_string() }, visibility: "public".to_string() };
        let contract = build_player_contract_for_args("s", "t", "sw2_5", None, &args, "2d6").unwrap();
        assert_eq!(contract.roll_visibility, RollVisibility::PlayerRollRequired);
        assert_eq!(contract.roll_authority, RollAuthority::Player);
        assert_eq!(contract.stakes.before_roll_public, "The jump is risky.");
    }
}
