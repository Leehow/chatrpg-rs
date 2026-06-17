//! 统一 domain_events 日志的类型（优化2 #6 起步，eventlog write-through）。
//!
//! `DomainEvent` 是一条**不耦合 tick** 的 append-only 领域事件，与既有
//! `WorldEvent`（world_tick/event_seq 耦合世界时间内核）刻意分离：回合生命周期 +
//! 切场景等事件塞进 world_events 会被迫造 tick，语义错位。本日志自带 `seq`
//! （db 层 bigserial）+ `created_at`，是后续 EventLog → Projection 演进的安全种子。
//!
//! 守理念：事件 kind/数据通用，不按规则集名（零规则集硬编码）；serde default
//! 向后兼容；本切片只表示 + 落库，projection 派生留后续。

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
    /// 模组实体（线索/NPC）首次进入本回合 GM context 即被"surfaced"——玩家已被
    /// 暴露给该实体（反剧透 TruthGraph 起步切片，观测层；幂等 per-session）。
    EntitySurfaced,
    /// 某条剧透事实（按 entity_id/node_id 作 fact_id）被揭示——revealed-facts 账本
    /// 落账即记，spoiler_guard 据此放行该实体的 secret_terms（幂等 per-session+fact）。
    FactRevealed,
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
            DomainEventKind::FactRevealed => "FactRevealed",
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
            "FactRevealed" => DomainEventKind::FactRevealed,
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
    fn fact_revealed_kind_token_and_serde_roundtrip() {
        // 反剧透账本事件：token 稳定契约 + serde 闭环 + 未知回退不受影响。
        let k = DomainEventKind::FactRevealed;
        assert_eq!(k.as_str(), "FactRevealed");
        assert_eq!(DomainEventKind::from_str_token("FactRevealed"), k);
        let v = serde_json::to_value(k).unwrap();
        assert_eq!(v.as_str(), Some("FactRevealed"), "serde token 必与 as_str 一致");
        let back: DomainEventKind = serde_json::from_value(v).unwrap();
        assert_eq!(back, k);
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
