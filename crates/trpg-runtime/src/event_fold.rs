//! P6.4 事件折叠（Projection trait + fold）——设计4 §19-#8 replay parity 的运行时载体。
//!
//! 框架契约（FRAMEWORK §1）：
//!   - Event Log = 权威源 ②（domain_events，append-only）。
//!   - Projection = **视图 ③**，不是第二事实源；它只能由事件折叠得出，回放同序事件
//!     必逐字段重建同一投影（replay #10）。
//!
//! 本模块提供纯函数式 [`Projection`] trait + 三个具体投影，复刻 trpg-db 的三条只读
//! 视图，使「fold(events) == db_view(session)」可被 live parity 测试钉死：
//!   - [`PlayerExposureProjection`]  ↔ `Db::list_surfaced_entities`（PlayerExposed+EntitySurfaced）
//!   - [`ContextSurfacedProjection`] ↔ `Db::list_context_surfaced_entities`（ContextSurfaced）
//!   - [`PlayerKnowledgeProjection`] ↔ **仅** PlayerLearnedFact-sourced 子集（见 codex#4 下注）。
//!
//! **codex#4 决策（DOWNGRADE，非 fold-all）**：`knowledge_edges` 由**多个无对应
//! domain_event 的写入源**共同填充——尤其 `CommitAction::PlayerPartyEdge` 直接调
//! `upsert_knowledge_edge_player_party` 而**不**落 domain_event（见
//! memory_proposal.rs::apply_commit_action）。故无法诚实声称
//! `PlayerKnowledgeProjection::fold == Db::player_knowledge_view`。本投影**只**折叠
//! `PlayerLearnedFact` 事件，覆盖 player_knowledge 的「事件账本驱动子集」；
//! `knowledge_edges` 本身是宪法③投影（其权威源是事件账本 + 旁路 upsert），不是第二
//! 事实源。我们绝不把 `knowledge_edges` 当成可与事件折叠对账的全集。

use std::collections::BTreeSet;
use trpg_model::{DomainEvent, DomainEventKind};

/// 确定性事件折叠投影。`Default` 给空投影；`apply` 吸收一条事件；`fold` 先按
/// `event_id` 去重（BTreeSet ⇒ 稳定序），再按去重后顺序 apply ⇒ 折两次字节相等、
/// 输入乱序结果不变、重复投递幂等。
pub trait Projection: Default {
    /// 吸收单条事件（typed-parse `ev.data`，Value 仍是 Value，不外溢类型）。
    fn apply(&mut self, ev: &DomainEvent);

    /// 折叠事件序列为投影：先按 `event_id` 去重（幂等），再按事件原序 apply。
    ///
    /// 去重用 `BTreeSet<&str>` 记已见 event_id；首次见才 apply。这样：
    ///   - **重复投递幂等**：同 event_id 二次出现被跳过。
    ///   - **乱序稳定**：每个具体投影内部用 BTreeSet 收集元素 ⇒ 与 apply 顺序无关。
    fn fold(events: &[DomainEvent]) -> Self
    where
        Self: Sized,
    {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut proj = Self::default();
        for ev in events {
            if seen.insert(ev.event_id.as_str()) {
                proj.apply(ev);
            }
        }
        proj
    }
}

/// 从事件 `data` jsonb 抽 `(entity_id, entity_kind)`；缺 entity_id ⇒ None（与 db 视图
/// `data->>'entity_id' is not null` 过滤对齐）。entity_kind 缺省空串（db `unwrap_or_default`）。
fn entity_pair(ev: &DomainEvent) -> Option<(String, String)> {
    let id = ev.data.get("entity_id").and_then(|v| v.as_str())?;
    if id.is_empty() {
        return None;
    }
    let kind = ev
        .data
        .get("entity_kind")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    Some((id.to_string(), kind))
}

/// 玩家暴露投影 ↔ `Db::list_surfaced_entities`：折叠 `PlayerExposed` + 遗留
/// `EntitySurfaced`，**绝不**含 `ContextSurfaced`（隐藏 context 装载 ≠ 玩家见过）。
/// BTreeSet ⇒ distinct + entity_id 稳定序，复刻 db 的 `select distinct ... order by entity_id`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlayerExposureProjection {
    pub entities: BTreeSet<(String, String)>,
}

impl Projection for PlayerExposureProjection {
    fn apply(&mut self, ev: &DomainEvent) {
        if matches!(
            ev.kind,
            DomainEventKind::PlayerExposed | DomainEventKind::EntitySurfaced
        ) {
            if let Some(pair) = entity_pair(ev) {
                self.entities.insert(pair);
            }
        }
    }
}

/// context 装载投影 ↔ `Db::list_context_surfaced_entities`：只折叠 `ContextSurfaced`。
/// 与玩家暴露严格区分——进 GM/context 不计入玩家可见集。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextSurfacedProjection {
    pub entities: BTreeSet<(String, String)>,
}

impl Projection for ContextSurfacedProjection {
    fn apply(&mut self, ev: &DomainEvent) {
        if matches!(ev.kind, DomainEventKind::ContextSurfaced) {
            if let Some(pair) = entity_pair(ev) {
                self.entities.insert(pair);
            }
        }
    }
}

