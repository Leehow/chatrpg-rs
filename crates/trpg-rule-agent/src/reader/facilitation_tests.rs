//! facilitation.rs 的单元测试(物理拆出守 ≤400 行)。`use super::*` 仍是 facilitation
//! 的子模块。测试用回放 LLM:complete_with_tools 回放一个 submit_facilitation tool_call。
use super::*;
use serde_json::json;
use std::sync::Mutex;
use trpg_model::ScenarioNode;

/// 回放 LLM:`args` 非 null → 回放一个 `tool_name` 的 tool_call(args 即提交内容);
/// `args` 为 null → 回放无 tool_calls 的空消息(模拟模型没调工具)。记录最后 user content。
struct FacilClient {
    tool_name: String,
    args: Value,
    seen_user: Mutex<String>,
}

impl FacilClient {
    fn submitting(args: Value) -> Self {
        Self { tool_name: "submit_facilitation".into(), args, seen_user: Mutex::new(String::new()) }
    }
    fn no_tool() -> Self {
        Self { tool_name: "x".into(), args: Value::Null, seen_user: Mutex::new(String::new()) }
    }
}

#[async_trait::async_trait]
impl LlmClient for FacilClient {
    async fn complete_text(&self, _m: Vec<trpg_model::ChatMessage>, _t: f32) -> anyhow::Result<String> {
        Ok(String::new())
    }
    async fn complete_json(&self, _m: Vec<trpg_model::ChatMessage>, _t: f32) -> anyhow::Result<Value> {
        Ok(json!({}))
    }
    async fn stream_chat(
        &self,
        _m: Vec<trpg_model::ChatMessage>,
        _t: f32,
    ) -> anyhow::Result<std::pin::Pin<Box<dyn futures_core::Stream<Item = anyhow::Result<String>> + Send>>>
    {
        anyhow::bail!("unused")
    }
    async fn complete_with_tools(&self, messages: Vec<Value>, _tools: Vec<Value>) -> anyhow::Result<Value> {
        if let Some(u) = messages.iter().rev().find(|m| m.get("role").and_then(Value::as_str) == Some("user")) {
            *self.seen_user.lock().unwrap() = u.get("content").and_then(Value::as_str).unwrap_or("").to_string();
        }
        if self.args.is_null() {
            return Ok(json!({"choices":[{"message":{}}]})); // 没调工具
        }
        Ok(json!({"choices":[{"message":{"tool_calls":[{
            "id":"c1","type":"function",
            "function":{"name": self.tool_name, "arguments": serde_json::to_string(&self.args).unwrap()}
        }]}}]}))
    }
}

fn readout_with_entry() -> ModuleReadout {
    let mut out = ModuleReadout::default();
    let mut s = ScenarioNode::default();
    s.node_id = "sc01".into();
    s.title = "加油站".into();
    s.node_type = "scene".into();
    s.read_aloud = Some("你们停在一座破败的加油站前,门半开着。".into());
    s.referenced_npc_ids = vec!["npc.attendant".into()];
    out.scenes = vec![s];
    out.npcs = vec![json!({"id":"npc.attendant","name":"加油站员工","bias":"想赶你们走"})];
    out.spine = json!({"title":"血色公路","overview":"一夜的求生公路之旅"});
    out
}

#[tokio::test]
async fn extracts_facts_from_entry_scene() {
    let args = json!({
        "scene_facts":[{"text":"加油站的门半开着","source":"read_aloud"}],
        "pressure_items":[{"text":"夜色渐深","severity":2}],
        "npc_advice":[{"npc_id":"npc.attendant","speaker_label":"加油站员工","advice_text":"快走吧","bias_or_goal":"想赶你们走","not_official_solution":true}],
        "open_question":"你们要不要进加油站?"
    });
    let client = FacilClient::submitting(args);
    let out = readout_with_entry();
    let cfg = extract_facilitation_facts(&client, &out, 0).await.expect("应抽出非空引导事实");
    assert_eq!(cfg.scene_facts.len(), 1);
    assert_eq!(cfg.scene_facts[0].text, "加油站的门半开着");
    assert_eq!(cfg.pressure_items.len(), 1);
    assert_eq!(cfg.npc_advice.len(), 1);
    assert_eq!(cfg.open_question.as_deref(), Some("你们要不要进加油站?"));
    // 证"从解析数据读":入口场景的 read_aloud 必须进了 prompt。
    assert!(client.seen_user.lock().unwrap().contains("门半开着"), "入口 read_aloud 应进 prompt");
}

#[tokio::test]
async fn no_tool_call_is_fail_closed_none() {
    let client = FacilClient::no_tool();
    let out = readout_with_entry();
    assert!(extract_facilitation_facts(&client, &out, 0).await.is_none(), "没调工具→None");
}

#[tokio::test]
async fn all_empty_config_is_none() {
    // 模型调了工具但什么都没填 → 视作"没找到",不存空配置。
    let client = FacilClient::submitting(json!({"scene_facts":[],"pressure_items":[]}));
    let out = readout_with_entry();
    assert!(extract_facilitation_facts(&client, &out, 0).await.is_none(), "全空→None");
}

#[tokio::test]
async fn entry_idx_out_of_range_is_none() {
    // 没有任何场景 → 不调 LLM,直接 None。
    let client = FacilClient::submitting(json!({"scene_facts":[{"text":"x","source":"y"}]}));
    let out = ModuleReadout::default();
    assert!(extract_facilitation_facts(&client, &out, 0).await.is_none(), "入口越界→None");
}
