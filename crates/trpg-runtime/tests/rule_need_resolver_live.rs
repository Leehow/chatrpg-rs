//! R2 收口后 — live e2e for the rule Need bus (the ONLY rule-retrieval path).
//!
//! 旧直连 `auto_search` + `learned_packet` 已删，A/B 比对不再可能。本测试断言 bus
//! 路径（RuleNeedResolver → RuleStewardAgent::assist）的功能覆盖：① BP1 active-kernel
//! 投影仍在；② 规则敏感输入产 >=1 检索块（覆盖旧 auto_search 信息）；③ source_refs
//! 接地增益。`assist` 已含 learned-packet 匹配 + Tantivy 搜索 + locator 回退。
//!
//! Run (CoC on :54347; cyberpunk lives on :54346 — swap the port if needed):
//!   TRPG_TEST_DATABASE_URL="postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg" \
//!   TRPG_DATA_DIR="$(pwd)/data" \
//!   cargo test -p trpg-runtime --test rule_need_resolver_live -- --ignored --nocapture
//!
//! Without TRPG_TEST_DATABASE_URL the test SKIPs (fail-closed, never blocks CI).
use std::path::PathBuf;

use trpg_db::Db;
use trpg_model::{CompiledContext, ContextRequest, RuntimeState, TokenBudget, VisibilityProfile};
use trpg_runtime::RuntimeEngine;
use trpg_search::{SearchConfig, SearchService};

fn skip_no_db() -> Option<String> {
    match std::env::var("TRPG_TEST_DATABASE_URL") {
        Ok(url) => Some(url),
        Err(_) => {
            eprintln!("SKIP: set TRPG_TEST_DATABASE_URL to the CoC DB on :54347");
            None
        }
    }
}

fn data_dir() -> PathBuf {
    std::env::var("TRPG_DATA_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("data"))
}

fn has_kernel(ctx: &CompiledContext) -> bool {
    ctx.prefix_blocks
        .iter()
        .chain(ctx.pinned_blocks.iter())
        .any(|b| b.block_id.starts_with("rule_steward.active_kernel."))
}

fn rule_retrieval_block_count(ctx: &CompiledContext) -> usize {
    ctx.dynamic_blocks
        .iter()
        .chain(ctx.pinned_blocks.iter())
        .filter(|b| {
            b.block_id.starts_with("rule_steward.lookup.")
                || b.block_id.starts_with("rule_steward.locators.")
                || b.tags.iter().any(|t| t == "rule_steward" || t == "lookup_hit")
        })
        .count()
}

fn any_source_refs(ctx: &CompiledContext) -> bool {
    ctx.prefix_blocks
        .iter()
        .chain(ctx.pinned_blocks.iter())
        .chain(ctx.dynamic_blocks.iter())
        .any(|b| !b.source_refs.is_empty())
}

#[tokio::test]
#[ignore]
async fn rule_need_bus_covers_rule_retrieval_and_grounds() {
    let Some(url) = skip_no_db() else { return };
    let db = match Db::connect(&url).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("SKIP: connect failed: {e}");
            return;
        }
    };
    // Discover the parsed ruleset from the DB (no per-ruleset hardcode). Prefer one
    // that has a rule kernel (so the BP1 active-kernel assertion is meaningful).
    let bundle = match db.load_latest_project_bundle().await {
        Ok(Some(b)) => b,
        _ => {
            eprintln!("SKIP: no parsed project bundle in this DB; run parse-all first");
            return;
        }
    };
    let mut ruleset_id = None;
    for rb in &bundle.rulesets {
        if db.load_rule_kernel(&rb.ruleset_id).await.ok().flatten().is_some() {
            ruleset_id = Some(rb.ruleset_id.clone());
            break;
        }
    }
    let ruleset_id = match ruleset_id.or_else(|| bundle.rulesets.first().map(|r| r.ruleset_id.clone())) {
        Some(r) => r,
        None => {
            eprintln!("SKIP: project bundle has no rulesets");
            return;
        }
    };

    let search = match SearchService::open(db.pool.clone(), SearchConfig::from_env_or_defaults(data_dir())) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("SKIP: search open failed: {e}");
            return;
        }
    };
    let engine = RuntimeEngine::new(db.clone()).with_search(search);

    let request = ContextRequest {
        ruleset_id: ruleset_id.clone(),
        module_id: None,
        session_id: format!("rule_need_live_{}", uuid::Uuid::new_v4().simple()),
        turn_id: "turn_test".into(),
        viewer: VisibilityProfile::gm(),
        token_budget: TokenBudget::default(),
    };
    let state = RuntimeState { ruleset_id: ruleset_id.clone(), ..Default::default() };
    // Rule-sensitive input (contains 攻击 / 检定 → passes looks_rule_or_module_sensitive).
    let input = "我用小刀攻击邪教徒，要做什么检定？";

    // 收口后唯一的获取路径：bus（NeedBus → RuleNeedResolver → assist），无 env 开关。
    let bus_ctx = engine
        .prepare_turn_context(&request, &state, Some(input), None)
        .await
        .expect("bus-path prepare_turn_context");

    // Assertion 1: BP1 active kernel projection survives (Task 7 did not touch
    // rule_steward_prefix_blocks_for_turn).
    assert!(has_kernel(&bus_ctx), "bus path must still carry BP1 active kernel projection");

    // Assertion 2: bus path produces >=1 rule retrieval block (covers the rule info
    // the legacy auto_search would have provided).
    let bus_rule_blocks = rule_retrieval_block_count(&bus_ctx);
    eprintln!("rule retrieval blocks (bus): {bus_rule_blocks}");
    assert!(
        bus_rule_blocks >= 1,
        "bus rule resolver must produce >=1 retrieval block covering legacy auto_search info"
    );

    // Assertion 3: grounding gain — the bus path surfaces non-empty source_refs.
    assert!(
        any_source_refs(&bus_ctx),
        "bus path must surface grounded source_refs (grounding gain)"
    );
}
