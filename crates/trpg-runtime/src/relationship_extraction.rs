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
    entity_body_prose, sha256_hex, ChatMessage, MemoryFact, MemoryStatus, ModuleGraph, Scope,
    ScopeType, Visibility,
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
            let name = v
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let prose = entity_body_prose(v).unwrap_or("").trim().to_string();
            if name.is_empty() && prose.is_empty() {
                return None;
            }
            Some(EntityRef {
                id: id.clone(),
                kind: kind.clone(),
                name,
                prose,
            })
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
        .map(|e| {
            format!(
                "- id={} kind={} name=\"{}\" :: {}",
                e.id, e.kind, e.name, e.prose
            )
        })
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
            if s.is_empty() {
                format!("{subject} {predicate} {object}")
            } else {
                s.to_string()
            }
        };
        // 稳定身份 = (session, subject, predicate, object) → 跨回合 upsert 幂等、批内去重。
        let fact_id = format!(
            "mf_rel_{}",
            &sha256_hex(format!("{session_id}|{subject}|{predicate}|{object}"))[..16]
        );
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
            scope: Scope {
                scope_type: ScopeType::Session,
                scope_id: session_id.to_string(),
            },
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

/// TC-D3-04 社交闸（确定性、纯函数、无 IO）：判定本回合是否值得跑关系抽取。
///
/// 触发条件（任一为真即跑）：
/// 1. **本回合 surface 了新实体**（`surfaced_new_this_turn`）——已知实体集变了，原有成本闸。
/// 2. **至少一个 active NPC 在场，且玩家输入或念白含明确社交互动信号**
///    （[`text_has_social_signal`]）——威胁/讨价/帮助/欺骗/说服等会改变关系但不 surface
///    新实体的重复社交。
///
/// 成本控制仍确定性：**仅 active NPC 非空并不触发**（环境念白里有 NPC 但无社交意图的普通
/// 回合照常跳过），必须配一个 source-backed 社交动词信号才在「无新实体」时开闸。
pub fn relationship_gate_should_run(
    surfaced_new_this_turn: bool,
    active_npc_count: usize,
    player_input: &str,
    narration: &str,
) -> bool {
    if surfaced_new_this_turn {
        return true;
    }
    active_npc_count >= 1
        && (text_has_social_signal(player_input) || text_has_social_signal(narration))
}

/// 确定性社交信号扫描（保守、source-backed）：仅在文本出现明确社交意图动词时为真，绝不
/// 因「有 NPC 在场」这种环境念白就判真。英文按**字母 token 前缀**匹配动词词干（避免
/// `task`⊅`ask` 这类子串误判，并覆盖时态/派生：asked/asking/threatening…）；中文用**多字**
/// 词条做子串匹配（避免裸「问/帮」命中「问题/帮派」）。
///
/// 刻意不收的（避免误判，见 handoff）：裸英文 `lie`（撞 lieutenant，欺骗已由 deceive/
/// deception 覆盖）、裸 `beg`（撞 begin/began）；裸中文单字 `问`/`帮`（撞 问题/帮派）。
pub fn text_has_social_signal(text: &str) -> bool {
    // 中文多字社交词条（子串匹配，CJK 无词边界）。
    const ZH_TERMS: &[&str] = &[
        "询问", "质问", "盘问", "告诉", "威胁", "恐吓", "威逼", "利诱", "谈判", "讨价", "还价",
        "帮助", "帮忙", "欺骗", "撒谎", "说服", "劝说", "道歉", "指责", "控诉", "交易", "审问",
        "请求", "恳求", "承诺", "保证", "警告", "贿赂", "侮辱", "背叛", "安慰",
    ];
    if ZH_TERMS.iter().any(|kw| text.contains(kw)) {
        return true;
    }
    // 英文动词词干，按 token 前缀匹配（覆盖时态/派生）。
    const EN_STEMS: &[&str] = &[
        "ask",
        "tell",
        "told",
        "threat",
        "bargain",
        "negotiat",
        "persuad",
        "convinc",
        "apolog",
        "accus",
        "interrogat",
        "bribe",
        "intimidat",
        "confess",
        "betray",
        "deceiv",
        "decept",
        "promis",
        "warn",
        "demand",
        "request",
        "plead",
        "implor",
        "insult",
        "befriend",
        "reassur",
        "consol",
        "trade",
        "help",
        "offer",
        "scold",
        "taunt",
        "flatter",
    ];
    let lower = text.to_lowercase();
    lower
        .split(|c: char| !c.is_ascii_alphabetic())
        .filter(|tok| !tok.is_empty())
        .any(|tok| EN_STEMS.iter().any(|stem| tok.starts_with(stem)))
}

