//! 统一 domain_events 日志的类型（优化2 #6 起步，eventlog write-through）。
//!
//! `DomainEvent` 是一条**不耦合 tick** 的 append-only 领域事件，与既有
//! `WorldEvent`（world_tick/event_seq 耦合世界时间内核）刻意分离：回合生命周期 +
//! 切场景等事件塞进 world_events 会被迫造 tick，语义错位。本日志自带 `seq`
//! （db 层 bigserial）+ `created_at`，是后续 EventLog → Projection 演进的安全种子。
//!
//! 守理念：事件 kind/数据通用，不按规则集名（零规则集硬编码）；serde default
//! 向后兼容；本切片只表示 + 落库，projection 派生留后续。
//!
//! ## 权威性诚实说明（P6 revision — 不可过度声称 / honest authority framing）
//! domain_events 是一条**与状态写并行的 additive 账本**，**fail-soft 写在状态写之后**
//! （`append_domain_event` 失败只 warn，绝不回滚已成功的状态写——见
//! `insert_dice_roll` / `DiceRolled`、`CheckResolved`、`resource_current::ResourceChanged`
//! 的同款写穿模式）。它**尚未**是唯一权威源：今天每个 kind 的权威值仍是各自的状态表
//! （dice_rolls / check_results / generic_parameter_states …）；本日志正按 blueprint §5.2
//! 的渐进迁移**逐步成为** source-of-truth，但当前阶段没有任何 projection 折叠它来重建状态。
//! 因此 doc-comment 不应声称「event-log 即权威源」——准确表述是「正在成为权威源的并行账本」。

