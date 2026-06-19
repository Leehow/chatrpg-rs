//! module_reader_loop 的单元测试（从 module_reader_loop.rs 物理拆出守 ≤400 行）。
//! 经 `#[cfg(test)] #[path = "module_reader_loop_tests.rs"] mod tests;` 引入，
//! 仍是 module_reader_loop 的子模块：`use super::*` + 私有 fn 可见性不变。
use super::*;
use trpg_model::{ScenarioNode, SceneExtractionStatus};

// ---- 一次性深抽（RW1）测试脚手架 ----

/// 测试用 LLM：complete_with_tools 回放一个 submit_deep 的 tool_call（args 来自构造）。
/// 同时记录最后一次收到的 user content，用于断言「页面文本确实被切进 prompt」。
struct ReplayClient {
    deep_args: Value,
    seen_user: std::sync::Mutex<String>,
}

#[async_trait::async_trait]
impl LlmClient for ReplayClient {
    async fn complete_text(
        &self,
        _m: Vec<trpg_model::ChatMessage>,
        _t: f32,
    ) -> anyhow::Result<String> {
        Ok(String::new())
    }
    async fn complete_json(
        &self,
        _m: Vec<trpg_model::ChatMessage>,
        _t: f32,
    ) -> anyhow::Result<Value> {
        Ok(json!({}))
    }
    async fn stream_chat(
        &self,
        _m: Vec<trpg_model::ChatMessage>,
        _t: f32,
    ) -> anyhow::Result<
        std::pin::Pin<Box<dyn futures_core::Stream<Item = anyhow::Result<String>> + Send>>,
    > {
        anyhow::bail!("unused")
    }
    async fn complete_with_tools(
        &self,
        messages: Vec<Value>,
        _tools: Vec<Value>,
    ) -> anyhow::Result<Value> {
        if let Some(u) = messages
            .iter()
            .rev()
            .find(|m| m.get("role").and_then(Value::as_str) == Some("user"))
        {
            *self.seen_user.lock().unwrap() = u
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
        }
        Ok(json!({"choices":[{"message":{"tool_calls":[{
            "id":"c1","type":"function",
            "function":{"name":"submit_deep","arguments": serde_json::to_string(&self.deep_args).unwrap()}
        }]}}]}))
    }
}

fn paged_scene(id: &str, title: &str, start: u32) -> ScenarioNode {
    let mut n = ScenarioNode::default();
    n.node_id = id.into();
    n.title = title.into();
    n.node_type = "scene".into();
    n.page_start = Some(start);
    n
}

fn sidecar(page: u32, body: &str) -> String {
    format!("<!-- source_id=x page={page} -->\n{body}\n")
}

#[tokio::test]
async fn oneshot_slices_page_and_filters_links_fail_closed() {
    // 入口场景 s0@p16 + 候选 s1（TOC 相邻）；s2 不存在于场景集。
    let mut out = ModuleReadout::default();
    out.scenes = vec![
        paged_scene("s0", "加油站", 16),
        paged_scene("s1", "小镇广场", 17),
    ];
    let ctx = ModuleReaderCtx {
        units: &[],
        sidecar_text: Some(sidecar(16, "你们看到一座破败的加油站。穿过铁门可到广场。")),
        ruleset_id: None,
    };
    // 一个好边（候选 s1 + anchor）、一个无 anchor 边、一个野 to（s2 不在场景集）。
    let deep = json!({"scene":{
        "read_aloud":"你们看到一座破败的加油站。",
        "links":[
            {"to_node_id":"s1","link_type":"spatial","reason":"穿门","source_anchor":"穿过铁门可到广场"},
            {"to_node_id":"s1","link_type":"trigger","reason":"无锚","source_anchor":""},
            {"to_node_id":"s2","link_type":"spatial","reason":"幻觉","source_anchor":"无中生有"}
        ]
    }});
    let client = ReplayClient {
        deep_args: deep,
        seen_user: std::sync::Mutex::new(String::new()),
    };
    let flipped = deep_extract_scene_in_place(&client, &ctx, &mut out, 0, 6).await;
    assert!(flipped, "有念白 → 翻 DeepExtracted");
    assert_eq!(
        out.scenes[0].extraction_status,
        SceneExtractionStatus::DeepExtracted
    );
    // fail-closed：只保留候选内 + 带 anchor 的那条。
    assert_eq!(out.scenes[0].links.len(), 1, "无 anchor 丢、野 to 丢");
    assert_eq!(out.scenes[0].links[0].to_node_id, "s1");
    assert_eq!(
        out.scenes[0].links[0].source_anchor.as_deref(),
        Some("穿过铁门可到广场")
    );
    // 验证页面文本确实被切进 prompt（一次性路径，非 ReAct）。
    let seen = client.seen_user.lock().unwrap().clone();
    assert!(
        seen.contains("你们看到一座破败的加油站"),
        "切页文本进了 prompt"
    );
    assert!(seen.contains("候选出口"), "候选出口列表进了 prompt");
}

