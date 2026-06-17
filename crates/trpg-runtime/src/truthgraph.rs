//! 反剧透 TruthGraph 起步切片（优化2 #5）——**观测层**。
//!
//! 本切片只做一件事：当模组实体（线索/NPC）随**当前场景**投影进 GM 回合
//! context，就记一条 `EntitySurfaced` 领域事件——"玩家已被暴露给该实体"。
//! 与影子 binding 同立场：**零行为变更、纯数据采集**，绝不做剧透裁剪/强制
//! （强制留后续切片）。
//!
//! 设计要点：
//! - **幂等 per-session**：`event_id = de_surfaced_{session}_{entity}`——每个实体
//!   全会话只记一次（再 surface 经 `on conflict do nothing` 化为 no-op）。语义即
//!   "玩家这局见过它"，与回合/场景无关；data 里仍带首次记录的 scene_id 供溯源。
//! - **纯函数**：`surfaced_events_for_scene` 只吃 `&ScenarioNode` 产 `Vec<DomainEvent>`，
//!   DB-free 可单测；append 由 runtime 调用方做（fail-soft）。
//! - **零规则集硬编码**：entity_kind 只用通用 "clue"/"npc"，不按规则集/模组名分支。

use trpg_model::{DomainEvent, DomainEventKind, ScenarioNode};

/// 把"当前场景引用的线索 + NPC"映射成幂等的 `EntitySurfaced` 事件列表（纯函数）。
///
/// 每个 `referenced_clue_ids` / `referenced_npc_ids` 项产一条事件：
/// - `event_id = de_surfaced_{session_id}_{entity_id}`（幂等 per-session 键）
/// - `data = {"entity_id", "entity_kind": "clue"|"npc", "scene_id"}`
/// - `turn_id` / `session_id` 透传
///
/// 调用方对返回的每条做 `append_domain_event`（on-conflict-do-nothing 保幂等）。
/// 空引用 → 空 Vec（fail-soft 无副作用）。
pub fn surfaced_events_for_scene(
    session_id: &str,
    turn_id: &str,
    scene_id: &str,
    scene: &ScenarioNode,
) -> Vec<DomainEvent> {
    let mut out = Vec::new();
    let mut push = |entity_id: &str, entity_kind: &str| {
        // 空 id 跳过：脏引用不该污染日志（fail-soft）。
        if entity_id.trim().is_empty() {
            return;
        }
        let data = serde_json::json!({
            "entity_id": entity_id,
            "entity_kind": entity_kind,
            "scene_id": scene_id,
        });
        out.push(DomainEvent::new(
            format!("de_surfaced_{session_id}_{entity_id}"),
            session_id,
            turn_id,
            DomainEventKind::EntitySurfaced,
            data,
        ));
    };
    for clue_id in &scene.referenced_clue_ids {
        push(clue_id, "clue");
    }
    for npc_id in &scene.referenced_npc_ids {
        push(npc_id, "npc");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene_with(clues: &[&str], npcs: &[&str]) -> ScenarioNode {
        ScenarioNode {
            node_id: "sc01".into(),
            referenced_clue_ids: clues.iter().map(|s| s.to_string()).collect(),
            referenced_npc_ids: npcs.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn emits_one_event_per_referenced_clue_and_npc() {
        let scene = scene_with(&["clue_letter", "clue_map"], &["npc_ras"]);
        let evs = surfaced_events_for_scene("sess_a", "turn1", "sc01", &scene);
        assert_eq!(evs.len(), 3, "2 clues + 1 npc = 3 events");
        // 全是 EntitySurfaced。
        assert!(evs.iter().all(|e| e.kind == DomainEventKind::EntitySurfaced));
        // session / turn 透传。
        assert!(evs.iter().all(|e| e.session_id == "sess_a" && e.turn_id == "turn1"));
        // entity_kind 区分 clue/npc。
        let clue = evs.iter().find(|e| e.data["entity_id"] == "clue_letter").unwrap();
        assert_eq!(clue.data["entity_kind"], "clue");
        assert_eq!(clue.data["scene_id"], "sc01");
        let npc = evs.iter().find(|e| e.data["entity_id"] == "npc_ras").unwrap();
        assert_eq!(npc.data["entity_kind"], "npc");
    }

    #[test]
    fn event_id_is_idempotent_per_session_entity() {
        // 同 session+entity 跨回合/场景必产同一 event_id（幂等键，on-conflict no-op）。
        let scene = scene_with(&["clue_letter"], &[]);
        let a = surfaced_events_for_scene("sess_a", "turn1", "sc01", &scene);
        let b = surfaced_events_for_scene("sess_a", "turn9", "sc07", &scene);
        assert_eq!(a[0].event_id, "de_surfaced_sess_a_clue_letter");
        assert_eq!(a[0].event_id, b[0].event_id, "幂等键不随 turn/scene 变");
        // 不同 session 必不同键（"玩家这局见过"是 per-session）。
        let c = surfaced_events_for_scene("sess_b", "turn1", "sc01", &scene);
        assert_ne!(a[0].event_id, c[0].event_id);
    }

    #[test]
    fn empty_refs_yield_no_events() {
        let scene = scene_with(&[], &[]);
        assert!(surfaced_events_for_scene("sess_a", "turn1", "sc01", &scene).is_empty());
    }

    #[test]
    fn blank_entity_ids_are_skipped() {
        // 脏引用（空/空白 id）fail-soft 跳过，不污染日志。
        let scene = scene_with(&["", "  "], &["npc_ok"]);
        let evs = surfaced_events_for_scene("sess_a", "turn1", "sc01", &scene);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data["entity_id"], "npc_ok");
    }
}