/// 薄 async 编排：给定已解析实体 + 念白，经注入 LLM 取三元组并解析。无 DB。
/// fail-closed：`should_run` 为 false（社交闸判定本回合不跑，见 [`relationship_gate_should_run`]）/
/// 实体不足 2 / 念白空 / LLM 出错 → 空 Vec（绝不 panic、绝不乱写、不调 LLM）。`should_run`
/// 是成本闸：闸关的回合既不调 LLM 也不写库。
pub async fn relationship_facts_from_inputs(
    llm: &dyn LlmClient,
    session_id: &str,
    turn_id: &str,
    entities: &[EntityRef],
    narration: &str,
    min_confidence: f32,
    should_run: bool,
) -> Vec<MemoryFact> {
    // 成本闸：闸关（无新实体且无 active-NPC 社交信号）→ 直接 fail-closed 返回空、不调 LLM。
    if !should_run || entities.len() < 2 || narration.trim().is_empty() {
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
        std::env::var("TRPG_RELATIONSHIP_EXTRACTION")
            .ok()
            .as_deref(),
        Some("0") | Some("false") | Some("off") | Some("no")
    )
}

/// L-H(PC↔NPC):合成「玩家角色」实体端点。让最基本的记忆——玩家与本场景在场 NPC 的关系/
/// 认知——在单 NPC(甚至隐名)场景也能成三元组(pc.current, predicate, npc)。零规则集/模组硬编码:
/// id 用通用 actor id `pc.current`,kind 通用 `pc`,名字/简介通用不含任何模组名。
pub fn pc_entity_ref() -> EntityRef {
    EntityRef {
        id: "pc.current".to_string(),
        kind: "pc".to_string(),
        name: "玩家角色".to_string(),
        prose: "本局由玩家操作的主角(player character)。".to_string(),
    }
}

/// L-H:把当前回合「在场 active NPC」(由 scene.referenced_npc_ids 派生,见 npc_activation)
/// 解析成带名字/简介的 [`EntityRef`]。复用 [`resolve_entity_refs`](统一 fail-soft:图谱里找不到/
/// 名字简介都空 → 跳过)。这些是玩家此刻真实面对的 NPC,不是名字匹配浮现集(故隐名无人机也算端点)。
pub fn resolve_active_npc_refs(active_npc_ids: &[String], graph: &ModuleGraph) -> Vec<EntityRef> {
    let pairs: Vec<(String, String)> = active_npc_ids
        .iter()
        .filter(|id| !id.trim().is_empty())
        .map(|id| (id.clone(), "npc".to_string()))
        .collect();
    resolve_entity_refs(&pairs, graph)
}

/// L-H 开关 `TRPG_PC_NPC_RELATIONSHIP`(默认 ON;仅 0/false/off/no 关)。关 ⇒ 不做 PC↔NPC
/// 兜底增广,关系抽取与历史 NPC↔NPC 路径字节等价。镜像 BUG-1/L-E/L-G/L-C 模式。
pub fn pc_npc_relationship_enabled() -> bool {
    !matches!(
        std::env::var("TRPG_PC_NPC_RELATIONSHIP")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "0" | "false" | "off" | "no"
    )
}

