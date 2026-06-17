//! KnowledgeEdge 账本（P0b）：揭示事实的统一超集/投影源。
//! P0b 仅 gm / player_party 两类 holder；NPC holder 留待 P1（actor-id 统一后）。
//! revealed-facts 兼容投影 = 本表 (player_party, knows_true) 子集；
//! 与 domain_events.FactRevealed 写穿对齐（见 trpg-db record_revealed_fact）。
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 知识 holder 种类。P0b 只落 gm / player_party，NPC 等具体角色 holder 见 P1 gate。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeHolderKind {
    Gm,
    PlayerParty,
}

/// holder 对某 fact 的知识态。knows_true = 已确知为真（revealed 兼容投影只取此态）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeState {
    Unknown,
    Exposed,
    KnowsTrue,
    BelievesFalse,
}

/// 一条知识边：某 session 内 holder 对某 fact 的知识态及其来源事件/理由。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeEdge {
    pub edge_id: String,
    pub session_id: String,
    pub holder_kind: KnowledgeHolderKind,
    pub holder_id: Option<String>,
    pub fact_id: String,
    pub knowledge_state: KnowledgeState,
    pub source_event_id: Option<String>,
    pub reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn knowledge_enums_are_snake_case() {
        assert_eq!(
            serde_json::to_value(KnowledgeHolderKind::PlayerParty).unwrap(),
            json!("player_party")
        );
        assert_eq!(
            serde_json::to_value(KnowledgeState::KnowsTrue).unwrap(),
            json!("knows_true")
        );
    }
}
