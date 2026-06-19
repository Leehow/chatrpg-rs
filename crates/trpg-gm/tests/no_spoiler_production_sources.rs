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

// ===================== R3：GmAdjudication 隐藏真相喂 GM 内部勘误（设计3 §11/§13，契约 C3）=====================
//
// 这些测试走真实生产投影链 `Db::gm_truth_view` + `Db::player_knowledge_view`
// → `project_for_gm_adjudication` → `hidden_truth_fact_ids()` →
// `annotate_findings_with_hidden_truth`（生产消费者），证明运行时 GM 真相账本确认的隐藏真相
// 确实抵达 GM 内部消费者并标注泄漏 finding（仅 GM-only），且绝不进玩家可见集。
// DB 缺失（DATABASE_URL 未设）时跳过——与既有 DB-backed 集成测试同惯例。
#[cfg(test)]
mod gm_adjudication_hidden_truth_db {
    use trpg_agent::{VerifierFinding, VerifierFindingKind, VerifierSeverity};
    use trpg_gm::turn_loop::{annotate_findings_with_hidden_truth, GM_HIDDEN_TRUTH_MARK};

    async fn connect_or_skip() -> Option<trpg_db::Db> {
        let url = match std::env::var("DATABASE_URL") {
            Ok(u) => u,
            Err(_) => {
                eprintln!("SKIP: DATABASE_URL unset");
                return None;
            }
        };
        let db = match trpg_db::Db::connect(&url).await {
            Ok(d) => d,
            Err(e) => {
                eprintln!("SKIP: connect failed: {e}");
                return None;
            }
        };
        if let Err(e) = db.migrate().await {
            eprintln!("SKIP: migrate failed: {e}");
            return None;
        }
        Some(db)
    }

    async fn purge(db: &trpg_db::Db, session: &str) {
        let _ = sqlx::query("delete from knowledge_edges where session_id=$1")
            .bind(session)
            .execute(&db.pool)
            .await;
    }

    async fn edge(db: &trpg_db::Db, session: &str, holder_kind: &str, fact: &str) {
        db.upsert_knowledge_edge(trpg_db::KnowledgeEdgeInput {
            session_id: session,
            holder_kind,
            holder_id: "",
            fact_id: fact,
            knowledge_state: "knows_true",
            confidence: None,
            learned_at_turn_id: None,
            disclosure_policy: None,
            source_event_id: None,
            reason: None,
        })
        .await
        .unwrap();
    }

    fn leak_finding(fact_id: &str) -> VerifierFinding {
        VerifierFinding {
            kind: VerifierFindingKind::SecretLeak,
            severity: VerifierSeverity::Blocker,
            detail: format!("player-visible narration exposes player-unknown fact '{fact_id}'"),
        }
    }

    /// 生产投影链：运行时 GM 真相账本确认的隐藏真相（gm_truth − player_known）抵达 GM 内部
    /// 消费者，给账本确认的泄漏 finding 追加 GM-internal 标记；玩家已知 fact 的泄漏不带标记，
    /// 且隐藏真相绝不进玩家可见集（player_narration_view 不含它）。
    #[tokio::test]
    async fn hidden_truth_from_ledger_reaches_gm_internal_consumer() {
        let Some(db) = connect_or_skip().await else {
            return;
        };
        let session = "sess_r3_gm_adjudication";
        purge(&db, session).await;
        // GM 账本：fact_confirmed 与 fact_shared 为真；玩家只知道 fact_shared。
        edge(&db, session, "gm", "fact_confirmed").await;
        edge(&db, session, "gm", "fact_shared").await;
        edge(&db, session, "player_party", "fact_shared").await;

        let proj = trpg_runtime::project_for_gm_adjudication(&db, session)
            .await
            .expect("生产 GM 裁决投影链应成功");
        // 隐藏真相 = gm_truth − player_known = {fact_confirmed}。
        let hidden = proj.hidden_truth_fact_ids();
        assert!(hidden.contains("fact_confirmed"));
        assert!(
            !hidden.contains("fact_shared"),
            "玩家已知 fact 不属隐藏真相"
        );
        // 玩家可见集绝不含隐藏真相（契约 C3 隔离）。
        let player_view = proj.player_narration_view();
        assert!(player_view.allows("fact_shared"));
        assert!(
            !player_view.allows("fact_confirmed"),
            "隐藏真相绝不进玩家可见集"
        );

        // GM 内部消费者：账本确认的泄漏带标记，玩家已知 fact 的泄漏不带标记。
        let findings = vec![leak_finding("fact_confirmed"), leak_finding("fact_shared")];
        let annotated = annotate_findings_with_hidden_truth(&findings, &hidden);
        assert!(
            annotated[0].detail.contains(GM_HIDDEN_TRUTH_MARK),
            "账本确认隐藏真相的泄漏应带 GM-internal 标记：{}",
            annotated[0].detail
        );
        assert!(
            !annotated[1].detail.contains(GM_HIDDEN_TRUTH_MARK),
            "玩家已知 fact 的泄漏不带 GM-internal 隐藏真相标记"
        );
        purge(&db, session).await;
    }
}
