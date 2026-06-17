//! 关系三元组抽取单测（无 DB / 无真 LLM）。覆盖：well-formed 三元组、fail-closed
//! 各分支（低置信/缺字段/自环/未知实体/空或不可解析）、去重、实体解析、消息构造、
//! 注入 LLM 的薄编排。
use super::*;
use async_trait::async_trait;
use serde_json::json;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use trpg_llm::LlmClient;
use trpg_model::{ChatMessage, ModuleGraph};

/// 可配置 stub LLM：`complete_json` 回传预设 Value。stream/text 不会被本模块调用。
struct StubLlm(Value);
#[async_trait]
impl LlmClient for StubLlm {
    async fn complete_text(&self, _m: Vec<ChatMessage>, _t: f32) -> anyhow::Result<String> {
        Ok(String::new())
    }
    async fn complete_json(&self, _m: Vec<ChatMessage>, _t: f32) -> anyhow::Result<Value> {
        Ok(self.0.clone())
    }
    async fn stream_chat(
        &self,
        _m: Vec<ChatMessage>,
        _t: f32,
    ) -> anyhow::Result<Pin<Box<dyn futures_util::Stream<Item = anyhow::Result<String>> + Send>>>
    {
        anyhow::bail!("stub: stream_chat unused")
    }
}

/// 计数 stub LLM：每次 `complete_json` 自增计数器，用于断言「门关时 LLM 一次都不调」。
struct CountingLlm {
    reply: Value,
    calls: AtomicUsize,
}
impl CountingLlm {
    fn new(reply: Value) -> Self {
        Self { reply, calls: AtomicUsize::new(0) }
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}
#[async_trait]
impl LlmClient for CountingLlm {
    async fn complete_text(&self, _m: Vec<ChatMessage>, _t: f32) -> anyhow::Result<String> {
        Ok(String::new())
    }
    async fn complete_json(&self, _m: Vec<ChatMessage>, _t: f32) -> anyhow::Result<Value> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.reply.clone())
    }
    async fn stream_chat(
        &self,
        _m: Vec<ChatMessage>,
        _t: f32,
    ) -> anyhow::Result<Pin<Box<dyn futures_util::Stream<Item = anyhow::Result<String>> + Send>>>
    {
        anyhow::bail!("stub: stream_chat unused")
    }
}

fn ents() -> Vec<EntityRef> {
    vec![
        EntityRef { id: "raul".into(), kind: "npc".into(), name: "Raul".into(), prose: "gas station owner".into() },
        EntityRef { id: "letter".into(), kind: "clue".into(), name: "Bloody Letter".into(), prose: "a torn note".into() },
    ]
}

fn one_triple(subject: &str, predicate: &str, object: &str, confidence: f64) -> Value {
    json!({"triples":[{"subject":subject,"predicate":predicate,"object":object,
        "summary":format!("{subject} {predicate} {object}"),"confidence":confidence}]})
}

#[test]
fn parse_returns_one_wellformed_fact_for_valid_triple() {
    let raw = json!({"triples":[{"subject":"raul","predicate":"wrote","object":"letter",
        "summary":"Raul wrote the bloody letter.","confidence":0.92}]});
    let facts = parse_relationship_triples(&raw, "sess_a", "turn7", &ents(), 0.6);
    assert_eq!(facts.len(), 1, "one well-formed triple expected");
    let f = &facts[0];
    assert_eq!(f.subject, "raul");
    assert_eq!(f.predicate, "wrote");
    assert_eq!(f.object, json!("letter"));
    assert_eq!(f.summary, "Raul wrote the bloody letter.");
    assert!((f.confidence - 0.92).abs() < 1e-5);
    assert_eq!(f.turn_id.as_deref(), Some("turn7"), "turn provenance recorded");
    assert_eq!(f.session_id, "sess_a");
    assert_eq!(f.scope.scope_type.as_str(), "session");
    assert_eq!(f.scope.scope_id, "sess_a");
    assert_eq!(f.visibility.as_str(), "gm_only");
    assert_eq!(f.status.as_str(), "active");
    assert!(f.tags.iter().any(|t| t == "relationship"), "generic relationship tag");
    // 溯源连回 EntitySurfaced 观测层键（truthgraph 格式 de_surfaced_{session}_{id}）。
    assert!(f.source_event_ids.contains(&"de_surfaced_sess_a_raul".to_string()));
    assert!(f.source_event_ids.contains(&"de_surfaced_sess_a_letter".to_string()));
    assert!(f.fact_id.starts_with("mf_rel_"), "stable relationship fact id");
}

#[test]
fn parse_drops_low_confidence() {
    let raw = one_triple("raul", "knows", "letter", 0.30);
    assert!(parse_relationship_triples(&raw, "s", "t", &ents(), 0.6).is_empty());
}

#[test]
fn parse_drops_missing_or_empty_field() {
    // 缺 predicate（空串）→ 丢。
    let raw = json!({"triples":[{"subject":"raul","predicate":"","object":"letter",
        "summary":"x","confidence":0.9}]});
    assert!(parse_relationship_triples(&raw, "s", "t", &ents(), 0.6).is_empty());
}

