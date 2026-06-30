//! P1 分层运行时：Adjudicator / Narrator 两层切分的结构化只读投影产物。
//!
//! - [`AdjudicationPacket`] = Adjudicator(沿用 15 工具 agent loop)的机械裁定结构化产物。
//!   零新存储，全部从 [`TurnLedgerSnapshot`] / TurnContext 投影。含 `adjudicator_prose`
//!   (buffer 下来的自由文本，供 verifier / fallback 用，**绝不直接喂 Narrator**)。
//! - [`NarrationPacket`] = AdjudicationPacket 的 **player-safe 收窄投影**，Narrator 唯一输入。
//!   只透传玩家可感知的机械事实摘要 + player_input(对齐 设计4 §9.1 可落地子集)；
//!   GM-only / secret / 规则原文 / tool-JSON / adjudicator_prose 一律不进。
//!
//! 全部纯函数、零 DB / 零 LLM / 零 IO ⇒ 可单测(fail-closed allowlist 投影)。
//! [codex F4] adjudicator_prose 留在 AdjudicationPacket；NarrationPacket 永不携带它。

use crate::tools::AwaitingPlayerRoll;
use serde::{Deserialize, Serialize};
use trpg_agent::TurnLedgerSnapshot;
use trpg_model::{CheckResultRecord, RollDisclosurePolicy, RollVisibility, Visibility};

/// 单条机械事实摘要类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MechanicalFactKind {
    /// 检定成败/掷骰结果。
    Check,
    /// 效果(伤害/治疗/状态)落账。
    Effect,
    /// 参数/资源轨变化。
    Parameter,
}

/// 一条机械事实摘要(从 ledger committed 记录投影)。
///
/// `player_visible` 钉死该事实是否玩家可感知(据既有 visibility 语义)；
/// NarrationPacket 投影时**只保留** player_visible==true 的事实(fail-closed)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MechanicalFact {
    pub kind: MechanicalFactKind,
    /// 玩家无关的稳定摘要文本(不含规则原文 / secret)。
    pub summary: String,
    /// 关联 ledger id(check_id / effect_id / impact_id)，供 verifier 对账。
    pub ledger_ref: String,
    /// 是否玩家可感知(据 roll/effect/impact visibility)。
    pub player_visible: bool,
}

/// Adjudicator 的结构化只读产物。零新存储，从本回合 ledger / ctx 投影。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AdjudicationPacket {
    /// 玩家本回合输入(原样透传)。
    pub player_input: String,
    /// 机械事实摘要全集(玩家可见 + GM 私有都在；投影到 Narration 时再按 player_visible 过滤)。
    pub mechanical_outcomes: Vec<MechanicalFact>,
    /// buffer 下来的 adjudicator 自由文本，供 verifier / fail-soft fallback 用。
    /// **绝不直接喂 Narrator**(NarrationPacket 不携带它)。
    pub adjudicator_prose: String,
    /// 本回合已解析的 gate facts(玩家可感知的桌面骰/裁定提示)。
    pub resolved_gate_facts: Vec<String>,
    /// request_player_roll 桌面骰早退终态(Some ⇒ 本回合不跑 Narrator)。
    pub awaiting: Option<AwaitingPlayerRoll>,
    /// 复用 `trpg_agent::ledger_id_set` 的**有序**(排序保证确定性)id 集。
    pub referenced_ledger_ids: Vec<String>,
}

