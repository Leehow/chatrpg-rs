//! 模组级"引导事实"(facilitation facts)抽取 —— 规则 agent 模组刀的收尾一刀,
//! 对标 module_reader_loop 的一次性 submit 契约。从**已解析**的入口场景 deep 内容
//! (read_aloud/gm_notes/summary)+ 被引用 NPC 人设 + spine 总览,语义抽取 director
//! 的模组级引导事实(`DirectorModuleConfig`),存进 `ModuleGraph.director_facilitation`,
//! 退役 `{id}.module_config.json` 的 REQUIRED 注入(override sidecar 仍最高优先级)。
//!
//! fail-closed:① 入口越界 → None(连 LLM 都不调);② 模型没调 submit_facilitation → None;
//! ③ 抽出来全空(没找到任何事实)→ None(绝不存空配置/编造)。
//! 锚定开场:懒抽取下解析时只有入口场景被深抽,引导事实自然 = 开场情境 + 模组总览。

use super::module_reader::ModuleReadout;
use super::tools;
use serde_json::{json, Value};
use trpg_llm::LlmClient;
use trpg_model::DirectorModuleConfig;

const FACIL_SYS: &str = "你是模组引导事实抽取器。基于**已解析的开场场景数据**(read_aloud/gm_notes/summary)\
与模组总览,为 GM director 抽取这个模组**开场**的引导事实:玩家一眼可见的现场事实、时间/局势压力、\
可着手的交互点、明显风险、开场 NPC 的有偏建议、最关键的待决问题、无地点时的开场地点概述。\n\
铁律(fail-closed):只抽文本里明写或强烈隐含的;**没有的就留空(空数组/不填),绝不编造**。\
npc_advice 只给确有立场/目的的 NPC 且 not_official_solution=true。最后**必须调用 submit_facilitation** 提交。";

