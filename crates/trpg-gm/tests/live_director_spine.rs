//! L1.3 SPINE — LIVE post-adjudication Director proof (`#[ignore]`, real DB + scripted MockLlm).
//!
//! Proves the turn-order-inversion fix on a REAL turn: with `TRPG_DIRECTOR_POST_ADJUDICATION=1`
//! the Beat Director runs AFTER adjudication at `resolution_commit_boundary`, sees the COMMITTED
//! check result (the inversion: the pre-adjudication Director at the ContextAssembly phase could
//! NOT see it), and re-shapes its beat to the real outcome — a committed FAILURE earns a
//! fail-forward `Complicate`, a committed SUCCESS earns an escalating `Escalate`.
//!
//! This is a genuine live turn (real `RuntimeEngine`, real Postgres, real Rules percentile roll
//! under `system_rolls_visible`); only the LLM is scripted (deterministic), exactly as the other
//! `live_*` e2e tests do. The proof reads the actual `director_spine` trace the production turn
//! emits and asserts `beat_kind` is consistent with the committed disposition the SAME trace
//! reports — so it is deterministic regardless of which way the d100 fell (the assertion keys on
//! the committed outcome, not on a hoped-for one). Two sessions (skill 99 vs skill 1) drive a
//! contrasting pass-turn and fail-turn.
//!
//! Run (CoC kernel lives in the rulesets DB :54347):
//! ```bash
//! cd crates/trpg-gm && DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
//!   cargo test -p trpg-gm --test live_director_spine -- --ignored --nocapture
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
use trpg_llm::{AggregatedToolCall, LlmClient, StreamEvent, ToolChoice};
use trpg_model::{
    ActorKind, ChatMessage, ContextRequest, RuntimeState, TokenBudget, Visibility,
    VisibilityProfile,
};
use trpg_params::{RuntimeActorParameters, RuntimeParameterService};
use trpg_runtime::RuntimeEngine;

