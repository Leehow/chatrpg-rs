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

fn default_skills_bucket() -> String {
    "skills".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct RollCheckArgs {
    pub check_label: String,
    #[serde(default)]
    pub tested_parameter: String, // 一期 required → 二期：无 mechanic_id 时仍必填（运行时校验）
    pub actor_id: Option<String>,
    pub opposed: Option<OpposedArgs>,
    #[serde(default = "default_public")]
    pub visibility: String,
    pub intent_kind: Option<String>,
    #[serde(default)]
    pub mechanic_id: Option<String>,
    #[serde(default)]
    pub scene_mechanic_id: Option<String>, // 字段+schema 本任务落；消费逻辑归 C4
}

#[derive(Debug, Clone, Deserialize)]
pub struct StakesArgs {
    pub before: String,
    pub success: String,
    pub failure: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RequestPlayerRollArgs {
    pub check_label: String,
    pub tested_parameter: String,
    pub stakes: StakesArgs,
    #[serde(default = "default_public")]
    pub visibility: String,
    #[serde(default)]
    pub scene_mechanic_id: Option<String>, // C7：gate 路径盖章入口
}

fn default_public() -> String {
    "public".to_string()
}

pub fn parse_roll_check_args(value: Value) -> Result<RollCheckArgs> {
    let args: RollCheckArgs = serde_json::from_value(value).map_err(|e| {
        ToolError::recoverable(
            "invalid_arguments",
            format!("roll_check arguments invalid: {e}"),
            Some("Provide check_label and tested_parameter.".to_string()),
        )
    })?;
    // 无 mechanic_id 时 tested_parameter 仍必填（一期语义保持）；
    // 有 mechanic_id 时延后到 call 内目录继承之后再校验。
    if args.mechanic_id.is_none() && args.tested_parameter.trim().is_empty() {
        return Err(ToolError::recoverable(
            "invalid_arguments",
            "tested_parameter is required",
            Some("Bind the check to the real actor parameter being tested.".to_string()),
        ));
    }
    Ok(args)
}

/// 纯函数：目录继承。显式 args 优先（目录是知识不是枷锁）：
/// - args.tested_parameter 为空且 entry.tested_parameter 为 Some → 写入 args；
/// - 返回 entry.procedure 首个 ProcedureStep::Roll 的 dice（Some 时调用方用它替代
///   kernel_dice_expr 缺省；显式骰式本工具本无入参，不存在覆盖冲突）。
pub fn apply_mechanic_inheritance(
    args: &mut RollCheckArgs,
    entry: &MechanicEntry,
) -> Option<String> {
    if args.tested_parameter.trim().is_empty() {
        if let Some(param) = entry
            .tested_parameter
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            args.tested_parameter = param.to_string();
        }
    }
    entry.procedure.iter().find_map(|step| match step {
        ProcedureStep::Roll { dice, .. } => Some(dice.clone()),
        _ => None,
    })
}

/// PURE（Phase3 §4.1）：GM 漏填 opposed 时，从对抗预 pass 备好的 binding 注入对抗
/// 参数。显式 args.opposed 优先（GM 自己填对走 GM 的，预 pass 只兜底）；binding=None
/// → 不动（fail-closed，退回单方检定）。注入后续仍经 ensure_npc_parameter（per-param
/// 缓存命中预 pass 已现搓的值）+ stamp_opposed_check，与显式 opposed 同一通路。
fn inject_opposed_binding(
    args: &mut RollCheckArgs,
    binding: Option<&crate::opposed_prepass::OpposedBinding>,
) {
    if args.opposed.is_some() {
        return;
    }
    if let Some(b) = binding {
        args.opposed = Some(OpposedArgs {
            npc_id: b.persona.actor_id.clone(),
            opponent_parameter: b.opponent_parameter.clone(),
            bucket: b.bucket.clone(),
        });
    }
}

/// PURE：GM 漏填 `opposed`/显式目标，但这是一次真实玩家攻击时，给契约备一个
/// 透明的兜底目标 `npc.opposition`（mechanics 已为该 id 预置 provisional HP
/// seeding），让无对抗的 CoC 火器命中骰也能驱动 `record_attack` → 伤害结算。
///
/// 仅在以下全部成立时返回 `Some`：
/// - 发起方是玩家（actor 缺省→`pc.current`），不是 `npc.*`（避免 NPC 自打/串话）；
/// - 没有 `opposed` 绑定（显式对抗自带 target，走 `stamp_opposed_check`）；
/// - label/intent/mechanic 确属攻击（fires/shoots/M1911/firearms/attack/…，
///   或攻击类 mechanic id 如 `*.ranged_and_thrown_attack_resolution`）；
/// - 且 label 不是状态/善后/效果/伤害/先手/装弹/闪避/潜行类（这些是攻击的
///   “后果/序列”而非攻击本身，绝不绑定兜底目标）。
///
/// fail-closed：任一不成立 → `None`（退回无目标，mechanics 端照常拒绝伪造目标）。
/// 注意：只落 `target_actor`，绝不盖 `opponent_tested_parameter`——无对抗形态。
fn fallback_attack_target_actor_for_roll(args: &RollCheckArgs) -> Option<ActorRef> {
    if args.opposed.is_some() {
        return None;
    }
    let actor_id = args
        .actor_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("pc.current");
    if matches!(actor_kind_for_actor_id(actor_id), ActorKind::Npc) {
        return None;
    }
    if !is_genuine_attack_roll(args) {
        return None;
    }
    Some(ActorRef {
        actor_id: "npc.opposition".to_string(),
        actor_kind: ActorKind::Npc,
        display_name: Some("npc.opposition".to_string()),
    })
}

/// PURE：label/intent/tested_parameter/mechanic_id 是否构成一次真实攻击骰。
/// 排除优先再扫纳入（与 mechanics `is_attack_like_check` 同序）：`gunshot` 含
/// `shot`、`attacker` 含 `attack`，必须先把状态/善后/序列类剔掉，再判攻击词。
fn is_genuine_attack_roll(args: &RollCheckArgs) -> bool {
    let mechanic = args
        .mechanic_id
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    let text = format!(
        "{} {} {} {}",
        args.check_label,
        args.intent_kind.as_deref().unwrap_or(""),
        args.tested_parameter,
        mechanic,
    )
    .to_ascii_lowercase();
    const EXCLUDE: &[&str] = &[
        "status",
        "aftermath",
        "aftereffect",
        "recovery",
        "zero hp",
        "zero hit",
        "dying",
        "conscious",
        "consciousness",
        "major wound",
        "minor wound",
        "wound state",
        "gunshot",
        "damage",
        "effect",
        "sanity",
        "chaos",
        "harm",
        "initiative",
        "reload",
        "reloading",
        "dodge",
        "stealth",
        "重伤",
        "昏迷",
        "濒死",
        "失去意识",
        "装弹",
        "换弹",
        "闪避",
        "潜行",
        "先手",
    ];
    if EXCLUDE.iter().any(|n| text.contains(n)) {
        return false;
    }
    if mechanic.contains("attack") {
        return true;
    }
    const INCLUDE: &[&str] = &[
        "attack",
        "counterattack",
        "firearms",
        "handgun",
        "pistol",
        "m1911",
        "shoot",
        "shot",
        "fires",
        "fire",
        "开火",
        "攻击",
        "射击",
        "开枪",
        "手枪",
    ];
    INCLUDE.iter().any(|n| text.contains(n))
}

fn parse_player_args(value: Value) -> Result<RequestPlayerRollArgs> {
    let args: RequestPlayerRollArgs = serde_json::from_value(value).map_err(|e| {
        ToolError::recoverable(
            "invalid_arguments",
            format!("request_player_roll arguments invalid: {e}"),
            Some("Provide check_label, tested_parameter, stakes, and visibility.".to_string()),
        )
    })?;
    if args.tested_parameter.trim().is_empty() {
        return Err(ToolError::recoverable(
            "invalid_arguments",
            "tested_parameter is required",
            Some("Bind the pending roll to the actor parameter.".to_string()),
        ));
    }
    Ok(args)
}

fn visibility_for_system(s: &str) -> Result<RollVisibility> {
    match s.trim().to_ascii_lowercase().as_str() {
        "public" => Ok(RollVisibility::PublicGmRoll),
        "secret" => Ok(RollVisibility::PrivateGmRoll),
        other => Err(ToolError::recoverable(
            "invalid_arguments",
            format!("invalid visibility: {other}"),
            Some("Use public or secret.".to_string()),
        )),
    }
}

fn actor_kind_for_actor_id(actor_id: &str) -> ActorKind {
    if actor_id.trim().to_ascii_lowercase().starts_with("npc.") {
        ActorKind::Npc
    } else {
        ActorKind::PlayerCharacter
    }
}

fn actor_display_for(actor_id: &str, actor_kind: ActorKind) -> String {
    if matches!(actor_kind, ActorKind::Npc) {
        actor_id.to_string()
    } else {
        "current PC".to_string()
    }
}

fn normalize_visibility_for_check_args(
    requested: RollVisibility,
    actor_kind: ActorKind,
    intent_kind: Option<&str>,
) -> RollVisibility {
    if requested == RollVisibility::PrivateGmRoll
        && matches!(actor_kind, ActorKind::PlayerCharacter)
        && !pc_private_check_intent_allowed(intent_kind)
    {
        return RollVisibility::PublicGmRoll;
    }
    requested
}

fn pc_private_check_intent_allowed(intent_kind: Option<&str>) -> bool {
    let Some(intent) = intent_kind.map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };
    let lowered = intent.to_ascii_lowercase();
    lowered.contains("stealth")
        || lowered.contains("hidden")
        || lowered.contains("secret")
        || lowered.contains("passive")
}