/// 引导事实抽取门:**默认开**;`TRPG_MODULE_FACILITATION=0/false/off/no` 关
/// (对标 trpg-parser `module_reader_enabled` 的默认开形态)。
pub fn facilitation_enabled() -> bool {
    std::env::var("TRPG_MODULE_FACILITATION")
        .map(|v| !matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no"))
        .unwrap_or(true)
}

/// 从已解析 readout 的入口场景(idx)语义抽取模组级引导事实。见模块文档。
pub async fn extract_facilitation_facts(
    client: &dyn LlmClient,
    readout: &ModuleReadout,
    entry_idx: usize,
) -> Option<DirectorModuleConfig> {
    let entry = readout.scenes.get(entry_idx)?; // 越界 → 不调 LLM,直接 None
    // 开场涉及的 NPC(按入口 referenced_npc_ids 过滤),供 npc_advice 偏见人设。
    let referenced: Vec<&Value> = entry
        .referenced_npc_ids
        .iter()
        .filter_map(|id| {
            readout.npcs.iter().find(|n| {
                let key = n.get("id").or_else(|| n.get("npc_id")).and_then(Value::as_str);
                key == Some(id.as_str())
            })
        })
        .collect();
    let scene_json = serde_json::to_string(&json!({
        "node_id": entry.node_id, "title": entry.title, "kind": entry.node_type,
        "summary": entry.summary, "read_aloud": entry.read_aloud, "gm_notes": entry.gm_notes,
    }))
    .unwrap_or_default();
    let npcs_json = serde_json::to_string(&referenced).unwrap_or_default();
    let spine_json = serde_json::to_string(&readout.spine).unwrap_or_default();
    let user = format!(
        "模组总览(spine):\n{spine_json}\n\n开场场景(已解析):\n{scene_json}\n\n\
开场涉及的 NPC(供 npc_advice 偏见,只给确有立场的):\n{npcs_json}\n\n\
请据上面**已解析的开场数据**抽取该模组开场的 director 引导事实,fail-closed:没有的留空,绝不编造。\
最后必须调用 submit_facilitation。"
    );
    let msgs = vec![
        json!({"role": "system", "content": FACIL_SYS}),
        json!({"role": "user", "content": user}),
    ];
    let resp = client.complete_with_tools(msgs, vec![submit_facilitation_tool()]).await.ok()?;
    let tcs = resp.pointer("/choices/0/message/tool_calls").and_then(Value::as_array)?;
    let args = tcs.iter().find_map(|tc| {
        if tc.pointer("/function/name").and_then(Value::as_str) == Some("submit_facilitation") {
            tc.pointer("/function/arguments")
                .and_then(Value::as_str)
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
        } else {
            None
        }
    })?;
    // DirectorModuleConfig 全字段 #[serde(default)] → 部分对象也能反序列化(缺键=空)。
    let cfg: DirectorModuleConfig = serde_json::from_value(args).ok()?;
    if is_empty(&cfg) {
        return None; // 调了工具但什么都没填 → 没找到 → 不存空配置。
    }
    Some(cfg)
}

/// 全空判定:6 条 vec 皆空且两个 Option 皆 None → 视作"没抽到任何事实"。
fn is_empty(c: &DirectorModuleConfig) -> bool {
    c.scene_facts.is_empty()
        && c.pressure_items.is_empty()
        && c.affordance_items.is_empty()
        && c.risk_items.is_empty()
        && c.npc_advice.is_empty()
        && c.known_facts.is_empty()
        && c.open_question.is_none()
        && c.place_summary_fallback.is_none()
}

/// submit_facilitation 工具 schema —— 镜像 `DirectorModuleConfig`(trpg-model)。
/// implies_vectors/related_vectors 是 ActionVector serde 名(如 Observe/Technical/Tactical/Social),
/// 消费侧 parse_vectors 已 fail-soft(未知名丢弃、空则用中性默认),此处不强约束枚举。
fn submit_facilitation_tool() -> Value {
    tools::submit_tool(
        "submit_facilitation",
        "Submit the module's OPENING director facilitation overlay extracted from the parsed opening \
scene + spine. Omit (empty array / unset) anything not present in the text; never invent.",
        json!({
            "scene_facts": {"type": "array", "description": "玩家一眼可见的现场事实",
                "items": {"type": "object", "properties": {
                    "text": {"type": "string"}, "source": {"type": "string"}}, "required": ["text"]}},
            "pressure_items": {"type": "array", "description": "开场的时间/局势压力",
                "items": {"type": "object", "properties": {
                    "text": {"type": "string"}, "clock_id": {"type": "string"},
                    "severity": {"type": "integer"}, "consequence_hint": {"type": "string"}}, "required": ["text"]}},
            "affordance_items": {"type": "array", "description": "可交互的着手点/handle",
                "items": {"type": "object", "properties": {
                    "description": {"type": "string"},
                    "implies_vectors": {"type": "array", "items": {"type": "string"}}}, "required": ["description"]}},
            "risk_items": {"type": "array", "description": "明显风险",
                "items": {"type": "object", "properties": {
                    "text": {"type": "string"},
                    "related_vectors": {"type": "array", "items": {"type": "string"}},
                    "severity": {"type": "integer"}}, "required": ["text"]}},
            "npc_advice": {"type": "array", "description": "开场 NPC 的有偏建议(只给确有立场/目的者,not_official_solution=true)",
                "items": {"type": "object", "properties": {
                    "npc_id": {"type": "string"}, "speaker_label": {"type": "string"},
                    "advice_text": {"type": "string"}, "bias_or_goal": {"type": "string"},
                    "not_official_solution": {"type": "boolean"}}, "required": ["npc_id", "speaker_label", "advice_text"]}},
            "known_facts": {"type": "array", "description": "玩家已知/背景事实", "items": {"type": "string"}},
            "open_question": {"type": "string", "description": "开场最关键的待决问题(没有就不填)"},
            "open_question_points_to": {"type": "array", "description": "open_question 的决策选项 tag", "items": {"type": "string"}},
            "place_summary_fallback": {"type": "string", "description": "无 location 时的开场地点一句话概述"}
        }),
        &["scene_facts"],
    )
}

#[cfg(test)]
#[path = "facilitation_tests.rs"]
mod tests;
