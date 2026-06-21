//! R2 SceneNeedResolver: the current-scene deep projection behind the Need bus.
//!
//! The resolver reproduces the original scene-block logic (module_id validation →
//! project membership check → DB load_module_graph → pick_scene_node →
//! scene_node_to_blocks). R2 收口后这是唯一的当前场景投影路径——旧直连
//! `module_scene_blocks_for_turn` 已删，prepare_turn_context 只经此 resolver。

use std::collections::HashSet;

use async_trait::async_trait;
use trpg_db::Db;
use trpg_model::{ContextBlock, ScenarioNode, SceneExtractionStatus};
use trpg_need::{Need, NeedKind, NeedOutcome, NeedResolver, SceneNeed};

use crate::scene_projection::{scene_node_to_blocks, scene_node_to_blocks_with_opts};
use crate::spoiler_guard::guard_scene;

/// 纯函数包装：给定已选中的 node + graph.npcs + graph.scenes，
/// 产出与 scene_node_to_blocks 字节等价的块。单独提取便于测试隔离。
/// `include_read_aloud=false`(L-G 开场已交付) ⇒ 跳过定场文正文复投,仅留锚提示。
pub(crate) fn resolve_scene_blocks(
    module_id: &str,
    node: &ScenarioNode,
    npcs: &[serde_json::Value],
    scenes: &[ScenarioNode],
    include_read_aloud: bool,
) -> Vec<ContextBlock> {
    let blocks =
        scene_node_to_blocks_with_opts(module_id, node, npcs, scenes, include_read_aloud);
    if !blocks.is_empty() {
        tracing::info!(
            target: "module_scene",
            module_id,
            scene = %node.title,
            "SceneNeedResolver: projected current module scene into turn context"
        );
    }
    blocks
}

/// 从 ModuleGraph 选出当前场景节点：
/// 显式 scene_id 优先，缺失/不匹配则回退首个 DeepExtracted（入口场景）。
/// fail-closed：无匹配 → None。与 `module_scene_blocks_for_turn` 逻辑完全相同。
pub(crate) fn pick_scene_node<'g>(
    graph: &'g trpg_model::ModuleGraph,
    scene_id: Option<&str>,
) -> Option<&'g ScenarioNode> {
    scene_id
        .and_then(|sid| graph.scenes.iter().find(|s| s.node_id == sid))
        .or_else(|| {
            graph
                .scenes
                .iter()
                .find(|s| s.extraction_status == SceneExtractionStatus::DeepExtracted)
        })
}

/// SceneNeedResolver: 包 module_scene_blocks_for_turn 的异步 DB 部分。
/// 薄 adapter，零检索逻辑：DB 取图 → pick_scene_node → resolve_scene_blocks。
pub(crate) struct SceneNeedResolver {
    pub db: Db,
}

#[async_trait]
impl NeedResolver for SceneNeedResolver {
    fn kind(&self) -> NeedKind {
        NeedKind::Scene
    }

