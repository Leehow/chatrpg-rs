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
    builtin_no_spoiler::{private_block_view, NO_SPOILER_GUARD_ID, TAG_FUTURE_SCENE, TAG_SECRET},
    derive_scene_block_view, harvest_module_secret_terms, PluginContext, PluginContributionKind,
    PluginHook, RuntimePlugin,
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

/// 显式 tag 标注的生产形态块（走 production `private_block_view`，§24-#8 块级 fail-closed）。
fn tagged_block(block_id: &str, visibility: Visibility, tags: &[&str]) -> ContextBlock {
    let mut b = ContextBlock::new(
        block_id,
        BlockKind::SceneStatic,
        block_id,
        BlockContent::Text("块正文".into()),
        visibility,
        Stability::SceneStable,
        CacheZone::DynamicTail,
        Scope::global(),
        60,
    );
    b.tags = tags.iter().map(|s| s.to_string()).collect();
    b
}

/// P3.8 边界 (c)：`future_scene ∧ GmOnly` 且 fact 未 player-known 的块 → 装配前被删。
/// 走 production `private_block_view`（显式 tag 派生）+ 真实 `NoSpoilerGuard.on_hook`。
#[tokio::test]
async fn no_spoiler_drops_future_scene_gm_only_block() {
    // future_scene + GmOnly，无 fact 绑定（或未 known）→ should_drop 命中 future-scene 分支。
    let block = tagged_block(
        "module.mod_manor.scene.sc_future",
        Visibility::GmOnly,
        &[TAG_FUTURE_SCENE],
    );
    let view = private_block_view(&block);
    assert!(view.future_scene, "tag 派生：future_scene 应为真");
    assert_eq!(view.visibility, Visibility::GmOnly);

    let mut ctx = PluginContext {
        module_id: Some("mod_manor".into()),
        hook: PluginHook::ContextAssembly,
        ..Default::default()
    };
    ctx.private_blocks = vec![view];
    ctx.player_known_fact_ids = vec![];

    let out = NoSpoilerGuard.on_hook(&ctx).await;
    assert_eq!(
        drop_ids(&out),
        vec!["module.mod_manor.scene.sc_future".to_string()],
        "future_scene ∧ GmOnly 未揭示块必须被删"
    );

    // 对照：同为 future_scene 但玩家可见（PlayerVisible）→ 不删（未来场景非 GM-only 不剧透）。
    let mut pub_block = tagged_block(
        "module.mod_manor.scene.sc_future_pub",
        Visibility::PlayerVisible,
        &[TAG_FUTURE_SCENE],
    );
    pub_block.visibility = Visibility::PlayerVisible;
    let pub_view = private_block_view(&pub_block);
    ctx.private_blocks = vec![pub_view];
    assert!(
        drop_ids(&NoSpoilerGuard.on_hook(&ctx).await).is_empty(),
        "future_scene 但玩家可见 → 不属 GM-only 剧透，放行"
    );
}

/// P3.8 边界 (d)：无剧透标注的普通玩家可见块（非 secret、非 future_scene）→ 经 ContextFilter
/// 放行。`NoSpoilerGuard::should_drop` 只看 `fact_id` / `secret` / `future_scene` / `visibility`
/// 与 `player_known`——**绝不**读 `surfaced_entities`，故此测试验证的是「无剧透标注的可见块不被
/// fail-closed 误删」这一块级 passthrough，而非任何 surfaced-entity 驱动的放行路径
/// （production 过滤路径里并不存在后者）。
#[tokio::test]
async fn no_spoiler_passes_plain_player_visible_block_without_secret_or_future_tag() {
    // 普通玩家可见块：无 secret / future_scene tag → should_drop 恒 false。
    let visible = tagged_block(
        "module.mod_manor.entity.npc_innkeeper",
        Visibility::PlayerVisible,
        &[],
    );
    let view = private_block_view(&visible);
    assert!(!view.secret && !view.future_scene, "普通可见块无剧透标注");

    let mut ctx = PluginContext {
        module_id: Some("mod_manor".into()),
        hook: PluginHook::ContextAssembly,
        ..Default::default()
    };
    ctx.private_blocks = vec![view];
    ctx.player_known_fact_ids = vec![];

    assert!(
        drop_ids(&NoSpoilerGuard.on_hook(&ctx).await).is_empty(),
        "无剧透标注的玩家可见块必放行（should_drop 恒 false）"
    );

    // 进一步：同一实体块即便标了 secret，但其 fact 已 player-known（已揭示）→ 仍放行。
    let mut known_secret = tagged_block(
        "module.mod_manor.entity.npc_butler",
        Visibility::GmOnly,
        &[TAG_SECRET, "fact:npc_butler"],
    );
    known_secret.tags = vec![TAG_SECRET.into(), "fact:npc_butler".into()];
    let ks_view = private_block_view(&known_secret);
    assert_eq!(ks_view.fact_id.as_deref(), Some("npc_butler"));
    ctx.private_blocks = vec![ks_view];
    ctx.player_known_fact_ids = vec!["npc_butler".into()]; // 已揭示。
    assert!(
        drop_ids(&NoSpoilerGuard.on_hook(&ctx).await).is_empty(),
        "已 player-known 的 fact 块放行优先于 secret 标注（揭示后允许）"
    );
}