fn mechanic_has_roll_step(entry: &MechanicEntry) -> bool {
    entry
        .procedure
        .iter()
        .any(|step| matches!(step, ProcedureStep::Roll { .. }))
}

fn mechanic_should_return_passive(entry: &MechanicEntry) -> bool {
    !mechanic_has_roll_step(entry) && !matches!(entry.kind, MechanicKind::SkillCheck)
}

fn passive_mechanic_result(mechanic_id: &str, entry: &MechanicEntry) -> Value {
    json!({
        "mechanic_id": mechanic_id,
        "mechanic_name": entry.name,
        "passive_mechanic": true,
        "rolled": false,
        "procedure": entry.procedure,
        "source_refs": entry.source_refs,
        "note": "This mechanics-catalog entry has no roll step; apply/narrate its gates without rolling a default die.",
        "narration_instruction": "This is not a check or roll result. Do not wrap it in [roll] and do not say a roll/check succeeded or failed; narrate only the gate/action state."
    })
}

fn bucket_for_active_actor_parameter(parameter: &str) -> &'static str {
    match parameter.trim().to_ascii_lowercase().as_str() {
        "str" | "con" | "siz" | "dex" | "app" | "int" | "pow" | "edu" => "stats",
        "hp" | "hit_points" | "san" | "sanity" | "mp" | "magic_points" | "luck" => "resources",
        _ => "skills",
    }
}

