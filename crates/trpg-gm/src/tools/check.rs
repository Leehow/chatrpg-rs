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
    #[serde(default)] pub tested_parameter: String,        // 一期 required → 二期：无 mechanic_id 时仍必填（运行时校验）
    pub actor_id: Option<String>,
    pub opposed: Option<OpposedArgs>,
    #[serde(default = "default_public")]
    pub visibility: String,
    pub intent_kind: Option<String>,
    #[serde(default)] pub mechanic_id: Option<String>,
    #[serde(default)] pub scene_mechanic_id: Option<String>, // 字段+schema 本任务落；消费逻辑归 C4
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
    #[serde(default)] pub scene_mechanic_id: Option<String>, // C7：gate 路径盖章入口
}

fn default_public() -> String { "public".to_string() }

pub fn parse_roll_check_args(value: Value) -> Result<RollCheckArgs> {
    let args: RollCheckArgs = serde_json::from_value(value)
        .map_err(|e| ToolError::recoverable("invalid_arguments", format!("roll_check arguments invalid: {e}"), Some("Provide check_label and tested_parameter.".to_string())))?;
    // 无 mechanic_id 时 tested_parameter 仍必填（一期语义保持）；
    // 有 mechanic_id 时延后到 call 内目录继承之后再校验。
    if args.mechanic_id.is_none() && args.tested_parameter.trim().is_empty() {
        return Err(ToolError::recoverable("invalid_arguments", "tested_parameter is required", Some("Bind the check to the real actor parameter being tested.".to_string())));
    }
    Ok(args)
}

/// 纯函数：目录继承。显式 args 优先（目录是知识不是枷锁）：
/// - args.tested_parameter 为空且 entry.tested_parameter 为 Some → 写入 args；
/// - 返回 entry.procedure 首个 ProcedureStep::Roll 的 dice（Some 时调用方用它替代
///   kernel_dice_expr 缺省；显式骰式本工具本无入参，不存在覆盖冲突）。
pub fn apply_mechanic_inheritance(args: &mut RollCheckArgs, entry: &MechanicEntry) -> Option<String> {
    if args.tested_parameter.trim().is_empty() {
        if let Some(param) = entry.tested_parameter.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            args.tested_parameter = param.to_string();
        }
    }
    entry.procedure.iter().find_map(|step| match step {
        ProcedureStep::Roll { dice, .. } => Some(dice.clone()),
        _ => None,
    })
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
    let mut sys_args = RollCheckArgs { check_label: args.check_label.clone(), tested_parameter: args.tested_parameter.clone(), actor_id: Some("pc.current".to_string()), opposed: None, visibility: args.visibility.clone(), intent_kind: Some("player_roll_requested".to_string()), mechanic_id: None, scene_mechanic_id: None };
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

/// PURE：政策转换——玩家亲掷契约 → 系统代掷（复用 runtime normalize 单一原语；
/// 政策关闭时 normalize 原样返回），advice_refs 打 provenance 标记（库面可审计）。
fn convert_player_gate_to_system(contract: &CheckContract) -> CheckContract {
    let mut c = trpg_runtime::normalize_contract_for_system_roll(contract);
    c.advice_refs.push("gm_agent.request_player_roll:converted:system_rolls_visible".to_string());
    c
}

/// 纯辅助：kernel 缺省骰式。B3 小重构——call 顶部 load_rule_kernel 一次后复用
/// （band_semantics 也要同一 kernel），不再每处各查一遍 db。
fn kernel_dice_expr(kernel: Option<&RuleKernel>) -> Result<String> {
    kernel
        .and_then(|k| k.dice_core.get("dice").and_then(Value::as_str).map(str::to_string))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ToolError::recoverable("missing_kernel_dice", "rule kernel has no dice_core.dice", Some("Call retrieve_rules for a source-backed procedure, ask for a player roll, or narrate without a mechanical roll.".to_string())).into())
}

