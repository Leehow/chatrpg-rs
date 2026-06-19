//! 设计3 §4 — 事实自身真假分类（`FactTruthStatus`）。
//!
//! 这是**事实真相**轴：一条 [`crate::WorldFactCandidate`] 描述的命题本身是真、是假、
//! 是传闻、是谎言、是主观判断，还是尚未分类。它与 [`crate::KnowledgeState`]（谁相信
//! 什么——*知识/信念*轴）**正交**：某 NPC 可以 `knows_true` 一条 `False` 事实（确知它
//! 是假的），也可以 `believes_true` 一条 `Lie`。二者不可混为一谈。
//!
//! 严格加性：本枚举只描述数据，不参与任何提交/投影管道接线（那是 runtime-owned
//! commit 路径的事，归 codex 协调）。默认缺省 = `None`（未标），DB 列默认 NULL。

use serde::{Deserialize, Serialize};

/// 一条世界事实命题自身的真假分类（与「谁相信它」的 [`crate::KnowledgeState`] 正交）。
///
/// snake_case token 与未来 `memory_facts.truth_status` 列对齐。`Unknown` 是 fail-closed
/// 缺省：未被分类的事实既不当真也不当假，避免把"没标"误读成"已确认为真"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactTruthStatus {
    /// 命题为真（世界真相确证）。
    True,
    /// 命题为假（已确证不成立）。
    False,
    /// 传闻：来源不可靠、真假未定的二手说法。
    Rumor,
    /// 谎言：有意编造的不实陈述（区别于无心传闻）。
    Lie,
    /// 主观判断 / 价值评价，无客观真假可言。
    Subjective,
    /// 未分类（缺省 fail-closed 态：既不当真也不当假）。
    Unknown,
}

impl FactTruthStatus {
    /// 与 `memory_facts.truth_status` 列对齐的稳定 token（snake_case）。
    pub fn as_token(&self) -> &'static str {
        match self {
            FactTruthStatus::True => "true",
            FactTruthStatus::False => "false",
            FactTruthStatus::Rumor => "rumor",
            FactTruthStatus::Lie => "lie",
            FactTruthStatus::Subjective => "subjective",
            FactTruthStatus::Unknown => "unknown",
        }
    }

    /// 从列 token 解析真假分类（与 [`Self::as_token`] 互逆）；未知 token 返回 None，
    /// 调用方据此 fail-closed（绝不把未知 token 误当某已知分类）。
    pub fn from_token(token: &str) -> Option<Self> {
        match token {
            "true" => Some(FactTruthStatus::True),
            "false" => Some(FactTruthStatus::False),
            "rumor" => Some(FactTruthStatus::Rumor),
            "lie" => Some(FactTruthStatus::Lie),
            "subjective" => Some(FactTruthStatus::Subjective),
            "unknown" => Some(FactTruthStatus::Unknown),
            _ => None,
        }
    }
}

impl Default for FactTruthStatus {
    /// 缺省 = `Unknown`：未标的事实 fail-closed，不预设真假。
    fn default() -> Self {
        FactTruthStatus::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_and_from_token_roundtrip_all_variants() {
        // as_token ↔ from_token 全变体闭环；token 是 DB 列契约，不可漂。
        for v in [
            FactTruthStatus::True,
            FactTruthStatus::False,
            FactTruthStatus::Rumor,
            FactTruthStatus::Lie,
            FactTruthStatus::Subjective,
            FactTruthStatus::Unknown,
        ] {
            assert_eq!(FactTruthStatus::from_token(v.as_token()), Some(v));
        }
    }

    #[test]
    fn serde_uses_snake_case_token() {
        // serde 字符串 token 必与 as_token 一致（同一列写法）。
        let v = serde_json::to_value(FactTruthStatus::Rumor).unwrap();
        assert_eq!(v.as_str(), Some("rumor"));
        let back: FactTruthStatus = serde_json::from_value(v).unwrap();
        assert_eq!(back, FactTruthStatus::Rumor);
    }

    #[test]
    fn unknown_token_fails_closed_to_none() {
        // 未知 token 不得被误当某已知分类。
        assert_eq!(FactTruthStatus::from_token("fabricated"), None);
        assert_eq!(FactTruthStatus::from_token(""), None);
    }

    #[test]
    fn default_is_unknown() {
        assert_eq!(FactTruthStatus::default(), FactTruthStatus::Unknown);
    }
}
