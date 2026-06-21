//! L6.2 — LIVE proof: under `TRPG_NARRATOR_SPLIT` the GM loses prose authorship and the tool-less
//! Narrator authors no mechanics (`#[ignore]`, real DB + scripted MockLlm).
//!
//! The split's structural guarantee (constitution ⑧): when `TRPG_NARRATOR_SPLIT=1` the GM
//! adjudicator (台下) settles ALL mechanics but its prose is REPLACED by a separate tool-less
//! Narrator that only describes committed facts. This live turn drives the PRODUCTION orchestrator
//! (`execute_turn` → `run_agent_loop` → `resolution_commit_boundary` → `run_narrator_phase`) on a
//! REAL Postgres-backed CoC session with a real d100 roll-under; only the LLM is scripted (as all
//! `live_*` tests). It proves, on a real turn:
//!   1. the player-visible output is the NARRATOR's prose, not the adjudicator's (GM lost authorship);
//!   2. that real Narrator output passes the L6.2 harness oracle
//!      (`trpg_harness::story_quality::classify_narrator_mechanics_checkpoint`) — no mechanical
//!      invention — while an invention-laced variant of the SAME output is caught (oracle is
//!      load-bearing on real-shaped text).
//!
//! Ruleset-agnostic (no name-branch): the split path + oracle key on generic mechanics, so the CoC
//! :54347 run is the authoritative L6.2 proof; the all-DB multi-scene validation is LV.1.
//!
//! Run (CoC kernel lives in the rulesets DB :54347):
//! ```bash
//! DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!   cargo test -p trpg-gm --test live_narrator_no_invention -- --ignored --nocapture
//! ```
//! No DATABASE_URL ⇒ SKIP (fail-closed, never blocks CI).

use anyhow::Result;
use async_stream::try_stream;
use async_trait::async_trait;
use futures_core::Stream;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tokio_stream::StreamExt;
use trpg_db::Db;
use trpg_gm::{
    execute_turn, GmLoop, LoopConfig, OwnedTurnRequest, ToolRegistry, TurnEvent, TurnOutcome,
    CANONICAL_TURN_PLAN,
};
use trpg_harness::story_quality::{
    classify_narrator_mechanics_checkpoint, NarratorMechanicsCheckpoint,
    NarratorMechanicsCheckpointState, NarratorMechanicsEvidence,
};
use trpg_llm::{AggregatedToolCall, LlmClient, StreamEvent, ToolChoice};
use trpg_model::{
    ActorKind, ChatMessage, ContextRequest, RuntimeState, TokenBudget, Visibility,
    VisibilityProfile,
};
use trpg_params::{RuntimeActorParameters, RuntimeParameterService};
use trpg_runtime::RuntimeEngine;

// ── deterministic LLM (mirrors live_director_spine::MockLlm) ──────────────────────────────────
struct MockLlm {
    scripts: Mutex<Vec<Vec<StreamEvent>>>,
}
impl MockLlm {
    fn push_script(&self, script: Vec<StreamEvent>) {
        self.scripts.lock().unwrap().push(script);
    }
}
#[async_trait]
impl LlmClient for MockLlm {
    async fn complete_text(&self, _: Vec<ChatMessage>, _: f32) -> Result<String> {
        unimplemented!("unused by e2e")
    }
    async fn complete_json(&self, _: Vec<ChatMessage>, _: f32) -> Result<Value> {
        Ok(json!({"hits": []}))
    }
    async fn stream_chat(
        &self,
        _: Vec<ChatMessage>,
        _: f32,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        unimplemented!("unused by e2e")
    }
    async fn complete_with_tools(&self, _: Vec<Value>, _: Vec<Value>) -> Result<Value> {
        unimplemented!("unused by e2e")
    }
    async fn stream_chat_with_tools(
        &self,
        _: Vec<Value>,
        _: Vec<Value>,
        _: ToolChoice,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let mut scripts = self.scripts.lock().unwrap();
        assert!(
            !scripts.is_empty(),
            "MockLlm script exhausted: the loop asked for one more round than scripted"
        );
        let script = scripts.remove(0);
        let s = try_stream! { for event in script { yield event; } };
        Ok(Box::pin(s))
    }
}