use crate::SourceRef;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 领域事件种类（起步子集）：回合生命周期 + 切场景。
///
/// doc §7 列了 ~20 种，本切片只取**安全子集**——回合三态 + 场景切换，其余随后续
/// 切片补。`Copy` + `Default` 便于在 write-through 接缝零成本传递。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum DomainEventKind {
    /// 回合开始（run_pipeline 入口）。
    #[default]
    TurnStarted,
    /// 回合成功收尾（TurnComplete 后）。
    TurnFinalized,
    /// 回合失败（与 TurnTrace.failure 同源）。
    TurnFailed,
    /// 场景切换（scene_commit 处）。
    SceneTransitioned,
    /// 掷骰落库（insert_dice_roll 写穿）。
    DiceRolled,
    /// 检定结算落库（insert_check_result 写穿）。
    CheckResolved,
    /// 模组实体（线索/NPC）首次进入本回合 GM context 即被"surfaced"——**遗留语义**
    /// （P0c 前把"进 context"误等同"玩家见过"）。保留仅为向后兼容旧账本；新写路径用
    /// 语义三分的 [`DomainEventKind::ContextSurfaced`] / [`DomainEventKind::PlayerExposed`]。
    EntitySurfaced,
    /// P0c 语义三分①：实体/事实**进入 GM/runtime context**（当前场景投影把线索/NPC
    /// 喂给回合上下文）。这是隐藏的内部装载，**不**代表玩家已见——故不计入玩家暴露
    /// 投影、不触发关系抽取。幂等 per-session。
    ContextSurfaced,
    /// P0c 语义三分②：实体被实际**暴露进玩家可见虚构**（玩家在念白里真见到了它）。
    /// 计入"玩家暴露"投影（与遗留 EntitySurfaced 并列），可触发关系抽取。
    PlayerExposed,
    /// P0c 语义三分③：player_party **习得/确认了某条事实**。这是写穿
    /// `KnowledgeEdge(player_party, knows_true)` 的事件（取代旧 FactRevealed 写路径），
    /// 由 GM/玩家显式驱动；幂等 per-session+fact。
    PlayerLearnedFact,
    /// P0c 语义三分④：某个**具体 NPC**（按 stable actor id）习得/确认了某条事实。
    /// 这是 NPC 心智的知识输入，**只**作用于目标 NPC holder，绝不触碰 player_party。
    /// 当前阶段 durable NPC holder 投影（knowledge_edges.holder_kind='npc'）尚未开放
    /// （schema CHECK 仅 gm/player_party；NPC actor-id 契约见 TC-KNOW-00 actor 身份门），
    /// 故本事件**只落 domain_events 事件账本**并按 npc_actor_id 投影、对 knowledge_edges
    /// fail-closed。幂等 per-session+npc+fact。
    NpcLearnedFact,
    /// 客户端在 SSE 回合流中途断开，且断开发生在本回合状态已变更之后——引擎续跑
    /// critical finalize 保一致后，在该回合上记此标记（P1-3）。幂等 per-turn。
    ClientDisconnected,
    /// 某条剧透事实（按 entity_id/node_id 作 fact_id）被揭示——revealed-facts 账本
    /// 落账即记，spoiler_guard 据此放行该实体的 secret_terms（幂等 per-session+fact）。
    FactRevealed,
    /// 设计3 §12 CommitCritical：关系提交路径（`CommitAction::Relationship`）写穿的领域事件——
    /// 某 NPC 对某 target 的结构化关系发生 bounded 变更。补齐既有 PlayerLearnedFact /
    /// NpcLearnedFact 的 write-through 缺口（关系提交此前是裸表 upsert、无事件账本）。data 带
    /// npc_id / target / delta 摘要 + 派生 stance/desire。幂等 per 证据集（on-conflict-do-nothing）。
    RelationshipChanged,
    /// P6.2 蓝图§五：actor 某 resource track 的 CURRENT 值经
    /// `write_resource_current`（generic_parameter_states 单源 upsert）发生变更——写穿的
    /// 领域事件。data 带 actor_id / track_id / value（capped 后实存）/ cap / visibility（见
    /// `write_resource_current` 的「schema contract」doc-block）。
    ///
    /// 幂等键 codex#2：world_tick 恒 0，不能进 event_id；改用 (prior_value→capped, cap,
    /// visibility) 的内容哈希作判别——**真不同的变更产不同 event_id**（两次不同 transition
    /// 落两行），**同一 transition 的重放仍幂等**（on conflict do nothing 折叠）。
    ResourceChanged,
    /// P6.3：World 层 NPC 攻击在 Rules 侧结算后写穿的领域事件（gated by
    /// `TRPG_WORLD_NPC_ACTION`）。data 从 typed [`crate::WorldAttackOutcome`] 的结构字段
    /// 取摘要（npc_id / check_id / success / success_tier / blocked）——**绝不**从
    /// `to_gate_fact()` 的字符串里反解。
    ///
    /// 幂等键：`de_npcaction_{check_id}`。`check_id` 是 Rules 侧确定性 `check_results`
    /// 行 id（`npc_attack:{turn}:{npc}`），故同一结算的重放仍幂等（on conflict do nothing）。
    NpcActionResolved,
    /// P6.3 (codex#3)：一个 [`crate::ClockTick`] 被应用到 clock 状态（`insert_clock_tick`
    /// 写穿点）后记的领域事件。data 带 clock_id / new_value（应用后的 `current`）/ delta
    /// （`current - previous`）。
    ///
    /// 幂等键 codex#3：键在**结算后的 clock 状态**上——`de_clock_{session}_{clock_id}_{new_value}`。
    /// 这样「retry-advance 到同一 value」幂等（同一 clock_id+value 折叠成一行），而真正推进
    /// 到不同 value 落新行。**不**用 uuid、**不**镜像 world_event_{uuid}（那不会让重试推进幂等）。
    ClockAdvanced,
    /// L3.1（设计§五 story 事件账本）：某 [`crate::StoryThread`] 从 `Dormant` 被开启
    /// （首次 floor 到 `Introduced`，见 `story_write::apply_thread_opened`）后写穿的领域事件。
    /// 加性 + fail-soft：story_state 快照仍是单一真相源（R2 promotion OutOfScope），此事件只
    /// 补 append-only 账本可观测性。data 带 thread_id / new_status。
    ///
    /// 幂等键 codex 模式（同 ClockAdvanced）：键在**结算后的线程状态**上——
    /// `de_thread_{session}_{thread_id}_{new_status}`。同一开启的重放折叠成一行，真推进到
    /// 不同 status 落新行。
    StoryThreadOpened,
    /// L3.1：某 StoryThread 状态**前进**（非开启、非解决、非休眠的任意前向迁移，如
    /// `Introduced→Active`、`Active→Escalating`/`ReadyForPayoff`）后写穿的领域事件。当前
    /// 确定性写循环尚不产生此迁移（P6.8b 推进 OutOfScope）；变体作为 Story Observer（L7.1）
    /// 的词汇先行落地，由通用 status-diff 派生，绝不按 ruleset 分支。
    StoryThreadAdvanced,
    /// L3.1：某 StoryThread 迁移到 `Resolved` 后写穿的领域事件（同上由通用 diff 派生）。
    StoryThreadResolved,
    /// L3.1：某 StoryThread 从非 `Dormant` 迁移**回** `Dormant`（休眠）后写穿的领域事件。
    StoryThreadDormant,
    /// L3.2：一条 [`crate::StoryPromise`] 被首次种下（在 story_state 中新出现）后写穿的领域事件。
    /// 加性 + fail-soft，快照仍是单一真相源。data 带 promise_id / thread_id / status。
    /// 幂等键在结算后状态上：`de_promise_{session}_{promise}_created`。
    StoryPromiseCreated,
    /// L3.2：一条 StoryPromise 的成熟度被推进/被再次铺垫（maturity 上升或 status 前进但未 PaidOff）
    /// 后写穿的领域事件。幂等键带结算后 status：`de_promise_{session}_{promise}_{status}`。
    StoryPromiseReinforced,
    /// L3.2：一条 StoryPromise 迁移到 `PaidOff` 后写穿的领域事件。data 可带 payoff_event_id。
    StoryPromisePaidOff,
    /// L3.2：一个场景计划（ScenePlan，L4.x）在场景起始被创建后写穿的领域事件。emit 站点在
    /// L4.2（SceneChanged 触发）；本 lane 先落词汇 + 构造器。data 带 scene_id。
    ScenePlanCreated,
    /// L3.2：一个 beat 被规划（post-adjudication DirectorPlan 产出）后写穿的领域事件。emit 站点
    /// 在 L6.x narrator 组合路径；本 lane 先落词汇 + 构造器。data 带 thread_id / beat_kind。
    BeatPlanned,
    /// L3.2：一个已规划 beat 在回合中被实际演出/记录（[`crate::BeatRecord`]）后写穿的领域事件。
    /// emit 站点在 Story Observer（L7.x）。data 带 turn_id / thread_id / beat_kind。
    BeatObserved,
    /// EV-1 `mutation_event_bridge_v1`：一条 **GM 世界事实被提交**（`CommitAction::WorldFact` /
    /// `LegacyFact` 落 `memory_facts`/`world_facts`）后，由 commit 写入口桥接发出的 canonical
    /// 领域事件。GPT Pro 进度证据层设计 Q4/§5.5 明令：「世界事实提交 ≠ 玩家已知晓」——故此 kind
    /// **必须**与 [`DomainEventKind::PlayerLearnedFact`]（玩家习得）/ [`DomainEventKind::FactRevealed`]
    /// （剧透揭示给玩家）语义分开，绝不复用它们冒充世界提交。既有 27 个 kind 无一表「GM 世界事实
    /// 提交」语义，故 EV-1 铸此新 kind（ledger 记理由）。data 带 fact_id + subject/predicate/object/
    /// truth_status 供后续 EV-2 精确投影；幂等键 `de_worldfact_{session}_{fact}_{turn}`。
    /// 当前阶段 progression adapter **尚未**消费它（EV-2 exact projector 的活），故 EV-1 接通后
    /// 不改任何 ProgressSignal（J3 仍 RED，符合设计预期）。
    WorldFactChanged,
}