#[tokio::test]
async fn deep_extract_preserves_preexisting_anchorless_links() {
    // 回归：场景深抽前已有骨架/桥接边（source_anchor=None）→ 收口 retain 必须保留它们，
    // 只对深抽**新提交**的无锚边做 fail-closed 过滤。否则到场深抽会清空该场景的导航边。
    use trpg_model::{LinkType, ScenarioLink};
    let mut out = ModuleReadout::default();
    out.scenes = vec![
        paged_scene("s0", "加油站", 16),
        paged_scene("s1", "广场", 17),
    ];
    // s0 已有一条指向 s1 的桥接边（无 anchor）。
    out.scenes[0].links = vec![ScenarioLink {
        to_node_id: "s1".into(),
        reason: "共享 1 个实体".into(),
        clue_id: None,
        link_type: LinkType::Spatial,
        source_anchor: None,
    }];
    let ctx = ModuleReaderCtx {
        units: &[],
        sidecar_text: Some(sidecar(16, "你们看到一座破败的加油站。")),
        ruleset_id: None,
    };
    // 深抽只提交一条「无锚新边」(s1 重复) 和一条「野 to」(s2)——都该被 fail-closed 丢。
    let deep = json!({"scene":{
        "read_aloud":"你们看到一座破败的加油站。",
        "links":[
            {"to_node_id":"s1","link_type":"trigger","reason":"无锚不升级","source_anchor":""},
            {"to_node_id":"s2","link_type":"spatial","reason":"幻觉","source_anchor":"无中生有"}
        ]
    }});
    let client = ReplayClient {
        deep_args: deep,
        seen_user: std::sync::Mutex::new(String::new()),
    };
    deep_extract_scene_in_place(&client, &ctx, &mut out, 0, 6).await;
    // 既有桥接边 s0→s1 存活（不被深抽锚要求误删）；野 to s2 丢。
    assert_eq!(out.scenes[0].links.len(), 1, "保留既有桥接边、丢野 to");
    assert_eq!(out.scenes[0].links[0].to_node_id, "s1");
    assert!(
        out.scenes[0].links[0].source_anchor.is_none(),
        "既有边保持原样未被无锚提交覆盖"
    );
}

#[tokio::test]
async fn oneshot_deep_carries_scene_mechanics_fail_closed() {
    // 深抽提交带 2 条 intents（1 条无 anchor）→ 只有带锚那条落进节点（fail-closed 不编造）。
    let mut out = ModuleReadout::default();
    out.scenes = vec![paged_scene("s0", "加油站", 16)];
    let ctx = ModuleReaderCtx {
        units: &[],
        sidecar_text: Some(sidecar(16, "强行剪断缆绳需要进行斗殴检定。")),
        ruleset_id: None,
    };
    let deep = json!({"scene":{
        "read_aloud":"你们看到缆绳横在路中。",
        "scene_mechanics":[
            {"intent_id":"m1","description":"强行剪断缆绳","tested_parameter":"brawling",
             "source_anchor":"强行剪断缆绳需要进行斗殴检定"},
            {"intent_id":"m2","description":"无锚编造","tested_parameter":"stealth",
             "source_anchor":""}
        ]
    }});
    let client = ReplayClient {
        deep_args: deep,
        seen_user: std::sync::Mutex::new(String::new()),
    };
    deep_extract_scene_in_place(&client, &ctx, &mut out, 0, 6).await;
    assert_eq!(
        out.scenes[0].scene_mechanics.len(),
        1,
        "无 anchor 的 intent 被 fail-closed 丢弃"
    );
    assert_eq!(out.scenes[0].scene_mechanics[0].intent_id, "m1");
}