pub struct RollCheckTool;

#[async_trait]
impl GmTool for RollCheckTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec { name: "roll_check", schema: json!({"type":"function","function":{"name":"roll_check","description":"Execute a source-backed system roll. tested_parameter is mandatory unless inherited via mechanic_id.","parameters":{"type":"object","properties":{"check_label":{"type":"string"},"tested_parameter":{"type":"string"},"actor_id":{"type":"string"},"opposed":{"type":"object","properties":{"npc_id":{"type":"string"},"opponent_parameter":{"type":"string"},"bucket":{"type":"string","default":"skills"}},"required":["npc_id","opponent_parameter"]},"visibility":{"type":"string","enum":["public","secret"]},"intent_kind":{"type":"string"},"mechanic_id":{"type":"string"},"scene_mechanic_id":{"type":"string","description":"intent_id from the current scene's mechanic-intents block; inherits the module-stated tested_parameter/difficulty and the engine enforces the stated consequences after settlement"}},"required":["check_label"]}}}) }
    }

    async fn call(&self, ctx: &ToolCtx<'_>, ledger: &mut TurnLedger, args: Value) -> Result<ToolOutput> {
        let mut args = parse_roll_check_args(args)?;
        // kernel 单次加载（B3）：目录继承、缺省骰式、band 语义共用同一份。
        let kernel = ctx.engine.db.load_rule_kernel(&ctx.request.ruleset_id).await?;
        // ① mechanic_id 给定 → 查目录继承（目录是知识不是枷锁：显式 args 优先）。
        let mut inherited_dice: Option<String> = None;
        if let Some(mechanic_id) = args.mechanic_id.clone() {
            let entry = kernel
                .as_ref()
                .and_then(|k| crate::tools::mechanic::find_mechanic(&k.mechanics_catalog, &mechanic_id));
            let Some(entry) = entry else {
                return Err(ToolError::recoverable(
                    "mechanic_not_found",
                    format!("mechanic not found in catalog: {mechanic_id}"),
                    Some("Check the BP1 mechanics index for valid ids, or use retrieve_rules.".to_string()),
                ));
            };
            inherited_dice = apply_mechanic_inheritance(&mut args, entry);
        }
        // ② 继承后 tested_parameter 仍空 → invalid_arguments（一期语义保持）。
        if args.tested_parameter.trim().is_empty() {
            return Err(ToolError::recoverable("invalid_arguments", "tested_parameter is required", Some("Bind the check to the real actor parameter being tested.".to_string())));
        }
        // C4：scene_mechanic_id → 当前场景 intent（解析/错误码细节在 scene_policy）。
        let scene_intent = crate::scene_policy::resolve_scene_intent(ctx.engine, ctx.request, ctx.state.scene_id.as_deref(), args.scene_mechanic_id.as_deref()).await?;
        // 显式 args 优先（意图是知识不是枷锁）：与 intent.tested_parameter 不一致用 args 值并标记（可观测）。
        let parameter_overridden = scene_intent.as_ref().is_some_and(|i| !args.tested_parameter.trim().is_empty() && args.tested_parameter.trim() != i.tested_parameter.trim());
        // 继承骰式为 Some 用之；None 仍走 kernel 缺省（missing_kernel_dice 语义不变）。
        let dice = match inherited_dice {
            Some(d) => d,
            None => kernel_dice_expr(kernel.as_ref())?,
        };
        let mut contract = build_check_contract_for_args(&ctx.request.session_id, &ctx.request.turn_id, &ctx.request.ruleset_id, ctx.request.module_id.as_deref(), &args, &dice)?;
        // ③ 结构化引用走 advice_refs，不动 CheckContract 5 处签名（契约 §6 注记）。
        if let Some(mechanic_id) = &args.mechanic_id {
            contract.advice_refs.push(format!("mechanic:{mechanic_id}"));
        }
        // C4：盖章收口 stamp_scene_intent（难度→target + scene_mechanic: 引用进 advice_refs，护栏 §3.5.1）。
        crate::scene_policy::stamp_scene_intent(&mut contract, scene_intent.as_ref());
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
        // 结算尾段（落库入账 → 系统掷骰 → band 语义 → effect_policy → 债务带回）
        // 收口 settle.rs 单点，与 request_player_roll 的政策转换路径共用。
        let output = crate::tools::settle::settle_system_check(ctx, ledger, kernel.as_ref(), scene_intent.as_ref(), parameter_overridden, &contract).await?;
        // J2 修复：带 mechanic_id 的结算成功 ⇒ 匹配该机制的 open dues 确定性置
        // resolved（内存+DB）——处理过的债务不再要求 GM 额外 waive。落库失败
        // `let _ =` 吞（内存侧已清，下回合 leftover 重捞由 DB 状态兜底）。
        if let (Some(mechanic_id), Some(cell)) = (args.mechanic_id.as_deref(), ctx.obligations) {
            let resolved = { cell.lock().unwrap_or_else(|p| p.into_inner()).resolve_dues_for_mechanic(mechanic_id) };
            for due_id in &resolved {
                let _ = ctx.engine.db.update_mechanic_due_status(due_id, "resolved", Some("settled via roll_check"), None).await;
            }
        }
        Ok(output)
    }
}