fn content(text: &str) -> StreamEvent {
    StreamEvent::ContentDelta(text.to_string())
}
fn done(reason: &str) -> StreamEvent {
    StreamEvent::Done {
        finish_reason: Some(reason.to_string()),
    }
}
fn tool_call(name: &str, args: Value) -> StreamEvent {
    StreamEvent::ToolCalls(vec![AggregatedToolCall {
        id: format!("call_{name}"),
        name: name.to_string(),
        arguments: args.to_string(),
    }])
}

const RULESET: &str = "call_of_cthulhu_7e";
// Distinctive markers: the GM adjudicator prose (must be REPLACED) vs the Narrator prose (must be
// what the player sees). The Narrator marker is pure description — zero mechanical vocabulary.
const ADJ_MARKER: &str = "ADJUDICATOR_RAW_PROSE_DO_NOT_SHIP";
const NARR_PROSE: &str =
    "你猛地向侧后翻滚，冰冷潮湿的空气掠过脸颊，墙上的影子随着烛火剧烈摇晃，腥味在喉头发紧。";

async fn fixture(llm: Arc<MockLlm>) -> Result<(GmLoop, String)> {
    let url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must point at the live rulesets DB (e.g. :54347)");
    let db = Db::connect(&url).await?;
    db.migrate().await?;
    db.load_rule_kernel(RULESET)
        .await?
        .unwrap_or_else(|| panic!("no active kernel for {RULESET}; run parse-all first"));
    let engine = RuntimeEngine::new(db.clone());
    let session_id = engine.start_session(RULESET, None).await?;
    let data_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data");
    let gm = GmLoop::new(
        engine,
        llm,
        ToolRegistry::standard(),
        LoopConfig::default(),
        data_dir,
    );
    Ok((gm, session_id))
}

async fn seed_pc_dodge(db: &Db, session: &str, skill: i64) -> Result<()> {
    let svc = RuntimeParameterService::new(db.clone());
    svc.upsert_actor_parameters(&RuntimeActorParameters {
        actor_param_id: format!("ap_l62_{session}"),
        session_id: session.into(),
        actor_id: "pc.current".into(),
        actor_kind: ActorKind::PlayerCharacter,
        ruleset_id: RULESET.into(),
        source_kind: "test_seed".into(),
        template_id: None,
        display_name: Some("调查员".into()),
        sheet_json: json!({ "skills": { "dodge": skill } }),
        mechanical_profile: json!({ "skills": { "dodge": skill }, "stats": {} }),
        status_json: json!({}),
        visibility: Visibility::default(),
        created_at_tick: Some(0),
        updated_at_tick: Some(0),
    })
    .await?;
    Ok(())
}

fn player_roll_args() -> Value {
    json!({
        "check_label": "闪避",
        "tested_parameter": "dodge",
        "stakes": {
            "before": "那东西的利爪带着腥风撕向你的咽喉。",
            "success": "你向侧后翻滚，爪锋擦着耳际掠过。",
            "failure": "利爪撕开你的肩膀，温热的血涌了出来。"
        }
    })
}

