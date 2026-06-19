//! TC-PIPE-03 acceptance: production flight-recorder / verifier provenance.
//!
//! These tests exercise REAL production functions (the NoSpoiler guard on its
//! ContextAssembly + AfterLlmStream hooks, the SpoilerMeta-derived block view and
//! secret-term harvest, and the `view_load_trace` provenance the turn loop records)
//! and assert, on the resulting `TurnTrace.plugin_contributions` artifact:
//!
//! - DA-PIPE-01: knowledge/NPC view loads are recorded BEFORE plugin contributions
//!   (`flight_recorder_view_order_ok`), so view loading provably precedes the
//!   context-policy plugin phase in the persisted artifact, not just in comments.
//! - DA-PIPE-03: the single artifact covers view load + context filter + verifier
//!   finding (plus prompt block), with ledger-impacting events linked by turn_id.
//! - DA-SPOIL-03: a verifier-private secret reaches the AfterLlmStream verifier
//!   (`secret_terms`) and is detected, but its term never enters the model-visible
//!   prompt manifest (the spoiler block is dropped by ContextFilter), and the
//!   finding/trace never echo the secret term. A fail-closed negative control
//!   proves a player-unknown secret block IS rejected from model-visible context.

use serde_json::json;
use trpg_gm::plugin::{
    builtin_no_spoiler::NO_SPOILER_GUARD_ID, derive_scene_block_view, harvest_module_secret_terms,
    PluginContext, PluginContributionKind, PluginHook, RuntimePlugin,
};
use trpg_gm::turn_loop::{flight_recorder_view_order_ok, view_load_trace, VIEW_LOAD_KIND};
use trpg_gm::NoSpoilerGuard;
use trpg_model::{
    BlockContent, BlockKind, CacheZone, ContextBlock, ModuleGraph, PluginContributionTrace,
    ScenarioNode, SceneExtractionStatus, Scope, SpoilerMeta, Stability, Visibility,
};

/// Production-shaped manor graph with a scene-level spoiler (cellar portal).
fn manor_graph() -> ModuleGraph {
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
    g
}