/// Narrator 唯一输入：AdjudicationPacket 的 player-safe 收窄投影。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NarrationPacket {
    pub player_input: String,
    /// 玩家可感知的"发生了什么"(check/roll 成败摘要)。
    pub what_happened: Vec<String>,
    /// 玩家可感知的"状态变化"(effect / parameter 摘要)。
    pub what_changed: Vec<String>,
    /// 玩家可感知事实(resolved_gate_facts 透传)。
    pub player_perceivable_facts: Vec<String>,
    /// 轻量风格串(从 gm_skill 取，或中性默认)。
    pub style_profile: String,
    /// 禁揭示项(P1 复用 verify 路径已采集的投影；为空也合法)。
    pub forbidden_reveals: Vec<String>,
    /// A2(§6 大考)：player-safe 场景上下文(感官/连续性 grounding)。来源**只能**是已对
    /// 玩家可见的散文(上一回合已交付 narration)——按定义已脱敏；`project` 恒置空(OFF 字节
    /// 等价),仅 ON 经 `with_scene_context` 注入。**绝不**承载 GM-only/secret/规则原文/prose。
    #[serde(default)]
    pub scene_context: Vec<String>,
    /// Q-MODULE DP-A'/DP-B'(§6 大考):模组授权的**进场 establishing 素材**(当前场景的
    /// NON-secret `read_aloud`,剧透裁剪后),由 runtime 经 `CompiledContext.scene_establishing`
    /// 在 `Enforce` 下提供。`project` 恒置空(OFF 字节等价),仅经 `with_scene_establishing` 注入。
    /// 由 Narrator 改写成画面(具名场景/氛围)、**绝不**逐字倾倒/列清单(尊重 Q-4 no-dump);
    /// **绝不**承载 GM-only `gm_notes`/secret/clue(那些仍走 A2 奖励门控)。
    #[serde(default)]
    pub scene_establishing: Vec<String>,
    /// OA2 (G-3)(§6 大考):player-safe 的 PC 能力档案(角色卡数值能力,玩家自知),由 runtime
    /// 经 `CompiledContext.character_context` 在 `Enforce` 下提供。`project` 恒置空(OFF 字节
    /// 等价),仅经 `with_character_context` 注入。Narrator 选择性引用 PC 能力/特长、**绝不**逐字
    /// 罗列数值(尊重 Q-4 no-dump)。角色卡是玩家自己的 ⇒ 不新增泄漏面。
    #[serde(default)]
    pub character_context: Vec<String>,
    /// L6.1 SPINE→narration:post-adjudication Beat `DirectorPlan` 的 **player-safe 结构化转向 token**
    /// (beat_kind / desired_change / dramatic_function 等枚举/短串),由 runtime 经
    /// `ctx.post_adjudication_plan()` 在 `TRPG_DIRECTOR_POST_ADJUDICATION` ON 下提供,与
    /// `scene_establishing`/`character_context` **并列**组合(§2 composition rule)。`project` 恒置空
    /// (OFF 字节等价),仅经 `with_director_plan` 注入。**只承载结构化 token**——绝不承载 reveal 事实
    /// 原文/world candidate/secret(reveal 仍是 proposal、forbidden_reveals 仍单独流向 Narrator)。
    #[serde(default)]
    pub director_plan: Vec<String>,
    /// M3 (decision #3)：player-safe **故事情绪/氛围** carrier —— 仅承载玩家已感知层 (PlayerPerceived)
    /// 蒸馏出的氛围/基调短串 (例:"雨后的潮湿"/"压抑的沉默"),供 Narrator 让念白贴合此刻的"感觉"。
    /// **绝不**承载 Story/Director 记忆 (线索/伏笔/未来转向 ⇒ 零 telegraph)。`project` 恒置空
    /// (carrier 留着但空 ⇒ 默认 OFF ⇒ 字节等价),仅当 `TRPG_NARRATOR_STORY_MOOD` ON 经
    /// `with_story_mood` 注入 (日后 opt-in)。
    #[serde(default)]
    pub story_mood: Vec<String>,
}

fn roll_is_player_visible(visibility: RollVisibility) -> bool {
    // 镜像 trpg_agent::gm_loop 既有语义(PublicGmRoll / PlayerRollRequired 玩家可见)。
    // PassiveResolution is also player-auditable once committed: the player did not roll by hand,
    // but the GM still owes a visible mechanical result instead of silently hiding the check.
    matches!(
        visibility,
        RollVisibility::PublicGmRoll
            | RollVisibility::PlayerRollRequired
            | RollVisibility::PassiveResolution
    )
}

fn visibility_is_player_visible(visibility: Visibility) -> bool {
    matches!(visibility, Visibility::Public | Visibility::PlayerVisible)
}

