//! Asset/Facet/Binding 契约类型（asset-binding 切片 T1）。
//!
//! 这些是整条切片其余任务（trpg-runtime::binding 影子 resolver、trpg-gm 影子记录、
//! trpg-cli explain）共同依赖的**单一事实源契约**：
//! - `AssetEnvelope`：source-backed JSON asset 的稳定外壳（kind/visibility/lifecycle/facets/data）。
//! - `AssetFacet`：asset 的可绑定切面（facet_kind + binding_candidates 候选 capability id）。
//! - `ExecutionTier`：统一执行层级（SourceOnly..VerifiedExecution），替散落 tier 概念的新主线。
//! - `BindingPlan` / `BindingVerdict`：BindingResolver 影子产出的绑定计划。
//! - capability id 常量集（11 个稳定 capability）+ `ALL_CAPABILITIES`。
//!
//! visibility 复用 `crate::Visibility`（不新造）；source_refs 复用 `crate::SourceRef`。
//! 全部字段 `#[serde(default)]`，新增/缺字段向后兼容（旧 JSON 可加载）。

use crate::{SourceRef, Visibility};
use serde::{Deserialize, Serialize};

/// Asset 类型枚举（解析产物大类）。字符串宽松层在 `data`，本枚举是稳定外壳维度。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum AssetKind {
    #[default]
    Generic,
    Rule,
    Module,
    Character,
    Scene,
    Npc,
    Object,
}

/// Asset 生命周期。Draft（草稿）→ Active（启用）→ Deprecated（弃用）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum AssetLifecycle {
    #[default]
    Draft,
    Active,
    Deprecated,
}

/// 统一执行层级（替散落 tier 概念的新主线）。
///
/// SourceOnly（仅来源、无机械执行）< GuidedRuling（GM 引导裁定）<
/// PartialExecution（部分机械执行）< ExactExecution（精确机械执行）<
/// VerifiedExecution（精确 + 已校验）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum ExecutionTier {
    #[default]
    SourceOnly,
    GuidedRuling,
    PartialExecution,
    ExactExecution,
    VerifiedExecution,
}

/// 绑定裁决（BindingResolver 影子产出）。
///
/// Unsupported（无任何可绑）/ SourceOnly（只 source 无 facet）/ Guided（有 source 无 capability）/
/// Partial（部分候选命中 registry）/ Exact（候选命中 registry）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum BindingVerdict {
    #[default]
    Unsupported,
    SourceOnly,
    Guided,
    Partial,
    Exact,
}

/// Asset 的可绑定切面。
///
/// facet_kind 用字符串（宽松、先不锁枚举，与现有 ParameterFacetKind 经字符串对齐）；
/// binding_candidates 是候选 capability id（见本模块 `CAP_*` 常量）。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct AssetFacet {
    /// 切面类型（如 "check" / "resource" / need_kind 派生），宽松字符串。
    #[serde(default)]
    pub facet_kind: String,
    /// 候选 capability id 列表（见 `CAP_*` / `ALL_CAPABILITIES`）。
    #[serde(default)]
    pub binding_candidates: Vec<String>,
    /// 切面置信度（0.0..1.0）。
    #[serde(default)]
    pub confidence: f32,
    /// 切面来源引用（页 / 锚 / 段路径）。
    #[serde(default)]
    pub source_refs: Vec<SourceRef>,
}

/// source-backed JSON asset 的稳定外壳。
///
/// 稳定维度（kind/visibility/lifecycle/facets/source_refs）在 envelope；宽松数据在 `data`。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct AssetEnvelope {
    /// Asset 唯一 id。
    #[serde(default)]
    pub asset_id: String,
    /// Asset 大类。
    #[serde(default)]
    pub asset_kind: AssetKind,
    /// 整体来源引用。
    #[serde(default)]
    pub source_refs: Vec<SourceRef>,
    /// 可见性（复用 `crate::Visibility`，默认 GmOnly）。
    #[serde(default)]
    pub visibility: Visibility,
    /// 整体置信度（0.0..1.0）。
    #[serde(default)]
    pub confidence: f32,
    /// 生命周期。
    #[serde(default)]
    pub lifecycle: AssetLifecycle,
    /// 可绑定切面集合。
    #[serde(default)]
    pub facets: Vec<AssetFacet>,
    /// 宽松数据负载（规则集 / 模组特定，不锁结构）。
    #[serde(default)]
    pub data: serde_json::Value,
}

/// BindingResolver 影子产出的绑定计划（advisory，不改实际结算）。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct BindingPlan {
    /// 绑定 id。
    #[serde(default)]
    pub binding_id: String,
    /// 触发的 need 类型。
    #[serde(default)]
    pub need_kind: String,
    /// 参与的 asset id 列表。
    #[serde(default)]
    pub asset_ids: Vec<String>,
    /// 命中的 capability id（None = 无 capability 绑定）。
    #[serde(default)]
    pub capability: Option<String>,
    /// 执行层级。
    #[serde(default)]
    pub execution_tier: ExecutionTier,
    /// 绑定裁决。
    #[serde(default)]
    pub verdict: BindingVerdict,
    /// 绑定置信度（0.0..1.0）。
    #[serde(default)]
    pub confidence: f32,
    /// 来源引用（透传自 facet / need）。
    #[serde(default)]
    pub source_refs: Vec<SourceRef>,
    /// 未解析原因（仅 Unsupported / SourceOnly 等填）。
    #[serde(default)]
    pub unresolved_reason: Option<String>,
}

