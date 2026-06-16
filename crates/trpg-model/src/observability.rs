//! 可观测性 + 失败语义切片的共享模型类型（obs slice T1）。
//!
//! 这些是整条切片其余任务（trpg-gm / trpg-runtime / trpg-need / trpg-db /
//! trpg-api / trpg-cli）共同依赖的**单一事实源契约**：
//! - `NeedResolutionTrace`：P1-4，单条 Need 取数的来源 trace（source_refs 不再丢弃）。
//! - `PhaseErrorPolicy`：P1-5，阶段失败处置策略（中止 / 警告续跑 / 后台仅 warn）。
//! - `TurnFailureRecord`：回合失败记录（落 turns.failure_kind + TurnTrace）。
//! - `TurnTrace`：Flight Recorder 初版，单回合飞行记录（持久化到 turn_traces）。
//!
//! 全部字段 `#[serde(default)]`，新增/缺字段向后兼容（旧/残行可加载）。

use crate::asset::BindingPlan;
use crate::SourceRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// 单条 Need 取数的来源 trace（P1-4：source_refs 不再丢弃）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
pub struct NeedResolutionTrace {
    /// Need 类型（规则 / 材料 / 场景 / 实体 / 参数 等）。
    #[serde(default)]
    pub need_kind: String,
    /// 本次取数命中的来源引用（页/锚/段路径），来自 NeedOutcome.source_refs。
    #[serde(default)]
    pub source_refs: Vec<SourceRef>,
    /// 取数原因 / 触发说明（人读）。
    #[serde(default)]
    pub reason: String,
    /// 本次注入的 context block 数量。
    #[serde(default)]
    pub block_count: usize,
}

/// 阶段失败处置策略（P1-5）。
///
/// - `AbortTurn`：critical（context_assembly / mode_inference / finalize(save_turn)）→ 中止回合。
/// - `EmitWarningContinue`：non-critical（verify / memory / audit）→ 发 TurnWarning 后继续。
/// - `BackgroundWarnOnly`：heavy 隔离内（深抽 / frontier / learning / carryover）→ 仅 `warn!`。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum PhaseErrorPolicy {
    /// 默认：critical 阶段失败 → 中止回合（fail-closed 不伪装成功）。
    #[default]
    AbortTurn,
    /// 发 TurnWarning 并继续主流程。
    EmitWarningContinue,
    /// 后台隔离阶段，仅 `warn!`，不发事件不阻塞。
    BackgroundWarnOnly,
}

/// 回合失败记录（落 turns.failure_kind + TurnTrace）。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct TurnFailureRecord {
    /// 失败发生的阶段 id（如 context_assembly / mode_inference / finalize）。
    #[serde(default)]
    pub phase: String,
    /// 人读失败信息。
    #[serde(default)]
    pub message: String,
    /// 落 turns.failure_kind 的归类值（如 failed_context / failed_mode / failed_finalize）。
    #[serde(default)]
    pub failure_kind: String,
}

/// 单回合飞行记录（Flight Recorder 初版）。
///
/// write-through、fail-soft 捕获：写失败仅 warn，绝不影响回合主流程或已发事件。
/// 只存 hash + need 来源元数据，**不存私密 prompt 正文**（避免 explain 泄底）。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct TurnTrace {
    /// 回合 id（turn_traces 主键）。
    #[serde(default)]
    pub turn_id: String,
    /// 所属会话 id。
    #[serde(default)]
    pub session_id: String,
    /// 本回合实际跑过的阶段序列。
    #[serde(default)]
    pub phases_run: Vec<String>,
    /// 本回合 Need 取数的来源 trace 列表（P1-4）。
    #[serde(default)]
    pub need_trace: Vec<NeedResolutionTrace>,
    /// BP1（Prefix）装配 hash。
    #[serde(default)]
    pub bp1_hash: Option<String>,
    /// BP2（PinnedMiddle）装配 hash。
    #[serde(default)]
    pub bp2_hash: Option<String>,
    /// BP3（DynamicTail）装配 hash。
    #[serde(default)]
    pub bp3_hash: Option<String>,
    /// 回合信号 / 终态标记（成功路径等价时取既有 signal）。
    #[serde(default)]
    pub signal: String,
    /// 失败记录（成功回合为 None）。
    #[serde(default)]
    pub failure: Option<TurnFailureRecord>,
    /// 本回合累积的警告（WarnContinue 阶段失败等）。
    #[serde(default)]
    pub warnings: Vec<String>,
    /// 后处理生命周期状态（streaming / critical_done / complete）。
    #[serde(default)]
    pub pp_lifecycle: String,
    /// 收尾算出的念白 hash。
    #[serde(default)]
    pub narration_hash: Option<String>,
    /// 本回合**影子** BindingResolver 产出的绑定计划（advisory，不改实际结算）。
    /// 附加 serde-default 字段：旧 JSON（无该字段）反序列化为空 Vec，向后兼容。
    #[serde(default)]
    pub binding_trace: Vec<BindingPlan>,
}

