//! Axis-1 NPC 激活派生（DP-2 "hybrid"，MAT.M1）。
//!
//! 每回合 context 装配时，从当前场景的 `referenced_npc_ids` 派生
//! `RuntimeState.active_npc_ids`，由 [`MaterializationAffordanceMode`] 门控。
//!
//! # 严格无操作保证
//!
//! 派生 active_npc_ids 会进入 `world_state_block` 装配进上下文（影响 World 反应、
//! 念白）——这是**玩家可见效果**。按 flag 基座契约（`is_enforce()` 才产生玩家可见
//! 效果），仅 [`MaterializationAffordanceMode::Enforce`] 时应用；`Off` 与 `Shadow`
//! 均**不触碰** `active_npc_ids`——调用方供给的任何值（含空 Vec）原样保留，字节一致。
//! （`Shadow` 目前无独立 audit sink，故对 M1 等同非应用；未来可经 tracing 记派生意图。）
//!
//! # 激活策略（Enforce）
//!
//! 仅当 `active_npc_ids` 为**空**时派生（additive，最低意外性）：
//! 若调用方已填写非空集合，则保留其值不覆盖。
//!
//! 派生结果：当前场景 `referenced_npc_ids` 去重、保持文档顺序（stable-dedup）、
//! fail-closed（无模组 / 无场景 / 无引用 NPC → 空 Vec，不报错）。

use trpg_model::{MaterializationAffordanceMode, ModuleBundle, ScenarioNode};

/// 从模组包列表中定位当前场景节点。
///
/// 以 `module_id` 过滤模组，再在其 `module_graph.scenes` 中按 `scene_id` 查找。
/// fail-closed：任一条件缺失 → `None`。
fn find_current_scene<'a>(
    modules: &'a [ModuleBundle],
    module_id: Option<&str>,
    scene_id: Option<&str>,
) -> Option<&'a ScenarioNode> {
    let mid = module_id?;
    let sid = scene_id?;
    modules
        .iter()
        .find(|m| m.module_id == mid)
        .and_then(|m| m.module_graph.scenes.iter().find(|s| s.node_id == sid))
}

/// 对 `referenced_npc_ids` 做 stable-dedup，保持首次出现顺序。
fn stable_dedup(ids: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    ids.iter()
        .filter(|id| seen.insert(id.as_str()))
        .cloned()
        .collect()
}

