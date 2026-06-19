//! 设计3 R1 知识抽取 KERNEL（纯函数、无 LLM / 无 DB / 无 turn-loop 接线）：把一次「学习
//! 事件」落成一对**原子配对**的记忆抽取提案。仅交付可单测的纯件，留给后续 commit 路径调用。
//!
//! 立场（与 [`crate::relationship_extraction`] 同源，保守、fail-closed、零硬编码）：
//! - **C1 知识状态语义分档**：一次学习事件按类别确定性映射到 [`KnowledgeState`]——
//!   看到/亲历（witnessed）→`KnowsTrue`；被骗/被误导（deceived）→`BelievesFalse`；
//!   听闻/传言（heard）→`HeardAbout`。只此三档，不发明额外映射。
//! - **C2 Fact 强制原子配对**：产出一个 `KnowledgeUpdate` 必同批产出其配对 `WorldFact`，
//!   二者共享同一 `fact_id` 与同一 evidence `source_event_ids`（knowledge_edges→memory_facts
//!   无 FK，原子性靠「永远两者一起发」保证）。本模块的唯一构造口
//!   [`build_atomic_knowledge_pair`] 不可能只产出孤立的 `KnowledgeUpdate`。
//! - **知识闸**（镜像 [`crate::relationship_extraction::relationship_gate_should_run`]）：
//!   确定性、纯函数、source-backed 地判定本回合是否值得跑知识抽取。
//!
//! OUT OF SCOPE（阻塞于人审架构决策 R4-D1）：真 LLM 抽取调用、heavy 后处理插件、以及把本
//! kernel 接入 proposal-commit 流水线的任何 execute.rs / turn_loop 接线。这里只留纯 `pub` 函数。

use trpg_model::{
    KnowledgeState, KnowledgeUpdateCandidate, MemoryExtractionProposal, ProposalHolder,
    WorldFactCandidate,
};

use crate::relationship_extraction::text_has_social_signal;

/// C1 学习模式（加性小枚举）：一次学习事件的**获知方式**类别。仅这三类是契约钉死、已测的；
/// 各自确定性映射到一个 [`KnowledgeState`]（见 [`grade_knowledge_state`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnowledgeLearningMode {
    /// 看到 / 亲历：第一手确证 → `KnowsTrue`。
    Witnessed,
    /// 被骗 / 被误导：持有与真相相反的错误信念 → `BelievesFalse`。
    Deceived,
    /// 听闻 / 传言：二手未确证 → `HeardAbout`。
    Heard,
}

impl KnowledgeLearningMode {
    /// C1 确定性分档：把学习模式映射到唯一的 [`KnowledgeState`]。纯函数、无副作用、全覆盖。
    /// witnessed→KnowsTrue，deceived→BelievesFalse，heard→HeardAbout。
    pub fn grade(self) -> KnowledgeState {
        match self {
            KnowledgeLearningMode::Witnessed => KnowledgeState::KnowsTrue,
            KnowledgeLearningMode::Deceived => KnowledgeState::BelievesFalse,
            KnowledgeLearningMode::Heard => KnowledgeState::HeardAbout,
        }
    }
}

/// C1 自由函数包装（与 [`KnowledgeLearningMode::grade`] 同义，便于按函数式调用点引用）。
pub fn grade_knowledge_state(mode: KnowledgeLearningMode) -> KnowledgeState {
    mode.grade()
}

/// C2 原子配对的输入（世界事实三元组 + 溯源 + 可选回合/置信/摘要）。holder 与已分档的
/// `knowledge_state` 另传——见 [`build_atomic_knowledge_pair`]。纯数据，无身份校验（校验在
/// 各候选的 `.validated()` 上，commit 路径调用）。
#[derive(Debug, Clone)]
pub struct KnowledgePairInput {
    /// 共享的稳定 fact id（WorldFact 与 KnowledgeUpdate 共用，保证可对账）。
    pub fact_id: String,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub summary: String,
    /// 共享 evidence：溯源事件 id（两候选共用同一份，非空才能通过 `.validated()`）。
    pub source_event_ids: Vec<String>,
    pub confidence: Option<f64>,
    pub turn_id: Option<String>,
    /// 写进 KnowledgeUpdate 的可选缘由（人审/调试用）。
    pub reason: Option<String>,
}

/// C2 强制原子配对（kernel 唯一构造口）：给定世界事实字段 + holder + 已分档的
/// [`KnowledgeState`] + 共享 evidence，返回**同时**含一个 `WorldFact` 与一个 `KnowledgeUpdate`
/// 的 `Vec<MemoryExtractionProposal>`，二者共享**同一 `fact_id`** 与**同一 `source_event_ids`**。
///
/// 通过本函数**不可能**得到孤立的 `KnowledgeUpdate`——这正是 C2 原子性的实现：
/// knowledge_edges→memory_facts 无 FK，原子性靠「永远成对发」在产出侧保证。
///
/// 纯函数：不校验、不落库、不调 LLM；身份/evidence 的 fail-closed 校验留给各候选的
/// `.validated()`（commit 路径执行）。`knowledge_state` 期望来自 [`grade_knowledge_state`]。
pub fn build_atomic_knowledge_pair(
    input: KnowledgePairInput,
    holder: ProposalHolder,
    knowledge_state: KnowledgeState,
) -> Vec<MemoryExtractionProposal> {
    let world_fact = WorldFactCandidate {
        fact_id: input.fact_id.clone(),
        subject: input.subject,
        predicate: input.predicate,
        object: input.object,
        summary: input.summary,
        confidence: input.confidence,
        source_event_ids: input.source_event_ids.clone(),
        turn_id: input.turn_id.clone(),
        truth_status: None,
    };
    let knowledge_update = KnowledgeUpdateCandidate {
        // 共享同一 fact_id：WorldFact 是真相身份，KnowledgeUpdate 仅按 id 引用它。
        fact_id: input.fact_id,
        holder,
        knowledge_state,
        confidence: input.confidence,
        // 共享同一 evidence：两候选溯源到同一批事件。
        source_event_ids: input.source_event_ids,
        learned_at_turn_id: input.turn_id,
        reason: input.reason,
    };
    // 顺序固定 [WorldFact, KnowledgeUpdate]：真相先行，知识随后；二者永远成对出现。
    vec![
        MemoryExtractionProposal::WorldFact(world_fact),
        MemoryExtractionProposal::KnowledgeUpdate(knowledge_update),
    ]
}

/// 知识闸（确定性、纯函数、无 IO；镜像 [`crate::relationship_extraction::relationship_gate_should_run`]）：
/// 判定本回合是否值得跑知识抽取。
///
/// 触发条件（任一为真即跑）：
/// 1. **本回合 surface 了新实体**（`surfaced_new_this_turn`）——已知世界变了，原有成本闸。
/// 2. **玩家输入或念白含明确社交/获知信号**（[`text_has_social_signal`]）——询问/告知/欺骗/
///    威胁等会改变「谁知道什么」却不 surface 新实体的回合。
///
/// 成本控制仍确定性：无新实体且无 source-backed 信号的普通回合一律跳过（fail-closed）。
pub fn knowledge_gate_should_run(
    surfaced_new_this_turn: bool,
    player_input: &str,
    narration: &str,
) -> bool {
    if surfaced_new_this_turn {
        return true;
    }
    text_has_social_signal(player_input) || text_has_social_signal(narration)
}

#[cfg(test)]
#[path = "knowledge_extraction_tests.rs"]
mod knowledge_extraction_tests;