impl TurnTrace {
    /// 便捷构造：返回 Default 并设好 turn_id / session_id（runtime 起 trace 用）。
    pub fn new(turn_id: &str, session_id: &str) -> Self {
        TurnTrace {
            turn_id: turn_id.to_string(),
            session_id: session_id.to_string(),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_trace_roundtrip() {
        let source_ref = SourceRef {
            source_id: "coc_rulebook".to_string(),
            page: Some(42),
            anchor_id: Some("anchor-7".to_string()),
            section_path: vec!["Chapter 3".to_string(), "Sanity".to_string()],
            char_start: Some(10),
            char_end: Some(120),
            text_hash: Some("sha256:abc".to_string()),
            note: Some("命中理智检定段".to_string()),
        };
        let need = NeedResolutionTrace {
            need_kind: "rule".to_string(),
            source_refs: vec![source_ref],
            reason: "Sanity roll 触发规则取数".to_string(),
            block_count: 2,
        };
        let mut trace = TurnTrace::new("turn-1", "session-9");
        trace.phases_run = vec!["context_assembly".to_string(), "finalize".to_string()];
        trace.need_trace = vec![need];
        trace.bp1_hash = Some("sha256:bp1".to_string());
        trace.bp2_hash = Some("sha256:bp2".to_string());
        trace.bp3_hash = Some("sha256:bp3".to_string());
        trace.signal = "turn_complete".to_string();
        trace.warnings = vec!["audit lag".to_string()];
        trace.pp_lifecycle = "complete".to_string();
        trace.narration_hash = Some("sha256:narr".to_string());
        trace.failure = Some(TurnFailureRecord {
            phase: "finalize".to_string(),
            message: "save_turn timeout".to_string(),
            failure_kind: "failed_finalize".to_string(),
        });

        let json = serde_json::to_string(&trace).expect("serialize TurnTrace");
        let back: TurnTrace = serde_json::from_str(&json).expect("deserialize TurnTrace");
        assert_eq!(trace, back);
    }

    #[test]
    fn turn_trace_back_compat() {
        // 旧/残行：只有 ids，其余字段缺失 —— #[serde(default)] 应补默认值。
        let json = r#"{"turn_id":"t","session_id":"s"}"#;
        let trace: TurnTrace = serde_json::from_str(json).expect("deserialize minimal TurnTrace");
        assert_eq!(trace.turn_id, "t");
        assert_eq!(trace.session_id, "s");
        assert!(trace.phases_run.is_empty());
        assert!(trace.need_trace.is_empty());
        assert!(trace.warnings.is_empty());
        assert_eq!(trace.bp1_hash, None);
        assert_eq!(trace.bp2_hash, None);
        assert_eq!(trace.bp3_hash, None);
        assert_eq!(trace.narration_hash, None);
        assert_eq!(trace.failure, None);
        assert_eq!(trace.signal, "");
        assert_eq!(trace.pp_lifecycle, "");
        assert!(trace.binding_trace.is_empty());
    }

    #[test]
    fn turn_trace_binding_trace_back_compat() {
        // 附加 serde-default 字段 binding_trace：旧 JSON（无该字段）应反序列化为空 Vec，
        // 证明这是向后兼容的附加字段（不破坏 obs 切片的 TurnTrace JSON）。
        let json = r#"{"turn_id":"t","session_id":"s"}"#;
        let trace: TurnTrace = serde_json::from_str(json).expect("deserialize minimal TurnTrace");
        assert_eq!(trace.turn_id, "t");
        assert_eq!(trace.session_id, "s");
        assert!(trace.binding_trace.is_empty());
    }

    #[test]
    fn compiled_context_default_need_trace_empty() {
        // 向后兼容（P1-4）：新增 need_trace 字段在 Default 下为空、旧 JSON（无该字段）
        // 经 #[serde(default)] 仍可加载且 need_trace 为空。
        let ctx = crate::CompiledContext::default();
        assert!(ctx.need_trace.is_empty());

        let json = r#"{"prefix_blocks":[],"pinned_blocks":[],"dynamic_blocks":[],
            "prefix_text":"","pinned_text":"","dynamic_text":"",
            "prefix_hash":"","pinned_hash":"","dynamic_hash":"",
            "visibility_signature":"","cache_key":"","token_estimate":0,
            "block_version_ids":[]}"#;
        let back: crate::CompiledContext =
            serde_json::from_str(json).expect("deserialize legacy CompiledContext");
        assert!(back.need_trace.is_empty());
    }

    #[test]
    fn phase_error_policy_serde() {
        for policy in [
            PhaseErrorPolicy::AbortTurn,
            PhaseErrorPolicy::EmitWarningContinue,
            PhaseErrorPolicy::BackgroundWarnOnly,
        ] {
            let json = serde_json::to_string(&policy).expect("serialize policy");
            let back: PhaseErrorPolicy = serde_json::from_str(&json).expect("deserialize policy");
            assert_eq!(policy, back);
        }
        // Default 应为 AbortTurn（fail-closed）。
        assert_eq!(PhaseErrorPolicy::default(), PhaseErrorPolicy::AbortTurn);
    }
}