/// Decide whether an active roll must have its NPC tested parameter
/// materialized before any roll or check contract is built. Returns
/// `(actor_id, bucket, parameter)` when the active-NPC synthesis gate applies,
/// or `None` when it does not (no/blank actor, non-NPC actor, or blank
/// parameter). Pure so the fail-closed precondition is unit-testable; the
/// actual `npc_synthesis_unavailable` decision lives in
/// `ensure_active_npc_parameter_if_needed`.
fn active_npc_parameter_gate<'a>(
    actor_id: Option<&'a str>,
    tested_parameter: &'a str,
) -> Option<(&'a str, &'static str, &'a str)> {
    let actor_id = actor_id.map(str::trim).filter(|s| !s.is_empty())?;
    if !matches!(actor_kind_for_actor_id(actor_id), ActorKind::Npc) {
        return None;
    }
    let param = tested_parameter.trim();
    if param.is_empty() {
        return None;
    }
    Some((actor_id, bucket_for_active_actor_parameter(param), param))
}

async fn ensure_active_npc_parameter_if_needed(
    ctx: &ToolCtx<'_>,
    args: &RollCheckArgs,
) -> Result<()> {
    let Some((actor_id, bucket, param)) =
        active_npc_parameter_gate(args.actor_id.as_deref(), &args.tested_parameter)
    else {
        return Ok(());
    };
    let persona = trpg_runtime::npc_synth::NpcPersona {
        actor_id: actor_id.to_string(),
        name: actor_id.to_string(),
        prose: "opposition selected by GM agent".to_string(),
    };
    match ctx
        .engine
        .ensure_npc_parameter(
            &ctx.request.session_id,
            &ctx.request.ruleset_id,
            &persona,
            bucket,
            param,
            &args.check_label,
        )
        .await
    {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(ToolError::recoverable(
            "npc_synthesis_unavailable",
            format!("active NPC parameter {bucket}.{param} for {actor_id} is not materialized"),
            Some(
                "Use a passive ruling or waive the obligation with an audited reason.".to_string(),
            ),
        )
        .into()),
        Err(err) => Err(ToolError::recoverable(
            "npc_synthesis_unavailable",
            format!(
                "could not materialize active NPC parameter {bucket}.{param} for {actor_id}: {err}"
            ),
            Some(
                "Use a passive ruling or waive the obligation with an audited reason.".to_string(),
            ),
        )
        .into()),
    }
}

