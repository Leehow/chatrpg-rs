//! GOLD live test：端到端验证「子项目2 关系三元组抽取」——给本局已 surface 的两个
//! 相关实体 + 本回合念白，`extract_relationship_facts` 经注入 LLM 抽出三元组、**写入
//! memory_facts**（低置信项 fail-closed 丢弃），且**后续回合可经 retrieve_memory /
//! list_memory_facts 召回**（带 turn_id 溯源 + EntitySurfaced 溯源）。
//!
//! TC-D3-03 起，heavy 尾段不再 raw `db.upsert_memory_fact`：抽出的三元组先经
//! `relationship_facts_to_proposals` 转 `MemoryExtractionProposal`，再由 runtime-owned
//! `review_and_commit_proposals` 复跑模型门 + 会话授权门后，沿 `LegacyFact` 路 upsert
//! 进 `memory_facts`。`extract_relationship_facts` 的返回值即 `CommitReport` 的 Done 数，
//! 因此下面对 `written` 的断言同时证明「确实经 proposal review/commit 路落库」——可观测
//! 结果（行落 memory_facts + 可召回）与旧 raw-upsert 路完全一致。
//!
//! 另含**成本闸**端到端验证：只在**本回合 surface 了新实体**时才调 LLM——没有新实体的
//! 回合（已知实体集没变）直接跳过、一次 LLM 都不烧；再 surface 新实体则闸重新打开。
//! 「新实体」经 append 时**冻结**在 `EntitySurfaced` 事件的 turn_id 上判定（append 是
//! on-conflict-do-nothing，重复 surface 是 no-op、保留首次 turn_id）。
//!
//! 需 DATABASE_URL（任一 chatrpg 库即可，建表幂等 migrate）。缺则 SKIP。
//! Run:
//!   DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!   cargo test -p trpg-runtime --test live_relationship_facts -- --nocapture
use async_trait::async_trait;
use serde_json::{json, Value};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use trpg_db::Db;
use trpg_model::{
    ChatMessage, DomainEvent, DomainEventKind, MemoryQuery, ModuleGraph, ValidationReport,
    VisibilityProfile,
};
use trpg_runtime::RuntimeEngine;
use uuid::Uuid;

