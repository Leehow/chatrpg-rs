//! 反剧透 TruthGraph 起步切片（优化2 #5）——**观测层**。
//!
//! 本切片只做一件事：当模组实体（线索/NPC）随**当前场景**投影进 GM 回合
//! context，就记一条 `ContextSurfaced` 领域事件——"实体进了 GM/runtime context"。
//! P0c 关键修正：这是**隐藏的内部装载**，**不**代表玩家见过该实体（玩家暴露用
//! `PlayerExposed`/遗留 `EntitySurfaced`）。与影子 binding 同立场：**零行为变更、
//! 纯数据采集**，绝不做剧透裁剪/强制（强制留后续切片）。
//!
//! 设计要点：
//! - **幂等 per-session**：`event_id = de_surfaced_{session}_{entity}`——每个实体
//!   全会话只记一次（再 surface 经 `on conflict do nothing` 化为 no-op）。键文本保持
//!   `de_surfaced_`（与关系抽取 `source_event_ids` 约定一致，跨切片不破坏）；data 里
//!   仍带首次记录的 scene_id 供溯源。
//! - **纯函数**：`context_surfaced_events_for_scene` 只吃 `&ScenarioNode` 产
//!   `Vec<DomainEvent>`，DB-free 可单测；append 由 runtime 调用方做（fail-soft）。
//! - **零规则集硬编码**：entity_kind 只用通用 "clue"/"npc"，不按规则集/模组名分支。

use std::collections::HashSet;

use serde_json::Value;
use trpg_model::{DomainEvent, DomainEventKind, ModuleGraph, ScenarioNode};

/// 把"当前场景引用的线索 + NPC"映射成幂等的 `ContextSurfaced` 事件列表（纯函数）。
///
/// 每个 `referenced_clue_ids` / `referenced_npc_ids` 项产一条事件：
/// - `event_id = de_surfaced_{session_id}_{entity_id}`（幂等 per-session 键，文本保持
///   `de_surfaced_` 以兼容关系抽取 source_event_id 约定）
/// - `kind = ContextSurfaced`（进 GM/context，**非**玩家暴露）
/// - `data = {"entity_id", "entity_kind": "clue"|"npc", "scene_id"}`
/// - `turn_id` / `session_id` 透传
///
/// 调用方对返回的每条做 `append_domain_event`（on-conflict-do-nothing 保幂等）。
/// 空引用 → 空 Vec（fail-soft 无副作用）。
pub fn context_surfaced_events_for_scene(
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
            DomainEventKind::ContextSurfaced,
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

/// 把"玩家可见叙事里实际出现的当前场景实体"映射成幂等的
/// `PlayerExposed` 事件列表（纯函数）。
///
/// 只在当前场景 `referenced_clue_ids` / `referenced_npc_ids` 的范围内匹配实体名字，
/// 避免全模组扫名把未来场景内容误记为玩家已见。输出中出现实体全名或常见短名
/// （如 `拉斯·威廉姆斯` 的 `拉斯`）才算暴露。
pub fn player_exposed_events_for_scene_narration(
    session_id: &str,
    turn_id: &str,
    scene_id: &str,
    scene: &ScenarioNode,
    graph: &ModuleGraph,
    narration: &str,
) -> Vec<DomainEvent> {
    let text = narration.to_lowercase();
    if text.trim().is_empty() {
        return Vec::new();
    }

    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (entity_id, entity_kind) in scene
        .referenced_clue_ids
        .iter()
        .map(|id| (id.as_str(), "clue"))
        .chain(
            scene
                .referenced_npc_ids
                .iter()
                .map(|id| (id.as_str(), "npc")),
        )
    {
        if entity_id.trim().is_empty() || !seen.insert(entity_id.to_string()) {
            continue;
        }
        let Some(entity) = find_entity(graph, entity_kind, entity_id) else {
            continue;
        };
        if !entity_aliases(entity)
            .iter()
            .any(|alias| text.contains(&alias.to_lowercase()))
        {
            continue;
        }
        let data = serde_json::json!({
            "entity_id": entity_id,
            "entity_kind": entity_kind,
            "scene_id": scene_id,
            "reason": "entity name appeared in player-visible narration for the current scene",
        });
        out.push(DomainEvent::new(
            format!("de_exposed_{session_id}_{entity_id}"),
            session_id,
            turn_id,
            DomainEventKind::PlayerExposed,
            data,
        ));
    }
    out
}

fn find_entity<'a>(
    graph: &'a ModuleGraph,
    entity_kind: &str,
    entity_id: &str,
) -> Option<&'a Value> {
    let items = match entity_kind {
        "clue" => &graph.clues,
        "npc" => &graph.npcs,
        _ => return None,
    };
    items
        .iter()
        .find(|value| value.get("id").and_then(Value::as_str) == Some(entity_id))
}