#[tokio::test]
async fn legacy_deep_payload_keeps_existing_mechanics() {
    // 节点预置 1 条 scene_mechanics，深抽提交不含该键 → 仍是原 1 条（空/缺不覆盖既有）。
    use trpg_model::SceneMechanicIntent;
    let mut out = ModuleReadout::default();
    out.scenes = vec![paged_scene("s0", "加油站", 16)];
    out.scenes[0].scene_mechanics = vec![SceneMechanicIntent {
        intent_id: "m0".into(),
        description: "既有意图".into(),
        tested_parameter: "spot_hidden".into(),
        difficulty: None,
        effect_policy: Default::default(),
        source_anchor: "原文片段".into(),
    }];
    let ctx = ModuleReaderCtx {
        units: &[],
        sidecar_text: Some(sidecar(16, "你们看到一座破败的加油站。")),
        ruleset_id: None,
    };
    let deep = json!({"scene":{"read_aloud":"你们看到一座破败的加油站。"}});
    let client = ReplayClient {
        deep_args: deep,
        seen_user: std::sync::Mutex::new(String::new()),
    };
    deep_extract_scene_in_place(&client, &ctx, &mut out, 0, 6).await;
    assert_eq!(
        out.scenes[0].scene_mechanics.len(),
        1,
        "旧式提交（缺键）不冲掉既有 intents"
    );
    assert_eq!(out.scenes[0].scene_mechanics[0].intent_id, "m0");
}

#[test]
fn deep_sys_mentions_scene_mechanics() {
    // prompt 字面回归（防手滑）：深抽 SYS 必须含 scene_mechanics 指引。
    assert!(DEEP_SYS.contains("scene_mechanics"));
}

#[tokio::test]
async fn fallback_when_no_pages_and_no_sidecar_is_fail_closed() {
    // 无页码 + 无 sidecar → 走 ReAct 回退；MockLlmClient 不支持 complete_with_tools →
    // run_module_loop None → 保持 SkeletonOnly，绝不编造。
    let mut out = ModuleReadout::default();
    let mut s = ScenarioNode::default();
    s.node_id = "s0".into();
    out.scenes = vec![s];
    let ctx = ModuleReaderCtx {
        units: &[],
        sidecar_text: None,
        ruleset_id: None,
    };
    let client = trpg_llm::MockLlmClient;
    let flipped = deep_extract_scene_in_place(&client, &ctx, &mut out, 0, 2).await;
    assert!(!flipped, "回退路径 LLM 不支持 → fail-closed false");
    assert_eq!(
        out.scenes[0].extraction_status,
        SceneExtractionStatus::SkeletonOnly
    );
    assert!(out.scenes[0].read_aloud.is_none(), "绝不编造念白");
}

#[test]
fn merge_gleaned_dedups_by_node_id() {
    let mut have = vec![json!({"node_id":"s1","title":"A"})];
    let glean = json!({"still_missing":"YES",
        "scenes":[{"node_id":"s1","title":"dup"},{"node_id":"s2","title":"B"}]});
    let still = merge_gleaned(&mut have, &glean);
    assert!(still, "still_missing YES");
    assert_eq!(have.len(), 2, "s1 去重、s2 新增");
    assert_eq!(have[1]["node_id"], "s2");
}
