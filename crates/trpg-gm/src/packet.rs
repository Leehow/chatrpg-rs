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
use trpg_model::{RollVisibility, Visibility};

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
}

fn roll_is_player_visible(visibility: RollVisibility) -> bool {
    // 镜像 trpg_agent::gm_loop 既有语义(PublicGmRoll / PlayerRollRequired 玩家可见)。
    matches!(
        visibility,
        RollVisibility::PublicGmRoll | RollVisibility::PlayerRollRequired
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
                summary: format!(
                    "check {}: {}",
                    result.check_id,
                    compact_value(&result.outcome)
                ),
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
        let adj = AdjudicationPacket::project("环顾", &TurnLedgerSnapshot::default(), &[], "", None);
        let base = NarrationPacket::project(&adj, "", &[]);
        assert!(base.scene_establishing.is_empty(), "project must leave it empty (OFF byte-equal)");

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
        let adj = AdjudicationPacket::project("环顾", &TurnLedgerSnapshot::default(), &[], "", None);
        let base = NarrationPacket::project(&adj, "", &[]);
        assert!(base.character_context.is_empty(), "project must leave it empty (OFF byte-equal)");

        let injected = NarrationPacket::project(&adj, "", &[]).with_character_context(&[
            "  stats: STR 55、DEX 70  ".to_string(),
            "   ".to_string(), // blank → filtered
            "skills: Spot Hidden 50".to_string(),
        ]);
        assert_eq!(
            injected.character_context,
            vec!["stats: STR 55、DEX 70".to_string(), "skills: Spot Hidden 50".to_string()]
        );
        let empty = NarrationPacket::project(&adj, "", &[]).with_character_context(&[]);
        assert!(empty.character_context.is_empty());
    }
}