/// 玩家知识投影（**PlayerLearnedFact-sourced 子集**，见模块头 codex#4）。
/// 折叠 `PlayerLearnedFact` 事件的 `data->>'fact_id'`，distinct + 稳定序。
///
/// 这**不**等于 `Db::player_knowledge_view`：后者读 `knowledge_edges`，其中有
/// `PlayerPartyEdge` 旁路 upsert 等无 domain_event 的写入源。本投影只覆盖事件账本
/// 驱动的那部分，故 live parity 只能断言 `fold ⊆ player_knowledge_view`，不可断言相等。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlayerKnowledgeProjection {
    pub fact_ids: BTreeSet<String>,
}

impl Projection for PlayerKnowledgeProjection {
    fn apply(&mut self, ev: &DomainEvent) {
        if matches!(ev.kind, DomainEventKind::PlayerLearnedFact) {
            if let Some(fact) = ev.data.get("fact_id").and_then(|v| v.as_str()) {
                if !fact.is_empty() {
                    self.fact_ids.insert(fact.to_string());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use serde_json::json;

    fn ev(id: &str, kind: DomainEventKind, data: serde_json::Value) -> DomainEvent {
        DomainEvent {
            event_id: id.into(),
            session_id: "s".into(),
            turn_id: "t".into(),
            kind,
            data,
            source_refs: Vec::new(),
            created_at: DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
        }
    }

    fn exposure_fixture() -> Vec<DomainEvent> {
        vec![
            ev(
                "e1",
                DomainEventKind::PlayerExposed,
                json!({"entity_id":"npc_b","entity_kind":"npc"}),
            ),
            ev(
                "e2",
                DomainEventKind::EntitySurfaced,
                json!({"entity_id":"sc_a","entity_kind":"scene"}),
            ),
            // ContextSurfaced 是噪声：绝不进玩家暴露集。
            ev(
                "e3",
                DomainEventKind::ContextSurfaced,
                json!({"entity_id":"npc_hidden","entity_kind":"npc"}),
            ),
            // 缺 entity_id ⇒ 被过滤。
            ev("e4", DomainEventKind::PlayerExposed, json!({"foo":"bar"})),
        ]
    }

    #[test]
    fn exposure_fold_twice_byte_equal() {
        let evs = exposure_fixture();
        let a = PlayerExposureProjection::fold(&evs);
        let b = PlayerExposureProjection::fold(&evs);
        assert_eq!(a, b, "折两次必逐字段相等");
        // 序列化字节也相等（BTreeSet 稳定序）。
        assert_eq!(
            serde_json_vec(&a.entities),
            serde_json_vec(&b.entities),
            "BTreeSet ⇒ 字节稳定"
        );
        assert_eq!(
            a.entities,
            BTreeSet::from([
                ("npc_b".to_string(), "npc".to_string()),
                ("sc_a".to_string(), "scene".to_string()),
            ]),
            "只含 PlayerExposed+EntitySurfaced，排除 ContextSurfaced 与缺 id 行"
        );
    }

    #[test]
    fn exposure_fold_shuffle_stable() {
        let mut evs = exposure_fixture();
        let forward = PlayerExposureProjection::fold(&evs);
        evs.reverse();
        let reversed = PlayerExposureProjection::fold(&evs);
        assert_eq!(forward, reversed, "输入乱序 → 投影不变（BTreeSet 稳定）");
    }

    #[test]
    fn fold_dedups_by_event_id_idempotent() {
        // 同 event_id 重复投递：折叠后与单次相等（幂等）。
        let evs = vec![
            ev(
                "dup",
                DomainEventKind::PlayerLearnedFact,
                json!({"fact_id":"f1"}),
            ),
            ev(
                "dup",
                DomainEventKind::PlayerLearnedFact,
                // 同 id、不同 payload：去重应只认第一条，第二条被跳过。
                json!({"fact_id":"f2_should_be_ignored"}),
            ),
        ];
        let proj = PlayerKnowledgeProjection::fold(&evs);
        assert_eq!(
            proj.fact_ids,
            BTreeSet::from(["f1".to_string()]),
            "同 event_id 第二条被去重跳过（幂等）"
        );
    }

    #[test]
    fn context_surfaced_only_folds_context() {
        let evs = exposure_fixture();
        let proj = ContextSurfacedProjection::fold(&evs);
        assert_eq!(
            proj.entities,
            BTreeSet::from([("npc_hidden".to_string(), "npc".to_string())]),
            "只折叠 ContextSurfaced；PlayerExposed/EntitySurfaced 不混入"
        );
    }

    #[test]
    fn player_knowledge_folds_learned_facts_only() {
        let evs = vec![
            ev(
                "k1",
                DomainEventKind::PlayerLearnedFact,
                json!({"fact_id":"npc_butler"}),
            ),
            ev(
                "k2",
                DomainEventKind::PlayerLearnedFact,
                json!({"fact_id":"sc_cellar"}),
            ),
            // NpcLearnedFact 是 NPC 心智，不进玩家知识。
            ev(
                "k3",
                DomainEventKind::NpcLearnedFact,
                json!({"fact_id":"npc_secret"}),
            ),
        ];
        let proj = PlayerKnowledgeProjection::fold(&evs);
        assert_eq!(
            proj.fact_ids,
            BTreeSet::from(["npc_butler".to_string(), "sc_cellar".to_string()]),
            "只折叠 PlayerLearnedFact；NpcLearnedFact 不混入"
        );
    }

    fn serde_json_vec(set: &BTreeSet<(String, String)>) -> Vec<u8> {
        serde_json::to_vec(&set.iter().collect::<Vec<_>>()).unwrap()
    }
}