impl AdjudicationPacket {
    /// 从本回合 ledger snapshot + ctx 字段投影(纯函数)。
    ///
    /// `adjudicator_prose` 是 buffer 下来的自由文本；`awaiting` 是桌面骰早退终态。
    /// mechanical_outcomes 收所有 committed check/effect/parameter 记录并标注 player_visible。
    pub fn project(
        player_input: &str,
        snapshot: &TurnLedgerSnapshot,
        resolved_gate_facts: &[String],
        adjudicator_prose: &str,
        awaiting: Option<&AwaitingPlayerRoll>,
    ) -> Self {
        let mut mechanical_outcomes = Vec::new();
        for result in &snapshot.check_results {
            let visible = roll_is_player_visible(result.roll.visibility);
            mechanical_outcomes.push(MechanicalFact {
                kind: MechanicalFactKind::Check,
                // OA-ROLLTRUTH: faithful canonical roll line (real expression + dice + target + band)
                // so the split Narrator copies "6d4" instead of fabricating the die size. Replaces the
                // old raw `compact_value(outcome)` JSON dump (which lacked the expression).
                summary: faithful_roll_line(result),
                ledger_ref: result.check_id.clone(),
                player_visible: visible,
            });
        }
        for effect in &snapshot.effect_contracts {
            mechanical_outcomes.push(MechanicalFact {
                kind: MechanicalFactKind::Effect,
                summary: format!(
                    "effect {} ({})",
                    effect.effect_id,
                    effect.effect_kind.as_str()
                ),
                ledger_ref: effect.effect_id.clone(),
                player_visible: visibility_is_player_visible(effect.visibility),
            });
        }
        for impact in &snapshot.parameter_impacts {
            mechanical_outcomes.push(MechanicalFact {
                kind: MechanicalFactKind::Parameter,
                summary: format!(
                    "{}.{} {} {}",
                    impact.target_id,
                    impact.parameter_path,
                    impact.operation.as_str(),
                    compact_value(&impact.value)
                ),
                ledger_ref: impact.impact_id.clone(),
                player_visible: visibility_is_player_visible(impact.visibility),
            });
        }
        let mut referenced_ledger_ids: Vec<String> =
            trpg_agent::ledger_id_set(snapshot).into_iter().collect();
        referenced_ledger_ids.sort();

        Self {
            player_input: player_input.to_string(),
            mechanical_outcomes,
            adjudicator_prose: adjudicator_prose.to_string(),
            resolved_gate_facts: resolved_gate_facts.to_vec(),
            awaiting: awaiting.cloned(),
            referenced_ledger_ids,
        }
    }
}

impl NarrationPacket {
    /// player-safe 收窄投影(纯函数)。只透传 player_visible 机械事实 + player_input；
    /// **绝不透传 adjudicator_prose**。`style_profile` 取轻量串(空则中性默认)。
    /// `forbidden_reveals` P1 暂取传入的投影(可为空)。
    pub fn project(
        adj: &AdjudicationPacket,
        style_profile: &str,
        forbidden_reveals: &[String],
    ) -> Self {
        let mut what_happened = Vec::new();
        let mut what_changed = Vec::new();
        for fact in &adj.mechanical_outcomes {
            if !fact.player_visible {
                continue; // fail-closed：非玩家可见的机械事实不进 Narration
            }
            match fact.kind {
                MechanicalFactKind::Check => what_happened.push(fact.summary.clone()),
                MechanicalFactKind::Effect | MechanicalFactKind::Parameter => {
                    what_changed.push(fact.summary.clone())
                }
            }
        }
        let style = if style_profile.trim().is_empty() {
            // A1(§6 大考)：空 style 默认 persona = **第二人称「你」**。OFF 永不进 split
            // 分支(此默认仅 split Narrator 可达)，故 OFF 字节不变；split Narrator 不读
            // gm_skill markdown(其第二人称源)，由此默认补回沉浸式第二人称视角。
            "以**第二人称「你」**称呼玩家角色，中性、克制、贴合已发生的机械事实地叙事。".to_string()
        } else {
            style_profile.trim().to_string()
        };
        Self {
            player_input: adj.player_input.clone(),
            what_happened,
            what_changed,
            player_perceivable_facts: adj.resolved_gate_facts.clone(),
            style_profile: style,
            forbidden_reveals: forbidden_reveals.to_vec(),
            // A2：project 恒置空 scene_context(OFF 字节等价)；ON 阶段经 with_scene_context 注入。
            scene_context: Vec::new(),
            // Q-MODULE：project 恒置空(OFF 字节等价)；仅 Enforce 经 with_scene_establishing 注入。
            scene_establishing: Vec::new(),
            // OA2：project 恒置空(OFF 字节等价)；仅 Enforce 经 with_character_context 注入。
            character_context: Vec::new(),
            // L6.1：project 恒置空(OFF 字节等价)；仅 spine ON 经 with_director_plan 注入。
            director_plan: Vec::new(),
            // M3 决策#3：project 恒置空(carrier 留着但空 ⇒ 默认 OFF 字节等价)；仅经 with_story_mood 注入。
            story_mood: Vec::new(),
        }
    }

    /// A2(§6 大考)：注入 player-safe 场景上下文(链式 builder)。`scenes` 的每条都**必须**
    /// 是已对玩家可见的散文(调用方保证 = 上一回合已交付 narration / player_knowledge_view)；
    /// 投影本身从不读 GM-only / adjudicator_prose ⇒ fail-closed,绝不新增泄漏面。
    pub fn with_scene_context(mut self, scenes: &[String]) -> Self {
        self.scene_context = scenes
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        self
    }

    /// Q-MODULE DP-B'(§6 大考):注入模组授权的进场 establishing 素材(链式 builder)。
    /// 调用方保证 `slices` 来自 `CompiledContext.scene_establishing`(runtime 仅 Enforce 填充、
    /// 已剧透裁剪、仅 NON-secret `read_aloud`)。空 slices ⇒ 字段为空 ⇒ 渲染侧零新增字节
    /// (OFF/非 Enforce 字节等价)。
    pub fn with_scene_establishing(mut self, slices: &[String]) -> Self {
        self.scene_establishing = slices
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        self
    }