impl DomainEventKind {
    /// 稳定 token——append 写 db、list 读 db 共用的单一映射源。
    ///
    /// 与 serde 默认 variant 名一致（`serde_json::to_value` 也会得到同名字符串），
    /// 但这里用显式 match 钉死契约，避免日后 `#[serde(rename)]` 悄悄漂移破坏 db。
    pub fn as_str(&self) -> &'static str {
        match self {
            DomainEventKind::TurnStarted => "TurnStarted",
            DomainEventKind::TurnFinalized => "TurnFinalized",
            DomainEventKind::TurnFailed => "TurnFailed",
            DomainEventKind::SceneTransitioned => "SceneTransitioned",
            DomainEventKind::DiceRolled => "DiceRolled",
            DomainEventKind::CheckResolved => "CheckResolved",
            DomainEventKind::EntitySurfaced => "EntitySurfaced",
            DomainEventKind::ContextSurfaced => "ContextSurfaced",
            DomainEventKind::PlayerExposed => "PlayerExposed",
            DomainEventKind::PlayerLearnedFact => "PlayerLearnedFact",
            DomainEventKind::NpcLearnedFact => "NpcLearnedFact",
            DomainEventKind::ClientDisconnected => "ClientDisconnected",
            DomainEventKind::FactRevealed => "FactRevealed",
            DomainEventKind::RelationshipChanged => "RelationshipChanged",
            DomainEventKind::ResourceChanged => "ResourceChanged",
            DomainEventKind::NpcActionResolved => "NpcActionResolved",
            DomainEventKind::ClockAdvanced => "ClockAdvanced",
            DomainEventKind::StoryThreadOpened => "StoryThreadOpened",
            DomainEventKind::StoryThreadAdvanced => "StoryThreadAdvanced",
            DomainEventKind::StoryThreadResolved => "StoryThreadResolved",
            DomainEventKind::StoryThreadDormant => "StoryThreadDormant",
            DomainEventKind::StoryPromiseCreated => "StoryPromiseCreated",
            DomainEventKind::StoryPromiseReinforced => "StoryPromiseReinforced",
            DomainEventKind::StoryPromisePaidOff => "StoryPromisePaidOff",
            DomainEventKind::ScenePlanCreated => "ScenePlanCreated",
            DomainEventKind::BeatPlanned => "BeatPlanned",
            DomainEventKind::BeatObserved => "BeatObserved",
            DomainEventKind::WorldFactChanged => "WorldFactChanged",
        }
    }

    /// db text 列 → 枚举。未知 token fail-closed 回退 `TurnStarted`（与 Default 一致），
    /// 绝不 panic——日志读取不该因脏 token 炸掉调用方。
    pub fn from_str_token(s: &str) -> Self {
        match s {
            "TurnStarted" => DomainEventKind::TurnStarted,
            "TurnFinalized" => DomainEventKind::TurnFinalized,
            "TurnFailed" => DomainEventKind::TurnFailed,
            "SceneTransitioned" => DomainEventKind::SceneTransitioned,
            "DiceRolled" => DomainEventKind::DiceRolled,
            "CheckResolved" => DomainEventKind::CheckResolved,
            "EntitySurfaced" => DomainEventKind::EntitySurfaced,
            "ContextSurfaced" => DomainEventKind::ContextSurfaced,
            "PlayerExposed" => DomainEventKind::PlayerExposed,
            "PlayerLearnedFact" => DomainEventKind::PlayerLearnedFact,
            "NpcLearnedFact" => DomainEventKind::NpcLearnedFact,
            "ClientDisconnected" => DomainEventKind::ClientDisconnected,
            "FactRevealed" => DomainEventKind::FactRevealed,
            "RelationshipChanged" => DomainEventKind::RelationshipChanged,
            "ResourceChanged" => DomainEventKind::ResourceChanged,
            "NpcActionResolved" => DomainEventKind::NpcActionResolved,
            "ClockAdvanced" => DomainEventKind::ClockAdvanced,
            "StoryThreadOpened" => DomainEventKind::StoryThreadOpened,
            "StoryThreadAdvanced" => DomainEventKind::StoryThreadAdvanced,
            "StoryThreadResolved" => DomainEventKind::StoryThreadResolved,
            "StoryThreadDormant" => DomainEventKind::StoryThreadDormant,
            "StoryPromiseCreated" => DomainEventKind::StoryPromiseCreated,
            "StoryPromiseReinforced" => DomainEventKind::StoryPromiseReinforced,
            "StoryPromisePaidOff" => DomainEventKind::StoryPromisePaidOff,
            "ScenePlanCreated" => DomainEventKind::ScenePlanCreated,
            "BeatPlanned" => DomainEventKind::BeatPlanned,
            "BeatObserved" => DomainEventKind::BeatObserved,
            "WorldFactChanged" => DomainEventKind::WorldFactChanged,
            _ => DomainEventKind::TurnStarted,
        }
    }
}

