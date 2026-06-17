//! 子项目2 知识图谱起步：从**已 surface 的实体**之间抽取关系三元组（subject-
//! predicate-object）写入 `memory_facts`，让 GM 能在后续回合回忆本局涌现的关系。
//!
//! 立场（与 [`crate::truthgraph`] 观测层接力）：
//! - **建在 EntitySurfaced 之上**：只在本局已暴露给玩家的实体之间连边，绝不扫原文
//!   找新实体（语义而非关键词；不发明节点）。
//! - **fail-closed**：低置信 / 缺字段 / 自环 / 引用未知实体 → 一律丢弃，宁可不写。
//! - **零规则集/模组硬编码**：实体只用通用 kind（"npc"/"clue"），prompt 不含任何
//!   规则集或模组名分支。
//! - **复用 [`MemoryFact`] 三元组形态**（subject/predicate/object + confidence +
//!   source_event_ids 溯源 + turn_id），不另造平行类型。
//!
//! 纯函数（无 DB/无真 LLM，可单测）：[`resolve_entity_refs`] /
//! [`build_relationship_messages`] / [`parse_relationship_triples`]；薄 async
//! 编排 [`relationship_facts_from_inputs`] 经注入的 `&dyn LlmClient` 取数。DB 落库 +
//! gather 在 [`crate::RuntimeEngine`] 上（见 `extract_relationship_facts`）。

use chrono::Utc;
use serde_json::Value;
use std::collections::BTreeSet;
use trpg_llm::{system, user, LlmClient};
use trpg_model::{
    sha256_hex, ChatMessage, MemoryFact, MemoryStatus, ModuleGraph, Scope, ScopeType, Visibility,
};

/// 交给抽取器的最小实体上下文：稳定 id + 通用 kind + 展示名/简介。
/// 由 [`resolve_entity_refs`] 从 `EntitySurfaced` 的 (id, kind) 对 + 模组图谱解析。
#[derive(Debug, Clone)]
pub struct EntityRef {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub prose: String,
}

/// 在 `graph.npcs` ∪ `graph.clues` 里按 id 找实体 Value（实体 id 全图谱唯一）。
fn find_entity_value<'a>(graph: &'a ModuleGraph, id: &str) -> Option<&'a Value> {
    graph
        .npcs
        .iter()
        .chain(graph.clues.iter())
        .find(|v| v.get("id").and_then(|x| x.as_str()) == Some(id))
}

/// 把 `list_surfaced_entities` 的 (entity_id, kind) 对解析成带名字/简介的 [`EntityRef`]，
/// 名字/简介取自模组图谱（name + body|summary，对齐 `scene_npc_personas`）。
/// fail-soft：图谱里找不到、或名字与简介都空 → 跳过该实体（不污染抽取上下文）。
pub fn resolve_entity_refs(surfaced: &[(String, String)], graph: &ModuleGraph) -> Vec<EntityRef> {
    surfaced
        .iter()
        .filter_map(|(id, kind)| {
            let v = find_entity_value(graph, id)?;
            let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
            let prose = v
                .get("body")
                .or_else(|| v.get("summary"))
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if name.is_empty() && prose.is_empty() {
                return None;
            }
            Some(EntityRef { id: id.clone(), kind: kind.clone(), name, prose })
        })
        .collect()
}

/// 构造关系抽取的 LLM 消息（system 契约 + user 实体清单与本回合念白）。纯函数、零硬编码。
pub fn build_relationship_messages(narration: &str, entities: &[EntityRef]) -> Vec<ChatMessage> {
    let sys = "You extract durable RELATIONSHIP facts (knowledge-graph triples) between entities \
        the player has ALREADY encountered this session. Work ONLY among the KNOWN entities listed \
        by the user; `subject` and `object` MUST each be one of their `id` values verbatim. \
        Output STRICT JSON only, shape: \
        {\"triples\":[{\"subject\":\"<entity id>\",\"predicate\":\"<short verb phrase>\",\
        \"object\":\"<entity id>\",\"summary\":\"<one neutral sentence>\",\"confidence\":<0..1>}]}. \
        Fail-closed: emit a triple ONLY when THIS turn's narration gives clear evidence of the \
        relationship; if you are unsure, OMIT it. Never invent entities, predicates, or \
        relationships, and never relate an entity to itself. When nothing is clearly evidenced, \
        return {\"triples\":[]}.";
    let entity_lines = entities
        .iter()
        .map(|e| format!("- id={} kind={} name=\"{}\" :: {}", e.id, e.kind, e.name, e.prose))
        .collect::<Vec<_>>()
        .join("\n");
    let usr = format!(
        "Known entities (use these ids as subject/object):\n{entity_lines}\n\n\
         This turn's narration:\n{narration}\n\n\
         Return the relationship triples as JSON."
    );
    vec![system(sys), user(usr)]
}