    /// OA2 (G-3)(§6 大考):注入 player-safe PC 能力档案(链式 builder)。调用方保证 `slices`
    /// 来自 `CompiledContext.character_context`(runtime 仅 Enforce 填充、玩家自知的角色卡数值
    /// 能力)。空 slices ⇒ 字段为空 ⇒ 渲染侧零新增字节(OFF/非 Enforce 字节等价)。
    pub fn with_character_context(mut self, slices: &[String]) -> Self {
        self.character_context = slices
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        self
    }

    /// L6.1(链式 builder):注入 post-adjudication DirectorPlan 的 player-safe 结构化转向 token。
    /// 调用方保证 `tokens` 来自 `ctx.post_adjudication_plan()` 的结构化投影(仅枚举/短串,无 reveal
    /// 事实原文/secret)。空 tokens(spine OFF ⇒ 无 plan) ⇒ 字段为空 ⇒ 渲染侧零新增字节(OFF 字节
    /// 等价)。与 `with_scene_establishing`/`with_character_context` 行为一致(trim+drop empties)。
    pub fn with_director_plan(mut self, tokens: &[String]) -> Self {
        self.director_plan = tokens
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        self
    }

    /// M3(链式 builder,决策#3):注入 player-safe **故事情绪/氛围** 短串。调用方保证 `tokens` 来自
    /// **玩家已感知层**(PlayerPerceived)的蒸馏,绝不含 Story/Director 记忆(零 telegraph)。空 tokens
    /// (`TRPG_NARRATOR_STORY_MOOD` OFF/无来源)⇒ 字段为空 ⇒ 渲染侧零新增字节(OFF 字节等价)。
    /// 与其它 carrier 行为一致(trim + drop empties)。
    pub fn with_story_mood(mut self, mood: &[String]) -> Self {
        self.story_mood = mood
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        self
    }
}