    async fn resolve(&self, need: &Need) -> anyhow::Result<NeedOutcome> {
        let scene_need: &SceneNeed = match need {
            Need::Scene(s) => s,
            _ => return Ok(NeedOutcome::default()),
        };

        let module_id = match scene_need.scopes.module_id.as_deref() {
            Some(id) if !id.trim().is_empty() => id,
            _ => return Ok(NeedOutcome::default()), // fail-closed: 无 module_id
        };

        // 复用 module_scene_blocks_for_turn 的 project 成员校验：
        // 仅当 module 隶属本 project 时才继续（防跨 project 泄漏）。
        let project_ok = scene_need
            .project_module_ids
            .iter()
            .any(|id| id == module_id);
        if !project_ok {
            tracing::warn!(
                module_id,
                "SceneNeedResolver: module not in project, skipping"
            );
            return Ok(NeedOutcome::default());
        }

        let Some(graph) = self.db.load_module_graph(module_id).await? else {
            return Ok(NeedOutcome::default()); // fail-closed: 图谱缺失
        };

        let Some(node) = pick_scene_node(&graph, scene_need.scopes.scene_id.as_deref()) else {
            return Ok(NeedOutcome::default()); // fail-closed: 无可用场景
        };

        // 反剧透 ENFORCEMENT：投影前对未揭示的场景/实体剧透做裁剪。revealed = 本会话已
        // 揭示 fact_id 集，P0b 起统一经 KnowledgeProjection 读 KnowledgeEdge 账本；DB 取不到
        //（抖动）→ 空集 = 全部按未揭示裁剪（fail-closed 宁可不泄，不赌 DB）。没标 spoiler 或
        // 已揭示的内容由 guard_scene 原样透传（别太严，不裁可玩内容）。
        let revealed: HashSet<String> = crate::knowledge_projection::player_knowledge_projection(
            &self.db,
            &scene_need.scopes.session_id,
        )
        .await
        .map(|p| p.revealed_fact_ids)
        .unwrap_or_default();
        let (guarded_node, guarded_npcs) = guard_scene(node, &graph.npcs, &revealed);
        // L-G 失忆锚:开场已交付 ⇒ 跳过 read_aloud 正文复投(仅锚提示),其余场景参考照常每回合在。
        let include_read_aloud = !scene_need.read_aloud_already_delivered;
        let blocks = resolve_scene_blocks(
            module_id,
            &guarded_node,
            &guarded_npcs,
            &graph.scenes,
            include_read_aloud,
        );
        // A1 (G-1): emit a real SourceRef for the bound scene node so the need-binding trace
        // shows genuine grounding (page/anchor) instead of an empty `Partial`/0-source_refs
        // signal. Pure observability (source_refs只进 NeedResolutionTrace),不改 blocks/叙事。
        let source_refs = scene_source_refs(module_id, node);
        Ok(NeedOutcome {
            blocks,
            source_refs,
        })
    }
}

