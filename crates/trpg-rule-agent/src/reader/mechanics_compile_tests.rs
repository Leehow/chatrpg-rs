//! mechanics_compile 的单元测试（物理拆出守 ≤400 行）。
//! 经 `#[cfg(test)] #[path = "mechanics_compile_tests.rs"] mod tests;` 引入，
//! 仍是 mechanics_compile 的子模块：`use super::*` + 私有 fn 可见性不变。
use super::*;
use std::collections::VecDeque;
use std::sync::Mutex;

/// 测试用脚本 LLM：complete_with_tools 依次回放预置响应，脚本耗尽后回
/// 「无 tool_calls」的空响应（驱动 loop 走回填提醒直至预算耗尽）。
/// 同时记录每次收到的 messages，用于断言工具结果确实被回填进对话。
struct ScriptClient {
    responses: Mutex<VecDeque<Value>>,
    seen: Mutex<Vec<Vec<Value>>>,
}

impl ScriptClient {
    fn new(responses: Vec<Value>) -> Self {
        Self { responses: Mutex::new(responses.into()), seen: Mutex::new(Vec::new()) }
    }
}

#[async_trait::async_trait]
impl LlmClient for ScriptClient {
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
        self.seen.lock().unwrap().push(messages);
        let next = self.responses.lock().unwrap().pop_front();
        Ok(next.unwrap_or_else(|| json!({"choices":[{"message":{"content":"done"}}]})))
    }
}

fn tool_call_resp(name: &str, args: Value) -> Value {
    json!({"choices":[{"message":{"tool_calls":[{
        "id":"c1","type":"function",
        "function":{"name":name,"arguments": serde_json::to_string(&args).unwrap()}
    }]}}]})
}

fn submit_resp(catalog: Value) -> Value {
    tool_call_resp("submit_mechanics", json!({ "mechanics_catalog": catalog }))
}

fn src_ref() -> Value {
    json!({"source_id":"book","page":12,"section_path":[]})
}

fn kernel_fixture() -> RuleKernel {
    let mut k = RuleKernel::default();
    k.ruleset_id = "testgame".into();
    k.character_sheet_schema = json!({
        "fields": [{"field_id":"strength"}],
        "derived_values": [{"field_id":"hp_max"}]
    });
    k.resource_tracks = vec![json!({
        "id": "sanity",
        "thresholds": [{"at":0,"direction":"at_or_below","consequence":"out of play"}]
    })];
    k.dice_core = json!({"success_bands":[{"id":"critical"},{"id":"regular"}]});
    k
}

fn mech_ctx(skills: &[&str]) -> MechCompileCtx<'static> {
    MechCompileCtx {
        units: &[],
        sidecar_text: None,
        located_pages: String::new(),
        skill_names: skills.iter().map(|s| s.to_string()).collect(),
    }
}

#[tokio::test]
async fn submit_two_rounds_merge_by_id() {
    let client = ScriptClient::new(vec![
        submit_resp(json!([
            {"id":"x.a","name":"v1","source_refs":[src_ref()]},
            {"id":"x.b","name":"b","source_refs":[src_ref()]}
        ])),
        submit_resp(json!([{"id":"X.A","name":"v2","source_refs":[src_ref()]}])),
    ]);
    let mut kernel = kernel_fixture();
    let _gaps = compile_mechanics_catalog(&client, &mut kernel, mech_ctx(&[]), 0).await;
    assert_eq!(kernel.mechanics_catalog.len(), 2, "union of both rounds");
    let a = kernel
        .mechanics_catalog
        .iter()
        .find(|e| e.id.eq_ignore_ascii_case("x.a"))
        .expect("entry x.a present");
    assert_eq!(a.name, "v2", "later round wins, case-insensitive merge");
    assert!(
        kernel.mechanics_catalog.iter().any(|e| e.id.eq_ignore_ascii_case("x.b")),
        "x.b kept (union)"
    );
}

#[tokio::test]
async fn no_submit_keeps_catalog_empty() {
    let client = ScriptClient::new(vec![]); // 耗尽预算，从不调 submit
    let mut kernel = kernel_fixture();
    let before_tracks = serde_json::to_string(&kernel.resource_tracks).unwrap();
    let before_dice = serde_json::to_string(&kernel.dice_core).unwrap();
    let gaps = compile_mechanics_catalog(&client, &mut kernel, mech_ctx(&[]), 0).await;
    assert!(kernel.mechanics_catalog.is_empty());
    assert!(!gaps.is_empty());
    assert!(gaps.iter().any(|g| g.contains("produced nothing")), "got: {gaps:?}");
    assert_eq!(
        serde_json::to_string(&kernel.resource_tracks).unwrap(),
        before_tracks,
        "resource_tracks serde bytes unchanged"
    );
    assert_eq!(
        serde_json::to_string(&kernel.dice_core).unwrap(),
        before_dice,
        "dice_core serde bytes unchanged"
    );
}