pub struct RequestPlayerRollTool;

#[async_trait]
impl GmTool for RequestPlayerRollTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec { name: "request_player_roll", schema: json!({"type":"function","function":{"name":"request_player_roll","description":"Open an InteractionGate and wait for the player to roll personally. Under a system-rolls table dice policy the engine instead rolls on the player's behalf immediately and returns the settled result; narrate the stakes and the outcome without asking the player to roll.","parameters":{"type":"object","properties":{"check_label":{"type":"string"},"tested_parameter":{"type":"string"},"stakes":{"type":"object","properties":{"before":{"type":"string"},"success":{"type":"string"},"failure":{"type":"string"}},"required":["before","success","failure"]},"visibility":{"type":"string","enum":["public","secret"]},"scene_mechanic_id":{"type":"string","description":"intent_id from the current scene's mechanic-intents block; inherits the module-stated difficulty and the engine enforces the stated consequences after the player's roll settles"}},"required":["check_label","tested_parameter","stakes"]}}}) }
    }

    async fn call(&self, ctx: &ToolCtx<'_>, ledger: &mut TurnLedger, args: Value) -> Result<ToolOutput> {
        let args = parse_player_args(args)?;
        let kernel = ctx.engine.db.load_rule_kernel(&ctx.request.ruleset_id).await?;
        let dice = kernel_dice_expr(kernel.as_ref())?;
        // C7：玩家亲掷路径同样盖章（解析失败在 cancel/insert 副作用前直接报错）；effect_policy 下回合 gate 结算后经引用恢复执行（spec §6）。
        let scene_intent = crate::scene_policy::resolve_scene_intent(ctx.engine, ctx.request, ctx.state.scene_id.as_deref(), args.scene_mechanic_id.as_deref()).await?;
        let mut contract = build_player_contract_for_args(&ctx.request.session_id, &ctx.request.turn_id, &ctx.request.ruleset_id, ctx.request.module_id.as_deref(), &args, &dice)?;
        crate::scene_policy::stamp_scene_intent(&mut contract, scene_intent.as_ref());
        // 桌面骰权政策守卫（对齐 legacy CLI/API 路径与 NarrationVerifier 的
        // ManualRollRequest 红线）：system_rolls_visible 下玩家从不手掷——此处若
        // 仍开 gate，下一回合的叙事输入不是骰值应答，gate 永不结算只会被新 gate
        // 作废，战斗冻在 awaiting_player_roll、零掷骰落账。转系统代掷当场结算
        // （normalize 原语是 runtime 单一事实源），盖章后果照常强制执行，
        // 戏剧 stakes 仍由 agent 叙入散文。政策关（真人摇骰桌）gate 路径原样。
        if trpg_runtime::system_rolls_visible_policy() {
            let contract = convert_player_gate_to_system(&contract);
            let parameter_overridden = scene_intent.as_ref().is_some_and(|i| args.tested_parameter.trim() != i.tested_parameter.trim());
            let mut output = crate::tools::settle::settle_system_check(ctx, ledger, kernel.as_ref(), scene_intent.as_ref(), parameter_overridden, &contract).await?;
            if let Some(obj) = output.result.as_object_mut() {
                obj.insert("player_roll_converted".into(), json!("table dice policy system_rolls_visible: the engine rolled on the player's behalf; narrate the stakes and this settled result, do not ask the player to roll"));
            }
            return Ok(output);
        }
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
    fn roll_check_without_mechanic_still_requires_tested_parameter() {
        let err = parse_roll_check_args(json!({"check_label":"Scan","visibility":"public"})).unwrap_err();
        assert!(err.to_string().contains("invalid_arguments"));
    }

    #[test]
    fn mechanic_inheritance_fills_tested_parameter_and_dice() {
        let entry = MechanicEntry {
            id: "coc.skill.jump".to_string(),
            name: "Jump".to_string(),
            tested_parameter: Some("jump".to_string()),
            procedure: vec![ProcedureStep::Roll { dice: "1d100".to_string(), vs: None, note: None }],
            ..Default::default()
        };
        let mut args = RollCheckArgs { check_label: "Jump the chasm".to_string(), tested_parameter: String::new(), actor_id: None, opposed: None, visibility: "public".to_string(), intent_kind: None, mechanic_id: Some("coc.skill.jump".to_string()), scene_mechanic_id: None };
        let dice = apply_mechanic_inheritance(&mut args, &entry);
        assert_eq!(args.tested_parameter, "jump");
        assert_eq!(dice, Some("1d100".to_string()));
    }

    #[test]
    fn explicit_args_beat_catalog() {
        let entry = MechanicEntry {
            id: "coc.skill.jump".to_string(),
            name: "Jump".to_string(),
            tested_parameter: Some("jump".to_string()),
            ..Default::default()
        };
        let mut args = RollCheckArgs { check_label: "Climb the wall".to_string(), tested_parameter: "climb".to_string(), actor_id: None, opposed: None, visibility: "public".to_string(), intent_kind: None, mechanic_id: Some("coc.skill.jump".to_string()), scene_mechanic_id: None };
        apply_mechanic_inheritance(&mut args, &entry);
        assert_eq!(args.tested_parameter, "climb");
    }

    #[test]
    fn build_contract_binds_tested_parameter_and_visibility() {
        let args = RollCheckArgs { check_label: "Stealth".to_string(), tested_parameter: "Stealth".to_string(), actor_id: Some("pc.current".to_string()), opposed: None, visibility: "secret".to_string(), intent_kind: Some("stealth_or_dangerous_movement".to_string()), mechanic_id: None, scene_mechanic_id: None };
        let contract = build_check_contract_for_args("s", "t", "cyberpunk_red", Some("homecoming"), &args, "1d10").unwrap();
        assert_eq!(contract.tested_parameter.as_ref().unwrap().key, "Stealth");
        assert_eq!(contract.roll_visibility, RollVisibility::PrivateGmRoll);
        assert_eq!(contract.roll_authority, RollAuthority::System);
        // CheckTargetModel 是带数据枚举，没有 as_str()，必须用 matches! 断言。
        assert!(matches!(contract.target, CheckTargetModel::UnknownUntilLookup));
    }

    // B3: roll_check 结果 JSON 的 band_semantics 渲染（纯函数级——构造
    // outcome+dice_core 调 band_semantics_line；工具级走真 db 的留 C5 e2e）。
    #[test]
    fn roll_check_result_carries_band_semantics_when_kernel_has_it() {
        let dice_core = json!({"success_bands":[{"id":"hard","label":"困难成功","semantics":"超出常人的表现"}]});
        let outcome = json!({"success": true, "success_tier": "hard"});
        let line = band_semantics_line(&dice_core, &outcome).expect("kernel band semantics must render");
        for seg in ["hard", "困难成功", "超出常人的表现"] {
            assert!(line.contains(seg), "line must contain `{seg}`: {line}");
        }
        // 无 semantics 的 kernel -> None（call 端整键缺省，裸 band 仍在 outcome 里）
        let bare = json!({"success_bands":[{"id":"hard","label":"困难成功"}]});
        assert_eq!(band_semantics_line(&bare, &outcome), None);
    }

    // C4：scene_mechanic_id 字段就位 + 向后兼容（不给 → None）。C7：player 路径同入口。
    #[test]
    fn roll_args_accept_scene_mechanic_id() {
        let args = parse_roll_check_args(json!({"check_label":"Cut the cable","tested_parameter":"brawling","scene_mechanic_id":"x"})).unwrap();
        assert_eq!(args.scene_mechanic_id.as_deref(), Some("x"));
        let args = parse_roll_check_args(json!({"check_label":"Cut the cable","tested_parameter":"brawling"})).unwrap();
        assert!(args.scene_mechanic_id.is_none());
        let p = parse_player_args(json!({"check_label":"c","tested_parameter":"brawling","stakes":{"before":"b","success":"s","failure":"f"},"scene_mechanic_id":"x"})).unwrap();
        assert_eq!(p.scene_mechanic_id.as_deref(), Some("x"));
    }

    #[test]
    fn request_player_roll_contract_uses_player_authority() {
        let args = RequestPlayerRollArgs { check_label: "Athletics".to_string(), tested_parameter: "Athletics".to_string(), stakes: StakesArgs { before: "The jump is risky.".to_string(), success: "You land cleanly.".to_string(), failure: "You fall short.".to_string() }, visibility: "public".to_string(), scene_mechanic_id: None };
        let contract = build_player_contract_for_args("s", "t", "sw2_5", None, &args, "2d6").unwrap();
        assert_eq!(contract.roll_visibility, RollVisibility::PlayerRollRequired);
        assert_eq!(contract.roll_authority, RollAuthority::Player);
        assert_eq!(contract.stakes.before_roll_public, "The jump is risky.");
    }

    // 桌面骰权政策守卫：system_rolls_visible 下玩家亲掷契约必须转系统代掷
    // （GM agent 战斗冻结修复——gate 开出去叙事输入永不结算）；stakes 与
    // provenance 标记保留。env 显式置位防外部 shell 注入 player 政策时误红。
    #[test]
    fn system_policy_converts_player_gate_to_system_authority() {
        std::env::set_var("TRPG_AGENT_TABLE_DICE_POLICY", "system_rolls_visible");
        let args = RequestPlayerRollArgs { check_label: "Dodge".to_string(), tested_parameter: "dodge".to_string(), stakes: StakesArgs { before: "Claws rake at your throat.".to_string(), success: "You twist away.".to_string(), failure: "It tears into you.".to_string() }, visibility: "public".to_string(), scene_mechanic_id: None };
        let player = build_player_contract_for_args("s", "t", "call_of_cthulhu_7e", None, &args, "1d100").unwrap();
        let converted = convert_player_gate_to_system(&player);
        assert_eq!(converted.roll_authority, RollAuthority::System);
        assert_eq!(converted.roll_visibility, RollVisibility::PublicGmRoll);
        assert_eq!(converted.stakes.before_roll_public, "Claws rake at your throat.");
        assert!(converted.advice_refs.iter().any(|r| r.contains("request_player_roll:converted:system_rolls_visible")), "provenance marker missing: {:?}", converted.advice_refs);
    }
}
