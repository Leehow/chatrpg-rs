//! L-E LIVE proof: the GM continuity anchor block is injected end-to-end when the
//! caller supplies NO recent_transcript (the CLI/eval path) but prior turns exist
//! in the `turns` table — and is ABSENT under the OFF flag (byte-equal baseline).
//!
//! Path proven against the LIVE rulesets DB (:54347):
//!   save_turn (real Player/GM prose into `turns`)
//!     -> prepare_turn_context(recent_transcript = None) with GM viewer
//!     -> load_recent_transcript fallback (flag ON)
//!     -> compiled.{pinned,dynamic}_blocks contains `runtime.continuity_anchor`
//!
//! DB-gated: skips when DATABASE_URL is unset. Single test file => own binary =>
//! the env set/remove is race-free.

use trpg_db::Db;
use trpg_model::{ContextRequest, RuntimeState, TokenBudget, VisibilityProfile};
use trpg_runtime::RuntimeEngine;

const RULESET: &str = "cyberpunk_red";
const SESSION: &str = "sess_live_continuity_anchor_le";
const ACTOR: &str = "pc.current";

async fn connect() -> Option<Db> {
    let url = std::env::var("DATABASE_URL").ok()?;
    Db::connect(&url).await.ok()
}

fn has_anchor(ctx: &trpg_model::CompiledContext) -> bool {
    ctx.prefix_blocks
        .iter()
        .chain(ctx.pinned_blocks.iter())
        .chain(ctx.dynamic_blocks.iter())
        .any(|b| b.block_id == "runtime.continuity_anchor")
}

#[tokio::test]
async fn live_continuity_anchor_injected_when_caller_passes_none() {
    let Some(db) = connect().await else {
        eprintln!("SKIP: DATABASE_URL unset");
        return;
    };

    // Establish the sessions row (FK target for turns) via the real engine.
    let engine = RuntimeEngine::new(db.clone());
    engine
        .create_and_bind_character(
            &trpg_llm::MockLlmClient,
            RULESET,
            None,
            Some(SESSION),
            ACTOR,
            "L-E live regression",
        )
        .await
        .expect("create and bind character");

    // Seed a prior delivered turn (Player/GM prose) — this is the authoritative
    // "established position" the anchor must carry forward.
    db.save_turn(
        SESSION,
        "turn_prior_le",
        "我退回第四街附近的住宅排屋门前，敲了敲家门。",
        "你站在自家旧排屋门前，夜风把远处的火药味带过来；门内没有回应。",
        serde_json::json!({}),
        "complete",
    )
    .await
    .expect("save prior turn");

    let request = ContextRequest {
        ruleset_id: RULESET.to_string(),
        module_id: None,
        session_id: SESSION.to_string(),
        turn_id: "turn_le_probe".into(),
        viewer: VisibilityProfile::gm(),
        token_budget: TokenBudget::default(),
    };
    let state = RuntimeState {
        ruleset_id: RULESET.to_string(),
        ..Default::default()
    };

    // ON (default): caller passes recent_transcript = None => server-load fallback
    // injects the continuity anchor.
    // DEBUG: what does the raw fallback source return?
    let raw = db.load_recent_transcript(SESSION, 2).await;
    eprintln!("DEBUG load_recent_transcript(2) => {raw:?}");

    std::env::set_var("TRPG_GM_CONTINUITY_ANCHOR", "1");
    let on = engine
        .prepare_turn_context(&request, &state, Some("我再敲一次门。"), None)
        .await
        .expect("ON prepare_turn_context");
    let ids: Vec<&str> = on
        .prefix_blocks
        .iter()
        .chain(on.pinned_blocks.iter())
        .chain(on.dynamic_blocks.iter())
        .map(|b| b.block_id.as_str())
        .collect();
    eprintln!("DEBUG compiled block_ids => {ids:?}");
    assert!(
        has_anchor(&on),
        "flag ON + None transcript + prior turn => continuity_anchor block MUST be injected"
    );
    let anchor_text = on
        .prefix_blocks
        .iter()
        .chain(on.pinned_blocks.iter())
        .chain(on.dynamic_blocks.iter())
        .find(|b| b.block_id == "runtime.continuity_anchor")
        .map(|b| b.content.render_text())
        .unwrap_or_default();
    assert!(
        anchor_text.contains("住宅排屋门前"),
        "anchor must carry the prior delivered narration: {anchor_text}"
    );
    assert!(
        anchor_text.contains("不要把玩家挪回场景入口"),
        "anchor must carry the anti-amnesia instruction: {anchor_text}"
    );

    // OFF: explicit off => no anchor block (byte-equal baseline for the None path).
    std::env::set_var("TRPG_GM_CONTINUITY_ANCHOR", "off");
    let off = engine
        .prepare_turn_context(&request, &state, Some("我再敲一次门。"), None)
        .await
        .expect("OFF prepare_turn_context");
    assert!(
        !has_anchor(&off),
        "flag OFF => no continuity_anchor block (baseline)"
    );

    std::env::remove_var("TRPG_GM_CONTINUITY_ANCHOR");
}