/// 解析 LLM 返回的三元组 JSON 成 [`MemoryFact`]（fail-closed）。纯函数。
///
/// 丢弃规则（任一不满足即跳过该三元组，绝不写）：缺/空 subject·predicate·object、
/// 自环（subject==object）、subject 或 object 不在已 surface 实体集（发明节点）、
/// 缺 confidence 或 < `min_confidence`。同一 (subject,predicate,object) 批内去重
/// （稳定 `fact_id` = `mf_rel_{hash}`，跨回合 upsert 幂等）。
pub fn parse_relationship_triples(
    raw: &Value,
    session_id: &str,
    turn_id: &str,
    entities: &[EntityRef],
    min_confidence: f32,
) -> Vec<MemoryFact> {
    let known: BTreeSet<&str> = entities.iter().map(|e| e.id.as_str()).collect();
    let kind_of = |id: &str| entities.iter().find(|e| e.id == id).map(|e| e.kind.clone());
    let Some(triples) = raw.get("triples").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let now = Utc::now();
    let mut out: Vec<MemoryFact> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for t in triples {
        let str_field = |k: &str| t.get(k).and_then(|v| v.as_str()).unwrap_or("").trim();
        let subject = str_field("subject");
        let predicate = str_field("predicate");
        let object = str_field("object");
        // fail-closed 门。
        if subject.is_empty() || predicate.is_empty() || object.is_empty() {
            continue;
        }
        if subject == object {
            continue; // 自环
        }
        if !known.contains(subject) || !known.contains(object) {
            continue; // 发明节点
        }
        let Some(conf) = t.get("confidence").and_then(|v| v.as_f64()) else {
            continue;
        };
        let conf = conf as f32;
        if conf < min_confidence {
            continue;
        }
        let summary = {
            let s = str_field("summary");
            if s.is_empty() { format!("{subject} {predicate} {object}") } else { s.to_string() }
        };
        // 稳定身份 = (session, subject, predicate, object) → 跨回合 upsert 幂等、批内去重。
        let fact_id =
            format!("mf_rel_{}", &sha256_hex(format!("{session_id}|{subject}|{predicate}|{object}"))[..16]);
        if !seen.insert(fact_id.clone()) {
            continue;
        }
        let mut tags = vec!["relationship".to_string()];
        for end in [subject, object] {
            if let Some(k) = kind_of(end) {
                if !tags.contains(&k) {
                    tags.push(k);
                }
            }
        }
        // 溯源连回观测层：EntitySurfaced 的 per-session 幂等键（见 truthgraph）。
        let source_event_ids = vec![
            format!("de_surfaced_{session_id}_{subject}"),
            format!("de_surfaced_{session_id}_{object}"),
        ];
        out.push(MemoryFact {
            fact_id,
            session_id: session_id.to_string(),
            scope: Scope { scope_type: ScopeType::Session, scope_id: session_id.to_string() },
            visibility: Visibility::GmOnly,
            subject: subject.to_string(),
            predicate: predicate.to_string(),
            object: Value::String(object.to_string()),
            summary,
            status: MemoryStatus::Active,
            confidence: conf,
            source_event_ids,
            tags,
            importance: if conf >= 0.8 { 2 } else { 1 },
            turn_id: Some(turn_id.to_string()),
            created_at: now,
            updated_at: now,
        });
    }
    out
}

/// 薄 async 编排：给定已解析实体 + 念白，经注入 LLM 取三元组并解析。无 DB。
/// fail-closed：实体不足 2 / 念白空 / LLM 出错 → 空 Vec（绝不 panic、绝不乱写）。
pub async fn relationship_facts_from_inputs(
    llm: &dyn LlmClient,
    session_id: &str,
    turn_id: &str,
    entities: &[EntityRef],
    narration: &str,
    min_confidence: f32,
) -> Vec<MemoryFact> {
    if entities.len() < 2 || narration.trim().is_empty() {
        return Vec::new();
    }
    let messages = build_relationship_messages(narration, entities);
    let raw = match llm.complete_json(messages, 0.2).await {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(error = %err, "relationship extraction LLM call failed (fail-closed)");
            return Vec::new();
        }
    };
    parse_relationship_triples(&raw, session_id, turn_id, entities, min_confidence)
}

/// 特性总开关 `TRPG_RELATIONSHIP_EXTRACTION`（默认开；0/false/off/no 关）。与
/// truthgraph 观测层同立场：后台 heavy 段跑、fail-soft，关掉只是不写关系三元组。
pub fn relationship_extraction_enabled() -> bool {
    !matches!(
        std::env::var("TRPG_RELATIONSHIP_EXTRACTION").ok().as_deref(),
        Some("0") | Some("false") | Some("off") | Some("no")
    )
}

/// 写入阈值 `TRPG_RELATIONSHIP_MIN_CONFIDENCE`（默认 0.6）。低于即丢（fail-closed）。
pub fn relationship_min_confidence() -> f32 {
    std::env::var("TRPG_RELATIONSHIP_MIN_CONFIDENCE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.6)
}

#[cfg(test)]
#[path = "relationship_extraction_tests.rs"]
mod relationship_extraction_tests;