/// 按照 [`MaterializationAffordanceMode`] 门控，计算并写回 `active_npc_ids`。
///
/// - `Off` / `Shadow`：不应用（无玩家可见效果），`active_npc_ids` 不被修改。
/// - `Enforce`：仅当 `active_npc_ids` 为空时，从当前场景的 `referenced_npc_ids`
///   派生（stable-dedup，fail-closed）。
pub fn apply_npc_activation(
    active_npc_ids: &mut Vec<String>,
    modules: &[ModuleBundle],
    module_id: Option<&str>,
    scene_id: Option<&str>,
    mode: MaterializationAffordanceMode,
) {
    // 玩家可见效果（改变装配上下文）→ 仅 Enforce 应用。
    // Off = 严格无操作（s17 修正：不得替换调用方供给的任何值）；Shadow 同样非应用。
    if !mode.is_enforce() {
        return;
    }
    // 仅在调用方集合为空时派生（additive，不覆盖已有非空集）
    if !active_npc_ids.is_empty() {
        return;
    }
    let derived = find_current_scene(modules, module_id, scene_id)
        .map(|node| stable_dedup(&node.referenced_npc_ids))
        .unwrap_or_default();
    *active_npc_ids = derived;
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::{ModuleBundle, ModuleGraph, ScenarioNode};

    fn make_modules(module_id: &str, scene_id: &str, npc_ids: Vec<&str>) -> Vec<ModuleBundle> {
        let mut scene = ScenarioNode::default();
        scene.node_id = scene_id.to_string();
        scene.referenced_npc_ids = npc_ids.into_iter().map(str::to_string).collect();

        let mut graph = ModuleGraph::default();
        graph.module_id = module_id.to_string();
        graph.scenes = vec![scene];

        let bundle = ModuleBundle {
            schema_version: String::new(),
            bundle_id: String::new(),
            module_id: module_id.to_string(),
            ruleset_id: None,
            title: String::new(),
            source_index: Default::default(),
            module_graph: graph,
            module_prep_packets: vec![],
            module_locators: vec![],
            material_index: vec![],
            context_blocks: vec![],
            validation_report: Default::default(),
            conversion_trace: vec![],
        };
        vec![bundle]
    }

    // ===== 1. 纯投影：referenced_npc_ids → active_npc_ids（Enforce）=====

    #[test]
    fn projection_stable_order_and_dedup() {
        let modules = make_modules("m1", "sc01", vec!["npc_a", "npc_b", "npc_a", "npc_c"]);
        let mut ids = Vec::new();
        apply_npc_activation(
            &mut ids,
            &modules,
            Some("m1"),
            Some("sc01"),
            MaterializationAffordanceMode::Enforce,
        );
        // npc_a 去重保首现顺序
        assert_eq!(ids, vec!["npc_a", "npc_b", "npc_c"]);
    }

    #[test]
    fn projection_empty_when_no_scene_refs() {
        let modules = make_modules("m1", "sc01", vec![]);
        let mut ids = Vec::new();
        apply_npc_activation(
            &mut ids,
            &modules,
            Some("m1"),
            Some("sc01"),
            MaterializationAffordanceMode::Enforce,
        );
        assert!(ids.is_empty(), "空引用 NPC 应派生空集");
    }

    #[test]
    fn projection_fail_closed_no_module() {
        let mut ids = Vec::new();
        apply_npc_activation(
            &mut ids,
            &[],
            None, // 无 module_id
            Some("sc01"),
            MaterializationAffordanceMode::Enforce,
        );
        assert!(ids.is_empty(), "无模组 → fail-closed 空");
    }

    #[test]
    fn projection_fail_closed_no_scene_id() {
        let modules = make_modules("m1", "sc01", vec!["npc_a"]);
        let mut ids = Vec::new();
        apply_npc_activation(
            &mut ids,
            &modules,
            Some("m1"),
            None, // 无 scene_id
            MaterializationAffordanceMode::Enforce,
        );
        assert!(ids.is_empty(), "无 scene_id → fail-closed 空");
    }

    #[test]
    fn projection_fail_closed_scene_not_found() {
        let modules = make_modules("m1", "sc01", vec!["npc_a"]);
        let mut ids = Vec::new();
        apply_npc_activation(
            &mut ids,
            &modules,
            Some("m1"),
            Some("sc_missing"), // 不存在的场景
            MaterializationAffordanceMode::Enforce,
        );
        assert!(ids.is_empty(), "场景不在图谱 → fail-closed 空");
    }

    // ===== 2. OFF / Shadow 非应用守卫(无玩家可见效果)=====

    #[test]
    fn off_preserves_nonempty_caller_set() {
        // s17 修正核心：Off 时调用方非空集合不得被清空或替换
        let modules = make_modules("m1", "sc01", vec!["npc_x", "npc_y"]);
        let mut ids = vec!["caller_npc_1".to_string(), "caller_npc_2".to_string()];
        let original = ids.clone();
        apply_npc_activation(
            &mut ids,
            &modules,
            Some("m1"),
            Some("sc01"),
            MaterializationAffordanceMode::Off,
        );
        assert_eq!(ids, original, "Off: 调用方非空集合必须原样保留");
    }

    #[test]
    fn off_preserves_empty_caller_set() {
        // Off 时调用方空集也不得被填充（严格无操作）
        let modules = make_modules("m1", "sc01", vec!["npc_x"]);
        let mut ids: Vec<String> = Vec::new();
        apply_npc_activation(
            &mut ids,
            &modules,
            Some("m1"),
            Some("sc01"),
            MaterializationAffordanceMode::Off,
        );
        assert!(ids.is_empty(), "Off: 调用方空集必须保持为空（不得派生）");
    }

    #[test]
    fn shadow_does_not_apply_no_player_visible_effect() {
        // Shadow 无独立 audit sink，对 M1 等同非应用：不得改变装配进上下文的 active 集。
        let modules = make_modules("m1", "sc01", vec!["npc_x"]);
        let mut ids: Vec<String> = Vec::new();
        apply_npc_activation(
            &mut ids,
            &modules,
            Some("m1"),
            Some("sc01"),
            MaterializationAffordanceMode::Shadow,
        );
        assert!(ids.is_empty(), "Shadow: 不应用派生(无玩家可见效果)");
    }

    // ===== 3. ON 派生测试（Enforce + 场景有引用 NPC + 调用方空集）=====

    #[test]
    fn enforce_derives_from_scene_when_caller_empty() {
        let modules = make_modules("m1", "sc01", vec!["npc_russell", "npc_ann"]);
        let mut ids: Vec<String> = Vec::new();
        apply_npc_activation(
            &mut ids,
            &modules,
            Some("m1"),
            Some("sc01"),
            MaterializationAffordanceMode::Enforce,
        );
        assert_eq!(ids, vec!["npc_russell", "npc_ann"]);
    }

    #[test]
    fn active_does_not_override_nonempty_caller_set() {
        // additive 策略：调用方已供给非空集 → 保留，不用场景派生替换
        let modules = make_modules("m1", "sc01", vec!["npc_from_scene"]);
        let mut ids = vec!["caller_npc".to_string()];
        apply_npc_activation(
            &mut ids,
            &modules,
            Some("m1"),
            Some("sc01"),
            MaterializationAffordanceMode::Enforce,
        );
        assert_eq!(
            ids,
            vec!["caller_npc"],
            "非空调用方集合在 Enforce 下也不被覆盖（additive）"
        );
    }
}
