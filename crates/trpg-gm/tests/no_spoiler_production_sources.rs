//! TC-D3-00 验收：NoSpoiler v2 在**生产源**（模组图谱 SpoilerMeta）上运行，而非合成空占位。
//!
//! 这些测试走真实生产函数链：
//! - `harvest_module_secret_terms`（AfterLlmStream 私有泄漏术语源）；
//! - `derive_scene_block_view`（ContextAssembly 私有 block 过滤源）；
//! 把其产物喂进真实 `NoSpoilerGuard.on_hook`，断言：未 player-known → 删/报，已 player-known
//! → 放行，且 verifier detail 绝不回写 secret 正文。
//!
//! 图谱在测试内直接构造（生产由 `Db::load_module_graph` 载入，turn_loop 已 fail-soft 接线）；
//! block_id 用生产投影格式 `module.<mid>.scene.<node_id>`（见 scene_projection），故为
//! "production-like context block"。

use serde_json::json;
use trpg_gm::plugin::{
    builtin_no_spoiler::NO_SPOILER_GUARD_ID, derive_scene_block_view, harvest_module_secret_terms,
    PluginContext, PluginContributionKind, PluginHook, RuntimePlugin,
};
use trpg_gm::NoSpoilerGuard;
use trpg_model::{
    BlockContent, BlockKind, CacheZone, ContextBlock, ModuleGraph, ScenarioNode,
    SceneExtractionStatus, Scope, SpoilerMeta, Stability, Visibility,
};