#[test]
fn parse_drops_self_loop() {
    let raw = one_triple("raul", "is", "raul", 0.95);
    assert!(parse_relationship_triples(&raw, "s", "t", &ents(), 0.6).is_empty());
}

#[test]
fn parse_drops_unknown_entity() {
    // object 引用未 surface 的实体（发明节点）→ 丢。
    let raw = one_triple("raul", "fears", "ghost", 0.95);
    assert!(parse_relationship_triples(&raw, "s", "t", &ents(), 0.6).is_empty());
}

#[test]
fn parse_drops_empty_or_unparseable() {
    assert!(parse_relationship_triples(&json!({"triples":[]}), "s", "t", &ents(), 0.6).is_empty());
    assert!(parse_relationship_triples(&json!({}), "s", "t", &ents(), 0.6).is_empty());
    // MockLlmClient 形态 {"mock":true} → fail-closed 空。
    assert!(parse_relationship_triples(&json!({"mock":true}), "s", "t", &ents(), 0.6).is_empty());
}

#[test]
fn parse_dedupes_identical_triples() {
    let raw = json!({"triples":[
        {"subject":"raul","predicate":"owns","object":"letter","summary":"a","confidence":0.9},
        {"subject":"raul","predicate":"owns","object":"letter","summary":"b","confidence":0.8}
    ]});
    let facts = parse_relationship_triples(&raw, "sess_a", "turn1", &ents(), 0.6);
    assert_eq!(facts.len(), 1, "identical (subject,predicate,object) collapse to one fact_id");
}

#[test]
fn resolve_maps_names_and_skips_unknown() {
    let graph = ModuleGraph {
        npcs: vec![json!({"id":"raul","name":"Raul","summary":"gas station owner"})],
        clues: vec![json!({"id":"letter","name":"Letter","body":"a torn note"})],
        ..Default::default()
    };
    let surfaced = vec![
        ("raul".to_string(), "npc".to_string()),
        ("ghost".to_string(), "npc".to_string()),
        ("letter".to_string(), "clue".to_string()),
    ];
    let refs = resolve_entity_refs(&surfaced, &graph);
    assert_eq!(refs.len(), 2, "ghost (not in graph) dropped");
    assert!(refs.iter().any(|r| r.id == "raul" && r.name == "Raul" && r.kind == "npc"));
    assert!(refs.iter().any(|r| r.id == "letter" && r.kind == "clue" && r.prose.contains("note")));
    assert!(!refs.iter().any(|r| r.id == "ghost"));
}

#[test]
fn build_messages_include_narration_and_entities() {
    let msgs = build_relationship_messages("Raul handed over the letter.", &ents());
    assert!(msgs.len() >= 2, "system + user");
    let joined = msgs.iter().map(|m| m.content.clone()).collect::<Vec<_>>().join("\n");
    assert!(joined.contains("Raul handed over the letter."), "narration present");
    assert!(joined.contains("raul") && joined.contains("letter"), "entity ids present");
    assert!(joined.to_lowercase().contains("json"), "JSON output contract present");
}

#[tokio::test]
async fn core_extracts_fact_from_related_entities() {
    let llm = StubLlm(one_triple("raul", "owns", "letter", 0.9));
    let facts = relationship_facts_from_inputs(&llm, "sess_a", "turn1", &ents(), "narration", 0.6, true).await;
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].subject, "raul");
}

#[tokio::test]
async fn core_fail_closed_when_no_relationship() {
    let llm = StubLlm(json!({"triples":[]}));
    let facts = relationship_facts_from_inputs(&llm, "sess_a", "turn1", &ents(), "narration", 0.6, true).await;
    assert!(facts.is_empty(), "no relationship → write nothing (fail-closed)");
}

#[tokio::test]
async fn no_new_surface_this_turn_skips_llm_call() {
    // 本回合**没 surface 新实体**（surfaced_new_this_turn=false）：即便已知实体 ≥2、念白非空
    // （旧逻辑会照样调 LLM 重抽同样的三元组），新闸必须**完全不调 LLM**、返回空。
    let llm = CountingLlm::new(one_triple("raul", "owns", "letter", 0.9));
    let facts = relationship_facts_from_inputs(
        &llm, "sess_a", "turn1", &ents(), "narration", 0.6, /* surfaced_new_this_turn = */ false,
    )
    .await;
    assert!(facts.is_empty(), "no new entity this turn → no triples written");
    assert_eq!(llm.calls(), 0, "no new entity surfaced → LLM must NOT be invoked (cost saved)");
}

#[tokio::test]
async fn new_surface_this_turn_invokes_llm() {
    // 本回合 surface 了新实体（surfaced_new_this_turn=true）：闸打开，LLM 照常调一次。
    let llm = CountingLlm::new(one_triple("raul", "owns", "letter", 0.9));
    let facts = relationship_facts_from_inputs(
        &llm, "sess_a", "turn1", &ents(), "narration", 0.6, /* surfaced_new_this_turn = */ true,
    )
    .await;
    assert_eq!(llm.calls(), 1, "new entity surfaced → LLM invoked exactly once");
    assert_eq!(facts.len(), 1);
}
