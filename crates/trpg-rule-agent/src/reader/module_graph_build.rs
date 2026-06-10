//! 图谱编排：把骨架装配成图谱（实体去重 + 全局桥接边）。连通诊断（validate）由 run_module_reader
//! 在 Pass B 入口深抽 + 桥接 + 入口兜底之后调用，那时测才反映真实可达性（此处 Pass B 前测会误报）。
//! 桥接边说明：骨架 Pass A 的 LLM links 非确定且稀疏（实测 4~34 条飘、入口常拿 0），单靠它图谱
//! 时常不连通。桥接是纯集合运算（对已抽的 referenced_*_ids 求交集），**零 token**，与"深抽懒化省
//! token"不冲突——省的是深抽，桥接免费、确定。Pass C 线索边仍留在 per-scene 深抽。fail-closed。
use super::module_graph_edges::{apply_bridge_edges, dedup_entities_by_key};
use super::module_reader::{ModuleReaderCtx, ModuleReadout};
use serde_json::Value;

/// 装配图谱：① 实体去重（同时把场景 referenced_*_ids 旧 id 重写到规范 id）② 全局桥接边。
/// 全程纯（无 LLM）、fail-closed。连通诊断不在此（见模块头注）。
///
/// run_module_reader 在 out 装配后、Pass B 前调一行。保持 client/budget 形参（占位、未使用）
/// 以最小化上层改动；返工后图谱构建已无 LLM 调用。
pub(super) async fn build_graph_with_quality_gate(
    _client: &dyn trpg_llm::LlmClient,
    _ctx: &ModuleReaderCtx<'_>,
    out: &mut ModuleReadout,
    _skeleton: &Value,
    _budget: usize,
) {
    // ① 实体去重（npc/clue/location；同时把场景 referenced_*_ids 里的旧 id 重写到规范 id）。
    dedup_entities_by_key(&mut out.npcs, &mut out.scenes);
    dedup_entities_by_key(&mut out.clues, &mut out.scenes);
    dedup_entities_by_key(&mut out.locations, &mut out.scenes);

    // ② 全局桥接边（零 LLM）：共享 referenced 实体的场景互连，按 to_node_id 去重（不与骨架/深抽边重复）。
    // dedup 之后调用 → referenced id 已规范化，跨场景实体匹配才准确。
    let bridges = apply_bridge_edges(&mut out.scenes);
    tracing::info!(target: "module_reader", phase = "bridge_edges", added = bridges, scenes = out.scenes.len(), "applied global entity-bridge edges (pre Pass B)");
}