/// L-R 开关 `TRPG_OPENING_DURABLE_SEED`(默认 ON;仅 0/false/off/no 关)。env-free 纯判定
/// （可单测、避免 env-race），env 包装见 [`opening_durable_seed_enabled`]。关 ⇒ 开场不落
/// durable 种子,与 L-P(纯念白投递)字节等价。
pub fn opening_durable_seed_flag_on(raw: Option<&str>) -> bool {
    !matches!(
        raw.map(|v| v.to_ascii_lowercase()).as_deref(),
        Some("0") | Some("false") | Some("off") | Some("no")
    )
}

/// L-R env 包装:读 `TRPG_OPENING_DURABLE_SEED`(默认 ON)。
pub fn opening_durable_seed_enabled() -> bool {
    opening_durable_seed_flag_on(std::env::var("TRPG_OPENING_DURABLE_SEED").ok().as_deref())
}

/// L-R(开场 durable 种子,J1-durable 根修):L-P 引擎开场投递把模组入口场景**确立**给玩家
/// (无人机 Athena 在警火下/缆线之谜/交火)——这是玩家**目睹的、source-backed 的既成事实**。
/// 本函数把开场场景 `referenced_npc_ids` 里的每个 NPC 确定性地落成一条记忆三元组
/// `(pc.current, encountered, npc)` 写入 `memory_facts`,让**任何**涌现路径(含单人潜入空内场、
/// 冷骰全败的探索局)从 turn 0 起 durable≥1。绝不发明事实(§二.9:只持久化模组自身确立的
/// 遭遇,不造新实体/谓词)——镜像 L-H 三元组形态,但**确定性种子**(无 LLM、无社交闸),因为
/// 开场遭遇是模组结构事实而非涌现社交。无 NPC 的开场场景 → 退回单条 `(pc.current, entered,
/// scene_id)`(玩家进入既定开场场景,仍 source-backed)。fail-closed:`scene_id` 空且无 NPC
/// → 返回空 Vec(绝不凭空造行)。
pub fn opening_seed_facts(
    session_id: &str,
    scene_id: Option<&str>,
    npc_ids: &[String],
) -> Vec<MemoryFact> {
    let now = Utc::now();
    let mut out: Vec<MemoryFact> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let pc = pc_entity_ref();
    let mk = |subject: &str, predicate: &str, object: &str, summary: String| -> MemoryFact {
        let fact_id = format!(
            "mf_open_{}",
            &sha256_hex(format!("{session_id}|{subject}|{predicate}|{object}"))[..16]
        );
        MemoryFact {
            fact_id,
            session_id: session_id.to_string(),
            scope: Scope {
                scope_type: ScopeType::Session,
                scope_id: session_id.to_string(),
            },
            visibility: Visibility::GmOnly,
            subject: subject.to_string(),
            predicate: predicate.to_string(),
            object: Value::String(object.to_string()),
            summary,
            status: MemoryStatus::Active,
            confidence: 0.9,
            source_event_ids: vec![format!("de_surfaced_{session_id}_{object}")],
            tags: vec!["relationship".to_string(), "opening_seed".to_string()],
            importance: 2,
            // 开场为「pre-turn 设场」,无既成回合;turn_id 留空(provenance:开场投递)。
            turn_id: None,
            created_at: now,
            updated_at: now,
        }
    };
    for npc in npc_ids.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let fid = format!(
            "mf_open_{}",
            &sha256_hex(format!("{session_id}|{}|encountered|{npc}", pc.id))[..16]
        );
        if !seen.insert(fid) {
            continue;
        }
        out.push(mk(
            &pc.id,
            "encountered",
            npc,
            "开场:玩家目睹并卷入开场场景中此 NPC 所在的局面。".to_string(),
        ));
    }
    if out.is_empty() {
        if let Some(sid) = scene_id.map(str::trim).filter(|s| !s.is_empty()) {
            out.push(mk(
                &pc.id,
                "entered",
                sid,
                "开场:玩家从引擎投递的模组入口场景进入本局。".to_string(),
            ));
        }
    }
    out
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
