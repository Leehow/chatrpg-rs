//! GOLD live test：端到端验证「子项目2 关系三元组抽取」——给本局已 surface 的两个
//! 相关实体 + 本回合念白，`extract_relationship_facts` 经注入 LLM 抽出三元组、**写入
//! memory_facts**（低置信项 fail-closed 丢弃），且**后续回合可经 retrieve_memory /
//! list_memory_facts 召回**（带 turn_id 溯源 + EntitySurfaced 溯源）。
//!
//! 需 DATABASE_URL（任一 chatrpg 库即可，建表幂等 migrate）。缺则 SKIP。
//! Run:
//!   DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!   cargo test -p trpg-runtime --test live_relationship_facts -- --nocapture
use async_trait::async_trait;
use serde_json::{json, Value};
use std::pin::Pin;
use trpg_db::Db;
use trpg_model::{
    ChatMessage, DomainEvent, DomainEventKind, MemoryQuery, ModuleGraph, ValidationReport,
    VisibilityProfile,
};
use trpg_runtime::RuntimeEngine;
use uuid::Uuid;

/// stub LLM：回传预设三元组（一条 valid 0.9 + 一条低置信 0.2）。验证整链写入 + fail-closed。
struct StubLlm(Value);
#[async_trait]
impl trpg_llm::LlmClient for StubLlm {
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

fn surfaced_event(session: &str, turn: &str, entity_id: &str, kind: &str) -> DomainEvent {
    DomainEvent::new(
        format!("de_surfaced_{session}_{entity_id}"),
        session,
        turn,
        DomainEventKind::EntitySurfaced,
        json!({"entity_id": entity_id, "entity_kind": kind, "scene_id": "sc01"}),
    )
}

#[tokio::test]
async fn relationship_triple_written_and_retrievable_next_turn() {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP: DATABASE_URL unset");
            return;
        }
    };
    let db = match Db::connect(&url).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("SKIP: connect failed: {e}");
            return;
        }
    };
    // 幂等 migrate（含 0031 add column if not exists turn_id）。
    if let Err(e) = db.migrate().await {
        eprintln!("SKIP: migrate failed: {e}");
        return;
    }

    let session = format!("session_rel_{}", Uuid::new_v4().simple());
    let module_id = format!("mod_rel_{}", Uuid::new_v4().simple());
    // memory_facts.session_id 有 FK→sessions：先建会话行（生产回合里本就存在）。
    db.create_session(&session, "call_of_cthulhu_7e", Some(&module_id))
        .await
        .expect("create session");

    // —— 1. 播种模组图谱（2 实体）+ EntitySurfaced 观测事件 ——
    let graph = ModuleGraph {
        module_id: module_id.clone(),
        npcs: vec![json!({"id":"raul","name":"Raul","summary":"the gas station owner"})],
        clues: vec![json!({"id":"letter","name":"Bloody Letter","body":"a torn, blood-stained note"})],
        ..Default::default()
    };
    db.upsert_parsed_bundle(
        &module_id,
        "module",
        "Relationship Test Module",
        "v1.20",
        None,
        &format!("src_{module_id}"),
        &format!("cfg_{module_id}"),
        &json!({"module_id": module_id, "module_graph": graph}),
        &ValidationReport::default(),
    )
    .await
    .expect("seed module graph");

    for ev in [
        surfaced_event(&session, "turn_rel_1", "raul", "npc"),
        surfaced_event(&session, "turn_rel_1", "letter", "clue"),
    ] {
        db.append_domain_event(&ev).await.expect("seed EntitySurfaced");
    }

    // —— 2. 抽取（valid 0.9 写入；低置信 0.2 fail-closed 丢弃）——
    let engine = RuntimeEngine { db: db.clone(), search: None };
    let llm = StubLlm(json!({"triples":[
        {"subject":"raul","predicate":"wrote","object":"letter",
         "summary":"Raul wrote the bloody letter found at the scene.","confidence":0.9},
        {"subject":"raul","predicate":"vaguely_recalls","object":"letter",
         "summary":"weak / uncertain","confidence":0.2}
    ]}));
    let written = engine
        .extract_relationship_facts(
            &llm,
            &session,
            "turn_rel_1",
            Some(&module_id),
            "Raul slid the bloody letter across the counter, hand trembling.",
        )
        .await;
    assert_eq!(written, 1, "exactly the high-confidence triple persists (low-conf dropped, fail-closed)");

    // —— 3. 落库校验（list_memory_facts）——
    let facts = db.list_memory_facts(&session, 50).await.expect("list facts");
    let rel: Vec<_> = facts.iter().filter(|f| f.tags.iter().any(|t| t == "relationship")).collect();
    assert_eq!(rel.len(), 1, "one relationship fact in memory_facts");
    let f = rel[0];
    assert_eq!(f.subject, "raul");
    assert_eq!(f.predicate, "wrote");
    assert_eq!(f.object, json!("letter"));
    assert_eq!(f.turn_id.as_deref(), Some("turn_rel_1"), "turn provenance round-trips");
    assert!(f.source_event_ids.contains(&format!("de_surfaced_{session}_raul")));
    assert!(f.source_event_ids.contains(&format!("de_surfaced_{session}_letter")));
    assert!((f.confidence - 0.9).abs() < 1e-5);

    // —— 4. 后续回合召回（retrieve_memory，模拟 context assembly 读取 facts）——
    let query = MemoryQuery {
        session_id: session.clone(),
        text: "raul".into(), // 后续回合玩家提到 Raul
        ruleset_id: None,
        module_id: Some(module_id.clone()),
        scene_id: None,
        location_id: None,
        actor_ids: vec![],
        tags: vec![],
        limit: 10,
        viewer: VisibilityProfile::gm(),
    };
    let retrieved = db.retrieve_memory(&query).await.expect("retrieve");
    assert!(
        retrieved.facts.iter().any(|rf| rf.predicate == "wrote" && rf.turn_id.as_deref() == Some("turn_rel_1")),
        "relationship triple retrievable for a subsequent turn"
    );

    // —— 5. 幂等：重跑同抽取不应制造重复 fact（稳定 fact_id upsert）——
    let written_again = engine
        .extract_relationship_facts(&llm, &session, "turn_rel_2", Some(&module_id),
            "Raul slid the bloody letter across the counter, hand trembling.")
        .await;
    assert_eq!(written_again, 1, "re-extraction upserts (same fact_id), no duplicate row");
    let facts2 = db.list_memory_facts(&session, 50).await.expect("list facts 2");
    let rel2 = facts2.iter().filter(|f| f.tags.iter().any(|t| t == "relationship")).count();
    assert_eq!(rel2, 1, "still exactly one relationship fact after re-run (idempotent upsert)");
}
