//! 源驱动 spoiler 元数据采集（TC-D3-00）：把 `ModuleGraph` 的**权威** `SpoilerMeta`
//! （场景级 + 实体级 `secret_terms`）折成 NoSpoiler v2 的私有 verifier / filter 输入
//! （[`SecretTerm`] / [`PrivateBlockView`]）。
//!
//! 这是 NoSpoiler v2 的**生产数据源**：在此之前 `PluginContext.secret_terms` 恒空、
//! `private_blocks` 只读不存在的 block tag，故防剧透只在合成单测里跑。本模块从
//! `spoiler_guard` 已信任的同一源（模组实体/场景的 `SpoilerMeta`）采集，与之单一事实源对齐。
//!
//! 守理念（与 task card 架构约束一致）：
//! - **只取显式声明的 `secret_terms`**，绝不做正文模糊扫描推断 secret。
//! - 每条 secret 绑定其 `fact_id`（场景级=`node_id`，实体级=entity `id`）；已 player-known
//!   的 fact 由 verifier / filter 侧放行（揭示后可自由复述）。
//! - 私有 secret 文本只活在 verifier 输入里，**绝不**渲染进面向模型的 prompt。

use trpg_model::{ContextBlock, ModuleGraph, SpoilerMeta};

use super::types::{PrivateBlockView, SecretTerm};

/// 从整张 `ModuleGraph` 采集所有声明的 secret_terms，每条绑定其 `fact_id`
/// （场景级=`node_id`，实体级=entity `id`）。
///
/// 采全模组（非仅当前场景）：泄漏校验是安全网——念白若提前点名**任何**未揭示
/// 真相（含未来场景反派）都该被抓。已揭示的由 verifier 侧按 `fact_id` 过滤放行。
/// 空白 term 跳过（防退化成空串全文误命中）。
pub fn harvest_module_secret_terms(graph: &ModuleGraph) -> Vec<SecretTerm> {
    let mut out = Vec::new();
    // 场景级剧透（node.spoiler，fact_id = node_id）。
    for scene in &graph.scenes {
        push_terms(&mut out, &scene.spoiler, Some(&scene.node_id));
    }
    // 实体级剧透（npc JSON 的 nested/flat spoiler，fact_id = entity id）。
    for npc in &graph.npcs {
        let id = npc.get("id").and_then(|v| v.as_str());
        let spoiler = SpoilerMeta::from_value(npc);
        push_terms(&mut out, &spoiler, id);
    }
    out
}

/// 把一组 `SpoilerMeta.secret_terms` 追加为绑定 `fact_id` 的 [`SecretTerm`]（跳空白）。
fn push_terms(out: &mut Vec<SecretTerm>, spoiler: &SpoilerMeta, fact_id: Option<&str>) {
    if spoiler.is_empty() {
        return;
    }
    for term in &spoiler.secret_terms {
        let t = term.trim();
        if t.is_empty() {
            continue;
        }
        out.push(SecretTerm {
            term: t.to_string(),
            fact_id: fact_id.map(str::to_string),
        });
    }
}

/// 当前场景投影块 `block_id` → 其 `node_id`。
///
/// 块 id 形如 `module.<module_id>.scene.<node_id>`，可带 `.skeleton` / `.mechanics` 后缀
/// （见 `trpg_runtime::scene_projection`）。非场景块 → None。
pub fn scene_node_id_from_block(block_id: &str) -> Option<String> {
    // 必须确是 `module.<module_id>.scene.<node_id>`（fail-closed，不瞎切）：
    // 逐段校验前三段，非场景块（如 `module.<mid>.entity.*` / `.npc.*`）一律 None。
    let mut parts = block_id.splitn(4, '.');
    if parts.next()? != "module" {
        return None;
    }
    if parts.next()?.is_empty() {
        return None; // 模组 id 必须非空
    }
    if parts.next()? != "scene" {
        return None; // 第三段必须确是 `scene`
    }
    let mut node = parts.next()?; // `module.<mid>.scene.` 之后即 node_id（可含已知后缀）
    for suffix in [".skeleton", ".mechanics"] {
        if let Some(stripped) = node.strip_suffix(suffix) {
            node = stripped;
        }
    }
    if node.is_empty() {
        return None; // 场景 node id 必须非空
    }
    Some(node.to_string())
}