/// Drive one real split-narrator turn: round 1 settles a real dodge check (system-rolls policy),
/// round 2 is the adjudicator's buffered prose (carries ADJ_MARKER — must be replaced under split),
/// round 3 is the tool-less Narrator authoring the player-visible prose (NARR_PROSE).
async fn run_split_turn(session_skill: i64) -> Result<String> {
    let llm = Arc::new(MockLlm {
        scripts: Mutex::new(Vec::new()),
    });
    llm.push_script(vec![
        tool_call("request_player_roll", player_roll_args()),
        done("tool_calls"),
    ]);
    // adjudicator post-settlement prose — buffered (not shipped) under split; carries the marker.
    llm.push_script(vec![
        content(&format!("{ADJ_MARKER} 你成功闪开了攻击。")),
        done("stop"),
    ]);
    // the tool-less Narrator phase (empty tools / ToolChoice::None) — the player-visible prose.
    llm.push_script(vec![content(NARR_PROSE), done("stop")]);

    let (gm, session) = fixture(llm).await?;
    seed_pc_dodge(&gm.engine.db, &session, session_skill).await?;

    let turn_id = format!("turn_{}", uuid::Uuid::new_v4().simple());
    let request = ContextRequest {
        ruleset_id: RULESET.to_string(),
        module_id: None,
        session_id: session.clone(),
        turn_id,
        viewer: VisibilityProfile::gm(),
        token_budget: TokenBudget::default(),
    };
    let req = OwnedTurnRequest {
        request,
        state: RuntimeState {
            ruleset_id: RULESET.to_string(),
            ..Default::default()
        },
        user_input: "我闪开它的扑击".to_string(),
        history: Vec::new(),
        recent_transcript: None,
        module_id: None,
        data_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data"),
        cancel: None,
    };
    let mut stream = Box::pin(execute_turn(gm, req, CANONICAL_TURN_PLAN));
    let mut outcome: Option<TurnOutcome> = None;
    while let Some(event) = stream.next().await {
        match event {
            TurnEvent::TurnComplete { outcome: o } => {
                outcome = Some(o);
                break;
            }
            TurnEvent::TurnFailed { phase, message, .. } => {
                anyhow::bail!("turn failed in {phase}: {message}");
            }
            _ => {}
        }
    }
    match outcome {
        Some(TurnOutcome::Narration(text)) => Ok(text),
        other => anyhow::bail!("split turn must settle to narration, got {other:?}"),
    }
}

#[tokio::test]
#[ignore]
async fn split_narrator_authors_no_mechanics_live() -> Result<()> {
    if std::env::var("DATABASE_URL").is_err() {
        eprintln!("SKIP: DATABASE_URL unset (need the live rulesets DB :54347)");
        return Ok(());
    }
    std::env::set_var("TRPG_NARRATOR_SPLIT", "1");
    std::env::set_var("TRPG_AGENT_TABLE_DICE_POLICY", "system_rolls_visible");

    // high skill ⇒ the dodge check commits (a real check_result reaches the post-adjudication seam).
    let visible = run_split_turn(99).await?;
    println!("\n──── L6.2 split-narrator player-visible output ────\n{visible}\n");

    // (1) GM lost prose authorship: the player sees the Narrator's prose, never the adjudicator's.
    assert!(
        visible.contains(NARR_PROSE),
        "player-visible output must be the Narrator's prose; got: {visible}"
    );
    assert!(
        !visible.contains(ADJ_MARKER),
        "the GM adjudicator's raw prose must NOT reach the player under split; got: {visible}"
    );

    // (2) the REAL Narrator output passes the L6.2 harness oracle (no mechanical invention). A real
    //     check committed this turn, so committed_check=true; no effect was applied.
    let spec = NarratorMechanicsCheckpoint {
        require_no_invention: true,
    };
    let ev = NarratorMechanicsEvidence {
        narrator_text: visible.clone(),
        committed_check: true,
        committed_effect: false,
    };
    let state = classify_narrator_mechanics_checkpoint(&spec, &ev);
    println!("L6.2 oracle on real narrator output: {}", state.as_str());
    assert!(
        !state.is_failing(),
        "the real split Narrator output must not author unbacked mechanics; got {state:?}"
    );

    // (3) oracle is load-bearing on real-shaped text: an invention-laced variant of the SAME output
    //     (a dice roll + damage the ledger never committed) is caught.
    let invented = NarratorMechanicsEvidence {
        narrator_text: format!("{visible} You roll a d20 and take 6 damage to your hp."),
        committed_check: false,
        committed_effect: false,
    };
    assert_eq!(
        classify_narrator_mechanics_checkpoint(&spec, &invented),
        NarratorMechanicsCheckpointState::MechanicalInvention,
        "the oracle must catch invented mechanics appended to a real narrator output"
    );

    println!("✓ L6.2 LIVE: GM lost prose authorship; real Narrator output authored no mechanics; oracle load-bearing.");
    Ok(())
}