#[tokio::test]
async fn uncoercible_entry_becomes_gap_not_panic() {
    let client = ScriptClient::new(vec![submit_resp(json!([
        {"id":"x.bad"},
        {"id":"x.good","name":"Good","source_refs":[src_ref()]}
    ]))]);
    let mut kernel = kernel_fixture();
    let gaps = compile_mechanics_catalog(&client, &mut kernel, mech_ctx(&[]), 0).await;
    assert_eq!(kernel.mechanics_catalog.len(), 1, "valid entry enters the catalog");
    assert_eq!(kernel.mechanics_catalog[0].id, "x.good");
    assert!(gaps.iter().any(|g| g.contains("x.bad")), "garbage entry becomes a gap note: {gaps:?}");
}

#[tokio::test]
async fn read_layout_without_sidecar_degrades() {
    let client = ScriptClient::new(vec![
        tool_call_resp("read_layout", json!({"pages":"3"})),
        submit_resp(json!([{"id":"x.a","name":"a","source_refs":[src_ref()]}])),
    ]);
    let mut kernel = kernel_fixture();
    let _gaps = compile_mechanics_catalog(&client, &mut kernel, mech_ctx(&[]), 0).await;
    let seen = client.seen.lock().unwrap();
    let degraded = seen.iter().flatten().any(|m| {
        m.get("role").and_then(Value::as_str) == Some("tool")
            && m.get("content").and_then(Value::as_str).unwrap_or("").contains("[no layout view available")
    });
    assert!(degraded, "read_layout without sidecar returns the degraded hint text");
    assert_eq!(kernel.mechanics_catalog.len(), 1, "loop continued (not aborted) after the degraded tool result");
}

/// （A3 集成半边）ReplayClient submit 坏条目 → kernel.validation_report.warnings
/// 含对应 code（写回接线生效的证据）。
#[tokio::test]
async fn validation_messages_written_to_kernel_report() {
    let client = ScriptClient::new(vec![submit_resp(json!([
        {"id":"x.bad_param","name":"Bad","tested_parameter":"nonexistent_xyz","source_refs":[src_ref()]},
        {"id":"x.good","name":"Good","source_refs":[src_ref()]}
    ]))]);
    let mut kernel = kernel_fixture();
    let _gaps = compile_mechanics_catalog(&client, &mut kernel, mech_ctx(&[]), 0).await;
    assert_eq!(kernel.mechanics_catalog.len(), 1, "only the good entry enters");
    assert_eq!(kernel.mechanics_catalog[0].id, "x.good");
    assert!(
        kernel.validation_report.warnings.iter().any(|w| {
            w.code == "mechanic_dropped_unknown_parameter" && w.target.as_deref() == Some("x.bad_param")
        }),
        "warnings: {:?}",
        kernel.validation_report.warnings
    );
}

/// on_outcome `=field` 守卫接线：resource_tracks 来自 reader 主 pass，即使本
/// pass 零提交也要被审计 —— 死引用规则被丢、写 validation_report、回声进 gaps。
#[tokio::test]
async fn on_outcome_guard_runs_even_without_submissions() {
    let client = ScriptClient::new(vec![]); // 从不 submit
    let mut kernel = kernel_fixture();
    kernel.resource_tracks = vec![json!({
        "id":"chaos","owner_kind":"scene",
        "on_outcome":[{"trigger":"always","op":"add","amount":"=chaos_generated"}]
    })];
    let gaps = compile_mechanics_catalog(&client, &mut kernel, mech_ctx(&[]), 0).await;
    assert!(
        kernel.resource_tracks[0]["on_outcome"].as_array().unwrap().is_empty(),
        "dead rule dropped: {:?}",
        kernel.resource_tracks
    );
    assert!(
        kernel.validation_report.warnings.iter().any(|w| w.code == "on_outcome_dropped_unknown_outcome_field"),
        "warnings: {:?}",
        kernel.validation_report.warnings
    );
    assert!(
        gaps.iter().any(|g| g.contains("on_outcome_dropped_unknown_outcome_field")),
        "echoed as gap note: {gaps:?}"
    );
}

#[test]
fn seed_contains_sheet_keys_and_track_ids() {
    let kernel = kernel_fixture();
    let ctx = mech_ctx(&["jump"]);
    let seed = build_mech_seed(&kernel, &ctx);
    assert!(seed.contains("strength"), "sheet field_id in seed: {seed}");
    assert!(seed.contains("sanity"), "resource track id in seed: {seed}");
    assert!(seed.contains("hp_max"), "derived field_id in seed");
    assert!(seed.contains("jump"), "option-catalog skill name in seed");
}