fn entity_aliases(entity: &Value) -> Vec<String> {
    let mut aliases = Vec::new();
    for key in ["name", "title", "label", "display_name"] {
        if let Some(text) = entity.get(key).and_then(Value::as_str) {
            push_alias(&mut aliases, text);
            for part in text.split(|c: char| matches!(c, '·' | ' ' | '-' | '_' | '.')) {
                push_alias(&mut aliases, part);
            }
        }
    }
    aliases
}

fn push_alias(aliases: &mut Vec<String>, alias: &str) {
    let alias = alias.trim();
    if alias.is_empty() {
        return;
    }
    let char_count = alias.chars().count();
    if char_count < 2 {
        return;
    }
    if alias.is_ascii() && alias.len() < 3 {
        return;
    }
    if !aliases.iter().any(|existing| existing == alias) {
        aliases.push(alias.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
        let evs = context_surfaced_events_for_scene("sess_a", "turn1", "sc01", &scene);
        assert_eq!(evs.len(), 3, "2 clues + 1 npc = 3 events");
        // P0c：场景把线索/NPC 装进 GM context 是 ContextSurfaced（隐藏内部装载），
        // **绝非** 玩家暴露 EntitySurfaced/PlayerExposed。
        assert!(evs
            .iter()
            .all(|e| e.kind == DomainEventKind::ContextSurfaced));
        assert!(
            evs.iter().all(|e| e.kind != DomainEventKind::EntitySurfaced
                && e.kind != DomainEventKind::PlayerExposed),
            "scene context loading 不得伪装成玩家暴露"
        );
        // session / turn 透传。
        assert!(evs
            .iter()
            .all(|e| e.session_id == "sess_a" && e.turn_id == "turn1"));
        // entity_kind 区分 clue/npc。
        let clue = evs
            .iter()
            .find(|e| e.data["entity_id"] == "clue_letter")
            .unwrap();
        assert_eq!(clue.data["entity_kind"], "clue");
        assert_eq!(clue.data["scene_id"], "sc01");
        let npc = evs
            .iter()
            .find(|e| e.data["entity_id"] == "npc_ras")
            .unwrap();
        assert_eq!(npc.data["entity_kind"], "npc");
    }

    #[test]
    fn event_id_is_idempotent_per_session_entity() {
        // 同 session+entity 跨回合/场景必产同一 event_id（幂等键，on-conflict no-op）。
        let scene = scene_with(&["clue_letter"], &[]);
        let a = context_surfaced_events_for_scene("sess_a", "turn1", "sc01", &scene);
        let b = context_surfaced_events_for_scene("sess_a", "turn9", "sc07", &scene);
        assert_eq!(a[0].event_id, "de_surfaced_sess_a_clue_letter");
        assert_eq!(a[0].event_id, b[0].event_id, "幂等键不随 turn/scene 变");
        // 不同 session 必不同键（"玩家这局见过"是 per-session）。
        let c = context_surfaced_events_for_scene("sess_b", "turn1", "sc01", &scene);
        assert_ne!(a[0].event_id, c[0].event_id);
    }

    #[test]
    fn empty_refs_yield_no_events() {
        let scene = scene_with(&[], &[]);
        assert!(context_surfaced_events_for_scene("sess_a", "turn1", "sc01", &scene).is_empty());
    }

    #[test]
    fn blank_entity_ids_are_skipped() {
        // 脏引用（空/空白 id）fail-soft 跳过，不污染日志。
        let scene = scene_with(&["", "  "], &["npc_ok"]);
        let evs = context_surfaced_events_for_scene("sess_a", "turn1", "sc01", &scene);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data["entity_id"], "npc_ok");
    }

    #[test]
    fn player_exposed_matches_current_scene_entity_short_name() {
        let scene = scene_with(&[], &["npc_russ_williams", "npc_future"]);
        let graph = ModuleGraph {
            npcs: vec![
                json!({"id":"npc_russ_williams","name":"拉斯·威廉姆斯"}),
                json!({"id":"npc_future","name":"未来真凶"}),
            ],
            ..Default::default()
        };

        let evs = player_exposed_events_for_scene_narration(
            "sess_a",
            "turn1",
            "sc01",
            &scene,
            &graph,
            "拉斯把零钱推回柜台边缘。",
        );

        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].kind, DomainEventKind::PlayerExposed);
        assert_eq!(evs[0].event_id, "de_exposed_sess_a_npc_russ_williams");
        assert_eq!(evs[0].data["entity_kind"], "npc");
    }

    #[test]
    fn player_exposed_does_not_scan_future_entities_outside_scene_refs() {
        let scene = scene_with(&[], &["npc_russ_williams"]);
        let graph = ModuleGraph {
            npcs: vec![
                json!({"id":"npc_russ_williams","name":"拉斯·威廉姆斯"}),
                json!({"id":"npc_future","name":"未来真凶"}),
            ],
            ..Default::default()
        };

        let evs = player_exposed_events_for_scene_narration(
            "sess_a",
            "turn1",
            "sc01",
            &scene,
            &graph,
            "未来真凶这个名字不应因为不在当前场景引用里而被记作暴露。",
        );

        assert!(evs.is_empty(), "{evs:?}");
    }
}