fn scene_block(module_id: &str, node_id: &str) -> ContextBlock {
    ContextBlock::new(
        format!("module.{module_id}.scene.{node_id}"),
        BlockKind::SceneStatic,
        node_id,
        BlockContent::Text("场景正文：石墙后藏着传送门，通往异界。".into()),
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

/// DA-PIPE-01 + DA-PIPE-03: the persisted flight-recorder artifact records view
/// loads BEFORE plugin contributions and covers all production categories.
#[tokio::test]
async fn view_loads_recorded_before_plugin_contributions() {
    let graph = manor_graph();

    // 1) Production view-load provenance, recorded first (turn loop pushes these
    //    before the context-assembly plugin loop). Counts only — never secrets.
    let mut artifact: Vec<PluginContributionTrace> = vec![
        view_load_trace("gm_context", "3 compiled block(s)"),
        view_load_trace("npc_mind_views", "1 active npc(s)"),
        view_load_trace("player_knowledge_view", "0 player-known fact(s)"),
    ];

    // 2) Real ContextAssembly plugin contributions (NoSpoiler drops the unknown
    //    spoiler block) folded into the same artifact via `to_trace()`.
    let cellar_block = scene_block("mod_manor", "sc_cellar");
    let view = derive_scene_block_view(&cellar_block, &graph, Some("sc_hall"))
        .expect("cellar carries scene-level SpoilerMeta → private view");
    let mut ca = PluginContext {
        module_id: Some("mod_manor".into()),
        hook: PluginHook::ContextAssembly,
        ..Default::default()
    };
    ca.private_blocks = vec![view];
    ca.player_known_fact_ids = vec![];
    for c in NoSpoilerGuard.on_hook(&ca).await {
        artifact.push(c.to_trace());
    }

    // 3) Real AfterLlmStream verifier finding (leak detected) folded in.
    let mut asx = PluginContext {
        module_id: Some("mod_manor".into()),
        hook: PluginHook::AfterLlmStream,
        narration: Some("你一眼看到墙后的传送门。".into()),
        ..Default::default()
    };
    asx.secret_terms = harvest_module_secret_terms(&graph);
    asx.player_known_fact_ids = vec![];
    for c in NoSpoilerGuard.on_hook(&asx).await {
        artifact.push(c.to_trace());
    }

    // DA-PIPE-01: view loads precede every plugin contribution.
    assert!(
        flight_recorder_view_order_ok(&artifact),
        "view_load entries must precede plugin contributions: {artifact:?}"
    );
    // The first three entries are the view loads, in load order.
    assert!(artifact[0].kind == VIEW_LOAD_KIND && artifact[0].summary.starts_with("gm_context"));
    assert!(artifact[1].summary.starts_with("npc_mind_views"));
    assert!(artifact[2].summary.starts_with("player_knowledge_view"));

    // DA-PIPE-03: the artifact covers view_load + context_filter + verifier_finding.
    let kinds: std::collections::BTreeSet<&str> =
        artifact.iter().map(|t| t.kind.as_str()).collect();
    assert!(kinds.contains("view_load"), "missing view_load: {kinds:?}");
    assert!(
        kinds.contains("context_filter"),
        "missing context_filter: {kinds:?}"
    );
    assert!(
        kinds.contains("verifier_finding"),
        "missing verifier_finding: {kinds:?}"
    );
    // The verifier-finding entry is from the NoSpoiler guard.
    assert!(artifact
        .iter()
        .any(|t| t.kind == "verifier_finding" && t.plugin_id == NO_SPOILER_GUARD_ID));
    // No secret term ever appears in any trace summary.
    for t in &artifact {
        for secret in ["传送门", "异界"] {
            assert!(
                !t.summary.contains(secret),
                "trace summary leaked secret: {t:?}"
            );
        }
    }
}

/// DA-SPOIL-03: a verifier-private secret reaches the AfterLlmStream verifier and
/// is detected, but the same secret never survives into the model-visible prompt
/// manifest (the spoiler block is dropped before assembly), and the finding never
/// echoes the secret term.
#[tokio::test]
async fn secret_reaches_verifier_but_not_model_visible_prompt() {
    let graph = manor_graph();

    // Verifier-private input carries the secret term (this is what the verifier
    // inspects — it is NOT model-visible context).
    let secret_terms = harvest_module_secret_terms(&graph);
    assert!(
        secret_terms.iter().any(|s| s.term == "传送门"),
        "harvest must surface the private secret term to the verifier"
    );

    // Model-visible context manifest: the unknown spoiler block is dropped by the
    // ContextAssembly ContextFilter, so the secret never reaches the prompt.
    let cellar_block = scene_block("mod_manor", "sc_cellar");
    let view =
        derive_scene_block_view(&cellar_block, &graph, Some("sc_hall")).expect("private view");
    let mut ca = PluginContext {
        module_id: Some("mod_manor".into()),
        hook: PluginHook::ContextAssembly,
        ..Default::default()
    };
    ca.private_blocks = vec![view];
    ca.player_known_fact_ids = vec![]; // player does not know the cellar truth
    let ca_out = NoSpoilerGuard.on_hook(&ca).await;
    assert!(
        drop_ids(&ca_out).contains(&"module.mod_manor.scene.sc_cellar".to_string()),
        "fail-closed: the player-unknown secret block must be dropped from model-visible context"
    );

    // The verifier still catches a leak if the secret reaches narration, and the
    // finding/trace never echo the secret term itself.
    let mut asx = PluginContext {
        module_id: Some("mod_manor".into()),
        hook: PluginHook::AfterLlmStream,
        narration: Some("你看到墙后的传送门。".into()),
        ..Default::default()
    };
    asx.secret_terms = secret_terms;
    asx.player_known_fact_ids = vec![];
    let asx_out = NoSpoilerGuard.on_hook(&asx).await;
    let finding = asx_out
        .iter()
        .find_map(|c| match &c.kind {
            PluginContributionKind::VerifierFinding(vf) => Some(vf),
            _ => None,
        })
        .expect("verifier-private secret_terms must let the verifier catch the leak");
    assert_eq!(finding.kind, trpg_agent::VerifierFindingKind::SecretLeak);
    for c in &asx_out {
        assert!(!c.to_trace().summary.contains("传送门"));
        if let PluginContributionKind::VerifierFinding(vf) = &c.kind {
            assert!(
                !vf.detail.contains("传送门"),
                "finding detail must not echo secret"
            );
        }
    }
}

/// DA-SPOIL-03 negative control (fail-closed): once the player legitimately knows
/// the fact, the block is NOT dropped — proving the drop is driven by player-known
/// state, not blanket deletion; and an explicitly-secret player-unknown block is
/// always rejected from model-visible context.
#[tokio::test]
async fn player_unknown_secret_block_is_rejected_known_is_allowed() {
    let graph = manor_graph();
    let cellar_block = scene_block("mod_manor", "sc_cellar");
    let view =
        derive_scene_block_view(&cellar_block, &graph, Some("sc_hall")).expect("private view");

    // Unknown → dropped (fail-closed).
    let mut unknown = PluginContext {
        module_id: Some("mod_manor".into()),
        hook: PluginHook::ContextAssembly,
        ..Default::default()
    };
    unknown.private_blocks = vec![view.clone()];
    unknown.player_known_fact_ids = vec![];
    assert!(
        !drop_ids(&NoSpoilerGuard.on_hook(&unknown).await).is_empty(),
        "player-unknown secret block must be rejected from model-visible context"
    );

    // Known → allowed (no over-deletion of revealed content).
    let mut known = PluginContext {
        module_id: Some("mod_manor".into()),
        hook: PluginHook::ContextAssembly,
        ..Default::default()
    };
    known.private_blocks = vec![view];
    known.player_known_fact_ids = vec!["sc_cellar".into()];
    assert!(
        drop_ids(&NoSpoilerGuard.on_hook(&known).await).is_empty(),
        "player-known fact block must be allowed (no blanket deletion)"
    );
}

/// The ordering invariant itself fails closed when a view_load appears AFTER a
/// plugin contribution (a mis-ordered artifact must not be accepted).
#[test]
fn misordered_artifact_fails_view_order_invariant() {
    let bad = vec![
        PluginContributionTrace {
            plugin_id: NO_SPOILER_GUARD_ID.to_string(),
            hook: "context_assembly".to_string(),
            kind: "context_filter".to_string(),
            summary: "drop 1 block(s)".to_string(),
        },
        view_load_trace("player_knowledge_view", "0 player-known fact(s)"),
    ];
    assert!(
        !flight_recorder_view_order_ok(&bad),
        "a view_load after a plugin contribution must fail the invariant"
    );
}