/// 一条领域事件。`event_id` 全局唯一（确定性幂等键，如 `de_{turn_id}_{kind}`）；
/// `turn_id`/`data`/`source_refs` 均 serde default 向后兼容。
///
/// 无 `Default` derive：`DateTime<Utc>` 无 const 默认。用 [`DomainEvent::new`]
/// 在运行时构造（`Utc::now()`），或在测试里给固定时间戳保持 round-trip 确定。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DomainEvent {
    pub event_id: String,
    pub session_id: String,
    #[serde(default)]
    pub turn_id: String,
    pub kind: DomainEventKind,
    #[serde(default)]
    pub data: serde_json::Value,
    #[serde(default)]
    pub source_refs: Vec<SourceRef>,
    pub created_at: DateTime<Utc>,
}

impl DomainEvent {
    /// 运行时构造器：填 `created_at = Utc::now()`，`source_refs` 留空。
    ///
    /// 这是运行时构造（非 workflow 脚本），用 `Utc::now()` OK。需确定性时间戳的
    /// 测试请直接结构体字面量构造并给固定 `created_at`。
    pub fn new(
        event_id: impl Into<String>,
        session_id: impl Into<String>,
        turn_id: impl Into<String>,
        kind: DomainEventKind,
        data: serde_json::Value,
    ) -> Self {
        Self {
            event_id: event_id.into(),
            session_id: session_id.into(),
            turn_id: turn_id.into(),
            kind,
            data,
            source_refs: Vec::new(),
            created_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 固定时间戳便于 round-trip 字节确定。
    fn fixed_ts() -> DateTime<Utc> {
        DateTime::from_timestamp(0, 0).unwrap()
    }

    fn sample() -> DomainEvent {
        DomainEvent {
            event_id: "de_turn1_TurnFinalized".into(),
            session_id: "sess_a".into(),
            turn_id: "turn1".into(),
            kind: DomainEventKind::TurnFinalized,
            data: json!({"signal": "turn_complete"}),
            source_refs: vec![SourceRef {
                source_id: "src1".into(),
                page: Some(7),
                anchor_id: None,
                section_path: vec!["ch1".into()],
                char_start: None,
                char_end: None,
                text_hash: None,
                note: None,
            }],
            created_at: fixed_ts(),
        }
    }

    #[test]
    fn domain_event_roundtrip() {
        let ev = sample();
        let v = serde_json::to_value(&ev).unwrap();
        let back: DomainEvent = serde_json::from_value(v).unwrap();
        assert_eq!(back, ev, "serde to_value→from_value 必字节等价");
    }

    #[test]
    fn domain_event_kind_serde() {
        for k in [
            DomainEventKind::TurnStarted,
            DomainEventKind::TurnFinalized,
            DomainEventKind::TurnFailed,
            DomainEventKind::SceneTransitioned,
            DomainEventKind::DiceRolled,
            DomainEventKind::CheckResolved,
            DomainEventKind::EntitySurfaced,
            DomainEventKind::ClientDisconnected,
        ] {
            let v = serde_json::to_value(k).unwrap();
            let back: DomainEventKind = serde_json::from_value(v).unwrap();
            assert_eq!(back, k, "7 variant 必 round-trip");
        }
    }

    #[test]
    fn domain_event_kind_str_roundtrip() {
        // DB-free：as_str ↔ from_str_token 全 variant 闭环；serde token 与 as_str 一致。
        for k in [
            DomainEventKind::TurnStarted,
            DomainEventKind::TurnFinalized,
            DomainEventKind::TurnFailed,
            DomainEventKind::SceneTransitioned,
            DomainEventKind::DiceRolled,
            DomainEventKind::CheckResolved,
            DomainEventKind::EntitySurfaced,
            DomainEventKind::ClientDisconnected,
        ] {
            assert_eq!(DomainEventKind::from_str_token(k.as_str()), k);
            // serde 序列化得到的字符串必与 as_str 钉死的契约一致。
            let serde_token = serde_json::to_value(k).unwrap();
            assert_eq!(serde_token.as_str(), Some(k.as_str()));
        }
        // 未知 token fail-closed 回退 Default。
        assert_eq!(
            DomainEventKind::from_str_token("Bogus"),
            DomainEventKind::TurnStarted
        );
    }

    #[test]
    fn client_disconnected_kind_roundtrips() {
        // P1-3：客户端中途断开后落在回合上的标记，经 append_domain_event 写库。
        let k = DomainEventKind::ClientDisconnected;
        assert_eq!(k.as_str(), "ClientDisconnected");
        assert_eq!(DomainEventKind::from_str_token("ClientDisconnected"), k);
        let v = serde_json::to_value(k).unwrap();
        assert_eq!(v.as_str(), Some("ClientDisconnected"));
        let back: DomainEventKind = serde_json::from_value(v).unwrap();
        assert_eq!(back, k);
    }

    #[test]
    fn fact_revealed_kind_token_and_serde_roundtrip() {
        // 反剧透账本事件：token 稳定契约 + serde 闭环 + 未知回退不受影响。
        let k = DomainEventKind::FactRevealed;
        assert_eq!(k.as_str(), "FactRevealed");
        assert_eq!(DomainEventKind::from_str_token("FactRevealed"), k);
        let v = serde_json::to_value(k).unwrap();
        assert_eq!(
            v.as_str(),
            Some("FactRevealed"),
            "serde token 必与 as_str 一致"
        );
        let back: DomainEventKind = serde_json::from_value(v).unwrap();
        assert_eq!(back, k);
    }

    #[test]
    fn semantic_split_kinds_token_and_serde_roundtrip() {
        // P0c 语义三分：ContextSurfaced（进 GM/context）≠ PlayerExposed（暴露给玩家可见
        // 虚构）≠ PlayerLearnedFact（玩家确知事实，写穿 KnowledgeEdge）。三者 token 稳定 +
        // serde 闭环；旧 EntitySurfaced/FactRevealed 保留不删（见上别测）。
        for k in [
            DomainEventKind::ContextSurfaced,
            DomainEventKind::PlayerExposed,
            DomainEventKind::PlayerLearnedFact,
            DomainEventKind::NpcLearnedFact,
        ] {
            assert_eq!(DomainEventKind::from_str_token(k.as_str()), k);
            let v = serde_json::to_value(k).unwrap();
            assert_eq!(v.as_str(), Some(k.as_str()), "serde token 必与 as_str 一致");
            let back: DomainEventKind = serde_json::from_value(v).unwrap();
            assert_eq!(back, k);
        }
        // 钉死 token 字面量，防日后改名悄悄破坏 db 兼容。
        assert_eq!(DomainEventKind::ContextSurfaced.as_str(), "ContextSurfaced");
        assert_eq!(DomainEventKind::PlayerExposed.as_str(), "PlayerExposed");
        assert_eq!(
            DomainEventKind::PlayerLearnedFact.as_str(),
            "PlayerLearnedFact"
        );
        assert_eq!(DomainEventKind::NpcLearnedFact.as_str(), "NpcLearnedFact");
        // 四个新 variant 互不相等、且与旧 EntitySurfaced/FactRevealed 区分。
        assert_ne!(
            DomainEventKind::ContextSurfaced,
            DomainEventKind::EntitySurfaced
        );
        assert_ne!(
            DomainEventKind::PlayerLearnedFact,
            DomainEventKind::FactRevealed
        );
        // NpcLearnedFact 是独立的 NPC 心智输入，绝不等同玩家习得事实。
        assert_ne!(
            DomainEventKind::NpcLearnedFact,
            DomainEventKind::PlayerLearnedFact
        );
    }

    #[test]
    fn relationship_changed_kind_token_and_serde_roundtrip() {
        // 设计3 §12：关系变更写穿事件。token 稳定契约 + serde 闭环 + 与既有 variant 区分。
        let k = DomainEventKind::RelationshipChanged;
        assert_eq!(k.as_str(), "RelationshipChanged");
        assert_eq!(DomainEventKind::from_str_token("RelationshipChanged"), k);
        let v = serde_json::to_value(k).unwrap();
        assert_eq!(
            v.as_str(),
            Some("RelationshipChanged"),
            "serde token 必与 as_str 一致"
        );
        let back: DomainEventKind = serde_json::from_value(v).unwrap();
        assert_eq!(back, k);
        // 与既有事件种类互不相等。
        assert_ne!(
            DomainEventKind::RelationshipChanged,
            DomainEventKind::PlayerLearnedFact
        );
        assert_ne!(
            DomainEventKind::RelationshipChanged,
            DomainEventKind::NpcLearnedFact
        );
    }

    #[test]
    fn resource_changed_kind_token_and_serde_roundtrip() {
        // P6.2：resource CURRENT 写穿事件。token 稳定契约 + serde 闭环 + 与既有 14 variant 区分。
        let k = DomainEventKind::ResourceChanged;
        assert_eq!(k.as_str(), "ResourceChanged");
        assert_eq!(DomainEventKind::from_str_token("ResourceChanged"), k);
        let v = serde_json::to_value(k).unwrap();
        assert_eq!(
            v.as_str(),
            Some("ResourceChanged"),
            "serde token 必与 as_str 一致"
        );
        let back: DomainEventKind = serde_json::from_value(v).unwrap();
        assert_eq!(back, k);
        // 新 token 必与既有 14 个全部不相交（不碰旧 token——round-trip 守卫锁死它们）。
        for existing in [
            "TurnStarted",
            "TurnFinalized",
            "TurnFailed",
            "SceneTransitioned",
            "DiceRolled",
            "CheckResolved",
            "EntitySurfaced",
            "ContextSurfaced",
            "PlayerExposed",
            "PlayerLearnedFact",
            "NpcLearnedFact",
            "ClientDisconnected",
            "FactRevealed",
            "RelationshipChanged",
        ] {
            assert_ne!(k.as_str(), existing, "ResourceChanged token 必与既有 14 个不同");
        }
        // fail-closed 未知回退不受影响。
        assert_eq!(
            DomainEventKind::from_str_token("Bogus"),
            DomainEventKind::TurnStarted
        );
    }

    #[test]
    fn npc_action_and_clock_advanced_kinds_token_and_serde_roundtrip() {
        // P6.3：NpcActionResolved + ClockAdvanced 写穿事件。token 稳定契约 + serde 闭环 +
        // 与既有 15 variant 区分（不碰旧 token——上方 round-trip 守卫锁死它们）。
        for k in [
            DomainEventKind::NpcActionResolved,
            DomainEventKind::ClockAdvanced,
        ] {
            assert_eq!(DomainEventKind::from_str_token(k.as_str()), k);
            let v = serde_json::to_value(k).unwrap();
            assert_eq!(v.as_str(), Some(k.as_str()), "serde token 必与 as_str 一致");
            let back: DomainEventKind = serde_json::from_value(v).unwrap();
            assert_eq!(back, k);
        }
        assert_eq!(
            DomainEventKind::NpcActionResolved.as_str(),
            "NpcActionResolved"
        );
        assert_eq!(DomainEventKind::ClockAdvanced.as_str(), "ClockAdvanced");
        // 两个新 token 必与既有 15 个全部不相交。
        for existing in [
            "TurnStarted",
            "TurnFinalized",
            "TurnFailed",
            "SceneTransitioned",
            "DiceRolled",
            "CheckResolved",
            "EntitySurfaced",
            "ContextSurfaced",
            "PlayerExposed",
            "PlayerLearnedFact",
            "NpcLearnedFact",
            "ClientDisconnected",
            "FactRevealed",
            "RelationshipChanged",
            "ResourceChanged",
        ] {
            assert_ne!(DomainEventKind::NpcActionResolved.as_str(), existing);
            assert_ne!(DomainEventKind::ClockAdvanced.as_str(), existing);
        }
        assert_ne!(
            DomainEventKind::NpcActionResolved,
            DomainEventKind::ClockAdvanced
        );
        // fail-closed 未知回退不受影响。
        assert_eq!(
            DomainEventKind::from_str_token("Bogus"),
            DomainEventKind::TurnStarted
        );
    }

    #[test]
    fn story_thread_kinds_token_and_serde_roundtrip() {
        // L3.1：4 个 StoryThread 账本事件。token 稳定契约 + serde 闭环 + 与既有 17 variant 区分
        // （不碰旧 token——上方 round-trip 守卫锁死它们）。
        let new_kinds = [
            DomainEventKind::StoryThreadOpened,
            DomainEventKind::StoryThreadAdvanced,
            DomainEventKind::StoryThreadResolved,
            DomainEventKind::StoryThreadDormant,
        ];
        for k in new_kinds {
            assert_eq!(DomainEventKind::from_str_token(k.as_str()), k);
            let v = serde_json::to_value(k).unwrap();
            assert_eq!(v.as_str(), Some(k.as_str()), "serde token 必与 as_str 一致");
            let back: DomainEventKind = serde_json::from_value(v).unwrap();
            assert_eq!(back, k);
        }
        // 钉死 token 字面量，防日后改名悄悄破坏 db 兼容。
        assert_eq!(DomainEventKind::StoryThreadOpened.as_str(), "StoryThreadOpened");
        assert_eq!(
            DomainEventKind::StoryThreadAdvanced.as_str(),
            "StoryThreadAdvanced"
        );
        assert_eq!(
            DomainEventKind::StoryThreadResolved.as_str(),
            "StoryThreadResolved"
        );
        assert_eq!(
            DomainEventKind::StoryThreadDormant.as_str(),
            "StoryThreadDormant"
        );
        // 4 个新 token 必与既有 17 个全部不相交，且彼此互不相同。
        for existing in [
            "TurnStarted",
            "TurnFinalized",
            "TurnFailed",
            "SceneTransitioned",
            "DiceRolled",
            "CheckResolved",
            "EntitySurfaced",
            "ContextSurfaced",
            "PlayerExposed",
            "PlayerLearnedFact",
            "NpcLearnedFact",
            "ClientDisconnected",
            "FactRevealed",
            "RelationshipChanged",
            "ResourceChanged",
            "NpcActionResolved",
            "ClockAdvanced",
        ] {
            for k in new_kinds {
                assert_ne!(k.as_str(), existing, "新 story token 必与既有 17 个不同");
            }
        }
        for i in 0..new_kinds.len() {
            for j in (i + 1)..new_kinds.len() {
                assert_ne!(new_kinds[i], new_kinds[j], "4 个 story 变体互不相同");
            }
        }
        // fail-closed 未知回退不受影响。
        assert_eq!(
            DomainEventKind::from_str_token("Bogus"),
            DomainEventKind::TurnStarted
        );
    }

    #[test]
    fn story_promise_and_scene_beat_kinds_token_and_serde_roundtrip() {
        // L3.2：6 个 promise/scene/beat 账本事件。token 稳定 + serde 闭环 + 与既有 21 variant 区分。
        let new_kinds = [
            DomainEventKind::StoryPromiseCreated,
            DomainEventKind::StoryPromiseReinforced,
            DomainEventKind::StoryPromisePaidOff,
            DomainEventKind::ScenePlanCreated,
            DomainEventKind::BeatPlanned,
            DomainEventKind::BeatObserved,
        ];
        for k in new_kinds {
            assert_eq!(DomainEventKind::from_str_token(k.as_str()), k);
            let v = serde_json::to_value(k).unwrap();
            assert_eq!(v.as_str(), Some(k.as_str()), "serde token 必与 as_str 一致");
            let back: DomainEventKind = serde_json::from_value(v).unwrap();
            assert_eq!(back, k);
        }
        // 与既有 21 个全部不相交（17 原始 + 4 个 L3.1 thread 变体），且彼此互不相同。
        for existing in [
            "TurnStarted",
            "TurnFinalized",
            "TurnFailed",
            "SceneTransitioned",
            "DiceRolled",
            "CheckResolved",
            "EntitySurfaced",
            "ContextSurfaced",
            "PlayerExposed",
            "PlayerLearnedFact",
            "NpcLearnedFact",
            "ClientDisconnected",
            "FactRevealed",
            "RelationshipChanged",
            "ResourceChanged",
            "NpcActionResolved",
            "ClockAdvanced",
            "StoryThreadOpened",
            "StoryThreadAdvanced",
            "StoryThreadResolved",
            "StoryThreadDormant",
        ] {
            for k in new_kinds {
                assert_ne!(k.as_str(), existing, "新 promise/scene/beat token 必与既有 21 个不同");
            }
        }
        for i in 0..new_kinds.len() {
            for j in (i + 1)..new_kinds.len() {
                assert_ne!(new_kinds[i], new_kinds[j], "6 个变体互不相同");
            }
        }
        assert_eq!(
            DomainEventKind::from_str_token("Bogus"),
            DomainEventKind::TurnStarted
        );
    }

    #[test]
    fn world_fact_changed_kind_token_and_serde_roundtrip() {
        // EV-1 mutation_event_bridge_v1: minted kind for "a GM world fact was committed"
        // (世界事实提交 ≠ 玩家已知晓 — GPT Pro design Q4/§5.5 demands this be distinct from
        // PlayerLearnedFact / FactRevealed). token 稳定契约 + serde 闭环 + 与既有 27 variant 区分。
        let k = DomainEventKind::WorldFactChanged;
        assert_eq!(k.as_str(), "WorldFactChanged");
        assert_eq!(DomainEventKind::from_str_token("WorldFactChanged"), k);
        let v = serde_json::to_value(k).unwrap();
        assert_eq!(
            v.as_str(),
            Some("WorldFactChanged"),
            "serde token 必与 as_str 一致"
        );
        let back: DomainEventKind = serde_json::from_value(v).unwrap();
        assert_eq!(back, k);
        // 与既有 27 个 variant 全部不相交（不碰旧 token——上方 round-trip 守卫锁死它们），
        // 尤其与 PlayerLearnedFact / FactRevealed 区分（设计 §5.5 反对的语义混淆）。
        for existing in [
            "TurnStarted",
            "TurnFinalized",
            "TurnFailed",
            "SceneTransitioned",
            "DiceRolled",
            "CheckResolved",
            "EntitySurfaced",
            "ContextSurfaced",
            "PlayerExposed",
            "PlayerLearnedFact",
            "NpcLearnedFact",
            "ClientDisconnected",
            "FactRevealed",
            "RelationshipChanged",
            "ResourceChanged",
            "NpcActionResolved",
            "ClockAdvanced",
            "StoryThreadOpened",
            "StoryThreadAdvanced",
            "StoryThreadResolved",
            "StoryThreadDormant",
            "StoryPromiseCreated",
            "StoryPromiseReinforced",
            "StoryPromisePaidOff",
            "ScenePlanCreated",
            "BeatPlanned",
            "BeatObserved",
        ] {
            assert_ne!(k.as_str(), existing, "WorldFactChanged token 必与既有 27 个不同");
        }
        // fail-closed 未知回退不受影响。
        assert_eq!(
            DomainEventKind::from_str_token("Bogus"),
            DomainEventKind::TurnStarted
        );
    }

    #[test]
    fn domain_event_back_compat() {
        // 缺 turn_id/data/source_refs 的旧载荷必反序列化为 default 空值。
        let raw = json!({
            "event_id": "e",
            "session_id": "s",
            "kind": "TurnStarted",
            "created_at": "1970-01-01T00:00:00Z"
        });
        let ev: DomainEvent = serde_json::from_value(raw).unwrap();
        assert_eq!(ev.turn_id, "");
        assert_eq!(ev.data, serde_json::Value::Null);
        assert!(ev.source_refs.is_empty());
        assert_eq!(ev.kind, DomainEventKind::TurnStarted);
        assert_eq!(ev.created_at, fixed_ts());
    }
}