async fn resolve_dues_for_mechanic_id(ctx: &ToolCtx<'_>, mechanic_id: &str) {
    let Some(cell) = ctx.obligations else {
        return;
    };
    let resolved = {
        cell.lock()
            .unwrap_or_else(|p| p.into_inner())
            .resolve_dues_for_mechanic(mechanic_id)
    };
    for due_id in &resolved {
        let _ = ctx
            .engine
            .db
            .update_mechanic_due_status(due_id, "resolved", Some("settled via roll_check"), None)
            .await;
    }
}

pub fn build_check_contract_for_args(
    session_id: &str,
    turn_id: &str,
    ruleset_id: &str,
    module_id: Option<&str>,
    args: &RollCheckArgs,
    dice: &str,
) -> Result<CheckContract> {
    let requested_visibility = visibility_for_system(&args.visibility)?;
    let actor_id = args
        .actor_id
        .clone()
        .unwrap_or_else(|| "pc.current".to_string());
    let actor_kind = actor_kind_for_actor_id(&actor_id);
    let visibility = normalize_visibility_for_check_args(
        requested_visibility,
        actor_kind,
        args.intent_kind.as_deref(),
    );
    let mut advice_refs = vec!["gm_agent.roll_check".to_string()];
    if requested_visibility != visibility {
        advice_refs.push("visibility_normalized:active_pc_check_public".to_string());
    }
    Ok(CheckContract {
        check_id: format!("check_{}", Uuid::new_v4().simple()),
        session_id: session_id.to_string(),
        turn_id: turn_id.to_string(),
        ruleset_id: ruleset_id.to_string(),
        module_id: module_id.map(str::to_string),
        initiator: ActorRef {
            display_name: Some(actor_display_for(&actor_id, actor_kind)),
            actor_id,
            actor_kind,
        },
        target_actor: None,
        opposition: OppositionModel::NoMechanicalOpposition,
        action_summary: args.check_label.chars().take(500).collect(),
        intent_kind: args
            .intent_kind
            .clone()
            .unwrap_or_else(|| "agent_selected_check".to_string()),
        check_label: args.check_label.clone(),
        dice_expression: dice.to_string(),
        modifiers: vec![],
        target: CheckTargetModel::UnknownUntilLookup,
        tested_parameter: Some(TestedParameter {
            domain: None,
            key: args.tested_parameter.clone(),
            label: args.tested_parameter.clone(),
        }),
        opponent_tested_parameter: None,
        actor_snapshot_ids: vec![],
        source_refs: vec![],
        learned_packet_ids: vec![],
        roll_visibility: visibility,
        roll_authority: RollAuthority::System,
        disclosure: RollDisclosurePolicy::for_visibility(visibility),
        stakes: CheckStakes {
            before_roll_public: format!(
                "A {} check is required; its result determines the immediate consequence.",
                args.check_label
            ),
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
        advice_refs,
        expires_at_turn: Some(turn_id.to_string()),
    })
}

pub fn build_player_contract_for_args(
    session_id: &str,
    turn_id: &str,
    ruleset_id: &str,
    module_id: Option<&str>,
    args: &RequestPlayerRollArgs,
    dice: &str,
) -> Result<CheckContract> {
    let mut sys_args = RollCheckArgs {
        check_label: args.check_label.clone(),
        tested_parameter: args.tested_parameter.clone(),
        actor_id: Some("pc.current".to_string()),
        opposed: None,
        visibility: args.visibility.clone(),
        intent_kind: Some("player_roll_requested".to_string()),
        mechanic_id: None,
        scene_mechanic_id: None,
    };
    sys_args.visibility = "public".to_string();
    let mut c =
        build_check_contract_for_args(session_id, turn_id, ruleset_id, module_id, &sys_args, dice)?;
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
    c.advice_refs
        .push("gm_agent.request_player_roll:converted:system_rolls_visible".to_string());
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
        ToolSpec {
            name: "roll_check",
            schema: json!({"type":"function","function":{"name":"roll_check","description":"Execute a source-backed system roll. tested_parameter is mandatory unless inherited via mechanic_id.","parameters":{"type":"object","properties":{"check_label":{"type":"string"},"tested_parameter":{"type":"string"},"actor_id":{"type":"string"},"opposed":{"type":"object","properties":{"npc_id":{"type":"string"},"opponent_parameter":{"type":"string"},"bucket":{"type":"string","default":"skills"}},"required":["npc_id","opponent_parameter"]},"visibility":{"type":"string","enum":["public","secret"]},"intent_kind":{"type":"string"},"mechanic_id":{"type":"string"},"scene_mechanic_id":{"type":"string","description":"intent_id from the current scene's mechanic-intents block; inherits the module-stated tested_parameter/difficulty and the engine enforces the stated consequences after settlement"}},"required":["check_label"]}}}),
        }
    }

    async fn call(
        &self,
        ctx: &ToolCtx<'_>,
        ledger: &mut TurnLedger,
        args: Value,
    ) -> Result<ToolOutput> {
        let mut args = parse_roll_check_args(args)?;
        // Phase3 §4.1 上游"双保险"下半段：GM 漏填对抗形态（roll_check 无 opposed）
        // 但回合头部对抗预 pass 语义判定了本回合是攻击场景对手 → 注入预备的对抗
        // 参数（真实场景 NPC id + 防御键），与 GM 显式 opposed 同走下方 synth+stamp
        // 通路。fail-closed：预 pass 无命中（无攻击意图/关门/现搓不成）则 binding=None
        // 不注入，roll_check 退回单方检定（既有行为字节不变）。
        inject_opposed_binding(&mut args, ctx.opposed_binding);
        // kernel 单次加载（B3）：目录继承、缺省骰式、band 语义共用同一份。
        let kernel = ctx
            .engine
            .db
            .load_rule_kernel(&ctx.request.ruleset_id)
            .await?;
        // ① mechanic_id 给定 → 查目录继承（目录是知识不是枷锁：显式 args 优先）。
        let mut inherited_dice: Option<String> = None;
        if let Some(mechanic_id) = args.mechanic_id.clone() {
            let entry = kernel.as_ref().and_then(|k| {
                crate::tools::mechanic::find_mechanic(&k.mechanics_catalog, &mechanic_id)
            });
            let Some(entry) = entry else {
                return Err(ToolError::recoverable(
                    "mechanic_not_found",
                    format!("mechanic not found in catalog: {mechanic_id}"),
                    Some(
                        "Check the BP1 mechanics index for valid ids, or use retrieve_rules."
                            .to_string(),
                    ),
                ));
            };
            if mechanic_should_return_passive(entry) {
                resolve_dues_for_mechanic_id(ctx, &mechanic_id).await;
                return Ok(ToolOutput::ok(passive_mechanic_result(&mechanic_id, entry)));
            }
            inherited_dice = apply_mechanic_inheritance(&mut args, entry);
        }
        // ② 继承后 tested_parameter 仍空 → invalid_arguments（一期语义保持）。
        if args.tested_parameter.trim().is_empty() {
            return Err(ToolError::recoverable(
                "invalid_arguments",
                "tested_parameter is required",
                Some("Bind the check to the real actor parameter being tested.".to_string()),
            ));
        }
        // C4：scene_mechanic_id → 当前场景 intent（解析/错误码细节在 scene_policy）。
        let scene_intent = crate::scene_policy::resolve_scene_intent(
            ctx.engine,
            ctx.request,
            ctx.state.scene_id.as_deref(),
            args.scene_mechanic_id.as_deref(),
        )
        .await?;
        // 显式 args 优先（意图是知识不是枷锁）：与 intent.tested_parameter 不一致用 args 值并标记（可观测）。
        let parameter_overridden = scene_intent.as_ref().is_some_and(|i| {
            !args.tested_parameter.trim().is_empty()
                && args.tested_parameter.trim() != i.tested_parameter.trim()
        });
        // 继承骰式为 Some 用之；None 仍走 kernel 缺省（missing_kernel_dice 语义不变）。
        let dice = match inherited_dice {
            Some(d) => d,
            None => kernel_dice_expr(kernel.as_ref())?,
        };
        ensure_active_npc_parameter_if_needed(ctx, &args).await?;
        let mut contract = build_check_contract_for_args(
            &ctx.request.session_id,
            &ctx.request.turn_id,
            &ctx.request.ruleset_id,
            ctx.request.module_id.as_deref(),
            &args,
            &dice,
        )?;
        // ③ 结构化引用走 advice_refs，不动 CheckContract 5 处签名（契约 §6 注记）。
        if let Some(mechanic_id) = &args.mechanic_id {
            contract.advice_refs.push(format!("mechanic:{mechanic_id}"));
        }
        // C4：盖章收口 stamp_scene_intent（难度→target + scene_mechanic: 引用进 advice_refs，护栏 §3.5.1）。
        crate::scene_policy::stamp_scene_intent(&mut contract, scene_intent.as_ref());
        if let Some(opposed) = &args.opposed {
            let persona = trpg_runtime::npc_synth::NpcPersona {
                actor_id: opposed.npc_id.clone(),
                name: opposed.npc_id.clone(),
                prose: "opposition selected by GM agent".to_string(),
            };
            // fail-closed 不静默：防御参数合成失败 ⇒ 结构化错误让 agent 改道
            // （request_player_roll / 非对抗 roll_check / 纯叙事），绝不带着未
            // 物化的防御值继续 stamp。注意 ensure_npc_parameter 返回
            // Result<Option<Value>>：合成门关（TRPG_NPC_PERSONA_SYNTHESIS=0）
            // 或 NPC 无参数卡时是 Ok(None) 而非 Err——两种形态都必须拦。
            let synthesized = ctx
                .engine
                .ensure_npc_parameter(
                    &ctx.request.session_id,
                    &ctx.request.ruleset_id,
                    &persona,
                    &opposed.bucket,
                    &opposed.opponent_parameter,
                    &args.check_label,
                )
                .await;
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
            trpg_runtime::stamp_opposed_check(
                &mut contract,
                &persona,
                &opposed.bucket,
                &opposed.opponent_parameter,
            );
        }
        // GM 漏填 opposed/显式目标但这是一次真实玩家攻击 → 绑定透明兜底目标
        // npc.opposition，让无对抗的 CoC 火器命中也能驱动 record_attack 的伤害
        // 结算。fail-closed 仍由 mechanics record_attack 把守；此处只在确属攻击
        // 且无显式目标/对抗时落 target_actor，绝不盖 opponent_tested_parameter
        // （无对抗形态，contract_is_opposed 仍为 false）。
        if contract.target_actor.is_none() {
            if let Some(target) = fallback_attack_target_actor_for_roll(&args) {
                contract.target_actor = Some(target);
            }
        }
        // 结算尾段（落库入账 → 系统掷骰 → band 语义 → effect_policy → 债务带回）
        // 收口 settle.rs 单点，与 request_player_roll 的政策转换路径共用。
        let output = crate::tools::settle::settle_system_check(
            ctx,
            ledger,
            kernel.as_ref(),
            scene_intent.as_ref(),
            parameter_overridden,
            &contract,
        )
        .await?;
        // J2 修复：带 mechanic_id 的结算成功 ⇒ 匹配该机制的 open dues 确定性置
        // resolved（内存+DB）——处理过的债务不再要求 GM 额外 waive。落库失败
        // `let _ =` 吞（内存侧已清，下回合 leftover 重捞由 DB 状态兜底）。
        if let Some(mechanic_id) = args.mechanic_id.as_deref() {
            resolve_dues_for_mechanic_id(ctx, mechanic_id).await;
        }
        Ok(output)
    }
}