/// 含场景级（地窖 sc_cellar）与实体级（管家 npc_butler）剧透的生产形态模组图谱。
fn butler_graph() -> ModuleGraph {
    let mut hall = ScenarioNode::default();
    hall.node_id = "sc_hall".into();
    hall.title = "大厅".into();
    hall.extraction_status = SceneExtractionStatus::DeepExtracted;

    let mut cellar = ScenarioNode::default();
    cellar.node_id = "sc_cellar".into();
    cellar.title = "地窖".into();
    cellar.extraction_status = SceneExtractionStatus::DeepExtracted;
    cellar.spoiler = SpoilerMeta {
        secret_terms: vec!["传送门".into(), "异界".into()],
        public_aliases: vec![],
        reveal_conditions: vec!["推开石墙后".into()],
    };

    let mut g = ModuleGraph::default();
    g.module_id = "mod_manor".into();
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

/// 生产投影格式的场景块（与 scene_node_to_blocks 同 block_id 约定）。
fn scene_block(module_id: &str, node_id: &str) -> ContextBlock {
    ContextBlock::new(
        format!("module.{module_id}.scene.{node_id}"),
        BlockKind::SceneStatic,
        node_id,
        BlockContent::Text("场景正文".into()),
        Visibility::GmOnly,
        Stability::SceneStable,
        CacheZone::DynamicTail,
        Scope::global(),
        60,
    )
}

fn drop_ids(out: &[trpg_gm::plugin::PluginContribution]) -> Vec<String> {
    out.iter()
        .filter_map(|c| match &c.kind {
            PluginContributionKind::ContextFilter(f) => Some(f.drop_block_ids.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

/// 验收 #1：用**生产 fact 元数据**（模组图谱 SpoilerMeta 派生的私有 block 视图）→
/// fact 未 player-known 时，该 spoiler 场景块在装配前被 ContextFilter 删除。
#[tokio::test]
async fn no_spoiler_context_filter_uses_production_fact_metadata() {
    let graph = butler_graph();
    // 当前活动场景=大厅；地窖（sc_cellar）是非活动 spoiler 场景 → 该块应被整块删。
    let cellar_block = scene_block("mod_manor", "sc_cellar");
    let view = derive_scene_block_view(&cellar_block, &graph, Some("sc_hall"))
        .expect("地窖含场景级 SpoilerMeta → 应派生私有视图");
    assert!(view.secret, "源派生：非活动 spoiler 场景应标 secret");
    assert_eq!(
        view.fact_id.as_deref(),
        Some("sc_cellar"),
        "fact_id 绑场景 node_id"
    );

    let mut ctx = PluginContext {
        module_id: Some("mod_manor".into()),
        hook: PluginHook::ContextAssembly,
        ..Default::default()
    };
    ctx.private_blocks = vec![view];
    ctx.player_known_fact_ids = vec![]; // 玩家尚未发现地窖真相。

    let out = NoSpoilerGuard.on_hook(&ctx).await;
    assert_eq!(
        drop_ids(&out),
        vec!["module.mod_manor.scene.sc_cellar".to_string()],
        "未 player-known 的源驱动 spoiler 块必须被删"
    );
}

/// 验收 #2：同一生产源派生的 block，一旦其 fact 变为 player-known → 放行（不删）。
#[tokio::test]
async fn no_spoiler_context_filter_allows_player_known_fact() {
    let graph = butler_graph();
    let cellar_block = scene_block("mod_manor", "sc_cellar");
    let view =
        derive_scene_block_view(&cellar_block, &graph, Some("sc_hall")).expect("应派生私有视图");

    let mut ctx = PluginContext {
        module_id: Some("mod_manor".into()),
        hook: PluginHook::ContextAssembly,
        ..Default::default()
    };
    ctx.private_blocks = vec![view];
    ctx.player_known_fact_ids = vec!["sc_cellar".into()]; // 地窖真相已揭示。

    let out = NoSpoilerGuard.on_hook(&ctx).await;
    assert!(
        drop_ids(&out).is_empty(),
        "已 player-known 的 fact 块必放行，绝不删已揭示内容"
    );
}

/// 验收 #3：AfterLlmStream 收到**生产源**（harvest_module_secret_terms）非空术语表 →
/// 玩家可见念白泄漏未揭示术语时发 SecretLeak finding。
#[tokio::test]
async fn no_spoiler_after_stream_uses_private_secret_terms_source() {
    let graph = butler_graph();
    let secret_terms = harvest_module_secret_terms(&graph);
    assert!(!secret_terms.is_empty(), "生产源应采到非空 secret_terms");

    let mut ctx = PluginContext {
        module_id: Some("mod_manor".into()),
        hook: PluginHook::AfterLlmStream,
        // 念白提前泄漏了管家的隐藏身份（未揭示）。
        narration: Some("你注意到管家其实是莫里亚蒂教授。".into()),
        ..Default::default()
    };
    ctx.secret_terms = secret_terms;
    ctx.player_known_fact_ids = vec![]; // 玩家尚不知情。

    let out = NoSpoilerGuard.on_hook(&ctx).await;
    let finding = out
        .iter()
        .find_map(|c| match &c.kind {
            PluginContributionKind::VerifierFinding(vf) => Some(vf),
            _ => None,
        })
        .expect("未揭示术语泄漏 → 应发 SecretLeak finding");
    assert_eq!(finding.kind, trpg_agent::VerifierFindingKind::SecretLeak);
    assert_eq!(out[0].meta.plugin_id, NO_SPOILER_GUARD_ID);

    // 该 fact 已揭示 → 同念白不再算泄漏。
    ctx.player_known_fact_ids = vec!["npc_butler".into()];
    assert!(
        NoSpoilerGuard.on_hook(&ctx).await.is_empty(),
        "已揭示 fact 不算泄漏（揭示后可自由复述）"
    );
}

/// 验收 #4：SecretLeak finding 的 detail（进 errata/trace）绝不回写 secret 术语正文。
#[tokio::test]
async fn no_spoiler_finding_does_not_echo_secret_term() {
    let graph = butler_graph();
    let secret_terms = harvest_module_secret_terms(&graph);

    let mut ctx = PluginContext {
        module_id: Some("mod_manor".into()),
        hook: PluginHook::AfterLlmStream,
        narration: Some("管家其实是莫里亚蒂教授，还提到了传送门。".into()),
        ..Default::default()
    };
    ctx.secret_terms = secret_terms;
    ctx.player_known_fact_ids = vec![];

    let out = NoSpoilerGuard.on_hook(&ctx).await;
    assert!(!out.is_empty(), "应至少一条泄漏 finding");
    for c in &out {
        if let PluginContributionKind::VerifierFinding(vf) = &c.kind {
            for forbidden in ["莫里亚蒂教授", "连环杀手", "传送门", "异界"] {
                assert!(
                    !vf.detail.contains(forbidden),
                    "finding detail 绝不回写 secret 正文 '{forbidden}'：{}",
                    vf.detail
                );
            }
        }
        // trace 摘要同样不得夹带 secret 正文。
        let trace = c.to_trace();
        for forbidden in ["莫里亚蒂教授", "连环杀手", "传送门", "异界"] {
            assert!(
                !trace.summary.contains(forbidden),
                "trace summary 绝不回写 secret 正文 '{forbidden}'：{}",
                trace.summary
            );
        }
    }
}