// ── Capability id 常量集（11 个稳定 capability）─────────────────────────────
// resolver/registry 按这些 id + facet 数据绑定，**不按规则集 / 模组名**（零硬编码守卫）。

/// 掷骰。
pub const CAP_ROLL_DICE: &str = "roll.dice";
/// 检定：达标或超过（meet_or_beat）。
pub const CAP_CHECK_MEET_OR_BEAT: &str = "check.meet_or_beat";
/// 检定：低于目标（roll_under，如 CoC d100）。
pub const CAP_CHECK_ROLL_UNDER: &str = "check.roll_under";
/// 检定：数成功面（count_faces，如 Triangle）。
pub const CAP_CHECK_COUNT_FACES: &str = "check.count_faces";
/// 检定：对抗（opposed）。
pub const CAP_CHECK_OPPOSED: &str = "check.opposed";
/// 资源增减（resource delta）。
pub const CAP_RESOURCE_DELTA: &str = "resource.delta";
/// 施加状态 / 条件。
pub const CAP_CONDITION_APPLY: &str = "condition.apply";
/// 查表。
pub const CAP_TABLE_LOOKUP: &str = "table.lookup";
/// 角色卡补丁（actor patch）。
pub const CAP_ACTOR_PATCH: &str = "actor.patch";
/// 揭示可见性。
pub const CAP_VISIBILITY_REVEAL: &str = "visibility.reveal";
/// 时钟推进（clock tick）。
pub const CAP_CLOCK_TICK: &str = "clock.tick";

/// 全部 11 个稳定 capability id（registry 默认注册集 / 守卫）。
pub const ALL_CAPABILITIES: &[&str] = &[
    CAP_ROLL_DICE,
    CAP_CHECK_MEET_OR_BEAT,
    CAP_CHECK_ROLL_UNDER,
    CAP_CHECK_COUNT_FACES,
    CAP_CHECK_OPPOSED,
    CAP_RESOURCE_DELTA,
    CAP_CONDITION_APPLY,
    CAP_TABLE_LOOKUP,
    CAP_ACTOR_PATCH,
    CAP_VISIBILITY_REVEAL,
    CAP_CLOCK_TICK,
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn asset_envelope_roundtrip() {
        let source_ref = SourceRef {
            source_id: "coc_rulebook".to_string(),
            page: Some(88),
            anchor_id: Some("anchor-sanity".to_string()),
            section_path: vec!["Chapter 3".to_string(), "Sanity".to_string()],
            char_start: Some(0),
            char_end: Some(240),
            text_hash: Some("sha256:san".to_string()),
            note: Some("理智检定段".to_string()),
        };
        let facet = AssetFacet {
            facet_kind: "check".to_string(),
            binding_candidates: vec![CAP_CHECK_ROLL_UNDER.to_string(), CAP_ROLL_DICE.to_string()],
            confidence: 0.82,
            source_refs: vec![source_ref.clone()],
        };
        let envelope = AssetEnvelope {
            asset_id: "rule:sanity_check".to_string(),
            asset_kind: AssetKind::Rule,
            source_refs: vec![source_ref],
            visibility: Visibility::PlayerVisible,
            confidence: 0.9,
            lifecycle: AssetLifecycle::Active,
            facets: vec![facet],
            data: serde_json::json!({
                "tested_parameter": "sanity",
                "compare": "roll_under",
                "target": 50
            }),
        };

        let value = serde_json::to_value(&envelope).expect("serialize AssetEnvelope");
        let back: AssetEnvelope = serde_json::from_value(value).expect("deserialize AssetEnvelope");
        assert_eq!(envelope, back);
    }

    #[test]
    fn binding_plan_roundtrip() {
        let plan = BindingPlan {
            binding_id: "bind-1".to_string(),
            need_kind: "check".to_string(),
            asset_ids: vec!["rule:sanity_check".to_string()],
            capability: Some(CAP_CHECK_ROLL_UNDER.to_string()),
            execution_tier: ExecutionTier::ExactExecution,
            verdict: BindingVerdict::Exact,
            confidence: 0.95,
            source_refs: vec![SourceRef {
                source_id: "coc_rulebook".to_string(),
                page: Some(88),
                ..Default::default()
            }],
            unresolved_reason: None,
        };

        let value = serde_json::to_value(&plan).expect("serialize BindingPlan");
        let back: BindingPlan = serde_json::from_value(value).expect("deserialize BindingPlan");
        assert_eq!(plan, back);
        assert_eq!(back.verdict, BindingVerdict::Exact);
        assert_eq!(back.execution_tier, ExecutionTier::ExactExecution);
        assert_eq!(back.capability.as_deref(), Some(CAP_CHECK_ROLL_UNDER));
    }

    #[test]
    fn all_capabilities_has_11_unique() {
        assert_eq!(ALL_CAPABILITIES.len(), 11);
        let unique: HashSet<&&str> = ALL_CAPABILITIES.iter().collect();
        assert_eq!(unique.len(), 11, "ALL_CAPABILITIES has duplicate ids");
    }
}