/// 从已选中的场景节点构造来源引用(grounding):module 作 source_id、page_start 作页码、
/// node_id 作 anchor、title 作 section_path。空字段省略,纯函数。
pub(crate) fn scene_source_refs(
    module_id: &str,
    node: &ScenarioNode,
) -> Vec<trpg_model::SourceRef> {
    let section_path = if node.title.trim().is_empty() {
        vec![]
    } else {
        vec![node.title.clone()]
    };
    vec![trpg_model::SourceRef {
        source_id: module_id.to_string(),
        page: node.page_start,
        anchor_id: Some(node.node_id.clone()),
        section_path,
        char_start: None,
        char_end: None,
        text_hash: None,
        note: Some("scene_need".to_string()),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::{LinkType, ModuleGraph, ScenarioLink, ScenarioNode, SceneExtractionStatus};

    /// 核心等价断言：resolver 内部调用的纯函数路径与直调 scene_node_to_blocks 字节相同。
    /// resolver 是"包同一逻辑"非新算法，此测试是函数级回归护栏。
    #[test]
    fn scene_resolver_pure_path_byte_equal_to_scene_node_to_blocks() {
        // 这两次调用都读进程级 env TRPG_SCENE_DEEP_BLOCK_CACHE_ZONE；与 scene_projection
        // 的 env 写测试共用同一把锁，保证 expected/actual 两次读到的 env 一致（防并行竞争）。
        let _env_guard = crate::scene_projection::N3_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut n = ScenarioNode::default();
        n.node_id = "loc1".into();
        n.title = "加油站".into();
        n.read_aloud = Some("你们看到一个褪色的广告牌……".into());
        n.gm_notes = Some("老板拉斯藏着钥匙。".into());
        n.extraction_status = SceneExtractionStatus::DeepExtracted;
        n.referenced_npc_ids = vec!["npc1".into()];
        let npcs = vec![serde_json::json!({"id":"npc1","name":"拉斯","summary":"老板"})];
        n.links = vec![ScenarioLink {
            to_node_id: "loc2".into(),
            reason: "主路通往".into(),
            clue_id: None,
            link_type: LinkType::Spatial,
            source_anchor: None,
        }];
        let mut loc2 = ScenarioNode::default();
        loc2.node_id = "loc2".into();
        loc2.title = "镇中心".into();
        let scenes = vec![n.clone(), loc2];

        // 旧直调路径
        let expected = scene_node_to_blocks("mod1", &n, &npcs, &scenes);
        // resolver 内纯函数路径（等价调用：include_read_aloud=true ⇒ 与旧直调字节等价）
        let actual = resolve_scene_blocks("mod1", &n, &npcs, &scenes, true);

        let e_bytes: Vec<_> = expected
            .iter()
            .map(|b| serde_json::to_vec(b).unwrap())
            .collect();
        let a_bytes: Vec<_> = actual
            .iter()
            .map(|b| serde_json::to_vec(b).unwrap())
            .collect();
        assert_eq!(e_bytes.len(), a_bytes.len(), "块数必须相同");
        for (i, (e, a)) in e_bytes.iter().zip(a_bytes.iter()).enumerate() {
            assert_eq!(e, a, "block[{i}] 必须字节等价");
        }
    }

    #[test]
    fn scene_resolver_skeleton_byte_equal() {
        // 同上：env 稳定性靠共享锁保证，避免与 scene_projection env 写测试并发竞争。
        let _env_guard = crate::scene_projection::N3_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut n = ScenarioNode::default();
        n.node_id = "sk1".into();
        n.title = "未深抽场景".into();
        n.extraction_status = SceneExtractionStatus::SkeletonOnly;
        let expected = scene_node_to_blocks("mod1", &n, &[], &[]);
        let actual = resolve_scene_blocks("mod1", &n, &[], &[], true);
        let e_bytes: Vec<_> = expected
            .iter()
            .map(|b| serde_json::to_vec(b).unwrap())
            .collect();
        let a_bytes: Vec<_> = actual
            .iter()
            .map(|b| serde_json::to_vec(b).unwrap())
            .collect();
        assert_eq!(e_bytes, a_bytes, "SkeletonOnly 降级块字节必须等价");
    }

    #[test]
    fn pick_scene_prefers_explicit_scene_id_over_deep_fallback() {
        let mk = |id: &str, st: SceneExtractionStatus| {
            let mut n = ScenarioNode::default();
            n.node_id = id.into();
            n.title = id.into();
            n.extraction_status = st;
            n
        };
        let mut g = ModuleGraph::default();
        g.scenes = vec![
            mk("entry", SceneExtractionStatus::DeepExtracted),
            mk("mid", SceneExtractionStatus::DeepExtracted),
        ];
        // 显式 scene_id 优先
        let picked = pick_scene_node(&g, Some("mid"));
        assert_eq!(picked.map(|n| n.node_id.as_str()), Some("mid"));
        // 无匹配 explicit → 回退首个 DeepExtracted
        let fallback = pick_scene_node(&g, Some("nonexistent"));
        assert_eq!(fallback.map(|n| n.node_id.as_str()), Some("entry"));
        // 无 scene_id → 回退首个 DeepExtracted
        let no_sid = pick_scene_node(&g, None);
        assert_eq!(no_sid.map(|n| n.node_id.as_str()), Some("entry"));
    }

    #[test]
    fn scene_source_refs_grounds_node_page_and_anchor() {
        let mut n = ScenarioNode::default();
        n.node_id = "scene_017_welcome_to_abattoir".into();
        n.title = "欢迎来到阿巴托尔".into();
        n.page_start = Some(42);
        let refs = scene_source_refs("call_of_cthulhu_7e.document", &n);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].source_id, "call_of_cthulhu_7e.document");
        assert_eq!(refs[0].page, Some(42));
        assert_eq!(refs[0].anchor_id.as_deref(), Some("scene_017_welcome_to_abattoir"));
        assert_eq!(refs[0].section_path, vec!["欢迎来到阿巴托尔".to_string()]);
    }

    #[test]
    fn pick_scene_fails_closed_when_no_deep_extracted() {
        let mut g = ModuleGraph::default();
        let mut n = ScenarioNode::default();
        n.node_id = "sk1".into();
        n.extraction_status = SceneExtractionStatus::SkeletonOnly;
        g.scenes = vec![n];
        let result = pick_scene_node(&g, None);
        assert!(
            result.is_none(),
            "无 DeepExtracted 场景应 fail-closed 返回 None"
        );
    }
}