/// 计数 stub LLM：回传预设三元组，并记录 `complete_json` 被调次数（验证成本闸真省调用）。
struct CountingLlm {
    reply: Value,
    calls: AtomicUsize,
}
impl CountingLlm {
    fn new(reply: Value) -> Self {
        Self {
            reply,
            calls: AtomicUsize::new(0),
        }
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}
#[async_trait]
impl trpg_llm::LlmClient for CountingLlm {
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
async fn relationship_triple_written_and_gated_by_new_surface() {
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

    // —— 1. 播种模组图谱（3 实体：raul/letter 起步两端 + knife 留作后续新实体）——
    let graph = ModuleGraph {
        module_id: module_id.clone(),
        npcs: vec![json!({"id":"raul","name":"Raul","summary":"the gas station owner"})],
        clues: vec![
            json!({"id":"letter","name":"Bloody Letter","body":"a torn, blood-stained note"}),
            json!({"id":"knife","name":"Hunting Knife","body":"a notched hunting knife"}),
        ],
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

    // 本回合 turn_rel_1 首次 surface 两个实体（这两行 EntitySurfaced 的 turn_id 冻结为 turn_rel_1）。
    for ev in [
        surfaced_event(&session, "turn_rel_1", "raul", "npc"),
        surfaced_event(&session, "turn_rel_1", "letter", "clue"),
    ] {
        db.append_domain_event(&ev)
            .await
            .expect("seed EntitySurfaced");
    }

    let engine = RuntimeEngine {
        db: db.clone(),
        search: None,
    };
    // stub 回一条 valid 0.9 + 一条低置信 0.2（fail-closed 丢弃后净写 1 条）。
    let llm = CountingLlm::new(json!({"triples":[
        {"subject":"raul","predicate":"wrote","object":"letter",
         "summary":"Raul wrote the bloody letter found at the scene.","confidence":0.9},
        {"subject":"raul","predicate":"vaguely_recalls","object":"letter",
         "summary":"weak / uncertain","confidence":0.2}
    ]}));

    // —— 2. turn_rel_1：本回合 surface 了新实体 → 闸开，LLM 调一次，valid 三元组写入 ——
    let written = engine
        .extract_relationship_facts(
            &llm,
            &session,
            "turn_rel_1",
            Some(&module_id),
            "Raul slid the bloody letter across the counter, hand trembling.",
            // 本测专测「新实体 surface」成本闸：社交信号置空，确保只由 surfaced_new 驱动。
            &[],
            "",
            None,
        )
        .await;
    assert_eq!(
        written, 1,
        "high-confidence triple persists via proposal commit (CommitReport Done==1; low-conf dropped, fail-closed)"
    );
    assert_eq!(
        llm.calls(),
        1,
        "turn surfacing a new entity DOES invoke the LLM once"
    );

    // —— 3. 落库校验（list_memory_facts）——
    let facts = db
        .list_memory_facts(&session, 50)
        .await
        .expect("list facts");
    let rel: Vec<_> = facts
        .iter()
        .filter(|f| f.tags.iter().any(|t| t == "relationship"))
        .collect();
    assert_eq!(rel.len(), 1, "one relationship fact in memory_facts");
    let f = rel[0];
    assert_eq!(f.subject, "raul");
    assert_eq!(f.predicate, "wrote");
    assert_eq!(f.object, json!("letter"));
    assert_eq!(
        f.turn_id.as_deref(),
        Some("turn_rel_1"),
        "turn provenance round-trips"
    );
    assert!(f
        .source_event_ids
        .contains(&format!("de_surfaced_{session}_raul")));
    assert!(f
        .source_event_ids
        .contains(&format!("de_surfaced_{session}_letter")));
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
        layers: vec![],
    };
    let retrieved = db.retrieve_memory(&query).await.expect("retrieve");
    assert!(
        retrieved
            .facts
            .iter()
            .any(|rf| rf.predicate == "wrote" && rf.turn_id.as_deref() == Some("turn_rel_1")),
        "relationship triple retrievable for a subsequent turn"
    );

    // —— 5. 成本闸：turn_rel_2 **没 surface 新实体**（已知实体集没变）→ 跳过、一次 LLM 都不调 ——
    let written_no_new = engine
        .extract_relationship_facts(
            &llm,
            &session,
            "turn_rel_2",
            Some(&module_id),
            "Raul slid the bloody letter across the counter, hand trembling.",
            &[],
            "",
            None,
        )
        .await;
    assert_eq!(
        written_no_new, 0,
        "no new entity this turn → extraction skipped, nothing written"
    );
    assert_eq!(
        llm.calls(),
        1,
        "no new entity surfaced → LLM NOT invoked again (cost saved)"
    );
    let rel_after_skip = db
        .list_memory_facts(&session, 50)
        .await
        .expect("list facts 2")
        .iter()
        .filter(|f| f.tags.iter().any(|t| t == "relationship"))
        .count();
    assert_eq!(
        rel_after_skip, 1,
        "still exactly one relationship fact (skip wrote nothing)"
    );

    // —— 6. 再 surface 一个新实体（knife@turn_rel_3）→ 闸重新打开、LLM 再调一次、upsert 幂等 ——
    db.append_domain_event(&surfaced_event(&session, "turn_rel_3", "knife", "clue"))
        .await
        .expect("seed new EntitySurfaced");
    let written_reopen = engine
        .extract_relationship_facts(
            &llm,
            &session,
            "turn_rel_3",
            Some(&module_id),
            "Raul slid the bloody letter across the counter, hand trembling.",
            &[],
            "",
            None,
        )
        .await;
    assert_eq!(
        written_reopen, 1,
        "gate re-opens; same triple upserted (stable fact_id, no dup)"
    );
    assert_eq!(
        llm.calls(),
        2,
        "a genuinely new entity re-opens the gate → LLM invoked again"
    );
    let rel_final = db
        .list_memory_facts(&session, 50)
        .await
        .expect("list facts 3")
        .iter()
        .filter(|f| f.tags.iter().any(|t| t == "relationship"))
        .count();
    assert_eq!(
        rel_final, 1,
        "still exactly one relationship fact after re-run (idempotent upsert)"
    );
}
