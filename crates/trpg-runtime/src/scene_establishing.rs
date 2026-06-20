//! Q-MODULE DP-A' / DP-B' — player-deliverable scene ESTABLISHING material.
//!
//! 架构师 Q4 DP-A' 拍板:当前场景的 **NON-secret** `read_aloud`(进场念白 / 定调 box-text)
//! 在**进场**时即可作为**无门 establishing 散文**浮现 —— 让开场就有模组质感(具名场景 / 氛围),
//! 而不是泛化叙事。它与 A2 的"奖励门控证词"是两回事:
//!   - 本模块只取 `read_aloud`(作者写给玩家听的进场文本) —— **绝不**取 GM-only `gm_notes`,
//!     **绝不**碰 secrets / clues / 隐藏事实(那些仍走 A2 reveal + presentation 闸,本模块不动)。
//!   - 仍做场景级剧透裁剪(`scene.spoiler.redact`)作纵深防御:剧透场景的 read_aloud 会被裁。
//!
//! 输出是 source-anchored 的**素材**:由分体 Narrator 改写成画面(第二人称、织进场景),
//! **绝不**逐字倾倒 / 列清单 / 当选项菜单(尊重 EXAM_QUALITY_BAR Q-4 no-dump;渲染侧文案约束)。
//!
//! # 加性 / 字节级基线
//! 仅 `Enforce` 下调用方 `prepare_turn_context` 填充返回的 `CompiledContext.scene_establishing`;
//! Off/Shadow 不调用本路径 ⇒ 字段为空 ⇒ 分体 Narrator 注入空 ⇒ OFF 字节等价基线。纯函数无副作用。
//!
//! # 零规则集硬编码
//! 全 data-driven(typed 场景 `read_aloud` 字段),绝不按 `ruleset_id`/`module_id` 分支。

use trpg_model::{ModuleBundle, ScenarioNode};

/// 在指定模组的场景图中按 `scene_id` 定位当前场景节点。
fn find_current_scene<'a>(
    modules: &'a [ModuleBundle],
    module_id: &str,
    scene_id: &str,
) -> Option<&'a ScenarioNode> {
    modules
        .iter()
        .find(|m| m.module_id == module_id)
        .and_then(|m| m.module_graph.scenes.iter().find(|s| s.node_id == scene_id))
}

/// 收集本回合玩家可交付的进场 establishing 素材:当前场景的 NON-secret `read_aloud`
/// (剧透裁剪后)。fail-closed:无模组 / 无场景 / 无 read_aloud / 裁剪后为空 ⇒ 空 vec。
///
/// 返回多条切片(当前仅一条 = 场景念白),保持 `Vec<String>` 以便未来加入更多 player-safe
/// 来源(如已激活 NPC 的公开外观),且与 `NarrationPacket::with_scene_establishing` 契约一致。
pub fn collect_scene_establishing(
    modules: &[ModuleBundle],
    module_id: Option<&str>,
    scene_id: Option<&str>,
) -> Vec<String> {
    let (Some(mid), Some(sid)) = (module_id, scene_id) else {
        return Vec::new();
    };
    let Some(scene) = find_current_scene(modules, mid, sid) else {
        return Vec::new();
    };
    let Some(raw) = scene.read_aloud.as_deref() else {
        return Vec::new();
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Vec::new();
    }
    // 场景级剧透词在进入任何下游前裁剪;非剧透场景 ⇒ 逐字透传(read_aloud 本就是写给玩家的)。
    let red = scene.spoiler.redact(raw);
    let red = red.trim();
    if red.is_empty() {
        return Vec::new();
    }
    vec![red.to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use trpg_model::{ModuleBundle, ModuleGraph, ScenarioNode, SpoilerMeta};

    fn scene(node_id: &str, read_aloud: Option<&str>, gm_notes: Option<&str>) -> ScenarioNode {
        ScenarioNode {
            node_id: node_id.to_string(),
            title: "门厅".into(),
            node_type: "scene".into(),
            summary: "S".into(),
            read_aloud: read_aloud.map(str::to_string),
            gm_notes: gm_notes.map(str::to_string),
            ..Default::default()
        }
    }

    fn module(scenes: Vec<ScenarioNode>) -> ModuleBundle {
        ModuleBundle {
            schema_version: String::new(),
            bundle_id: String::new(),
            module_id: "m1".into(),
            ruleset_id: None,
            title: String::new(),
            source_index: Default::default(),
            module_graph: ModuleGraph {
                module_id: "m1".into(),
                scenes,
                ..Default::default()
            },
            module_prep_packets: vec![],
            module_locators: vec![],
            material_index: vec![],
            context_blocks: vec![],
            validation_report: Default::default(),
            conversion_trace: vec![],
        }
    }

    #[test]
    fn read_aloud_surfaced_as_establishing() {
        let m = module(vec![scene(
            "sc1",
            Some("春雨敲打着加油站锈蚀的雨棚，霓虹在水洼里碎成一片猩红。"),
            Some("GM-only：店主其实是线人。"),
        )]);
        let out = collect_scene_establishing(&[m], Some("m1"), Some("sc1"));
        assert_eq!(out.len(), 1);
        assert!(out[0].contains("加油站"));
        // gm_notes (GM-only) must NEVER appear in player-deliverable establishing.
        assert!(!out[0].contains("线人"), "{}", out[0]);
    }

    #[test]
    fn scene_secret_terms_redacted() {
        let mut sc = scene("sc1", Some("门后站着真凶莫里亚蒂教授。"), None);
        sc.spoiler = SpoilerMeta::from_value(&json!({
            "spoiler": {"secret_terms": ["莫里亚蒂教授"]}
        }));
        let m = module(vec![sc]);
        let out = collect_scene_establishing(&[m], Some("m1"), Some("sc1"));
        assert_eq!(out.len(), 1);
        assert!(!out[0].contains("莫里亚蒂教授"), "secret redacted: {}", out[0]);
    }

    #[test]
    fn no_module_no_scene_no_read_aloud_yields_empty() {
        let m = module(vec![scene("sc1", None, Some("仅 GM 备注"))]);
        assert!(collect_scene_establishing(&[m.clone()], Some("m1"), Some("sc1")).is_empty());
        assert!(collect_scene_establishing(&[m.clone()], None, Some("sc1")).is_empty());
        assert!(collect_scene_establishing(&[m.clone()], Some("m1"), None).is_empty());
        assert!(collect_scene_establishing(&[m], Some("other"), Some("sc1")).is_empty());
        assert!(collect_scene_establishing(&[], Some("m1"), Some("sc1")).is_empty());
    }

    #[test]
    fn generic_no_ruleset_or_module_branch() {
        let m1 = module(vec![scene("s", Some("同一念白"), None)]);
        let m2 = module(vec![scene("s", Some("同一念白"), None)]);
        assert_eq!(
            collect_scene_establishing(&[m1], Some("m1"), Some("s")),
            collect_scene_establishing(&[m2], Some("m1"), Some("s"))
        );
    }
}