/// 从源 `SpoilerMeta` 派生场景块的私有视图（fail-closed 过滤用，**不进 prompt**）。
///
/// 把 compiled 场景块映射回其场景节点，读节点级 `SpoilerMeta`。仅当节点显式带 spoiler
/// 才产视图（无 spoiler → None，别太严，不裁可玩内容）。
///
/// `active_scene_id`：当前活动场景。其块由 runtime `spoiler_guard` 负责**逐词裁剪**，
/// 绝不整块删（否则玩家失去当前所在场景），故 `secret=false`；非活动场景的 fact-bound
/// secret 块才标 `secret=true`（未 player-known 时保守整块删）。`fact_id = node_id`：
/// 已揭示则放行。
pub fn derive_scene_block_view(
    block: &ContextBlock,
    graph: &ModuleGraph,
    active_scene_id: Option<&str>,
) -> Option<PrivateBlockView> {
    let node_id = scene_node_id_from_block(&block.block_id)?;
    let node = graph.scenes.iter().find(|s| s.node_id == node_id)?;
    if node.spoiler.is_empty() {
        return None;
    }
    let is_active = active_scene_id == Some(node_id.as_str());
    Some(PrivateBlockView {
        block_id: block.block_id.clone(),
        visibility: block.visibility,
        cache_zone: block.cache_zone,
        tags: block.tags.clone(),
        source_refs: block.source_refs.clone(),
        load_reason: block.load_reason.clone(),
        fact_id: Some(node_id),
        secret: !is_active,
        future_scene: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use trpg_model::{
        BlockContent, BlockKind, CacheZone, ContextBlock, ModuleGraph, ScenarioNode,
        SceneExtractionStatus, Scope, SpoilerMeta, Stability, Visibility,
    };

    fn butler_graph() -> ModuleGraph {
        let mut cellar = ScenarioNode::default();
        cellar.node_id = "sc_cellar".into();
        cellar.title = "地窖".into();
        cellar.extraction_status = SceneExtractionStatus::DeepExtracted;
        cellar.spoiler = SpoilerMeta {
            secret_terms: vec!["传送门".into(), "异界".into()],
            public_aliases: vec![],
            reveal_conditions: vec!["推开石墙后".into()],
        };
        let mut hall = ScenarioNode::default();
        hall.node_id = "sc_hall".into();
        hall.title = "大厅".into();
        hall.extraction_status = SceneExtractionStatus::DeepExtracted;
        let mut g = ModuleGraph::default();
        g.module_id = "mod1".into();
        g.scenes = vec![hall, cellar];
        g.npcs = vec![json!({
            "id": "npc_butler",
            "name": "管家詹姆斯",
            "body": "他其实是连环杀手，真名莫里亚蒂教授。",
            "spoiler": {
                "secret_terms": ["连环杀手", "莫里亚蒂教授"],
                "public_aliases": ["管家詹姆斯"],
                "reveal_conditions": ["在地窖发现尸体后"]
            }
        })];
        g
    }

    /// 采集：场景级 + 实体级 secret_terms 全收，各绑定其 fact_id。
    #[test]
    fn harvest_collects_scene_and_entity_terms_with_fact_ids() {
        let terms = harvest_module_secret_terms(&butler_graph());
        // 场景级两词绑 sc_cellar；实体级两词绑 npc_butler。
        let find = |t: &str| terms.iter().find(|s| s.term == t).cloned();
        assert_eq!(
            find("传送门").unwrap().fact_id.as_deref(),
            Some("sc_cellar")
        );
        assert_eq!(find("异界").unwrap().fact_id.as_deref(), Some("sc_cellar"));
        assert_eq!(
            find("莫里亚蒂教授").unwrap().fact_id.as_deref(),
            Some("npc_butler")
        );
        assert_eq!(
            find("连环杀手").unwrap().fact_id.as_deref(),
            Some("npc_butler")
        );
        assert_eq!(terms.len(), 4, "恰四条声明 secret");
    }

    /// 别太严：无任何 spoiler 的模组 → 空采集（不编造 secret）。
    #[test]
    fn harvest_empty_when_no_spoiler() {
        let mut g = ModuleGraph::default();
        let mut n = ScenarioNode::default();
        n.node_id = "sc".into();
        g.scenes = vec![n];
        g.npcs = vec![json!({"id": "npc1", "name": "店员", "body": "普通人。"})];
        assert!(harvest_module_secret_terms(&g).is_empty());
    }

    /// 空白 term 跳过（防全文误命中）。
    #[test]
    fn harvest_skips_blank_terms() {
        let mut g = ModuleGraph::default();
        let mut n = ScenarioNode::default();
        n.node_id = "sc".into();
        n.spoiler = SpoilerMeta {
            secret_terms: vec!["  ".into(), "真凶".into()],
            ..Default::default()
        };
        g.scenes = vec![n];
        let terms = harvest_module_secret_terms(&g);
        assert_eq!(terms.len(), 1);
        assert_eq!(terms[0].term, "真凶");
    }

    #[test]
    fn block_id_to_node_id_handles_suffixes() {
        assert_eq!(
            scene_node_id_from_block("module.mod1.scene.sc_cellar").as_deref(),
            Some("sc_cellar")
        );
        assert_eq!(
            scene_node_id_from_block("module.mod1.scene.sc_cellar.mechanics").as_deref(),
            Some("sc_cellar")
        );
        assert_eq!(
            scene_node_id_from_block("module.mod1.scene.sc_cellar.skeleton").as_deref(),
            Some("sc_cellar")
        );
        // 非场景块 → None。
        assert_eq!(scene_node_id_from_block("plugin.no_spoiler_guard"), None);
        assert_eq!(scene_node_id_from_block("turn.player_input"), None);
    }

    /// 仅认 `module.<mid>.scene.<node>`：其它 `module.*` 块（实体/npc）及空段一律 None。
    #[test]
    fn block_id_rejects_non_scene_module_ids() {
        // 第三段非 `scene`（实体 / npc 块）→ None。
        assert_eq!(
            scene_node_id_from_block("module.mod1.entity.sc_cellar"),
            None
        );
        assert_eq!(scene_node_id_from_block("module.mod1.npc.sc_cellar"), None);
        // 模组 id 为空 → None。
        assert_eq!(scene_node_id_from_block("module..scene.sc_cellar"), None);
        // 场景 node id 为空 → None。
        assert_eq!(scene_node_id_from_block("module.mod1.scene."), None);
    }

    fn scene_block(block_id: &str) -> ContextBlock {
        ContextBlock::new(
            block_id,
            BlockKind::SceneStatic,
            "场景",
            BlockContent::Text("正文".into()),
            Visibility::GmOnly,
            Stability::SceneStable,
            CacheZone::DynamicTail,
            Scope::global(),
            60,
        )
    }

    /// 派生：非活动 spoiler 场景块 → secret=true，绑 fact_id=node_id。
    #[test]
    fn derive_marks_nonactive_spoiler_scene_secret() {
        let g = butler_graph();
        let block = scene_block("module.mod1.scene.sc_cellar");
        let view =
            derive_scene_block_view(&block, &g, Some("sc_hall")).expect("有 spoiler → 产视图");
        assert!(view.secret, "非活动 spoiler 场景 → secret");
        assert_eq!(view.fact_id.as_deref(), Some("sc_cellar"));
    }

    /// 派生：当前活动 spoiler 场景块 → secret=false（逐词裁剪归 guard_scene，不整块删）。
    #[test]
    fn derive_active_scene_not_secret() {
        let g = butler_graph();
        let block = scene_block("module.mod1.scene.sc_cellar");
        let view =
            derive_scene_block_view(&block, &g, Some("sc_cellar")).expect("有 spoiler → 产视图");
        assert!(
            !view.secret,
            "活动场景块不得整块删（guard_scene 负责逐词裁剪）"
        );
        assert_eq!(view.fact_id.as_deref(), Some("sc_cellar"));
    }

    /// 派生：无 spoiler 的场景块 → None（别太严）。
    #[test]
    fn derive_none_for_plain_scene() {
        let g = butler_graph();
        let block = scene_block("module.mod1.scene.sc_hall");
        assert!(derive_scene_block_view(&block, &g, Some("sc_cellar")).is_none());
    }
}
