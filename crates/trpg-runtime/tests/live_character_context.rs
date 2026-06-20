//! OA2 (G-3) LIVE proof: the split Narrator's `NarrationPacket` receives the PC's
//! player-safe competency profile end-to-end from REAL live data.
//!
//! Path proven against the LIVE rulesets DB (:54347):
//!   create_and_bind_character (real CoC 45+ skills / 8 attrs into
//!   runtime_actor_parameters)
//!     -> prepare_turn_context under materialization Enforce
//!     -> RuntimeParameterService::load_actor_parameters
//!     -> collect_character_context
//!     -> compiled.character_context (carried to the split Narrator)
//!
//! DB-gated: skips when DATABASE_URL is unset or the CoC ruleset is not loaded.
//! Single test per file => its own test binary => the explicit env set is race-free.

use trpg_db::Db;
use trpg_model::{ContextRequest, RuntimeState, TokenBudget, VisibilityProfile};
use trpg_params::RuntimeParameterService;
use trpg_runtime::RuntimeEngine;

const RULESET: &str = "call_of_cthulhu_7e";
const SESSION: &str = "sess_live_character_context_oa2";
const ACTOR: &str = "pc.current";

async fn connect() -> Option<Db> {
    let url = std::env::var("DATABASE_URL").ok()?;
    Db::connect(&url).await.ok()
}

async fn reset(db: &Db) {
    for tbl in ["runtime_actor_parameters", "sessions"] {
        sqlx::query(&format!("delete from {tbl} where session_id=$1"))
            .bind(SESSION)
            .execute(&db.pool)
            .await
            .ok();
    }
}

#[tokio::test]
async fn live_pc_sheet_reaches_split_narrator_under_enforce() {
    let Some(db) = connect().await else {
        eprintln!("SKIP: DATABASE_URL unset");
        return;
    };
    if db
        .load_character_onboarding_pack(RULESET)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        eprintln!("SKIP: no {RULESET} onboarding pack");
        return;
    }
    reset(&db).await;

    // create_and_bind_character establishes the sessions row (FK target for turns).
    // The MockLlmClient produces a mock chargen-spec sheet, so we OVERWRITE the actor
    // parameters with a REAL-SHAPED CoC sheet (the exact stats/skills structure a live
    // relay-created character persists — verified against DB) to keep the ON-path proof
    // deterministic (no LLM flakiness) while exercising the genuine engine wiring:
    //   prepare_turn_context -> load_actor_parameters -> collect_character_context.
    let created = RuntimeEngine::new(db.clone())
        .create_and_bind_character(
            &trpg_llm::MockLlmClient,
            RULESET,
            Some("document"),
            Some(SESSION),
            ACTOR,
            "oa2 live regression",
        )
        .await
        .expect("create and bind character");
    assert_eq!(created.session_id, SESSION);

    let svc = RuntimeParameterService::new(db.clone());
    let mut params = svc
        .load_actor_parameters(SESSION, ACTOR)
        .await
        .expect("load")
        .expect("actor params exist");
    params.display_name = Some("林景修".to_string());
    params.sheet_json = serde_json::json!({
        "name": "林景修",
        "stats": {"STR": 55, "CON": 60, "DEX": 70, "INT": 80, "POW": 65, "EDU": 75, "APP": 50, "SIZ": 65},
        "skills": {"Listen": 55, "Spot Hidden": 50, "Library Use": 60, "Psychology": 40,
                   "Persuade": 45, "Dodge": 35, "Law": 40, "con_half": 30},
        "anomaly_gm_only": {"truth": 99}
    });
    svc.upsert_actor_parameters(&params)
        .await
        .expect("upsert real-shaped sheet");

    let engine = RuntimeEngine::new(db.clone());
    let request = ContextRequest {
        ruleset_id: RULESET.to_string(),
        module_id: Some("call_of_cthulhu_7e.document".to_string()),
        session_id: SESSION.to_string(),
        turn_id: "turn_oa2".into(),
        viewer: VisibilityProfile::player("p", ACTOR),
        token_budget: TokenBudget::default(),
    };
    let state = RuntimeState {
        ruleset_id: RULESET.to_string(),
        module_id: Some("call_of_cthulhu_7e.document".to_string()),
        ..Default::default()
    };

    // ON path: Enforce => runtime fills character_context from the real sheet.
    std::env::set_var("TRPG_MATERIALIZATION_AFFORDANCE", "enforce");
    let on = engine
        .prepare_turn_context(&request, &state, Some("我环顾四周。"), None)
        .await
        .expect("enforce prepare_turn_context");
    let on_text = on.character_context.join("\n");
    assert!(
        !on.character_context.is_empty(),
        "Enforce must populate character_context from the real PC sheet; got empty"
    );
    // The carrier must hold real competency buckets with real named skills/stats.
    assert!(on_text.contains("stats:") && on_text.contains("skills:"), "buckets: {on_text}");
    assert!(on_text.contains("Spot Hidden 50"), "named skill missing: {on_text}");
    assert!(on_text.contains("STR 55"), "named stat missing: {on_text}");
    // Defense-in-depth: derived noise + GM-only never leak into the player-safe carrier.
    assert!(!on_text.contains("con_half"), "derived noise leaked: {on_text}");
    assert!(!on_text.contains("truth") && !on_text.contains("anomaly"), "gm_only leaked: {on_text}");

    // OFF path: explicit Off => field stays empty (byte-equal-for-this-field baseline).
    std::env::set_var("TRPG_MATERIALIZATION_AFFORDANCE", "off");
    let off = engine
        .prepare_turn_context(&request, &state, Some("我环顾四周。"), None)
        .await
        .expect("off prepare_turn_context");
    assert!(
        off.character_context.is_empty(),
        "Off must leave character_context empty (additive proof); got: {:?}",
        off.character_context
    );

    std::env::remove_var("TRPG_MATERIALIZATION_AFFORDANCE");
    sqlx::query("delete from characters where character_id=$1")
        .bind(&created.character_id)
        .execute(&db.pool)
        .await
        .ok();
    reset(&db).await;
}