/// OA-ROLLTRUTH: render a faithful, player-safe roll line from the REAL kernel check record so the
/// split Narrator copies the canonical dice expression (e.g. `6d4`) verbatim instead of fabricating
/// the die size from the values it sees (the `6d?`/`6d6`/`6d3` bug). GENERIC — no ruleset/module name
/// branch; derives the target description from `resolution_model.kind`. NEVER emits `?` or raw JSON.
/// Player-safe: only mechanical numbers + the human check_label; respects `disclosure.show_dc_to_player`
/// (explicit `false` ⇒ omit the target). check_id ref is kept (verifier reconciliation; the Narrator
/// uses the human label, not the id).
fn faithful_roll_line(result: &CheckResultRecord) -> String {
    let oc = &result.outcome;
    let label = oc
        .get("check_label")
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let head = match label {
        Some(l) => format!("检定[{}] {}", result.check_id, l),
        None => format!("检定[{}]", result.check_id),
    };

    // Resolved-ness: a real bound roll has a degree (str) or an explicit success bool. Provisional /
    // awaiting_binding / success:null must NOT be dressed as a bound [roll] (codex #5).
    let degree = oc.get("degree").and_then(|v| v.as_str());
    let success = oc.get("success").and_then(|v| v.as_bool());
    let awaiting = oc.get("awaiting_binding").is_some()
        || matches!(oc.get("success"), Some(serde_json::Value::Null));
    if (degree.is_none() && success.is_none()) || awaiting {
        return format!("{head}（待结算，尚未绑定真实检定，勿当作已掷骰检定呈现）");
    }

    // Disclosure: prefer outcome.disclosure object; if the whole object is absent, derive from roll
    // visibility (RollDisclosurePolicy::for_visibility); a missing individual flag ⇒ hide (fail-closed).
    let fallback = RollDisclosurePolicy::for_visibility(result.roll.visibility);
    let disc = oc.get("disclosure");
    let flag = |name: &str, fb: bool| -> bool {
        match disc {
            Some(d) => d.get(name).and_then(|v| v.as_bool()).unwrap_or(false),
            None => fb,
        }
    };
    let show_formula = flag("show_formula_to_player", fallback.show_formula_to_player);
    let show_roll = flag("show_roll_to_player", fallback.show_roll_to_player);
    let show_dc = flag("show_dc_to_player", fallback.show_dc_to_player);
    let show_band = flag(
        "show_success_failure_to_player",
        fallback.show_success_failure_to_player,
    );

    let expr = result.roll.expression.trim();
    let dice = oc
        .get("dice")
        .or_else(|| result.roll.result.get("rolls"))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_i64())
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(",")
        });
    let total = oc
        .get("total")
        .or_else(|| result.roll.result.get("total"))
        .and_then(|v| v.as_i64());
    let values = match (&dice, total) {
        (Some(d), _) if !d.is_empty() => format!("[{d}]"),
        (_, Some(t)) => t.to_string(),
        _ => String::new(),
    };

    // Target description — GENERIC from the real CheckResolutionModel kind (no ruleset name branch).
    let rm = oc.get("resolution_model");
    let kind = rm.and_then(|r| r.get("kind")).and_then(|k| k.as_str());
    let geti = |obj: Option<&serde_json::Value>, key: &str| -> Option<i64> {
        obj.and_then(|o| o.get(key)).and_then(|v| v.as_i64())
    };
    let target = if !show_dc {
        String::new()
    } else {
        match kind {
            // dice_pool_count: count dice whose face EQUALS target_face; success when count ≥ threshold.
            Some("dice_pool_count") | Some("dice_pool_opposed") => {
                let face = geti(rm, "target_face");
                let thr = geti(rm, "threshold").or_else(|| geti(Some(oc), "threshold"));
                match (face, thr) {
                    (Some(f), Some(t)) => format!(" 目标:出现面值={f}的骰子≥{t}个"),
                    (Some(f), None) => format!(" 目标:出现面值={f}的骰子"),
                    _ => String::new(),
                }
            }
            // d100 roll-under: success when total ≤ ability_value.
            Some("percentile_roll_under") => match geti(rm, "ability_value") {
                Some(v) => format!(" 目标:≤{v}"),
                None => String::new(),
            },
            Some("static_target_number") => match geti(rm, "value") {
                Some(v) => format!(" 目标:{v}"),
                None => String::new(),
            },
            Some("saving_throw") => match geti(rm, "dc") {
                Some(v) => format!(" 目标:DC{v}"),
                None => String::new(),
            },
            Some("attack_vs_defense") => match geti(rm, "defense_value") {
                Some(v) => format!(" 目标:对方防御{v}"),
                None => String::new(),
            },
            // opposed / provisional / lookup: contest verdict carried by the band; no static target.
            _ => match geti(Some(oc), "target") {
                Some(v) => format!(" 目标:{v}"),
                None => String::new(),
            },
        }
    };

    let band = if show_band {
        degree
            .map(band_label)
            .or_else(|| success.map(|s| if s { "成功" } else { "失败" }.to_string()))
            .unwrap_or_default()
    } else {
        String::new()
    };

    let mut line = head;
    let expr_part = if show_formula { expr } else { "" };
    let val_part = if show_roll { values.as_str() } else { "" };
    match (expr_part.is_empty(), val_part.is_empty()) {
        (false, false) => line.push_str(&format!(": {expr_part}={val_part}")),
        (false, true) => line.push_str(&format!(": {expr_part}")),
        (true, false) => line.push_str(&format!(": {val_part}")),
        (true, true) => {}
    }
    line.push_str(&target);
    if !band.is_empty() {
        line.push_str(&format!("，结果:{band}"));
    }
    line
}

/// Map a kernel degree token to a player-facing band label (GENERIC, no ruleset branch).
fn band_label(degree: &str) -> String {
    match degree {
        "critical_success" | "critical" => "大成功",
        "success" => "成功",
        "partial" | "partial_success" => "部分成功",
        "failure" | "fail" => "失败",
        "fumble" | "critical_failure" => "大失败",
        other => other,
    }
    .to_string()
}