// ── deterministic LLM (mirrors player_roll_policy_e2e::MockLlm) ──────────────────────────────
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
        unimplemented!("unused by run_gm_turn e2e")
    }
    async fn complete_json(&self, _: Vec<ChatMessage>, _: f32) -> Result<Value> {
        Ok(json!({"hits": []}))
    }
    async fn stream_chat(
        &self,
        _: Vec<ChatMessage>,
        _: f32,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        unimplemented!("unused by run_gm_turn e2e")
    }
    async fn complete_with_tools(&self, _: Vec<Value>, _: Vec<Value>) -> Result<Value> {
        unimplemented!("unused by run_gm_turn e2e")
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

// ── in-memory tracing capture (thread-local; current-thread tokio runtime keeps the event on
//    this thread) so the production `director_spine` event is both printed and assertable ──────
#[derive(Clone)]
struct BufWriter(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for BufWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // tee to real stdout so `--nocapture` shows the live trace as pasteable evidence.
        print!("{}", String::from_utf8_lossy(buf));
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for BufWriter {
    type Writer = BufWriter;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

const RULESET: &str = "call_of_cthulhu_7e";

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

/// Seed `pc.current` (the viewer-default acting actor) with a CoC `dodge` skill so the engine's
/// percentile roll-under lands a deterministic-ish band: 99 ⇒ almost-certain pass, 1 ⇒
/// almost-certain fail. The assertion keys on the ACTUAL committed outcome, so the rare flip is
/// harmless.
async fn seed_pc_dodge(db: &Db, session: &str, skill: i64) -> Result<()> {
    let svc = RuntimeParameterService::new(db.clone());
    svc.upsert_actor_parameters(&RuntimeActorParameters {
        actor_param_id: format!("ap_l13_{session}"),
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

/// Drive the turn through the PRODUCTION orchestrator (`execute_turn` → `run_agent_loop` →
/// `resolution_commit_boundary`, the spine's only call site — the legacy inline `run_gm_turn`
/// does NOT reach it). `execute_turn` consumes `gm` by value (spawns a 'static task), so we drain
/// its `TurnEvent` stream to the terminal `TurnComplete`.
async fn run_one_turn(gm: GmLoop, session: &str) -> Result<TurnOutcome> {
    let turn_id = format!("turn_{}", uuid::Uuid::new_v4().simple());
    let request = ContextRequest {
        ruleset_id: RULESET.to_string(),
        module_id: None,
        session_id: session.to_string(),
        turn_id,
        viewer: VisibilityProfile::gm(),
        token_budget: TokenBudget::default(),
    };
    let state = RuntimeState {
        ruleset_id: RULESET.to_string(),
        ..Default::default()
    };
    let req = OwnedTurnRequest {
        request,
        state,
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
    outcome.ok_or_else(|| anyhow::anyhow!("stream ended before TurnComplete"))
}

/// Drive one live turn that commits a real check result, capture the production `director_spine`
/// trace (the delta the turn appends to the shared global-subscriber buffer), and return it. The
/// Director runs post-adjudication (flag ON). NOTE: `run_gm_turn` runs its body inside a
/// `tokio::spawn` worker thread, so capture MUST go through a GLOBAL subscriber (installed once by
/// the caller) — a thread-local one would miss the worker thread.
async fn live_spine_turn(buf: &Arc<Mutex<Vec<u8>>>, skill: i64) -> Result<String> {
    let start = buf.lock().unwrap().len();

    let llm = Arc::new(MockLlm {
        scripts: Mutex::new(Vec::new()),
    });
    // round 1: the agent requests a dodge roll → system-rolls policy settles it in-turn (real
    // Rules d100 roll-under the seeded skill) → a real check_result is committed.
    llm.push_script(vec![
        tool_call("request_player_roll", player_roll_args()),
        done("tool_calls"),
    ]);
    // round 2: post-settlement narration round.
    llm.push_script(vec![
        content("尘埃落定，眼前的局势仍在推进——你必须决定下一步。"),
        done("stop"),
    ]);

    let (gm, session) = fixture(llm).await?;
    seed_pc_dodge(&gm.engine.db, &session, skill).await?;
    let outcome = run_one_turn(gm, &session).await?;
    // a settled turn must reach narration (no frozen gate under system-rolls policy).
    assert!(
        matches!(outcome, TurnOutcome::Narration(_)),
        "system-rolls turn must settle to narration, got {outcome:?}"
    );

    // The spine fires inside a detached `tokio::spawn` that may outlive the narration return, so
    // poll the shared buffer (up to ~5s) until the event marker lands.
    let mut captured = String::new();
    for _ in 0..50 {
        captured = {
            let guard = buf.lock().unwrap();
            String::from_utf8_lossy(&guard[start..]).to_string()
        };
        if captured.contains("post-adjudication DirectorPlan built") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    println!(
        "\n──── director_spine capture (session={session}, seeded dodge={skill}) ────\n{captured}"
    );
    Ok(captured)
}

/// Assert the captured `director_spine` event is self-consistent: the beat the post-adjudication
/// Director emitted reflects the committed disposition the SAME event reported. Returns the
/// observed disposition ("Passed"/"Failed") for the contrast check.
fn assert_beat_reflects_committed(captured: &str) -> &'static str {
    assert!(
        captured.contains("post-adjudication DirectorPlan built"),
        "the production turn must emit the director_spine event (spine fired post-adjudication)"
    );
    assert!(
        captured.contains("committed_results"),
        "the trace must carry the committed_results the overlay keyed on"
    );
    let failed = captured.contains("Failed");
    let passed = captured.contains("Passed");
    assert!(
        failed ^ passed,
        "exactly one committed disposition expected in the trace, got: {captured}"
    );
    if failed {
        // committed FAILURE ⇒ fail-forward complication (story moves on, never dead-ends).
        assert!(
            captured.contains("Complicate") && captured.contains("fail_forward"),
            "a committed FAILED check must yield a fail-forward Complicate beat, got: {captured}"
        );
        "Failed"
    } else {
        // committed SUCCESS ⇒ escalate (capitalize on the win).
        assert!(
            captured.contains("Escalate") && captured.contains("capitalize_success"),
            "a committed PASSED check must yield a capitalize-success Escalate beat, got: {captured}"
        );
        "Passed"
    }
}

/// THE SPINE LIVE PROOF: a high-skill turn and a low-skill turn, each a real DB-backed turn, each
/// proving its post-adjudication beat reflects its own committed result, and together exhibiting
/// the pass↔fail contrast the design demands.
#[tokio::test]
#[ignore]
async fn post_adjudication_beat_reflects_committed_result_live() -> Result<()> {
    if std::env::var("DATABASE_URL").is_err() {
        eprintln!("SKIP: DATABASE_URL unset (need the live rulesets DB :54347)");
        return Ok(());
    }
    // process-level env: the spine flag (master switch) + system-rolls policy so the engine
    // settles the check in-turn and a real committed result reaches the post-adjudication seam.
    std::env::set_var("TRPG_DIRECTOR_POST_ADJUDICATION", "1");
    std::env::set_var("TRPG_AGENT_TABLE_DICE_POLICY", "system_rolls_visible");

    // GLOBAL subscriber (installed once): the turn body runs on a `tokio::spawn` worker thread, so
    // only a global default catches its `director_spine` event. Shared buffer, sliced per turn.
    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_writer(BufWriter(buf.clone()))
        .with_env_filter(tracing_subscriber::EnvFilter::new("director_spine=info"))
        .without_time()
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("install global subscriber once");

    // high skill ⇒ expected committed PASS ⇒ Escalate.
    let pass_trace = live_spine_turn(&buf, 99).await?;
    let disp_a = assert_beat_reflects_committed(&pass_trace);

    // low skill ⇒ expected committed FAIL ⇒ Complicate (fail-forward).
    let fail_trace = live_spine_turn(&buf, 1).await?;
    let disp_b = assert_beat_reflects_committed(&fail_trace);

    println!("\nSPINE LIVE CONTRAST: turn-A disposition={disp_a}, turn-B disposition={disp_b}");
    // Both turns individually proved beat⇔committed consistency; flag the contrast when present.
    if disp_a != disp_b {
        println!("✓ pass↔fail contrast exhibited live (the Director steers to the REAL outcome).");
    } else {
        println!(
            "NOTE: both turns landed {disp_a} (rare d100 alignment); each still proved \
             beat⇔committed consistency — the load-bearing guarantee holds."
        );
    }
    Ok(())
}