pub struct RequestPlayerRollTool;

#[async_trait]
impl GmTool for RequestPlayerRollTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "request_player_roll",
            schema: json!({"type":"function","function":{"name":"request_player_roll","description":"Open an InteractionGate and wait for the player to roll personally. Under a system-rolls table dice policy the engine instead rolls on the player's behalf immediately and returns the settled result; narrate the stakes and the outcome without asking the player to roll.","parameters":{"type":"object","properties":{"check_label":{"type":"string"},"tested_parameter":{"type":"string"},"stakes":{"type":"object","properties":{"before":{"type":"string"},"success":{"type":"string"},"failure":{"type":"string"}},"required":["before","success","failure"]},"visibility":{"type":"string","enum":["public","secret"]},"scene_mechanic_id":{"type":"string","description":"intent_id from the current scene's mechanic-intents block; inherits the module-stated difficulty and the engine enforces the stated consequences after the player's roll settles"}},"required":["check_label","tested_parameter","stakes"]}}}),
        }
    }

    async fn call(
        &self,
        ctx: &ToolCtx<'_>,
        ledger: &mut TurnLedger,
        args: Value,
    ) -> Result<ToolOutput> {
        let args = parse_player_args(args)?;
        let kernel = ctx
            .engine
            .db
            .load_rule_kernel(&ctx.request.ruleset_id)
            .await?;
        let dice = kernel_dice_expr(kernel.as_ref())?;
        // C7：玩家亲掷路径同样盖章（解析失败在 cancel/insert 副作用前直接报错）；effect_policy 下回合 gate 结算后经引用恢复执行（spec §6）。
        let scene_intent = crate::scene_policy::resolve_scene_intent(
            ctx.engine,
            ctx.request,
            ctx.state.scene_id.as_deref(),
            args.scene_mechanic_id.as_deref(),
        )
        .await?;
        let mut contract = build_player_contract_for_args(
            &ctx.request.session_id,
            &ctx.request.turn_id,
            &ctx.request.ruleset_id,
            ctx.request.module_id.as_deref(),
            &args,
            &dice,
        )?;
        crate::scene_policy::stamp_scene_intent(&mut contract, scene_intent.as_ref());
        // 桌面骰权政策守卫（对齐 legacy CLI/API 路径与 NarrationVerifier 的
        // ManualRollRequest 红线）：system_rolls_visible 下玩家从不手掷——此处若
        // 仍开 gate，下一回合的叙事输入不是骰值应答，gate 永不结算只会被新 gate
        // 作废，战斗冻在 awaiting_player_roll、零掷骰落账。转系统代掷当场结算
        // （normalize 原语是 runtime 单一事实源），盖章后果照常强制执行，
        // 戏剧 stakes 仍由 agent 叙入散文。政策关（真人摇骰桌）gate 路径原样。
        if trpg_runtime::system_rolls_visible_policy() {
            let contract = convert_player_gate_to_system(&contract);
            let parameter_overridden = scene_intent
                .as_ref()
                .is_some_and(|i| args.tested_parameter.trim() != i.tested_parameter.trim());
            let mut output = crate::tools::settle::settle_system_check(
                ctx,
                ledger,
                kernel.as_ref(),
                scene_intent.as_ref(),
                parameter_overridden,
                &contract,
            )
            .await?;
            if let Some(obj) = output.result.as_object_mut() {
                obj.insert("player_roll_converted".into(), json!("table dice policy system_rolls_visible: the engine rolled on the player's behalf; narrate the stakes and this settled result, do not ask the player to roll"));
            }
            return Ok(output);
        }
        // 对齐 persist_agent_plan 既有行为：插新 pending 前先作废旧 open gate，
        // 防同会话累积多个 open pending check。
        ctx.engine
            .db
            .cancel_open_pending_checks_for_session(
                &ctx.request.session_id,
                PendingCheckStatus::Superseded,
            )
            .await?;
        ctx.engine
            .db
            .insert_check_contract(&contract, "created")
            .await?;
        let pending = make_pending_check(&contract);
        let gate = InteractionGate::from_pending_check(&pending);
        ctx.engine.db.insert_pending_check(&pending).await?;
        ctx.engine.db.insert_interaction_gate(&gate).await?;
        ledger.record_contract(&contract);
        ledger.record_gate(&gate);
        Ok(ToolOutput::awaiting(
            json!({"check_id": contract.check_id, "prompt_public": pending.prompt_public}),
            AwaitingPlayerRoll {
                check_id: contract.check_id,
                prompt_public: pending.prompt_public,
            },
        ))
    }
}

#[cfg(test)]
#[path = "check_tests.rs"]
mod tests;