/// 紧凑渲染一个 JSON value 为稳定短摘要(不展开大对象/数组的全部正文)。
fn compact_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use trpg_model::{
        ActorKind, CheckResultRecord, DiceRollRecord, EffectContract, EffectKind, ParameterImpact,
        ParameterOperation, RollVisibility, Visibility,
    };

    fn roll(check_id: &str, visibility: RollVisibility) -> DiceRollRecord {
        DiceRollRecord {
            roll_id: format!("roll_{check_id}"),
            session_id: "s1".into(),
            turn_id: "t1".into(),
            check_id: Some(check_id.into()),
            roller_kind: ActorKind::default(),
            roller_id: None,
            visibility,
            expression: "1d100".into(),
            result: serde_json::json!({"total": 42}),
            seed_commitment: String::new(),
            revealed_at: None,
            created_at: Utc::now(),
        }
    }

    fn check_result(check_id: &str, visibility: RollVisibility) -> CheckResultRecord {
        let r = roll(check_id, visibility);
        CheckResultRecord {
            check_id: check_id.into(),
            roll: r,
            outcome: serde_json::json!({"success": true}),
            committed_patches: vec![],
            created_at: Utc::now(),
        }
    }

    fn effect(effect_id: &str, visibility: Visibility) -> EffectContract {
        EffectContract {
            effect_id: effect_id.into(),
            effect_kind: EffectKind::Damage,
            target_actor_ids: vec!["pc".into()],
            proposed_patches: vec![],
            visibility,
            ..Default::default()
        }
    }

    fn impact(impact_id: &str, visibility: Visibility) -> ParameterImpact {
        ParameterImpact {
            impact_id: impact_id.into(),
            target_id: "pc".into(),
            parameter_path: "hp".into(),
            operation: ParameterOperation::Subtract,
            value: serde_json::json!(3),
            visibility,
            ..Default::default()
        }
    }

    fn snapshot_with(
        checks: Vec<CheckResultRecord>,
        effects: Vec<EffectContract>,
        impacts: Vec<ParameterImpact>,
    ) -> TurnLedgerSnapshot {
        TurnLedgerSnapshot {
            check_results: checks,
            effect_contracts: effects,
            parameter_impacts: impacts,
            ..Default::default()
        }
    }

    #[test]
    fn adjudication_packet_projects_all_mechanical_facts_and_sorted_ids() {
        let snap = snapshot_with(
            vec![check_result("c1", RollVisibility::PublicGmRoll)],
            vec![effect("e1", Visibility::PlayerVisible)],
            vec![impact("i1", Visibility::GmOnly)],
        );
        let adj = AdjudicationPacket::project(
            "我攻击它",
            &snap,
            &["桌面骰已解析".to_string()],
            "GM 内部推理:它其实是伪装的盟友",
            None,
        );
        assert_eq!(adj.player_input, "我攻击它");
        assert_eq!(adj.mechanical_outcomes.len(), 3);
        // adjudicator_prose 保留在 AdjudicationPacket
        assert!(adj.adjudicator_prose.contains("GM 内部推理"));
        // referenced_ledger_ids 有序且去重
        let mut sorted = adj.referenced_ledger_ids.clone();
        sorted.sort();
        assert_eq!(adj.referenced_ledger_ids, sorted);
        // referenced_ledger_ids 源自 ledger_id_set(check_contracts/dice_rolls/effect/impact),
        // 不含 check_results 的 check_id;本 snapshot 只设了 effect_contracts/parameter_impacts。
        assert!(adj.referenced_ledger_ids.contains(&"e1".to_string()));
        assert!(adj.referenced_ledger_ids.contains(&"i1".to_string()));
    }

    #[test]
    fn narration_packet_never_carries_adjudicator_prose_or_gm_only_facts() {
        let snap = snapshot_with(
            vec![
                check_result("c_public", RollVisibility::PublicGmRoll),
                check_result("c_private", RollVisibility::PrivateGmRoll),
            ],
            vec![
                effect("e_visible", Visibility::PlayerVisible),
                effect("e_gm", Visibility::GmOnly),
            ],
            vec![impact("i_gm", Visibility::GmOnly)],
        );
        let secret_prose = "SECRET: 黑曜石密钥藏在祭坛下";
        let adj = AdjudicationPacket::project("调查祭坛", &snap, &[], secret_prose, None);
        let narration = NarrationPacket::project(&adj, "", &[]);

        // adjudicator_prose 绝不出现在 NarrationPacket 任何字段
        let serialized = serde_json::to_string(&narration).unwrap();
        assert!(
            !serialized.contains("SECRET"),
            "NarrationPacket 泄漏了 adjudicator_prose"
        );
        assert!(!serialized.contains("黑曜石密钥"));

        // 只保留玩家可见机械事实
        assert_eq!(
            narration.what_happened.len(),
            1,
            "只该有 1 条玩家可见 check"
        );
        assert!(narration.what_happened[0].contains("c_public"));
        assert_eq!(
            narration.what_changed.len(),
            1,
            "只该有 1 条玩家可见 effect"
        );
        assert!(narration.what_changed[0].contains("e_visible"));
        // GM-only / private 事实被 fail-closed 滤除
        assert!(!serialized.contains("c_private"));
        assert!(!serialized.contains("e_gm"));
        assert!(!serialized.contains("i_gm"));
    }

    #[test]
    fn narration_packet_projects_passive_resolution_checks_as_player_visible() {
        let snap = snapshot_with(
            vec![check_result("c_passive", RollVisibility::PassiveResolution)],
            vec![],
            vec![],
        );

        let adj = AdjudicationPacket::project("谨慎下楼查看地下室", &snap, &[], "", None);
        let narration = NarrationPacket::project(&adj, "", &[]);

        assert_eq!(
            narration.what_happened.len(),
            1,
            "committed passive checks must still be available to player-visible narration"
        );
        assert!(narration.what_happened[0].contains("c_passive"));
    }

    #[test]
    fn faithful_roll_line_renders_real_pool_and_d100() {
        // OA-ROLLTRUTH: the Check fact summary must carry the REAL canonical dice expression +
        // rolled values + target + band (so the split Narrator copies "6d4" instead of fabricating
        // "6d?"/"6d6"/"6d3"). It must NEVER emit "?" or raw JSON braces.
        // Triangle dice-pool shape (the exact DB shape that broke).
        let mut pool = check_result("check_pool", RollVisibility::PublicGmRoll);
        pool.roll.expression = "6d4".into();
        pool.roll.result = serde_json::json!({"rolls":[2,2,3,2,1,1],"total":11,"expression":"6d4"});
        pool.outcome = serde_json::json!({
            "dice":[2,2,3,2,1,1], "degree":"success", "success":true,
            "check_label":"Talk your way past the guard",
            "resolution_model":{"kind":"dice_pool_count","threshold":1,"target_face":3}
        });
        let line = faithful_roll_line(&pool);
        assert!(
            line.contains("check_pool"),
            "must keep check_id ref for verifier: {line}"
        );
        assert!(
            line.contains("6d4"),
            "must carry canonical expression: {line}"
        );
        assert!(
            line.contains("2,2,3,2,1,1") || line.contains("[2, 2, 3, 2, 1, 1]"),
            "real dice: {line}"
        );
        // dice_pool_count = count dice whose face EQUALS target_face (codex #3), not ≥.
        assert!(
            line.contains("面值=3"),
            "pool target face EQUALS semantics: {line}"
        );
        assert!(!line.contains('?'), "NEVER an unbound '?' marker: {line}");
        assert!(
            !line.contains('{') && !line.contains('}'),
            "NEVER raw JSON braces: {line}"
        );
        assert!(line.contains("成功"), "outcome band: {line}");

        // CoC d100 percentile_roll_under (the REAL CheckResolutionModel kind, codex #2).
        let mut d100 = check_result("check_d100", RollVisibility::PublicGmRoll);
        d100.roll.expression = "1d100".into();
        d100.roll.result = serde_json::json!({"total":63,"rolls":[63]});
        d100.outcome = serde_json::json!({
            "total":63, "degree":"success", "success":true,
            "check_label":"Spot Hidden",
            "resolution_model":{"kind":"percentile_roll_under","ability_label":"Spot Hidden","ability_value":65}
        });
        let l2 = faithful_roll_line(&d100);
        assert!(l2.contains("1d100"), "{l2}");
        assert!(l2.contains("63"), "{l2}");
        assert!(l2.contains("≤65"), "roll-under target: {l2}");
        assert!(!l2.contains('?'), "{l2}");
        assert!(!l2.contains('{'), "{l2}");

        // Provisional / unresolved must NOT be dressed as a bound roll (codex #5).
        let mut prov = check_result("check_prov", RollVisibility::PublicGmRoll);
        prov.outcome = serde_json::json!({
            "check_label":"Heavy Pistol attack", "success": serde_json::Value::Null,
            "resolution_model":{"kind":"provisional","reason":"no source-backed DV","suggested_target":null}
        });
        let l3 = faithful_roll_line(&prov);
        assert!(l3.contains("待结算"), "unresolved must be pending: {l3}");
        assert!(
            !l3.contains("6d") && !l3.contains("1d"),
            "no fake dice on pending: {l3}"
        );
    }

    #[test]
    fn narration_packet_default_style_when_empty() {
        let adj =
            AdjudicationPacket::project("看四周", &TurnLedgerSnapshot::default(), &[], "", None);
        let narration = NarrationPacket::project(&adj, "  ", &[]);
        assert!(!narration.style_profile.is_empty());
        assert_eq!(narration.player_input, "看四周");
        assert!(narration.what_happened.is_empty());
    }

    #[test]
    fn narration_packet_passes_through_gate_facts_and_forbidden_reveals() {
        let adj = AdjudicationPacket::project(
            "撬锁",
            &TurnLedgerSnapshot::default(),
            &["需要玩家投掷 DEX".to_string()],
            "",
            None,
        );
        let narration =
            NarrationPacket::project(&adj, "硬汉侦探腔", &["不可提及凶手身份".to_string()]);
        assert_eq!(narration.player_perceivable_facts, vec!["需要玩家投掷 DEX"]);
        assert_eq!(narration.forbidden_reveals, vec!["不可提及凶手身份"]);
        assert_eq!(narration.style_profile, "硬汉侦探腔");
    }

    #[test]
    fn scene_establishing_project_empty_builder_injects_q_module() {
        // DP-B': project() leaves scene_establishing empty (OFF byte-equal); with_scene_establishing
        // injects the gated, redacted slices and trims/filters empties.
        let adj =
            AdjudicationPacket::project("环顾", &TurnLedgerSnapshot::default(), &[], "", None);
        let base = NarrationPacket::project(&adj, "", &[]);
        assert!(
            base.scene_establishing.is_empty(),
            "project must leave it empty (OFF byte-equal)"
        );

        let injected = NarrationPacket::project(&adj, "", &[]).with_scene_establishing(&[
            "  春雨敲打着加油站的雨棚。  ".to_string(),
            "   ".to_string(), // blank → filtered
            "霓虹在水洼里碎成猩红。".to_string(),
        ]);
        assert_eq!(
            injected.scene_establishing,
            vec![
                "春雨敲打着加油站的雨棚。".to_string(),
                "霓虹在水洼里碎成猩红。".to_string()
            ]
        );
        // Empty slices ⇒ field stays empty ⇒ render adds zero bytes.
        let empty = NarrationPacket::project(&adj, "", &[]).with_scene_establishing(&[]);
        assert!(empty.scene_establishing.is_empty());
    }

    #[test]
    fn character_context_project_empty_builder_injects_oa2() {
        // OA2 (G-3): project() leaves character_context empty (OFF byte-equal);
        // with_character_context injects the gated player-safe PC competency slices,
        // trimming/filtering empties exactly like with_scene_establishing.
        let adj =
            AdjudicationPacket::project("环顾", &TurnLedgerSnapshot::default(), &[], "", None);
        let base = NarrationPacket::project(&adj, "", &[]);
        assert!(
            base.character_context.is_empty(),
            "project must leave it empty (OFF byte-equal)"
        );

        let injected = NarrationPacket::project(&adj, "", &[]).with_character_context(&[
            "  stats: STR 55、DEX 70  ".to_string(),
            "   ".to_string(), // blank → filtered
            "skills: Spot Hidden 50".to_string(),
        ]);
        assert_eq!(
            injected.character_context,
            vec![
                "stats: STR 55、DEX 70".to_string(),
                "skills: Spot Hidden 50".to_string()
            ]
        );
        let empty = NarrationPacket::project(&adj, "", &[]).with_character_context(&[]);
        assert!(empty.character_context.is_empty());
    }

    #[test]
    fn director_plan_project_empty_builder_injects_l61() {
        // L6.1: project() leaves director_plan empty (spine OFF ⇒ byte-equal); with_director_plan
        // injects the player-safe structured steering tokens, trimming/filtering empties exactly
        // like with_scene_establishing/with_character_context.
        let adj =
            AdjudicationPacket::project("环顾", &TurnLedgerSnapshot::default(), &[], "", None);
        let base = NarrationPacket::project(&adj, "", &[]);
        assert!(
            base.director_plan.is_empty(),
            "project must leave it empty (OFF byte-equal)"
        );

        let injected = NarrationPacket::project(&adj, "", &[]).with_director_plan(&[
            "  beat:complicate  ".to_string(),
            "   ".to_string(), // blank → filtered
            "desired_change:fail_forward".to_string(),
        ]);
        assert_eq!(
            injected.director_plan,
            vec![
                "beat:complicate".to_string(),
                "desired_change:fail_forward".to_string()
            ]
        );
        // Empty slices ⇒ field stays empty ⇒ render adds zero bytes.
        let empty = NarrationPacket::project(&adj, "", &[]).with_director_plan(&[]);
        assert!(empty.director_plan.is_empty());
    }

    #[test]
    fn story_mood_project_empty_builder_injects_m3() {
        // M3 decision #3: project() leaves story_mood empty (carrier kept but empty ⇒ default OFF ⇒
        // byte-equal, zero telegraph risk); with_story_mood injects player-perceived mood tokens,
        // trimming/filtering empties exactly like the other carriers.
        let adj =
            AdjudicationPacket::project("环顾", &TurnLedgerSnapshot::default(), &[], "", None);
        let base = NarrationPacket::project(&adj, "", &[]);
        assert!(
            base.story_mood.is_empty(),
            "project must leave it empty (OFF byte-equal)"
        );

        let injected = NarrationPacket::project(&adj, "", &[]).with_story_mood(&[
            "  雨后的潮湿  ".to_string(),
            "   ".to_string(), // blank → filtered
            "压抑的沉默".to_string(),
        ]);
        assert_eq!(
            injected.story_mood,
            vec!["雨后的潮湿".to_string(), "压抑的沉默".to_string()]
        );
        let empty = NarrationPacket::project(&adj, "", &[]).with_story_mood(&[]);
        assert!(empty.story_mood.is_empty());
    }
}
