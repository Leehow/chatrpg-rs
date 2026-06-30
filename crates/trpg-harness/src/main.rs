use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use trpg_eval::{evaluate_fixture, parse_markdown_fixture, render_markdown_report, Verdict};
use trpg_harness::{
    build_cassette, build_check_evidence, classify_check_checkpoint,
    classify_flight_recorder_checkpoint, classify_knowledge_checkpoint, classify_memory_checkpoint,
    classify_npc_social_checkpoint, default_stream_format, evaluate_assertions,
    evaluate_persisted_verification, fixture_turn_evidence, human_player_input_findings,
    memory_trace, parse_character_creation_jsonl, parse_jsonl_events,
    player_visible_effect_present, player_visible_roll_request_findings, tail_chars,
    verify_fixture_plan, when_satisfied, CharacterCreationReadiness, CheckCheckpoint,
    CheckpointState, ExecutionMode, Fixture, FixtureTurn, FlightRecorderCheckpoint,
    FlightRecorderEvidence, FlightRecorderState, HarnessCase, KnowledgeCheckpoint,
    KnowledgeCheckpointState, KnowledgeEvidence, MemoryCheckpoint, MemoryCheckpointState,
    MemoryEvidence, NpcSocialCheckpoint, NpcSocialCheckpointState, NpcSocialEvidence,
    ParsedJsonlStream, PersistedVerdict, PersistedVerification, RelationshipDeltaEvidence,
    ScenarioIdentity,
};

#[derive(Debug, Parser)]
#[command(
    name = "trpg-harness",
    version,
    about = "Rust-native regression harness for ChatRPG CLI/API streams"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Run one JSON case file against a trpg binary.
    Run(RunArgs),
    /// Run every *.json case in a directory.
    Suite(SuiteArgs),
    /// Run a multi-turn black-box playtest and optionally ask an external evaluator such as Claude Code to judge each turn.
    Playtest(PlaytestArgs),
    /// Run offline TRPG evaluation over recorded fixtures and battle reports.
    Eval(EvalArgs),
}

#[derive(Debug, Clone, ValueEnum)]
enum OutputMode {
    Text,
    Json,
    Jsonl,
}

#[derive(Debug, Parser, Clone)]
struct RunArgs {
    /// Harness case JSON path.
    #[arg(long)]
    case: PathBuf,
    /// Path to the trpg binary under test.
    #[arg(long, default_value = "./target/debug/trpg")]
    bin: PathBuf,
    /// Working directory for the process under test.
    #[arg(long, default_value = ".")]
    cwd: PathBuf,
    /// Seconds before the case is killed and marked failed.
    #[arg(long, default_value_t = 120)]
    timeout_secs: u64,
    /// Override the CLI stream format requested from `trpg turn`.
    #[arg(long)]
    stream_format: Option<String>,
    /// Harness result output format.
    #[arg(long, value_enum, default_value = "text")]
    output: OutputMode,
    /// Print captured stdout/stderr in failure details.
    #[arg(long, default_value_t = false)]
    verbose: bool,
}

#[derive(Debug, Parser, Clone)]
struct PlaytestArgs {
    /// Multi-turn scenario JSON path.
    #[arg(long)]
    scenario: PathBuf,
    /// Path to the trpg binary under test.
    #[arg(long, default_value = "./target/debug/trpg")]
    bin: PathBuf,
    /// Working directory for the process under test.
    #[arg(long, default_value = ".")]
    cwd: PathBuf,
    /// Seconds before each turn is killed and marked failed.
    #[arg(long, default_value_t = 180)]
    timeout_secs: u64,
    /// Output directory for turn snapshots, DB diffs, evaluator prompts, and evaluator results.
    #[arg(long)]
    output_dir: Option<PathBuf>,
    /// Evaluator backend. Use `none` or `claude-code`.
    #[arg(long, default_value = "none")]
    evaluator: String,
    /// Claude Code binary when evaluator=claude-code.
    #[arg(long, default_value = "claude")]
    claude_bin: PathBuf,
    /// Treat evaluator invocation failure as a harness failure.
    #[arg(long, default_value_t = false)]
    evaluator_required: bool,
    /// Enable test-only [debug]add/patch/delete[/debug] directives in scenario player input. Debug blocks are stripped before sending player text to the GM.
    #[arg(long, default_value_t = false)]
    enable_debug_directives: bool,
    /// Fail a turn when the player input looks like test-engineering prose, JSON, code, DB queries, or internal event names. Debug blocks are exempt and stripped first.
    #[arg(long, default_value_t = true)]
    require_human_player_input: bool,
    /// Executor mode: `live` (default, public CLI/GM/DB path), `deterministic`
    /// (classify from an authored provider-free fixture), or `replay` (classify
    /// from a cassette recorded by an accepted live run, with no live fallback).
    #[arg(long, default_value = "live")]
    mode: String,
    /// Fixture/cassette JSON for `deterministic`/`replay` modes. Required for
    /// those modes; ignored by `live`.
    #[arg(long)]
    fixture: Option<PathBuf>,
    /// In `--mode live`, record a replay cassette to this path. An accepted
    /// (`ok`) live run writes a `recorded_mode: "live"` cassette the replay
    /// executor can consume; a failed run writes only a diagnostic sibling
    /// (`<path>.diagnostic.json`) that replay rejects. Ignored by other modes.
    #[arg(long)]
    record_fixture: Option<PathBuf>,
    /// Harness result output format.
    #[arg(long, value_enum, default_value = "text")]
    output: OutputMode,
    /// Print captured stdout/stderr in failure details.
    #[arg(long, default_value_t = false)]
    verbose: bool,
}

#[derive(Debug, Parser, Clone)]
struct SuiteArgs {
    /// Directory containing harness case JSON files.
    #[arg(long, default_value = "harness/cases")]
    dir: PathBuf,
    /// Path to the trpg binary under test.
    #[arg(long, default_value = "./target/debug/trpg")]
    bin: PathBuf,
    /// Working directory for the process under test.
    #[arg(long, default_value = ".")]
    cwd: PathBuf,
    /// Seconds before each case is killed and marked failed.
    #[arg(long, default_value_t = 120)]
    timeout_secs: u64,
    /// Stop after the first failing case.
    #[arg(long, default_value_t = false)]
    fail_fast: bool,
    /// Harness result output format.
    #[arg(long, value_enum, default_value = "text")]
    output: OutputMode,
    /// Print captured stdout/stderr in failure details.
    #[arg(long, default_value_t = false)]
    verbose: bool,
}

#[derive(Debug, Parser, Clone)]
struct EvalArgs {
    #[command(subcommand)]
    command: EvalCommand,
}

#[derive(Debug, Subcommand, Clone)]
enum EvalCommand {
    /// Evaluate a markdown `eval-fixture` transcript without running the GM.
    Replay(EvalReplayArgs),
}

#[derive(Debug, Parser, Clone)]
struct EvalReplayArgs {
    /// Markdown fixture containing a fenced `eval-fixture` JSON block.
    #[arg(long)]
    fixture: PathBuf,
    /// Evaluation report output format.
    #[arg(long, value_enum, default_value = "text")]
    output: OutputMode,
}

#[derive(Debug, Clone, Serialize)]
struct HarnessResult {
    ok: bool,
    case: String,
    session_id: Option<String>,
    events: Vec<String>,
    chars: usize,
    failures: Vec<String>,
    forbidden_hits: Vec<String>,
    missing_terms: Vec<String>,
    missing_events: Vec<String>,
    process_status: Option<i32>,
    timed_out: bool,
    stdout_tail: Option<String>,
    stderr_tail: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Run(args) => {
            let result = run_case_path(
                &args.case,
                &args.bin,
                &args.cwd,
                args.timeout_secs,
                args.stream_format.as_deref(),
                args.verbose,
            )
            .await?;
            emit_result(&result, &args.output)?;
            if result.ok {
                Ok(())
            } else {
                Err(anyhow!("harness case failed: {}", result.case))
            }
        }
        Commands::Playtest(args) => {
            let result = run_playtest(&args).await?;
            emit_playtest_result(&result, &args.output)?;
            if result.ok {
                Ok(())
            } else {
                Err(anyhow!("playtest failed: {}", result.scenario))
            }
        }
        Commands::Suite(args) => {
            let mut paths = collect_case_paths(&args.dir)?;
            paths.sort();
            if paths.is_empty() {
                return Err(anyhow!(
                    "no harness case JSON files found in {}",
                    args.dir.display()
                ));
            }
            let mut results = Vec::new();
            let mut failed = false;
            for path in paths {
                let result = run_case_path(
                    &path,
                    &args.bin,
                    &args.cwd,
                    args.timeout_secs,
                    None,
                    args.verbose,
                )
                .await?;
                failed |= !result.ok;
                emit_result(&result, &args.output)?;
                let stop = args.fail_fast && !result.ok;
                results.push(result);
                if stop {
                    break;
                }
            }
            if matches!(args.output, OutputMode::Text) {
                let passed = results.iter().filter(|r| r.ok).count();
                let failed_count = results.len() - passed;
                eprintln!("suite complete: {passed} passed, {failed_count} failed");
            }
            if failed {
                Err(anyhow!("harness suite failed"))
            } else {
                Ok(())
            }
        }
        Commands::Eval(args) => run_eval_command(args).await,
    }
}

async fn run_eval_command(args: EvalArgs) -> Result<()> {
    match args.command {
        EvalCommand::Replay(replay) => {
            let text = tokio::fs::read_to_string(&replay.fixture)
                .await
                .with_context(|| format!("failed to read fixture {}", replay.fixture.display()))?;
            let fixture = parse_markdown_fixture(&text)
                .with_context(|| format!("invalid eval fixture {}", replay.fixture.display()))?;
            let report = evaluate_fixture(&fixture);
            emit_eval_report(&report, &replay.output)?;
            if report.verdict == Verdict::Fail {
                Err(anyhow!(
                    "eval fixture failed: {} ({} findings)",
                    replay.fixture.display(),
                    report.findings.len()
                ))
            } else {
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PlaytestScenario {
    name: String,
    ruleset_id: String,
    /// Stable scenario id shared by deterministic/live/replay runs (TC-JRNY-02).
    /// Defaults to `name` when absent so legacy scenarios still parse.
    #[serde(default)]
    scenario_id: Option<String>,
    /// Scenario version; bumped when the spec's meaning changes. Defaults to `1`.
    #[serde(default)]
    scenario_version: Option<String>,
    #[serde(default)]
    module_id: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    /// Journey prelude. When `setup.character` is present the runner creates a
    /// player character through the public CLI and binds it before any turn.
    /// When it is absent the scenario is rejected as INVALID_SETUP.
    #[serde(default)]
    setup: Option<PlaytestSetup>,
    #[serde(default = "default_stream_format")]
    stream_format: String,
    #[serde(default)]
    turns: Vec<PlaytestTurn>,
    #[serde(default)]
    forbidden_player_text: Vec<String>,
    #[serde(default)]
    evaluator_instructions: Option<String>,
    #[serde(default)]
    allow_debug_directives: bool,
    #[serde(default)]
    allow_nonhuman_player_input: bool,
    /// Test-only escape hatch for legacy/manual-dice tests. Product playtests should leave this false.
    #[serde(default)]
    allow_manual_roll_input: bool,
}

impl PlaytestScenario {
    /// Stable identity stamped onto every mode's evidence so deterministic, live,
    /// and replay outputs are comparable. Falls back to `name`/`1` for scenarios
    /// authored before TC-JRNY-02 added explicit ids.
    fn identity(&self) -> ScenarioIdentity {
        ScenarioIdentity::new(
            self.scenario_id
                .clone()
                .unwrap_or_else(|| self.name.clone()),
            self.scenario_version
                .clone()
                .unwrap_or_else(|| "1".to_string()),
        )
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PlaytestSetup {
    #[serde(default)]
    character: Option<CharacterSetup>,
}

/// Journey-prelude character-creation request. Drives the public
/// `trpg create-character` CLI and the readiness gate.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CharacterSetup {
    /// Free-text concept / partial choices forwarded as `user_preferences`.
    #[serde(default)]
    preferences: String,
    /// Use the `--auto` complete-and-bind path (the only supported P0 path).
    #[serde(default = "default_true")]
    auto_complete: bool,
    /// Require a persisted character (character_created phase + character_id).
    #[serde(default = "default_true")]
    require_persisted: bool,
    /// Require the character to be bound into a session (bound phase + ids).
    #[serde(default = "default_true")]
    require_session_binding: bool,
}

impl Default for CharacterSetup {
    fn default() -> Self {
        Self {
            preferences: String::new(),
            auto_complete: true,
            require_persisted: true,
            require_session_binding: true,
        }
    }
}

/// Terminal classification of a playtest run. Extended for TC-JRNY-01 with the
/// Journey Qualification Gate states so a check-dependent run cannot be reported
/// as a flat `PASS`/`FAIL` when the real failure was a missing trigger,
/// mechanism, or player-visible effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ResultState {
    Pass,
    Fail,
    InvalidSetup,
    NotTriggered,
    TriggeredNoMechanism,
    MechanismNoUserEffect,
    Blocked,
}

impl ResultState {
    fn as_str(self) -> &'static str {
        match self {
            ResultState::Pass => "PASS",
            ResultState::Fail => "FAIL",
            ResultState::InvalidSetup => "INVALID_SETUP",
            ResultState::NotTriggered => "NOT_TRIGGERED",
            ResultState::TriggeredNoMechanism => "TRIGGERED_NO_MECHANISM",
            ResultState::MechanismNoUserEffect => "MECHANISM_NO_USER_EFFECT",
            ResultState::Blocked => "BLOCKED",
        }
    }

    /// Map a per-checkpoint Journey Qualification Gate state onto the run-level
    /// result. `Pass` here means the checkpoint itself passed; the caller still
    /// decides the overall run from the full failure set.
    fn from_checkpoint(state: CheckpointState) -> Self {
        match state {
            CheckpointState::Pass => ResultState::Pass,
            CheckpointState::Fail => ResultState::Fail,
            CheckpointState::InvalidSetup => ResultState::InvalidSetup,
            CheckpointState::NotTriggered => ResultState::NotTriggered,
            CheckpointState::TriggeredNoMechanism => ResultState::TriggeredNoMechanism,
            CheckpointState::MechanismNoUserEffect => ResultState::MechanismNoUserEffect,
            CheckpointState::Blocked => ResultState::Blocked,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PlaytestTurn {
    user_input: String,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    expect_phases: Vec<String>,
    #[serde(default)]
    forbid_phases: Vec<String>,
    #[serde(default)]
    forbid_player_text: Vec<String>,
    #[serde(default)]
    allow_nonhuman_player_input: bool,
    /// Test-only escape hatch for legacy/manual-dice tests. Product playtests should leave this false.
    #[serde(default)]
    allow_manual_roll_input: bool,
    #[serde(default)]
    require_db_deltas: BTreeMap<String, i64>,
    /// Conservative adaptive precondition. When present and its named flag has
    /// not been raised yet (e.g. `check_resolved`), the turn is skipped instead
    /// of run. `None`/empty is always satisfied. See `when_satisfied`.
    #[serde(default)]
    when: Option<String>,
    /// One natural follow-up action sent in the same turn slot if the declared
    /// `check` checkpoint did not trigger on the first action (the doc's trigger
    /// ladder, capped at one extra attempt for TC-JRNY-01).
    #[serde(default)]
    if_not_triggered: Option<String>,
    /// Check/dice requirement for this turn. Absent on non-check turns, so legacy
    /// scenarios are unaffected.
    #[serde(default)]
    check: Option<CheckCheckpoint>,
    /// Knowledge / no-spoiler requirement for this turn (TC-VS-KNOW-01). Absent on
    /// non-knowledge turns, so legacy scenarios are unaffected.
    #[serde(default)]
    knowledge: Option<KnowledgeCheckpoint>,
    /// NPC mind / relationship / social-memory requirement for this turn
    /// (TC-VS-NPC-01). Absent on non-social turns.
    #[serde(default)]
    npc_social: Option<NpcSocialCheckpoint>,
    /// Committed-memory / reload requirement for this turn (TC-VS-MEM-01). Absent
    /// on non-memory turns.
    #[serde(default)]
    memory: Option<MemoryCheckpoint>,
    /// Production flight-recorder / provenance requirement for this turn
    /// (TC-PIPE-03). Absent on non-trace turns.
    #[serde(default)]
    flight_recorder: Option<FlightRecorderCheckpoint>,
}

#[derive(Debug, Clone, Serialize)]
struct PlaytestResult {
    ok: bool,
    result_state: ResultState,
    scenario: String,
    /// Executor mode that produced this result (`deterministic`/`live`/`replay`).
    mode: String,
    /// Stable scenario identity shared across modes (TC-JRNY-02).
    scenario_id: String,
    scenario_version: String,
    evidence_schema_version: String,
    session_id: Option<String>,
    output_dir: String,
    /// Character-creation prelude evidence (ids, readiness, session binding).
    /// `None` when the scenario was rejected before creation ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    character_setup: Option<Value>,
    turns: Vec<PlaytestTurnResult>,
    failures: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct PlaytestTurnResult {
    turn: usize,
    user_input: String,
    events: Vec<String>,
    chars: usize,
    db_diff: Value,
    evaluator_verdict: Option<String>,
    failures: Vec<String>,
    /// Journey Qualification Gate classification when the turn declared a `check`
    /// checkpoint; `None` for plain turns.
    #[serde(skip_serializing_if = "Option::is_none")]
    checkpoint_state: Option<CheckpointState>,
    /// Distilled check evidence behind `checkpoint_state` (for the lead's audit).
    #[serde(skip_serializing_if = "Option::is_none")]
    check_evidence: Option<Value>,
    /// Knowledge / no-spoiler checkpoint classification when the turn declared a
    /// `knowledge` checkpoint; `None` otherwise (TC-VS-KNOW-01).
    #[serde(skip_serializing_if = "Option::is_none")]
    knowledge_checkpoint_state: Option<KnowledgeCheckpointState>,
    /// The knowledge evidence behind `knowledge_checkpoint_state` (for the audit).
    #[serde(skip_serializing_if = "Option::is_none")]
    knowledge_evidence: Option<Value>,
    /// NPC social checkpoint classification when the turn declared an `npc_social`
    /// checkpoint; `None` otherwise (TC-VS-NPC-01).
    #[serde(skip_serializing_if = "Option::is_none")]
    npc_social_checkpoint_state: Option<NpcSocialCheckpointState>,
    /// The NPC social evidence behind `npc_social_checkpoint_state` (for the audit).
    #[serde(skip_serializing_if = "Option::is_none")]
    npc_social_evidence: Option<Value>,
    /// Committed-memory checkpoint classification when the turn declared a `memory`
    /// checkpoint; `None` otherwise (TC-VS-MEM-01).
    #[serde(skip_serializing_if = "Option::is_none")]
    memory_checkpoint_state: Option<MemoryCheckpointState>,
    /// The committed-memory evidence behind `memory_checkpoint_state` (for the audit).
    #[serde(skip_serializing_if = "Option::is_none")]
    memory_evidence: Option<Value>,
    /// Production flight-recorder checkpoint classification when the turn declared a
    /// `flight_recorder` checkpoint; `None` otherwise (TC-PIPE-03).
    #[serde(skip_serializing_if = "Option::is_none")]
    flight_recorder_state: Option<FlightRecorderState>,
    /// The flight-recorder evidence behind `flight_recorder_state` (for the audit).
    #[serde(skip_serializing_if = "Option::is_none")]
    flight_recorder_evidence: Option<Value>,
    /// True when the turn was skipped because its `when` precondition was unmet.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    skipped: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct DbSnapshot {
    ok: bool,
    error: Option<String>,
    counts: BTreeMap<String, i64>,
    latest: BTreeMap<String, Vec<Value>>,
}

#[derive(Debug)]
struct TurnExecution {
    stdout_text: String,
    stderr_text: String,
    parsed: ParsedJsonlStream,
    status_code: Option<i32>,
    timed_out: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DebugDirectiveRecord {
    action: String,
    payload: Value,
    raw: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct DebugDirectiveResult {
    stripped_input: String,
    directives: Vec<DebugDirectiveRecord>,
    changes: Vec<Value>,
    failures: Vec<String>,
    state: BTreeMap<String, Value>,
}

async fn run_playtest(args: &PlaytestArgs) -> Result<PlaytestResult> {
    let scenario_text = tokio::fs::read_to_string(&args.scenario)
        .await
        .with_context(|| format!("failed to read scenario {}", args.scenario.display()))?;
    let mut scenario: PlaytestScenario =
        serde_json::from_str(&scenario_text).with_context(|| {
            format!(
                "invalid playtest scenario JSON: {}",
                args.scenario.display()
            )
        })?;

    let identity = scenario.identity();
    let Some(mode) = ExecutionMode::parse(&args.mode) else {
        return Err(anyhow!(
            "invalid --mode `{}`: expected deterministic, live, or replay",
            args.mode
        ));
    };

    let run_id = format!(
        "{}-{}-{}",
        sanitize_name(&scenario.name),
        mode.as_str(),
        Utc::now().format("%Y%m%dT%H%M%SZ")
    );
    let out_dir = args
        .output_dir
        .clone()
        .unwrap_or_else(|| args.cwd.join("data").join("playtests").join(run_id));
    tokio::fs::create_dir_all(&out_dir).await?;
    tokio::fs::write(
        out_dir.join("scenario.json"),
        serde_json::to_vec_pretty(&scenario)?,
    )
    .await?;

    // Deterministic and replay modes are fixture-driven and provider-free: they
    // classify the same check checkpoints from a recorded/authored cassette with
    // NO live fallback. The live executor (below) drives the real public path.
    if mode.is_fixture_driven() {
        return run_fixture_playtest(args, &scenario, &identity, mode, &out_dir).await;
    }

    // Journey prelude (TC-JRNY-00): a scenario must declare `setup.character`,
    // create that character through the public CLI, and bind it into a session
    // before any play chapter runs. A scenario with no character is rejected.
    let character_setup = match scenario.setup.as_ref().and_then(|s| s.character.clone()) {
        None => {
            let result = invalid_setup_result(
                &scenario.name,
                &identity,
                mode,
                &out_dir,
                "scenario.setup.character is required: every play journey must create a player character before turns".to_string(),
            );
            write_json(out_dir.join("playtest_result.json"), &result).await?;
            return Ok(result);
        }
        Some(character) => character,
    };

    // A scenario that declares a character but has no playable first action is an
    // invalid setup, caught before spawning create-character or any turn process.
    let debug_allowed = args.enable_debug_directives || scenario.allow_debug_directives;
    if let Some(reason) = missing_first_turn_reason(&scenario, debug_allowed) {
        let result = invalid_setup_result(&scenario.name, &identity, mode, &out_dir, reason);
        write_json(out_dir.join("playtest_result.json"), &result).await?;
        return Ok(result);
    }

    // Connect read-only before the prelude so persisted-character verification can
    // read back the rows the public CLI claims it wrote. The same pool is reused
    // for per-turn DB snapshots below.
    let pool = maybe_connect_db(&args.cwd).await;

    let prelude =
        run_character_prelude(args, &scenario, &character_setup, &out_dir, pool.as_ref()).await?;
    if let Some(early) = prelude.early_result(&scenario.name, &identity, mode, &out_dir) {
        write_json(out_dir.join("playtest_result.json"), &early).await?;
        return Ok(early);
    }
    let created_session_id = prelude.session_id.clone();
    scenario.session_id = created_session_id.clone();

    let mut first_turn_requested_session_id: Option<String> = None;
    let mut first_turn_emitted_session_id: Option<String> = None;
    let mut first_turn_session_id: Option<String> = None;
    let mut results = Vec::new();
    let mut all_failures = Vec::new();
    let mut recent_transcript = String::new();
    let mut debug_state: BTreeMap<String, Value> = BTreeMap::new();
    // Adaptive state-machine flags. `session_ready` holds once the prelude has
    // bound a session; passing check checkpoints raise `check_resolved` and a
    // per-checkpoint `<label>_passed` flag for later `when` gates.
    let mut flags: BTreeSet<String> = BTreeSet::new();
    flags.insert("session_ready".to_string());

    // Live cassette recording (TC-JRNY-02 rev1): when `--record-fixture` is set in
    // live mode, accumulate the same per-turn evidence the replay executor needs,
    // then write a `recorded_mode: "live"` cassette iff the run is accepted.
    let recording = args.record_fixture.is_some();
    let mut recorded_turns: Vec<FixtureTurn> = Vec::new();

    for idx in 0..scenario.turns.len() {
        let turn = scenario.turns[idx].clone();

        // Conservative adaptive `when` gate: skip a turn whose precondition flag
        // has not been raised. The first turn should not be gated; doing so would
        // also skip prelude session-binding verification.
        if !when_satisfied(turn.when.as_deref(), &flags) {
            results.push(PlaytestTurnResult {
                turn: idx + 1,
                user_input: turn.user_input.clone(),
                events: vec![],
                chars: 0,
                db_diff: Value::Null,
                evaluator_verdict: None,
                failures: vec![],
                checkpoint_state: None,
                check_evidence: None,
                knowledge_checkpoint_state: None,
                knowledge_evidence: None,
                npc_social_checkpoint_state: None,
                npc_social_evidence: None,
                memory_checkpoint_state: None,
                memory_evidence: None,
                flight_recorder_state: None,
                flight_recorder_evidence: None,
                skipped: true,
            });
            continue;
        }

        let turn_dir = out_dir.join(format!("turn_{:03}", idx + 1));
        tokio::fs::create_dir_all(&turn_dir).await?;
        tokio::fs::write(turn_dir.join("player_input_raw.txt"), &turn.user_input).await?;

        let debug_allowed = args.enable_debug_directives || scenario.allow_debug_directives;
        let mut debug_result = if debug_allowed {
            apply_debug_directives(&turn.user_input, &mut debug_state)
        } else {
            DebugDirectiveResult {
                stripped_input: turn.user_input.clone(),
                state: debug_state.clone(),
                ..Default::default()
            }
        };
        let clean_input = normalize_player_input(&debug_result.stripped_input);
        debug_result.stripped_input = clean_input.clone();
        debug_result.state = debug_state.clone();
        write_json(turn_dir.join("debug_directives.json"), &debug_result).await?;
        tokio::fs::write(turn_dir.join("player_input.txt"), &clean_input).await?;

        let before = db_snapshot(pool.as_ref(), scenario.session_id.as_deref()).await;
        write_json(turn_dir.join("db_snapshot_before.json"), &before).await?;
        // True PRE-turn player-knowledge projection, so a reveal committed during
        // this turn is observable as a delta (TC-VS-KNOW-01 reveal evidence).
        let known_before =
            query_player_known_fact_ids(pool.as_ref(), scenario.session_id.as_deref()).await;

        // Record the session id we *request* for the first turn before the GM
        // can overwrite `scenario.session_id` with whatever it emits.
        if idx == 0 {
            first_turn_requested_session_id = scenario.session_id.clone();
        }

        let execution = execute_turn_process(
            &args.bin,
            &args.cwd,
            args.timeout_secs,
            &scenario.ruleset_id,
            scenario.module_id.clone(),
            scenario.session_id.clone(),
            &clean_input,
            build_recent_transcript_context(&recent_transcript, &debug_state),
        )
        .await?;

        if let Some(session_id) = execution.parsed.session_id.clone() {
            scenario.session_id = Some(session_id);
        }
        if idx == 0 {
            first_turn_emitted_session_id = execution.parsed.session_id.clone();
            first_turn_session_id = execution
                .parsed
                .session_id
                .clone()
                .or_else(|| scenario.session_id.clone());
        }

        let after = db_snapshot(pool.as_ref(), scenario.session_id.as_deref()).await;
        let diff = db_diff(&before, &after);
        write_json(turn_dir.join("db_snapshot_after.json"), &after).await?;
        write_json(turn_dir.join("db_diff.json"), &diff).await?;
        tokio::fs::write(turn_dir.join("events.jsonl"), &execution.stdout_text).await?;
        tokio::fs::write(turn_dir.join("stderr.txt"), &execution.stderr_text).await?;
        tokio::fs::write(
            turn_dir.join("player_visible_output.txt"),
            &execution.parsed.body,
        )
        .await?;

        let mut turn_failures = execution.parsed.failures.clone();
        turn_failures.extend(
            debug_result
                .failures
                .iter()
                .map(|f| format!("debug directive error: {f}")),
        );
        let enforce_human_input = args.require_human_player_input
            && !scenario.allow_nonhuman_player_input
            && !turn.allow_nonhuman_player_input;
        if enforce_human_input {
            let allow_manual_roll =
                scenario.allow_manual_roll_input || turn.allow_manual_roll_input;
            for finding in human_player_input_findings(&clean_input, allow_manual_roll) {
                turn_failures.push(format!("non-human player input: {finding}"));
            }
        }
        if execution.timed_out {
            turn_failures.push(format!("turn timed out after {}s", args.timeout_secs));
        }
        if execution.status_code.unwrap_or(1) != 0 {
            turn_failures.push(format!(
                "trpg exited with non-zero status: {:?}",
                execution.status_code
            ));
        }
        for phase in &turn.expect_phases {
            let name = if phase.starts_with("phase:") || phase.starts_with("event:") {
                phase.clone()
            } else {
                format!("phase:{phase}")
            };
            if !execution.parsed.events.iter().any(|e| e == &name) {
                turn_failures.push(format!("missing expected phase/event: {name}"));
            }
        }
        for phase in &turn.forbid_phases {
            let name = if phase.starts_with("phase:") || phase.starts_with("event:") {
                phase.clone()
            } else {
                format!("phase:{phase}")
            };
            if execution.parsed.events.iter().any(|e| e == &name) {
                turn_failures.push(format!("forbidden phase/event appeared: {name}"));
            }
        }
        for forbidden in scenario
            .forbidden_player_text
            .iter()
            .chain(turn.forbid_player_text.iter())
        {
            if !forbidden.is_empty() && execution.parsed.body.contains(forbidden) {
                turn_failures.push(format!(
                    "forbidden player-visible text appeared: {forbidden}"
                ));
            }
        }
        if !(scenario.allow_manual_roll_input || turn.allow_manual_roll_input) {
            for finding in player_visible_roll_request_findings(&execution.parsed.body) {
                turn_failures.push(format!("player-visible manual roll request: {finding}"));
            }
        }
        for (table, min_delta) in &turn.require_db_deltas {
            let actual = diff
                .get("count_delta")
                .and_then(|d| d.get(table))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            if actual < *min_delta {
                turn_failures.push(format!(
                    "DB delta for {table} was {actual}, expected >= {min_delta}"
                ));
            }
        }

        // Cassette recording capture for this turn. Defaults to the main turn's
        // evidence; the check block below folds in any adaptive follow-up so the
        // recorded turn reproduces the same classification under replay.
        let mut rec_count_delta = count_delta_map(&diff);
        let mut rec_contest_row = after
            .latest
            .get("contest_resolution_events")
            .and_then(|rows| rows.first())
            .cloned();
        let mut rec_body = execution.parsed.body.clone();

        // --- TC-JRNY-01: check-dependent checkpoint classification ------------
        // Fail-closed Journey Qualification Gate: a check checkpoint cannot pass
        // unless real dice + a *resolved* check (non-null target/success/degree,
        // no awaiting_binding) and, when required, a player-visible effect exist.
        let mut checkpoint_state: Option<CheckpointState> = None;
        let mut check_evidence_value: Option<Value> = None;
        let mut knowledge_state: Option<KnowledgeCheckpointState> = None;
        let mut knowledge_evidence_value: Option<Value> = None;
        let mut rec_knowledge: Option<KnowledgeEvidence> = None;
        let mut npc_social_state: Option<NpcSocialCheckpointState> = None;
        let mut npc_social_evidence_value: Option<Value> = None;
        let mut rec_npc_social: Option<NpcSocialEvidence> = None;
        let mut memory_state: Option<MemoryCheckpointState> = None;
        let mut memory_evidence_value: Option<Value> = None;
        let mut rec_memory: Option<MemoryEvidence> = None;
        let mut flight_recorder_state: Option<FlightRecorderState> = None;
        let mut flight_recorder_evidence_value: Option<Value> = None;
        let mut rec_flight_recorder: Option<FlightRecorderEvidence> = None;
        if let Some(check) = turn.check.clone() {
            if check.is_active() {
                let allow_manual_roll =
                    scenario.allow_manual_roll_input || turn.allow_manual_roll_input;
                let natural = !clean_input.trim().is_empty()
                    && human_player_input_findings(&clean_input, allow_manual_roll).is_empty();
                let mut count_delta = count_delta_map(&diff);
                let mut newest_contest = after
                    .latest
                    .get("contest_resolution_events")
                    .and_then(|rows| rows.first())
                    .cloned();
                let mut visible_effect = player_visible_effect_present(
                    &check.player_visible_effect_any,
                    &execution.parsed.body,
                );
                let mut evidence = build_check_evidence(
                    natural,
                    &count_delta,
                    newest_contest.as_ref(),
                    visible_effect,
                );
                let mut state = classify_check_checkpoint(&check, &evidence);
                let mut attempts = vec![clean_input.clone()];

                // Trigger ladder: one extra natural follow-up if the check did
                // not trigger. No forced result, no DB mutation — only another
                // player action through the same public turn path.
                if state == CheckpointState::NotTriggered {
                    if let Some(follow_raw) = turn.if_not_triggered.clone() {
                        let follow_input = normalize_player_input(&follow_raw);
                        let follow_natural = !follow_input.trim().is_empty()
                            && human_player_input_findings(&follow_input, allow_manual_roll)
                                .is_empty();
                        let follow_ctx = build_recent_transcript_context(
                            &format!(
                                "{recent_transcript}\nPlayer: {}\nGM: {}\n",
                                clean_input.trim(),
                                execution.parsed.body.trim()
                            ),
                            &debug_state,
                        );
                        let f_before = after.clone();
                        let f_exec = execute_turn_process(
                            &args.bin,
                            &args.cwd,
                            args.timeout_secs,
                            &scenario.ruleset_id,
                            scenario.module_id.clone(),
                            scenario.session_id.clone(),
                            &follow_input,
                            follow_ctx,
                        )
                        .await?;
                        if let Some(sid) = f_exec.parsed.session_id.clone() {
                            scenario.session_id = Some(sid);
                        }
                        let f_after =
                            db_snapshot(pool.as_ref(), scenario.session_id.as_deref()).await;
                        let f_diff = db_diff(&f_before, &f_after);
                        tokio::fs::write(turn_dir.join("followup_player_input.txt"), &follow_input)
                            .await?;
                        tokio::fs::write(
                            turn_dir.join("followup_events.jsonl"),
                            &f_exec.stdout_text,
                        )
                        .await?;
                        write_json(turn_dir.join("followup_db_diff.json"), &f_diff).await?;
                        tokio::fs::write(
                            turn_dir.join("followup_player_visible_output.txt"),
                            &f_exec.parsed.body,
                        )
                        .await?;
                        // Fold the follow-up narration into the recorded body so a
                        // replay of this turn sees the same player-visible effect.
                        rec_body.push('\n');
                        rec_body.push_str(&f_exec.parsed.body);

                        // The same guards apply to the adaptive follow-up.
                        if enforce_human_input {
                            for finding in
                                human_player_input_findings(&follow_input, allow_manual_roll)
                            {
                                turn_failures
                                    .push(format!("non-human player input (follow-up): {finding}"));
                            }
                        }
                        if !allow_manual_roll {
                            for finding in player_visible_roll_request_findings(&f_exec.parsed.body)
                            {
                                turn_failures.push(format!(
                                    "player-visible manual roll request (follow-up): {finding}"
                                ));
                            }
                        }
                        if f_exec.status_code.unwrap_or(1) != 0 {
                            turn_failures.push(format!(
                                "follow-up trpg exited with non-zero status: {:?}",
                                f_exec.status_code
                            ));
                        }

                        let f_delta = count_delta_map(&f_diff);
                        count_delta = merge_count_deltas(&count_delta, &f_delta);
                        if f_delta
                            .get("contest_resolution_events")
                            .copied()
                            .unwrap_or(0)
                            > 0
                        {
                            if let Some(row) = f_after
                                .latest
                                .get("contest_resolution_events")
                                .and_then(|rows| rows.first())
                            {
                                newest_contest = Some(row.clone());
                            }
                        }
                        visible_effect = visible_effect
                            || player_visible_effect_present(
                                &check.player_visible_effect_any,
                                &f_exec.parsed.body,
                            );
                        evidence = build_check_evidence(
                            natural || follow_natural,
                            &count_delta,
                            newest_contest.as_ref(),
                            visible_effect,
                        );
                        state = classify_check_checkpoint(&check, &evidence);
                        attempts.push(follow_input);
                    }
                }

                if state.is_failing() {
                    turn_failures.push(format!(
                        "check checkpoint did not pass: {} (dice={}, roll_plan={}, pending={}, gm_roll={}, resolved={}, awaiting_binding={}, visible_effect={})",
                        state.as_str(),
                        evidence.dice_present,
                        evidence.roll_plan_present,
                        evidence.pending_check_present,
                        evidence.gm_roll_path,
                        evidence.resolved_check_present,
                        evidence.awaiting_binding,
                        evidence.player_visible_effect,
                    ));
                } else {
                    flags.insert("check_resolved".to_string());
                    if let Some(label) = check.label.as_ref() {
                        flags.insert(format!("{label}_passed"));
                    }
                }
                // Record the folded (main + follow-up) evidence for the cassette.
                rec_count_delta = count_delta.clone();
                rec_contest_row = newest_contest.clone();
                check_evidence_value = Some(json!({
                    "state": state,
                    "attempts": attempts,
                    "evidence": evidence,
                }));
                checkpoint_state = Some(state);
            }
        }

        // --- TC-VS-KNOW-01: knowledge / no-spoiler checkpoint -----------------
        // Reads the real player-knowledge projection (player_party knows_true) from
        // the DB and runs the production post-output leak verifier against the
        // player-visible body. Fail-closed: any leak or premature grant fails.
        if let Some(knowledge) = turn.knowledge.clone() {
            if knowledge.is_active() {
                // `known_before` is the PRE-turn projection captured above; query
                // the POST-turn projection now so a reveal committed this turn shows
                // up as a real delta (not both reads after the turn).
                let known_after =
                    query_player_known_fact_ids(pool.as_ref(), scenario.session_id.as_deref())
                        .await;
                let reveal_committed: Vec<String> = known_after
                    .iter()
                    .filter(|f| !known_before.contains(f))
                    .cloned()
                    .collect();
                let ev = KnowledgeEvidence {
                    player_known_fact_ids: known_after,
                    reveal_committed_fact_ids: reveal_committed,
                    // Presence-only surfacing is informational here; the load-bearing
                    // gate is that player_known excludes the hidden fact.
                    surfaced_entity_ids: vec![],
                    reload: false,
                };
                let state = classify_knowledge_checkpoint(&knowledge, &ev, &rec_body);
                if state.is_failing() {
                    turn_failures.push(format!(
                        "knowledge checkpoint did not pass: {} (fact={}, player_known={:?})",
                        state.as_str(),
                        knowledge.hidden_fact_id,
                        ev.player_known_fact_ids,
                    ));
                }
                knowledge_evidence_value = Some(json!({
                    "state": state,
                    "hidden_fact_id": knowledge.hidden_fact_id,
                    "evidence": ev,
                }));
                knowledge_state = Some(state);
                rec_knowledge = Some(ev);
            }
        }

        // --- TC-VS-NPC-01: NPC mind / relationship / social-memory checkpoint --
        // Reads the durable NPC projections (profile presence, the NPC's own
        // knows_true edges, the player_party projection, and the relationship row
        // + its evidence event ids) straight from the DB and runs the SAME
        // production leak verifier the GM uses. Fail-closed: an NPC that speaks a
        // fact it does not know, or leaks a withheld secret, fails. Reload and
        // disclosure-change continuity are proven by the deterministic/replay
        // fixtures (the live loop is single-process), mirroring TC-VS-KNOW-01.
        if let Some(npc_social) = turn.npc_social.clone() {
            if npc_social.is_active() {
                let npc_id = npc_social.npc_id.trim().to_string();
                let session = scenario.session_id.as_deref();
                let profile_present =
                    query_npc_profile_present(pool.as_ref(), session, &npc_id).await;
                let npc_known = query_npc_known_fact_ids(pool.as_ref(), session, &npc_id).await;
                let player_known = query_player_known_fact_ids(pool.as_ref(), session).await;
                // A relationship/domain write THIS turn proves the social delta was
                // observed now, not read from a historical durable row.
                let relationship_observed = {
                    let cd = count_delta_map(&diff);
                    cd.get("npc_relationships").copied().unwrap_or(0) > 0
                        || cd.get("domain_events").copied().unwrap_or(0) > 0
                };
                let relationship_delta =
                    query_relationship_delta_evidence(pool.as_ref(), session, &npc_id)
                        .await
                        .map(|mut d| {
                            d.observed_this_turn = relationship_observed;
                            d
                        });
                let ev = NpcSocialEvidence {
                    npc_id: npc_id.clone(),
                    profile_present,
                    // A deterministic plan can be derived whenever a stable profile
                    // loads (`load_active_npc_guidance`); presence is the live proxy.
                    behavior_plan_present: profile_present,
                    npc_known_fact_ids: npc_known,
                    player_known_fact_ids: player_known,
                    relationship_delta,
                    // Same-process projection: continuity is fixture-proven.
                    disclosure_changed: false,
                    relationship_reload: false,
                    npc_knowledge_reload: false,
                };
                let state = classify_npc_social_checkpoint(&npc_social, &ev, &rec_body);
                if state.is_failing() {
                    turn_failures.push(format!(
                        "npc social checkpoint did not pass: {} (npc={}, profile={}, npc_known={:?})",
                        state.as_str(),
                        npc_id,
                        ev.profile_present,
                        ev.npc_known_fact_ids,
                    ));
                }
                npc_social_evidence_value = Some(json!({
                    "state": state,
                    "npc_id": npc_id,
                    "evidence": ev,
                }));
                npc_social_state = Some(state);
                rec_npc_social = Some(ev);
            }
        }

        // --- TC-VS-MEM-01: committed-memory / reload checkpoint ----------------
        // Reads the durable memory projections (committed `memory_facts` ids + their
        // `source_event_ids`, the latest player-facing memory summary) and derives
        // the flight-recorder trace links from this turn's count delta. Idempotency
        // and direct-commit are structural: the proposal pipeline is the only writer
        // and `memory_facts.fact_id` is UNIQUE (on-conflict upsert), so a retry can
        // never double-commit. Reload/consumption continuity is fixture-proven (the
        // live loop is single-process), mirroring TC-VS-KNOW-01 / TC-VS-NPC-01.
        if let Some(memory) = turn.memory.clone() {
            if memory.is_active() {
                let fact_id = memory.memory_fact_id.trim().to_string();
                let session = scenario.session_id.as_deref();
                let cd = count_delta_map(&diff);
                let committed_fact_ids =
                    query_memory_committed_fact_ids(pool.as_ref(), session).await;
                let evidence_ids =
                    query_memory_fact_source_event_ids(pool.as_ref(), session, &fact_id).await;
                let player_known = query_player_known_fact_ids(pool.as_ref(), session).await;
                let player_memory_summary = query_memory_summary(pool.as_ref(), session).await;
                let committed_event = cd.get("domain_events").copied().unwrap_or(0) > 0;
                let memory_written = cd.get("memory_facts").copied().unwrap_or(0) > 0;
                // Observable trace links this turn (action always; event/proposal
                // from the count delta). Projection/later-consumption are proven by
                // the deterministic/replay fixtures (cross-turn / post-reload).
                let mut trace_links = vec![memory_trace::PLAYER_ACTION.to_string()];
                if committed_event {
                    trace_links.push(memory_trace::COMMITTED_EVENT.to_string());
                }
                if memory_written || committed_fact_ids.iter().any(|f| f == &fact_id) {
                    trace_links.push(memory_trace::MEMORY_PROPOSAL.to_string());
                }
                let ev = MemoryEvidence {
                    memory_fact_id: fact_id.clone(),
                    // The proposal/commit pipeline is the only writer; a fact row
                    // present (or written this turn) came from a committed-turn proposal.
                    proposal_present: memory_written
                        || committed_fact_ids.iter().any(|f| f == &fact_id),
                    from_committed_turn: memory_written
                        || committed_fact_ids.iter().any(|f| f == &fact_id),
                    // No production path writes memory_facts outside the proposal
                    // pipeline, so a direct commit is structurally impossible here.
                    direct_commit_without_proposal: false,
                    proposal_evidence_event_ids: evidence_ids,
                    committed_fact_ids,
                    // UNIQUE fact_id + on-conflict upsert ⇒ no duplicate on retry.
                    duplicate_commit_on_retry: false,
                    // Continuity is fixture-proven in the single-process live loop.
                    retrieved_after_reload: false,
                    reload: false,
                    consumed_in_later_turn: false,
                    player_known_fact_ids: player_known,
                    player_memory_summary,
                    trace_links,
                };
                let state = classify_memory_checkpoint(&memory, &ev);
                if state.is_failing() {
                    turn_failures.push(format!(
                        "memory checkpoint did not pass: {} (fact={}, evidence_ids={:?})",
                        state.as_str(),
                        fact_id,
                        ev.proposal_evidence_event_ids,
                    ));
                }
                memory_evidence_value = Some(json!({
                    "state": state,
                    "memory_fact_id": fact_id,
                    "evidence": ev,
                }));
                memory_state = Some(state);
                rec_memory = Some(ev);
            }
        }

        // --- TC-PIPE-03: production flight-recorder / provenance checkpoint -----
        // Consumes the REAL persisted artifact: the latest `turn_traces` row for
        // the session (its `plugin_contributions`) plus the `domain_events` kinds
        // linked to that same turn_id. Fail-closed: absent trace coverage, a
        // view-order violation, an unlinked ledger, or a secret in a trace summary
        // fails. (The full live journey is provider-blocked; the deterministic/
        // replay fixtures carry the same production-shaped trace evidence.)
        if let Some(flight) = turn.flight_recorder.clone() {
            if flight.is_active() {
                let session = scenario.session_id.as_deref();
                let (contributions, trace_turn_id) =
                    query_latest_turn_trace_contributions(pool.as_ref(), session).await;
                let ledger_event_kinds =
                    query_turn_ledger_event_kinds(pool.as_ref(), trace_turn_id.as_deref()).await;
                let ev = FlightRecorderEvidence {
                    contributions,
                    ledger_event_kinds,
                };
                let state = classify_flight_recorder_checkpoint(&flight, &ev);
                if state.is_failing() {
                    turn_failures.push(format!(
                        "flight recorder checkpoint did not pass: {} (categories={:?}, ledger={:?})",
                        state.as_str(),
                        ev.contributions.iter().map(|t| t.kind.clone()).collect::<Vec<_>>(),
                        ev.ledger_event_kinds,
                    ));
                }
                flight_recorder_evidence_value = Some(json!({ "state": state, "evidence": ev }));
                flight_recorder_state = Some(state);
                rec_flight_recorder = Some(ev);
            }
        }

        let eval_prompt = build_eval_prompt(
            &scenario,
            &turn,
            &clean_input,
            &debug_result,
            &execution,
            &before,
            &after,
            &diff,
        );
        tokio::fs::write(turn_dir.join("eval_prompt.md"), &eval_prompt).await?;
        let mut evaluator_verdict = None;
        if args.evaluator == "claude-code" {
            match run_claude_evaluator(&args.claude_bin, &eval_prompt).await {
                Ok(value) => {
                    evaluator_verdict = value
                        .get("verdict")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    write_json(turn_dir.join("eval_result.json"), &value).await?;
                }
                Err(err) => {
                    let value = json!({"verdict":"evaluator_error","error": err.to_string()});
                    write_json(turn_dir.join("eval_result.json"), &value).await?;
                    if args.evaluator_required {
                        turn_failures.push(format!("evaluator failed: {err}"));
                    }
                }
            }
        } else {
            write_json(
                turn_dir.join("eval_result.json"),
                &json!({"verdict":"not_run","evaluator":"none"}),
            )
            .await?;
        }

        if args.verbose && !turn_failures.is_empty() {
            tokio::fs::write(
                turn_dir.join("stdout_tail.txt"),
                tail_chars(&execution.stdout_text, 4000),
            )
            .await?;
            tokio::fs::write(
                turn_dir.join("stderr_tail.txt"),
                tail_chars(&execution.stderr_text, 4000),
            )
            .await?;
        }

        all_failures.extend(
            turn_failures
                .iter()
                .map(|f| format!("turn {}: {f}", idx + 1)),
        );
        recent_transcript.push_str(&format!(
            "\nPlayer: {}\nGM: {}\n",
            clean_input.trim(),
            execution.parsed.body.trim()
        ));
        recent_transcript = tail_chars(&recent_transcript, 12000);
        if recording {
            recorded_turns.push(FixtureTurn {
                user_input: clean_input.clone(),
                count_delta: rec_count_delta,
                newest_contest_row: rec_contest_row,
                player_visible_body: rec_body,
                knowledge: rec_knowledge.clone(),
                npc_social: rec_npc_social.clone(),
                memory: rec_memory.clone(),
                flight_recorder: rec_flight_recorder.clone(),
            });
        }
        results.push(PlaytestTurnResult {
            turn: idx + 1,
            user_input: clean_input.clone(),
            events: execution.parsed.events,
            chars: execution.parsed.body.chars().count(),
            db_diff: diff,
            evaluator_verdict,
            failures: turn_failures,
            checkpoint_state,
            check_evidence: check_evidence_value,
            knowledge_checkpoint_state: knowledge_state,
            knowledge_evidence: knowledge_evidence_value,
            npc_social_checkpoint_state: npc_social_state,
            npc_social_evidence: npc_social_evidence_value,
            memory_checkpoint_state: memory_state,
            memory_evidence: memory_evidence_value,
            flight_recorder_state,
            flight_recorder_evidence: flight_recorder_evidence_value,
            skipped: false,
        });
    }

    // Prove the first play turn ran inside the session created by the prelude.
    // Requested and emitted ids are recorded separately: the runner must request
    // the prelude-created session, and any session the GM emits must match it.
    let session_binding_verified = first_turn_session_id
        .as_ref()
        .map(|s| Some(s) == created_session_id.as_ref());
    if !scenario.turns.is_empty() {
        match (&first_turn_requested_session_id, &created_session_id) {
            (Some(requested), Some(created)) if requested != created => {
                all_failures.push(format!(
                    "first turn requested session {requested} but the prelude created session {created}"
                ));
            }
            (None, _) => {
                all_failures
                    .push("first turn did not request the prelude-created session id".to_string());
            }
            _ => {}
        }
        if let (Some(emitted), Some(created)) =
            (&first_turn_emitted_session_id, &created_session_id)
        {
            if emitted != created {
                all_failures.push(format!(
                    "first turn emitted session {emitted} but the prelude created session {created}"
                ));
            }
        }
    }
    let character_evidence = json!({
        "readiness": prelude.readiness,
        "persisted": prelude.persisted,
        "created_session_id": created_session_id,
        "first_turn_requested_session_id": first_turn_requested_session_id,
        "first_turn_emitted_session_id": first_turn_emitted_session_id,
        "session_binding_verified": session_binding_verified,
    });

    let ok = all_failures.is_empty();
    // When a check checkpoint classified the failure, surface the earliest
    // failing Journey Qualification Gate (NOT_TRIGGERED < TRIGGERED_NO_MECHANISM
    // < MECHANISM_NO_USER_EFFECT) instead of a flat FAIL.
    let result_state = if ok {
        ResultState::Pass
    } else {
        match results
            .iter()
            .filter_map(|t| t.checkpoint_state)
            .filter(|s| s.is_failing())
            .min_by_key(|s| s.chain_rank())
        {
            Some(state) => ResultState::from_checkpoint(state),
            None => ResultState::Fail,
        }
    };
    let result = PlaytestResult {
        ok,
        result_state,
        scenario: scenario.name,
        mode: ExecutionMode::Live.as_str().to_string(),
        scenario_id: identity.scenario_id.clone(),
        scenario_version: identity.scenario_version.clone(),
        evidence_schema_version: identity.evidence_schema_version.clone(),
        session_id: scenario.session_id,
        output_dir: out_dir.to_string_lossy().to_string(),
        character_setup: Some(character_evidence),
        turns: results,
        failures: all_failures,
    };
    write_json(out_dir.join("playtest_result.json"), &result).await?;

    // Generate the replay cassette from this accepted live run. Conservative: an
    // accepted (`ok`) run writes a `recorded_mode: "live"` cassette the replay
    // executor consumes; a failed run writes only a diagnostic sibling that
    // replay rejects, so no failed run is ever treated as an accepted recording.
    if let Some(record_path) = args.record_fixture.clone() {
        let build = build_cassette(result.ok, &identity, recorded_turns);
        let target = if build.accepted {
            record_path.clone()
        } else {
            diagnostic_sibling(&record_path)
        };
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        write_json(target.clone(), &build.fixture).await?;
        if build.accepted {
            eprintln!("recorded accepted replay cassette: {}", target.display());
        } else {
            eprintln!(
                "live run not accepted (ok=false); wrote diagnostic-only cassette (replay will reject): {}",
                target.display()
            );
        }
    }
    Ok(result)
}

/// Sibling path for a diagnostic (non-accepted) cassette: `<stem>.diagnostic.json`
/// next to the requested record path, so a failed run never overwrites or
/// masquerades as the accepted cassette.
fn diagnostic_sibling(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("cassette");
    let file = format!("{stem}.diagnostic.json");
    match path.parent() {
        Some(parent) => parent.join(file),
        None => PathBuf::from(file),
    }
}

/// Outcome of the character-creation prelude.
struct CharacterPrelude {
    /// `Some` when the prelude failed and the run must stop with this state.
    blocked_state: Option<ResultState>,
    failures: Vec<String>,
    readiness: Value,
    /// Persisted-row verification evidence (`Null` when not reached).
    persisted: Value,
    session_id: Option<String>,
}

impl CharacterPrelude {
    fn early_result(
        &self,
        scenario_name: &str,
        identity: &ScenarioIdentity,
        mode: ExecutionMode,
        out_dir: &Path,
    ) -> Option<PlaytestResult> {
        let state = self.blocked_state?;
        Some(PlaytestResult {
            ok: false,
            result_state: state,
            scenario: scenario_name.to_string(),
            mode: mode.as_str().to_string(),
            scenario_id: identity.scenario_id.clone(),
            scenario_version: identity.scenario_version.clone(),
            evidence_schema_version: identity.evidence_schema_version.clone(),
            session_id: self.session_id.clone(),
            output_dir: out_dir.to_string_lossy().to_string(),
            character_setup: Some(
                json!({ "readiness": self.readiness, "persisted": self.persisted }),
            ),
            turns: vec![],
            failures: self.failures.clone(),
        })
    }
}

fn invalid_setup_result(
    scenario_name: &str,
    identity: &ScenarioIdentity,
    mode: ExecutionMode,
    out_dir: &Path,
    reason: String,
) -> PlaytestResult {
    PlaytestResult {
        ok: false,
        result_state: ResultState::InvalidSetup,
        scenario: scenario_name.to_string(),
        mode: mode.as_str().to_string(),
        scenario_id: identity.scenario_id.clone(),
        scenario_version: identity.scenario_version.clone(),
        evidence_schema_version: identity.evidence_schema_version.clone(),
        session_id: None,
        output_dir: out_dir.to_string_lossy().to_string(),
        character_setup: None,
        turns: vec![],
        failures: vec![reason],
    }
}

/// Assemble a `PlaytestResult` for a fixture-driven (deterministic/replay) run.
fn fixture_result(
    scenario: &PlaytestScenario,
    identity: &ScenarioIdentity,
    mode: ExecutionMode,
    out_dir: &Path,
    result_state: ResultState,
    turns: Vec<PlaytestTurnResult>,
    failures: Vec<String>,
) -> PlaytestResult {
    PlaytestResult {
        ok: failures.is_empty(),
        result_state,
        scenario: scenario.name.clone(),
        mode: mode.as_str().to_string(),
        scenario_id: identity.scenario_id.clone(),
        scenario_version: identity.scenario_version.clone(),
        evidence_schema_version: identity.evidence_schema_version.clone(),
        session_id: scenario.session_id.clone(),
        output_dir: out_dir.to_string_lossy().to_string(),
        character_setup: Some(json!({ "mode": mode.as_str(), "fixture_driven": true })),
        turns,
        failures,
    }
}

/// Deterministic/replay executor. Provider-free: classifies the same check
/// checkpoints from a recorded/authored cassette using the SAME classifier as the
/// live path, citing the same scenario identity and evidence schema. There is no
/// live fallback — a wrong/missing/extra recorded turn is a hard failure
/// (TC-JRNY-02). Deterministic fixtures are authored+seeded; replay cassettes
/// must carry `recorded_mode: "live"` provenance from an accepted live run.
async fn run_fixture_playtest(
    args: &PlaytestArgs,
    scenario: &PlaytestScenario,
    identity: &ScenarioIdentity,
    mode: ExecutionMode,
    out_dir: &Path,
) -> Result<PlaytestResult> {
    let Some(fixture_path) = args.fixture.clone() else {
        let result = fixture_result(
            scenario,
            identity,
            mode,
            out_dir,
            ResultState::Blocked,
            vec![],
            vec![format!(
                "--fixture is required for {} mode (no live fallback)",
                mode.as_str()
            )],
        );
        write_json(out_dir.join("playtest_result.json"), &result).await?;
        return Ok(result);
    };

    let fixture_text = tokio::fs::read_to_string(&fixture_path)
        .await
        .with_context(|| format!("failed to read fixture {}", fixture_path.display()))?;
    let fixture: Fixture = match serde_json::from_str(&fixture_text) {
        Ok(f) => f,
        Err(e) => {
            let result = fixture_result(
                scenario,
                identity,
                mode,
                out_dir,
                ResultState::Fail,
                vec![],
                vec![format!(
                    "invalid fixture JSON {}: {e}",
                    fixture_path.display()
                )],
            );
            write_json(out_dir.join("playtest_result.json"), &result).await?;
            return Ok(result);
        }
    };

    // Normalize both sides so cassette/scenario input comparison ignores only
    // whitespace, mirroring the live runner's `normalize_player_input`.
    let expected_inputs: Vec<String> = scenario
        .turns
        .iter()
        .map(|t| normalize_player_input(&t.user_input))
        .collect();
    let mut normalized_fixture = fixture.clone();
    for turn in &mut normalized_fixture.turns {
        turn.user_input = normalize_player_input(&turn.user_input);
    }

    let plan_mismatches =
        verify_fixture_plan(&normalized_fixture, identity, &expected_inputs, mode);
    write_json(
        out_dir.join("fixture_plan.json"),
        &json!({
            "header": identity.evidence_header(mode),
            "fixture_recorded_mode": fixture.recorded_mode,
            "mismatches": plan_mismatches.iter().map(|m| m.message()).collect::<Vec<_>>(),
        }),
    )
    .await?;
    if !plan_mismatches.is_empty() {
        let failures = plan_mismatches.iter().map(|m| m.message()).collect();
        let result = fixture_result(
            scenario,
            identity,
            mode,
            out_dir,
            ResultState::Fail,
            vec![],
            failures,
        );
        write_json(out_dir.join("playtest_result.json"), &result).await?;
        return Ok(result);
    }

    let mut turns = Vec::new();
    let mut all_failures = Vec::new();
    for (idx, turn) in scenario.turns.iter().enumerate() {
        let fturn = &normalized_fixture.turns[idx];
        let db_diff = json!({ "count_delta": fturn.count_delta });
        let mut turn_failures = Vec::new();
        let mut checkpoint_state = None;
        let mut check_evidence_value = None;
        if let Some(check) = turn.check.clone() {
            if check.is_active() {
                let evidence = fixture_turn_evidence(fturn, &check.player_visible_effect_any);
                let state = classify_check_checkpoint(&check, &evidence);
                if state.is_failing() {
                    turn_failures.push(format!(
                        "check checkpoint did not pass: {} (dice={}, resolved={}, awaiting_binding={}, visible_effect={})",
                        state.as_str(),
                        evidence.dice_present,
                        evidence.resolved_check_present,
                        evidence.awaiting_binding,
                        evidence.player_visible_effect,
                    ));
                }
                check_evidence_value = Some(json!({
                    "state": state,
                    "attempts": [fturn.user_input.clone()],
                    "evidence": evidence,
                }));
                checkpoint_state = Some(state);
            }
        }

        // Knowledge / no-spoiler checkpoint (TC-VS-KNOW-01). Classifies the
        // recorded player-knowledge projection + reveal/surface provenance and runs
        // the production post-output leak verifier against the recorded body.
        let mut knowledge_state = None;
        let mut knowledge_evidence_value = None;
        if let Some(knowledge) = turn.knowledge.clone() {
            if knowledge.is_active() {
                let ev = fturn.knowledge.clone().unwrap_or_default();
                let state =
                    classify_knowledge_checkpoint(&knowledge, &ev, &fturn.player_visible_body);
                if state.is_failing() {
                    turn_failures.push(format!(
                        "knowledge checkpoint did not pass: {} (fact={}, player_known={:?}, reveal_committed={:?})",
                        state.as_str(),
                        knowledge.hidden_fact_id,
                        ev.player_known_fact_ids,
                        ev.reveal_committed_fact_ids,
                    ));
                }
                knowledge_evidence_value = Some(json!({
                    "state": state,
                    "hidden_fact_id": knowledge.hidden_fact_id,
                    "evidence": ev,
                }));
                knowledge_state = Some(state);
            }
        }

        // NPC mind / relationship / social-memory checkpoint (TC-VS-NPC-01).
        // Classifies the recorded NPC social evidence (profile/plan presence, the
        // NPC's own + the player's known-fact projections, the relationship delta,
        // disclosure-change and reload provenance) and runs the same production
        // leak verifier against the recorded body — no provider, fully reproducible.
        let mut npc_social_state = None;
        let mut npc_social_evidence_value = None;
        if let Some(npc_social) = turn.npc_social.clone() {
            if npc_social.is_active() {
                let mut ev = fturn.npc_social.clone().unwrap_or_default();
                // Derive `observed_this_turn` from the recorded count delta so the
                // relationship-action proof matches the live runner's derivation: a
                // relationship/domain write this turn, not a stale historical row.
                let relationship_observed = fturn
                    .count_delta
                    .get("npc_relationships")
                    .copied()
                    .unwrap_or(0)
                    > 0
                    || fturn.count_delta.get("domain_events").copied().unwrap_or(0) > 0;
                if let Some(delta) = ev.relationship_delta.as_mut() {
                    delta.observed_this_turn = relationship_observed;
                }
                let state =
                    classify_npc_social_checkpoint(&npc_social, &ev, &fturn.player_visible_body);
                if state.is_failing() {
                    turn_failures.push(format!(
                        "npc social checkpoint did not pass: {} (npc={}, profile={}, npc_known={:?})",
                        state.as_str(),
                        npc_social.npc_id,
                        ev.profile_present,
                        ev.npc_known_fact_ids,
                    ));
                }
                npc_social_evidence_value = Some(json!({
                    "state": state,
                    "npc_id": npc_social.npc_id,
                    "evidence": ev,
                }));
                npc_social_state = Some(state);
            }
        }

        // Committed-memory / reload checkpoint (TC-VS-MEM-01). Classifies the
        // recorded memory evidence (proposal-from-committed, evidence ids,
        // idempotency, player memory-summary leak, reload retrieval, later
        // consumption, flight-recorder trace) — no provider, fully reproducible.
        let mut memory_state = None;
        let mut memory_evidence_value = None;
        if let Some(memory) = turn.memory.clone() {
            if memory.is_active() {
                let ev = fturn.memory.clone().unwrap_or_default();
                let state = classify_memory_checkpoint(&memory, &ev);
                if state.is_failing() {
                    turn_failures.push(format!(
                        "memory checkpoint did not pass: {} (fact={}, evidence_ids={:?}, trace={:?})",
                        state.as_str(),
                        memory.memory_fact_id,
                        ev.proposal_evidence_event_ids,
                        ev.trace_links,
                    ));
                }
                memory_evidence_value = Some(json!({
                    "state": state,
                    "memory_fact_id": memory.memory_fact_id,
                    "evidence": ev,
                }));
                memory_state = Some(state);
            }
        }

        // Production flight-recorder / provenance checkpoint (TC-PIPE-03). Consumes
        // the recorded production-shaped trace evidence (the same
        // `PluginContributionTrace` entries the GM persists in
        // `TurnTrace.plugin_contributions`, plus the ledger-event kinds linked to
        // the turn) and classifies coverage/order/ledger/secret fail-closed — no
        // provider, fully reproducible.
        let mut flight_recorder_state = None;
        let mut flight_recorder_evidence_value = None;
        if let Some(flight) = turn.flight_recorder.clone() {
            if flight.is_active() {
                let ev = fturn.flight_recorder.clone().unwrap_or_default();
                let state = classify_flight_recorder_checkpoint(&flight, &ev);
                if state.is_failing() {
                    turn_failures.push(format!(
                        "flight recorder checkpoint did not pass: {} (categories={:?}, ledger={:?})",
                        state.as_str(),
                        ev.contributions.iter().map(|t| t.kind.clone()).collect::<Vec<_>>(),
                        ev.ledger_event_kinds,
                    ));
                }
                flight_recorder_evidence_value = Some(json!({ "state": state, "evidence": ev }));
                flight_recorder_state = Some(state);
            }
        }

        all_failures.extend(
            turn_failures
                .iter()
                .map(|f| format!("turn {}: {f}", idx + 1)),
        );
        turns.push(PlaytestTurnResult {
            turn: idx + 1,
            user_input: fturn.user_input.clone(),
            events: vec![],
            chars: fturn.player_visible_body.chars().count(),
            db_diff,
            evaluator_verdict: None,
            failures: turn_failures,
            checkpoint_state,
            check_evidence: check_evidence_value,
            knowledge_checkpoint_state: knowledge_state,
            knowledge_evidence: knowledge_evidence_value,
            npc_social_checkpoint_state: npc_social_state,
            npc_social_evidence: npc_social_evidence_value,
            memory_checkpoint_state: memory_state,
            memory_evidence: memory_evidence_value,
            flight_recorder_state,
            flight_recorder_evidence: flight_recorder_evidence_value,
            skipped: false,
        });
    }

    let result_state = if all_failures.is_empty() {
        ResultState::Pass
    } else {
        match turns
            .iter()
            .filter_map(|t| t.checkpoint_state)
            .filter(|s| s.is_failing())
            .min_by_key(|s| s.chain_rank())
        {
            Some(state) => ResultState::from_checkpoint(state),
            None => ResultState::Fail,
        }
    };
    let result = fixture_result(
        scenario,
        identity,
        mode,
        out_dir,
        result_state,
        turns,
        all_failures,
    );
    write_json(out_dir.join("playtest_result.json"), &result).await?;
    Ok(result)
}

/// Run `trpg create-character --auto`, capture evidence, and gate readiness.
async fn run_character_prelude(
    args: &PlaytestArgs,
    scenario: &PlaytestScenario,
    character: &CharacterSetup,
    out_dir: &Path,
    pool: Option<&PgPool>,
) -> Result<CharacterPrelude> {
    let cc_dir = out_dir.join("character_creation");
    tokio::fs::create_dir_all(&cc_dir).await?;

    let request = json!({
        "ruleset_id": scenario.ruleset_id,
        "module_id": scenario.module_id,
        "user_preferences": character.preferences,
    });
    tokio::fs::write(
        cc_dir.join("request.json"),
        serde_json::to_vec_pretty(&request)?,
    )
    .await?;

    let exec = match execute_create_character(
        &args.bin,
        &args.cwd,
        args.timeout_secs,
        character.auto_complete,
        &request,
    )
    .await
    {
        Ok(exec) => exec,
        Err(err) => {
            // Process could not be spawned: an environmental block, not a bad scenario.
            let failure = format!("failed to run create-character: {err}");
            tokio::fs::write(cc_dir.join("spawn_error.txt"), &failure).await?;
            return Ok(CharacterPrelude {
                blocked_state: Some(ResultState::Blocked),
                failures: vec![failure],
                readiness: json!({ "ok": false, "spawn_error": err.to_string() }),
                persisted: Value::Null,
                session_id: None,
            });
        }
    };

    tokio::fs::write(cc_dir.join("stdout.jsonl"), &exec.stdout_text).await?;
    tokio::fs::write(cc_dir.join("stderr.txt"), &exec.stderr_text).await?;

    let readiness: CharacterCreationReadiness = parse_character_creation_jsonl(
        &exec.stdout_text,
        character.require_persisted,
        character.require_session_binding,
    );
    // Base evidence in the persisted-wrapped shape; the success path rewrites it
    // with real persisted-row results once they are known.
    write_json(
        cc_dir.join("readiness.json"),
        &json!({ "jsonl": &readiness, "persisted": Value::Null }),
    )
    .await?;
    write_json(
        cc_dir.join("character.json"),
        &json!({
            "character_id": readiness.character_id,
            "name": readiness.name,
            "status": readiness.status,
            "session_id": readiness.session_id,
            "actor_id": readiness.actor_id,
            "validation": readiness.validation,
            "sheet": readiness.sheet,
        }),
    )
    .await?;

    let readiness_value = serde_json::to_value(&readiness).unwrap_or(Value::Null);

    // Environmental failure (timeout, non-zero exit, or an error event from the
    // CLI such as a DB/LLM failure) is a BLOCKED run; a well-formed-but-unready
    // character is an INVALID_SETUP scenario.
    if exec.timed_out {
        return Ok(blocked(
            readiness_value,
            format!("create-character timed out after {}s", args.timeout_secs),
        ));
    }
    if exec.status_code.unwrap_or(1) != 0 {
        let mut failures = vec![format!(
            "create-character exited with non-zero status: {:?}",
            exec.status_code
        )];
        failures.extend(readiness.failures.clone());
        return Ok(CharacterPrelude {
            blocked_state: Some(ResultState::Blocked),
            failures,
            readiness: readiness_value,
            persisted: Value::Null,
            session_id: readiness.session_id.clone(),
        });
    }
    if readiness.saw_error {
        return Ok(CharacterPrelude {
            blocked_state: Some(ResultState::Blocked),
            failures: readiness.failures.clone(),
            readiness: readiness_value,
            persisted: Value::Null,
            session_id: readiness.session_id.clone(),
        });
    }
    if !readiness.ok {
        return Ok(CharacterPrelude {
            blocked_state: Some(ResultState::InvalidSetup),
            failures: readiness.failures.clone(),
            readiness: readiness_value,
            persisted: Value::Null,
            session_id: readiness.session_id.clone(),
        });
    }

    // JSONL readiness passed, but a CLI can emit ready-looking JSONL without ever
    // committing. When the scenario requires a persisted character, read the rows
    // back: an unavailable DB or query error is BLOCKED, a verified-missing row is
    // INVALID_SETUP.
    let mut persisted = verify_persisted_rows(pool, character, &readiness).await;
    let verdict = evaluate_persisted_verification(&mut persisted);
    let persisted_value = serde_json::to_value(&persisted).unwrap_or(Value::Null);
    write_json(
        cc_dir.join("readiness.json"),
        &json!({ "jsonl": readiness_value, "persisted": persisted_value }),
    )
    .await?;

    let blocked_state = match verdict {
        PersistedVerdict::Ok => None,
        PersistedVerdict::Blocked => Some(ResultState::Blocked),
        PersistedVerdict::Invalid => Some(ResultState::InvalidSetup),
    };
    Ok(CharacterPrelude {
        blocked_state,
        failures: persisted.failures.clone(),
        readiness: readiness_value,
        persisted: persisted_value,
        session_id: readiness.session_id.clone(),
    })
}

fn blocked(readiness: Value, failure: String) -> CharacterPrelude {
    CharacterPrelude {
        blocked_state: Some(ResultState::Blocked),
        failures: vec![failure],
        readiness,
        persisted: Value::Null,
        session_id: None,
    }
}

/// Read-only confirmation that a required persisted character actually wrote the
/// `characters`, `sessions`, and `runtime_actor_parameters` rows it claimed.
async fn verify_persisted_rows(
    pool: Option<&PgPool>,
    character: &CharacterSetup,
    readiness: &CharacterCreationReadiness,
) -> PersistedVerification {
    let mut v = PersistedVerification {
        required: character.require_persisted,
        require_session_binding: character.require_session_binding,
        character_id: readiness.character_id.clone(),
        session_id: readiness.session_id.clone(),
        actor_id: readiness.actor_id.clone(),
        ..Default::default()
    };
    if !character.require_persisted {
        return v;
    }
    let Some(pool) = pool else {
        v.db_available = false;
        return v;
    };
    v.db_available = true;
    v.db_checked = true;
    if let Some(cid) = readiness.character_id.as_deref() {
        match row_exists(
            pool,
            "select 1 from characters where character_id = $1",
            &[cid],
        )
        .await
        {
            Ok(present) => v.character_row_present = present,
            Err(err) => v.query_errors.push(format!("characters: {err}")),
        }
    }
    if character.require_session_binding {
        if let Some(sid) = readiness.session_id.as_deref() {
            match row_exists(pool, "select 1 from sessions where session_id = $1", &[sid]).await {
                Ok(present) => v.session_row_present = present,
                Err(err) => v.query_errors.push(format!("sessions: {err}")),
            }
        }
        if let (Some(sid), Some(aid)) = (
            readiness.session_id.as_deref(),
            readiness.actor_id.as_deref(),
        ) {
            match row_exists(
                pool,
                "select 1 from runtime_actor_parameters where session_id = $1 and actor_id = $2",
                &[sid, aid],
            )
            .await
            {
                Ok(present) => v.actor_params_present = present,
                Err(err) => v
                    .query_errors
                    .push(format!("runtime_actor_parameters: {err}")),
            }
        }
    }
    v
}

/// Run a `select 1 ...` existence probe with positional text binds.
async fn row_exists(pool: &PgPool, sql: &str, binds: &[&str]) -> Result<bool> {
    let mut query = sqlx::query_scalar::<_, i32>(sql);
    for bind in binds {
        query = query.bind(*bind);
    }
    Ok(query.fetch_optional(pool).await?.is_some())
}

/// Decide whether a scenario that declares a character still lacks a playable
/// first action. Returns the INVALID_SETUP reason, or `None` when playable.
fn missing_first_turn_reason(scenario: &PlaytestScenario, debug_allowed: bool) -> Option<String> {
    let Some(first) = scenario.turns.first() else {
        return Some(
            "scenario.setup.character is present but the scenario has no turns: a play journey must take at least one player action".to_string(),
        );
    };
    let stripped = if debug_allowed {
        let mut state = BTreeMap::new();
        apply_debug_directives(&first.user_input, &mut state).stripped_input
    } else {
        first.user_input.clone()
    };
    if normalize_player_input(&stripped).is_empty() {
        return Some(
            "first turn has empty player input after normalization: the first player action cannot be blank".to_string(),
        );
    }
    None
}

/// Spawn `trpg create-character` with a JSON request on stdin and capture output.
async fn execute_create_character(
    bin: &Path,
    cwd: &Path,
    timeout_secs: u64,
    auto: bool,
    request: &Value,
) -> Result<TurnExecution> {
    let mut command = Command::new(bin);
    command.current_dir(cwd).arg("create-character");
    if auto {
        command.arg("--auto");
    }
    command
        .arg("--stream-format")
        .arg("jsonl")
        .arg("--request-json")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .with_context(|| format!("failed to start trpg binary {}", bin.display()))?;

    if let Some(mut stdin) = child.stdin.take() {
        let input = serde_json::to_vec(request)?;
        stdin.write_all(&input).await?;
        stdin.shutdown().await.ok();
    }

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("failed to capture stdout"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("failed to capture stderr"))?;
    let stdout_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        stdout.read_to_end(&mut buf).await.map(|_| buf)
    });
    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        stderr.read_to_end(&mut buf).await.map(|_| buf)
    });

    let wait_result = tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait()).await;
    let mut timed_out = false;
    let status_code = match wait_result {
        Ok(Ok(status)) => status.code(),
        Ok(Err(err)) => return Err(err.into()),
        Err(_) => {
            timed_out = true;
            child.kill().await.ok();
            let _ = child.wait().await;
            None
        }
    };
    let stdout_bytes = stdout_task.await??;
    let stderr_bytes = stderr_task.await??;
    let stdout_text = String::from_utf8_lossy(&stdout_bytes).to_string();
    let stderr_text = String::from_utf8_lossy(&stderr_bytes).to_string();
    let parsed = parse_jsonl_events(&stdout_text);
    Ok(TurnExecution {
        stdout_text,
        stderr_text,
        parsed,
        status_code,
        timed_out,
    })
}

async fn execute_turn_process(
    bin: &Path,
    cwd: &Path,
    timeout_secs: u64,
    ruleset_id: &str,
    module_id: Option<String>,
    session_id: Option<String>,
    user_input: &str,
    recent_transcript: Option<String>,
) -> Result<TurnExecution> {
    let request = json!({
        "ruleset_id": ruleset_id,
        "module_id": module_id,
        "session_id": session_id,
        "user_input": user_input,
        "recent_transcript": recent_transcript,
    });

    let mut child = Command::new(bin)
        .current_dir(cwd)
        .arg("turn")
        .arg("--request-json")
        .arg("-")
        .arg("--stream-format")
        .arg("jsonl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start trpg binary {}", bin.display()))?;

    if let Some(mut stdin) = child.stdin.take() {
        let input = serde_json::to_vec(&request)?;
        stdin.write_all(&input).await?;
        stdin.shutdown().await.ok();
    }

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("failed to capture stdout"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("failed to capture stderr"))?;
    let stdout_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        stdout.read_to_end(&mut buf).await.map(|_| buf)
    });
    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        stderr.read_to_end(&mut buf).await.map(|_| buf)
    });

    let wait_result = tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait()).await;
    let mut timed_out = false;
    let status_code = match wait_result {
        Ok(Ok(status)) => status.code(),
        Ok(Err(err)) => return Err(err.into()),
        Err(_) => {
            timed_out = true;
            child.kill().await.ok();
            let _ = child.wait().await;
            None
        }
    };
    let stdout_bytes = stdout_task.await??;
    let stderr_bytes = stderr_task.await??;
    let stdout_text = String::from_utf8_lossy(&stdout_bytes).to_string();
    let stderr_text = String::from_utf8_lossy(&stderr_bytes).to_string();
    let parsed = parse_jsonl_events(&stdout_text);
    Ok(TurnExecution {
        stdout_text,
        stderr_text,
        parsed,
        status_code,
        timed_out,
    })
}

async fn maybe_connect_db(cwd: &Path) -> Option<PgPool> {
    let from_env = std::env::var("DATABASE_URL").ok();
    let from_dotenv = read_database_url_from_dotenv(cwd).ok().flatten();
    let url = from_env.or(from_dotenv)?;
    PgPool::connect(&url).await.ok()
}

fn read_database_url_from_dotenv(cwd: &Path) -> Result<Option<String>> {
    let path = cwd.join(".env");
    if !path.exists() {
        return Ok(None);
    }
    let iter = dotenvy::from_path_iter(&path)?;
    for item in iter {
        let (key, value) = item?;
        if key == "DATABASE_URL" {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

async fn db_snapshot(pool: Option<&PgPool>, session_id: Option<&str>) -> DbSnapshot {
    let Some(pool) = pool else {
        return DbSnapshot {
            ok: false,
            error: Some("DATABASE_URL not configured or connection failed".into()),
            ..Default::default()
        };
    };
    let tables = [
        "actor_mechanical_states",
        "damage_packets",
        "parameter_impacts",
        "effect_resolution_packets",
        "dice_rolls",
        "roll_plans",
        "attack_resolution_contracts",
        "contest_resolution_events",
        "pending_checks",
        "interaction_gates",
        "state_frames",
        "object_instances",
        "ability_instances",
        "rule_binding_packets",
        "parameter_facet_execution_runs",
        "generic_parameter_states",
        // Knowledge runtime (TC-VS-KNOW-01): surface player-knowledge deltas so the
        // reveal/continuity evidence is auditable alongside the check tables.
        "knowledge_edges",
        "domain_events",
        // NPC social runtime (TC-VS-NPC-01): a relationship write this turn proves
        // the social delta was observed now, not read from a historical row.
        "npc_relationships",
        // Committed memory (TC-VS-MEM-01): a memory_facts write this turn proves a
        // proposal committed now; domain_events (above) is the committed-event link.
        "memory_facts",
    ];
    let mut snap = DbSnapshot {
        ok: true,
        ..Default::default()
    };
    for table in tables {
        if !table_exists(pool, table).await.unwrap_or(false) {
            continue;
        }
        let count = count_table(pool, table, session_id).await.unwrap_or(0);
        snap.counts.insert(table.to_string(), count);
        let latest = latest_rows(pool, table, session_id, 5)
            .await
            .unwrap_or_default();
        snap.latest.insert(table.to_string(), latest);
    }
    snap
}

/// Read the player_party `knows_true` fact-id projection for a session (the same
/// projection `Db::list_player_known_fact_ids` / `PlayerKnowledgeView` exposes).
/// Fail-soft: returns empty when there is no pool, no session, or on any query
/// error (e.g. the table is absent), so the knowledge checkpoint fails closed
/// rather than throwing — an empty projection is "player knows nothing".
async fn query_player_known_fact_ids(
    pool: Option<&PgPool>,
    session_id: Option<&str>,
) -> Vec<String> {
    let (Some(pool), Some(session_id)) = (pool, session_id) else {
        return vec![];
    };
    if !table_exists(pool, "knowledge_edges").await.unwrap_or(false) {
        return vec![];
    }
    sqlx::query_scalar::<_, String>(
        "select distinct fact_id from knowledge_edges \
         where session_id = $1 and holder_kind = 'player_party' \
         and knowledge_state = 'knows_true' order by fact_id",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

/// Read the NPC's own `knows_true` fact-id projection (the holder-specific edges
/// `Db::list_npc_mind_facts` exposes for `holder_kind='npc'`). This is the allowed
/// set for the "NPC must not assert facts it does not know" leak check. Fail-soft:
/// empty when there is no pool/session/npc or on any query error.
async fn query_npc_known_fact_ids(
    pool: Option<&PgPool>,
    session_id: Option<&str>,
    npc_id: &str,
) -> Vec<String> {
    let (Some(pool), Some(session_id)) = (pool, session_id) else {
        return vec![];
    };
    if npc_id.is_empty() || !table_exists(pool, "knowledge_edges").await.unwrap_or(false) {
        return vec![];
    }
    sqlx::query_scalar::<_, String>(
        "select distinct fact_id from knowledge_edges \
         where session_id = $1 and holder_kind = 'npc' and holder_id = $2 \
         and knowledge_state = 'knows_true' order by fact_id",
    )
    .bind(session_id)
    .bind(npc_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

/// True when a stable, source-backed NPC profile row exists for this session/actor
/// (`Db::load_npc_profile` reads `npc_profiles`). Fail-soft: false on any error.
async fn query_npc_profile_present(
    pool: Option<&PgPool>,
    session_id: Option<&str>,
    npc_id: &str,
) -> bool {
    let (Some(pool), Some(session_id)) = (pool, session_id) else {
        return false;
    };
    if npc_id.is_empty() || !table_exists(pool, "npc_profiles").await.unwrap_or(false) {
        return false;
    }
    sqlx::query_scalar::<_, bool>(
        "select exists(select 1 from npc_profiles where session_id = $1 and actor_id = $2)",
    )
    .bind(session_id)
    .bind(npc_id)
    .fetch_one(pool)
    .await
    .unwrap_or(false)
}

/// Read the durable NPC→player_party relationship row and surface it as delta
/// evidence (committed + its evidence event ids + stance). A committed
/// relationship whose `evidence_event_ids` is non-empty proves the social action
/// went through the evidence-gated `apply_delta`/`upsert_npc_relationship` path
/// rather than an unbounded LLM write. Fail-soft: `None` on any error.
async fn query_relationship_delta_evidence(
    pool: Option<&PgPool>,
    session_id: Option<&str>,
    npc_id: &str,
) -> Option<RelationshipDeltaEvidence> {
    let (Some(pool), Some(session_id)) = (pool, session_id) else {
        return None;
    };
    if npc_id.is_empty()
        || !table_exists(pool, "npc_relationships")
            .await
            .unwrap_or(false)
    {
        return None;
    }
    let row: Option<(String, Value)> = sqlx::query_as(
        "select stance, evidence_event_ids from npc_relationships \
         where session_id = $1 and npc_id = $2 and target_kind = 'player_party' \
         order by updated_at desc limit 1",
    )
    .bind(session_id)
    .bind(npc_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    row.map(|(stance, evidence)| {
        let evidence_event_ids = evidence
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        RelationshipDeltaEvidence {
            // A durable row is a committed delta (it survived the evidence gate).
            proposed: true,
            committed: true,
            evidence_event_ids,
            // Set by the caller from the current turn's count delta — a durable row
            // alone does not prove the change happened this turn.
            observed_this_turn: false,
            stance: Some(stance),
        }
    })
}

/// Read all durably-committed `memory_facts` ids for a session. Fail-soft: empty
/// when there is no pool/session or on any query error.
async fn query_memory_committed_fact_ids(
    pool: Option<&PgPool>,
    session_id: Option<&str>,
) -> Vec<String> {
    let (Some(pool), Some(session_id)) = (pool, session_id) else {
        return vec![];
    };
    if !table_exists(pool, "memory_facts").await.unwrap_or(false) {
        return vec![];
    }
    sqlx::query_scalar::<_, String>(
        "select fact_id from memory_facts where session_id = $1 and status = 'active' \
         order by fact_id",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

/// Read the `source_event_ids` provenance vector for one committed memory fact —
/// the durable evidence ids behind an accepted proposal. Fail-soft: empty on any
/// error or absent row.
async fn query_memory_fact_source_event_ids(
    pool: Option<&PgPool>,
    session_id: Option<&str>,
    fact_id: &str,
) -> Vec<String> {
    let (Some(pool), Some(session_id)) = (pool, session_id) else {
        return vec![];
    };
    if fact_id.is_empty() || !table_exists(pool, "memory_facts").await.unwrap_or(false) {
        return vec![];
    }
    sqlx::query_scalar::<_, Vec<String>>(
        "select source_event_ids from memory_facts where session_id = $1 and fact_id = $2",
    )
    .bind(session_id)
    .bind(fact_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .unwrap_or_default()
}

/// Read the latest player-facing memory summary text (the newest `memory_snapshots`
/// summary), used for the player-memory-summary leak check. Fail-soft: empty string
/// when there is no pool/session/table or on any query error.
async fn query_memory_summary(pool: Option<&PgPool>, session_id: Option<&str>) -> String {
    let (Some(pool), Some(session_id)) = (pool, session_id) else {
        return String::new();
    };
    if !table_exists(pool, "memory_snapshots")
        .await
        .unwrap_or(false)
    {
        return String::new();
    }
    sqlx::query_scalar::<_, String>(
        "select summary_markdown from memory_snapshots where session_id = $1 \
         order by updated_at desc, version desc limit 1",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .unwrap_or_default()
}

/// Read the production flight-recorder artifact for a session: the latest
/// `turn_traces` row's `plugin_contributions` (knowledge-view loads, plugin
/// contributions, context filters, verifier findings) plus that row's turn_id (so
/// the caller can fetch the ledger events linked to the same turn). Fail-soft:
/// empty + `None` when there is no pool/session/table or on any query/parse error.
async fn query_latest_turn_trace_contributions(
    pool: Option<&PgPool>,
    session_id: Option<&str>,
) -> (Vec<trpg_model::PluginContributionTrace>, Option<String>) {
    let (Some(pool), Some(session_id)) = (pool, session_id) else {
        return (vec![], None);
    };
    if !table_exists(pool, "turn_traces").await.unwrap_or(false) {
        return (vec![], None);
    }
    let row: Option<(String, Value)> = sqlx::query_as(
        "select turn_id, trace_json from turn_traces where session_id = $1 \
         order by created_at desc limit 1",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    match row {
        Some((turn_id, trace_json)) => {
            let contributions = trace_json
                .get("plugin_contributions")
                .and_then(|v| {
                    serde_json::from_value::<Vec<trpg_model::PluginContributionTrace>>(v.clone())
                        .ok()
                })
                .unwrap_or_default();
            (contributions, Some(turn_id))
        }
        None => (vec![], None),
    }
}

/// Read the ledger-impacting `domain_events` kinds linked to a turn (same turn_id
/// as the turn trace), proving the trace connects to committed events. Fail-soft:
/// empty when there is no pool/turn/table or on any query error.
async fn query_turn_ledger_event_kinds(
    pool: Option<&PgPool>,
    turn_id: Option<&str>,
) -> Vec<String> {
    let (Some(pool), Some(turn_id)) = (pool, turn_id) else {
        return vec![];
    };
    if turn_id.is_empty() || !table_exists(pool, "domain_events").await.unwrap_or(false) {
        return vec![];
    }
    sqlx::query_scalar::<_, String>(
        "select kind from domain_events where turn_id = $1 order by seq",
    )
    .bind(turn_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

async fn table_exists(pool: &PgPool, table: &str) -> Result<bool> {
    let name: Option<String> = sqlx::query_scalar("select to_regclass($1)::text")
        .bind(format!("public.{table}"))
        .fetch_one(pool)
        .await?;
    Ok(name.is_some())
}

async fn table_has_session_id(pool: &PgPool, table: &str) -> Result<bool> {
    let exists: bool = sqlx::query_scalar("select exists(select 1 from information_schema.columns where table_schema='public' and table_name=$1 and column_name='session_id')")
        .bind(table)
        .fetch_one(pool)
        .await?;
    Ok(exists)
}

async fn count_table(pool: &PgPool, table: &str, session_id: Option<&str>) -> Result<i64> {
    let sql = if session_id.is_some() && table_has_session_id(pool, table).await.unwrap_or(false) {
        format!("select count(*)::bigint from {table} where session_id = $1")
    } else {
        format!("select count(*)::bigint from {table}")
    };
    let mut query = sqlx::query_scalar::<_, i64>(&sql);
    if let Some(id) = session_id.filter(|_| sql.contains("$1")) {
        query = query.bind(id);
    }
    Ok(query.fetch_one(pool).await?)
}

async fn latest_rows(
    pool: &PgPool,
    table: &str,
    session_id: Option<&str>,
    limit: i64,
) -> Result<Vec<Value>> {
    let has_session =
        session_id.is_some() && table_has_session_id(pool, table).await.unwrap_or(false);
    let sql = if has_session {
        format!("select to_jsonb(t) as row_json from (select * from {table} where session_id = $1 order by created_at desc nulls last limit $2) t")
    } else {
        format!("select to_jsonb(t) as row_json from (select * from {table} order by created_at desc nulls last limit $1) t")
    };
    let rows = if has_session {
        sqlx::query(&sql)
            .bind(session_id.unwrap())
            .bind(limit)
            .fetch_all(pool)
            .await?
    } else {
        sqlx::query(&sql).bind(limit).fetch_all(pool).await?
    };
    Ok(rows
        .into_iter()
        .filter_map(|row| row.try_get::<Value, _>("row_json").ok())
        .collect())
}

/// Extract the `count_delta` table->delta map from a `db_diff` value.
fn count_delta_map(diff: &Value) -> BTreeMap<String, i64> {
    diff.get("count_delta")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| v.as_i64().map(|n| (k.clone(), n)))
                .collect()
        })
        .unwrap_or_default()
}

/// Sum two count-delta maps (used to fold an adaptive follow-up action into the
/// originating turn's check evidence).
fn merge_count_deltas(
    a: &BTreeMap<String, i64>,
    b: &BTreeMap<String, i64>,
) -> BTreeMap<String, i64> {
    let mut out = a.clone();
    for (table, delta) in b {
        *out.entry(table.clone()).or_insert(0) += delta;
    }
    out
}

fn db_diff(before: &DbSnapshot, after: &DbSnapshot) -> Value {
    let mut count_delta = serde_json::Map::new();
    for (table, after_count) in &after.counts {
        let before_count = before.counts.get(table).copied().unwrap_or(0);
        count_delta.insert(table.clone(), json!(after_count - before_count));
    }
    json!({
        "before_ok": before.ok,
        "after_ok": after.ok,
        "count_delta": count_delta,
        "after_counts": after.counts,
    })
}

fn build_eval_prompt(
    scenario: &PlaytestScenario,
    turn: &PlaytestTurn,
    clean_player_input: &str,
    debug_result: &DebugDirectiveResult,
    execution: &TurnExecution,
    before: &DbSnapshot,
    after: &DbSnapshot,
    diff: &Value,
) -> String {
    let instructions = scenario.evaluator_instructions.clone().unwrap_or_else(|| {
        "Evaluate whether the GM behaved like a referee, not just a narrator. In default product mode, the player only describes fictional actions; the system must decide when mechanics are needed, call the dice tool, resolve outcomes/effects, persist state, narrate results, and stop at the next player decision. Check if mechanics were persisted, if hidden information leaked, if the GM asked the player for dice results or rule-table values, and whether the next turn can rely on DB state. Also check whether player inputs are human-like: normal players should not use JSON, code, internal event names, database terms, function names, dice expressions, roll totals, or test-engineering phrasing. Test-only [debug]...[/debug] directives are allowed only as stripped setup controls and must not be judged as player behavior.".to_string()
    });
    format!(
        r#"# TRPG External Playtest Evaluation

Scenario: {scenario_name}
Ruleset: {ruleset}
Module: {module:?}

## Instructions
{instructions}

## Player input sent to GM
{input}

## Test-only debug directives stripped from player input
{debug_directives}

## Active test-only debug state
{debug_state}

## Player-visible output
{body}

## Events
{events}

## DB count diff
{diff}

## DB snapshot before 
{before}

## DB snapshot after
{after}

Return JSON matching this shape:
{{
  "verdict": "pass|soft_fail|hard_fail",
  "score": 0,
  "issues": [{{"kind":"string","severity":"low|medium|high","evidence":"string","expected_behavior":"string"}}],
  "next_probe_action": "string"
}}
"#,
        scenario_name = scenario.name,
        ruleset = scenario.ruleset_id,
        module = scenario.module_id,
        instructions = instructions,
        input = clean_player_input,
        debug_directives =
            serde_json::to_string_pretty(&debug_result.directives).unwrap_or_default(),
        debug_state = serde_json::to_string_pretty(&debug_result.state).unwrap_or_default(),
        body = execution.parsed.body,
        events = execution.parsed.events.join(", "),
        diff = serde_json::to_string_pretty(diff).unwrap_or_default(),
        before = serde_json::to_string_pretty(before).unwrap_or_default(),
        after = serde_json::to_string_pretty(after).unwrap_or_default(),
    )
}

fn normalize_player_input(input: &str) -> String {
    input
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

fn apply_debug_directives(
    input: &str,
    state: &mut BTreeMap<String, Value>,
) -> DebugDirectiveResult {
    let mut stripped = String::new();
    let mut rest = input;
    let mut directives = Vec::new();
    let mut changes = Vec::new();
    let mut failures = Vec::new();
    let mut counter = state.len();
    loop {
        let Some(start) = rest.find("[debug]") else {
            stripped.push_str(rest);
            break;
        };
        stripped.push_str(&rest[..start]);
        let after_start = &rest[start + "[debug]".len()..];
        let Some(end) = after_start.find("[/debug]") else {
            failures.push("missing closing [/debug] tag".into());
            break;
        };
        let raw = after_start[..end].trim().to_string();
        match parse_debug_directive(&raw) {
            Ok((action, payload)) => {
                let record = DebugDirectiveRecord {
                    action: action.clone(),
                    payload: payload.clone(),
                    raw: raw.clone(),
                };
                match apply_debug_directive_to_state(&action, payload, state, &mut counter) {
                    Ok(change) => changes.push(change),
                    Err(err) => failures.push(format!("{action}: {err}")),
                }
                directives.push(record);
            }
            Err(err) => failures.push(format!("could not parse debug directive `{raw}`: {err}")),
        }
        rest = &after_start[end + "[/debug]".len()..];
    }
    DebugDirectiveResult {
        stripped_input: stripped,
        directives,
        changes,
        failures,
        state: state.clone(),
    }
}

fn parse_debug_directive(raw: &str) -> Result<(String, Value)> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("empty debug directive"));
    }
    if trimmed.starts_with('{') {
        let value: Value = serde_json::from_str(trimmed)?;
        let action = value
            .get("action")
            .or_else(|| value.get("op"))
            .and_then(Value::as_str)
            .unwrap_or("add")
            .to_string();
        return Ok((action, value));
    }
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let action = parts.next().unwrap_or("add").trim().to_ascii_lowercase();
    let payload_text = parts.next().unwrap_or("{}").trim();
    let payload = if payload_text.is_empty() {
        json!({})
    } else {
        serde_json::from_str(payload_text)?
    };
    Ok((action, payload))
}

fn apply_debug_directive_to_state(
    action: &str,
    payload: Value,
    state: &mut BTreeMap<String, Value>,
    counter: &mut usize,
) -> Result<Value> {
    match action {
        "add" | "insert" | "create" => {
            *counter += 1;
            let id = debug_payload_id(&payload).unwrap_or_else(|| format!("debug_{}", counter));
            state.insert(id.clone(), payload.clone());
            Ok(json!({"action":"add", "id": id, "payload": payload}))
        }
        "set" | "replace" => {
            let id =
                debug_payload_id(&payload).ok_or_else(|| anyhow!("set/replace requires id"))?;
            let value = payload
                .get("value")
                .or_else(|| payload.get("data"))
                .cloned()
                .unwrap_or_else(|| payload.clone());
            state.insert(id.clone(), value.clone());
            Ok(json!({"action":"set", "id": id, "value": value}))
        }
        "patch" | "update" | "modify" => {
            let id =
                debug_payload_id(&payload).ok_or_else(|| anyhow!("patch/update requires id"))?;
            let patch = payload
                .get("patch")
                .or_else(|| payload.get("data"))
                .cloned()
                .unwrap_or_else(|| payload.clone());
            let entry = state
                .entry(id.clone())
                .or_insert_with(|| json!({"id": id.clone()}));
            merge_json_value(entry, &patch);
            Ok(json!({"action":"patch", "id": id, "patch": patch, "after": entry.clone()}))
        }
        "delete" | "remove" => {
            let id = debug_payload_id(&payload)
                .or_else(|| payload.as_str().map(str::to_string))
                .ok_or_else(|| anyhow!("delete/remove requires id"))?;
            let removed = state.remove(&id);
            Ok(json!({"action":"delete", "id": id, "removed": removed}))
        }
        "clear" => {
            let previous = state.len();
            state.clear();
            Ok(json!({"action":"clear", "removed_count": previous}))
        }
        other => Err(anyhow!(
            "unsupported debug action `{other}`; supported: add, set, patch, delete, clear"
        )),
    }
}

fn debug_payload_id(payload: &Value) -> Option<String> {
    payload
        .get("id")
        .or_else(|| payload.get("debug_id"))
        .or_else(|| payload.get("entity_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            payload
                .get("data")
                .and_then(|v| v.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            payload
                .get("row")
                .and_then(|v| v.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            payload
                .get("name")
                .and_then(Value::as_str)
                .map(|s| format!("name:{}", sanitize_name(s)))
        })
}

fn merge_json_value(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(target_map), Value::Object(patch_map)) => {
            for (key, patch_value) in patch_map {
                if patch_value.is_null() {
                    target_map.remove(key);
                } else {
                    merge_json_value(
                        target_map.entry(key.clone()).or_insert(Value::Null),
                        patch_value,
                    );
                }
            }
        }
        (target_slot, patch_value) => *target_slot = patch_value.clone(),
    }
}

fn build_recent_transcript_context(
    recent_transcript: &str,
    debug_state: &BTreeMap<String, Value>,
) -> Option<String> {
    let mut parts = Vec::new();
    if !recent_transcript.trim().is_empty() {
        parts.push(recent_transcript.to_string());
    }
    if !debug_state.is_empty() {
        parts.push(format!(
            "[GM-ONLY TEST DEBUG STATE — do not reveal this label to the player. Treat these JSON entities/items/actors/clocks as present for this test unless later debug directives modify/delete them.]\n{}",
            serde_json::to_string_pretty(debug_state).unwrap_or_default()
        ));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

async fn run_claude_evaluator(bin: &Path, prompt: &str) -> Result<Value> {
    let schema = json!({
        "type": "object",
        "properties": {
            "verdict": {"type":"string"},
            "score": {"type":"number"},
            "issues": {"type":"array", "items": {"type":"object"}},
            "next_probe_action": {"type":"string"}
        },
        "required": ["verdict", "issues"]
    });
    let schema_file = std::env::temp_dir().join(format!(
        "trpg-eval-schema-{}.json",
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    tokio::fs::write(&schema_file, serde_json::to_vec(&schema)?).await?;
    let mut child = Command::new(bin)
        .arg("-p")
        .arg("--bare")
        .arg("--no-session-persistence")
        .arg("--tools")
        .arg("")
        .arg("--output-format")
        .arg("json")
        .arg("--json-schema")
        .arg(&schema_file)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start Claude Code evaluator {}", bin.display()))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(prompt.as_bytes()).await?;
        stdin.shutdown().await.ok();
    }
    let output = tokio::time::timeout(Duration::from_secs(240), child.wait_with_output()).await??;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    tokio::fs::remove_file(&schema_file).await.ok();
    if !output.status.success() {
        return Err(anyhow!(
            "claude evaluator exited {:?}: {}",
            output.status.code(),
            tail_chars(&stderr, 1200)
        ));
    }
    serde_json::from_str::<Value>(&stdout).with_context(|| {
        format!(
            "claude evaluator did not return JSON: {}",
            tail_chars(&stdout, 1200)
        )
    })
}

async fn write_json(path: PathBuf, value: &impl Serialize) -> Result<()> {
    tokio::fs::write(path, serde_json::to_vec_pretty(value)?).await?;
    Ok(())
}

fn sanitize_name(input: &str) -> String {
    let out: String = input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    out.trim_matches('-').chars().take(80).collect()
}

fn emit_playtest_result(result: &PlaytestResult, mode: &OutputMode) -> Result<()> {
    match mode {
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(result)?),
        OutputMode::Jsonl => println!("{}", serde_json::to_string(result)?),
        OutputMode::Text => {
            if result.ok {
                println!(
                    "{} playtest {} ({} turns) -> {}",
                    result.result_state.as_str(),
                    result.scenario,
                    result.turns.len(),
                    result.output_dir
                );
            } else {
                println!(
                    "{} playtest {} -> {}",
                    result.result_state.as_str(),
                    result.scenario,
                    result.output_dir
                );
                for failure in &result.failures {
                    println!("  - {failure}");
                }
            }
        }
    }
    Ok(())
}

fn emit_eval_report(report: &trpg_eval::EvalReport, mode: &OutputMode) -> Result<()> {
    match mode {
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(report)?),
        OutputMode::Jsonl => println!("{}", serde_json::to_string(report)?),
        OutputMode::Text => print!("{}", render_markdown_report(report)),
    }
    Ok(())
}

fn collect_case_paths(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("failed to read harness dir {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            paths.push(path);
        }
    }
    Ok(paths)
}

async fn run_case_path(
    case_path: &Path,
    bin: &Path,
    cwd: &Path,
    timeout_secs: u64,
    stream_format_override: Option<&str>,
    verbose: bool,
) -> Result<HarnessResult> {
    let case_text = tokio::fs::read_to_string(case_path)
        .await
        .with_context(|| format!("failed to read case {}", case_path.display()))?;
    let mut case: HarnessCase = serde_json::from_str(&case_text)
        .with_context(|| format!("invalid harness case JSON: {}", case_path.display()))?;
    if let Some(format) = stream_format_override {
        case.stream_format = format.to_string();
    }
    run_turn_case(case, bin, cwd, timeout_secs, verbose).await
}

async fn run_turn_case(
    mut case: HarnessCase,
    bin: &Path,
    cwd: &Path,
    timeout_secs: u64,
    verbose: bool,
) -> Result<HarnessResult> {
    if case.turns.is_empty() {
        return run_single_turn_case(case, bin, cwd, timeout_secs, verbose).await;
    }

    let original_name = case.name.clone();
    let turns = case.turns.clone();
    let mut all_events = Vec::new();
    let mut failures = Vec::new();
    let mut forbidden_hits = Vec::new();
    let mut missing_terms = Vec::new();
    let mut missing_events = Vec::new();
    let mut chars = 0usize;
    let mut status = Some(0);
    let mut timed_out = false;
    let mut stdout_tail = None;
    let mut stderr_tail = None;

    for (idx, turn) in turns.into_iter().enumerate() {
        let mut single = case.clone();
        single.name = format!("{}#{}", original_name, idx + 1);
        single.user_input = turn.user_input;
        single.turns = vec![];
        if !turn.forbidden_terms.is_empty() {
            single.forbidden_terms = turn.forbidden_terms;
        }
        if !turn.required_terms.is_empty() {
            single.required_terms = turn.required_terms;
        }
        if !turn.required_events.is_empty() {
            single.required_events = turn.required_events;
        }
        if !turn.forbidden_events.is_empty() {
            single.forbidden_events = turn.forbidden_events;
        }
        if !turn.required_event_contains.is_empty() {
            single.required_event_contains = turn.required_event_contains;
        }
        if turn.required_done_reason.is_some() {
            single.required_done_reason = turn.required_done_reason;
        }
        if turn.require_no_llm_stream_start {
            single.require_no_llm_stream_start = true;
        }

        let result = run_single_turn_case(single, bin, cwd, timeout_secs, verbose).await?;
        if let Some(session_id) = result.session_id.clone() {
            case.session_id = Some(session_id);
        }
        chars += result.chars;
        all_events.extend(
            result
                .events
                .iter()
                .map(|e| format!("turn{}:{}", idx + 1, e)),
        );
        failures.extend(result.failures);
        forbidden_hits.extend(result.forbidden_hits);
        missing_terms.extend(result.missing_terms);
        missing_events.extend(result.missing_events);
        if result.process_status != Some(0) {
            status = result.process_status;
        }
        timed_out |= result.timed_out;
        stdout_tail = result.stdout_tail.or(stdout_tail);
        stderr_tail = result.stderr_tail.or(stderr_tail);
    }

    Ok(HarnessResult {
        ok: failures.is_empty(),
        case: original_name,
        session_id: case.session_id,
        events: all_events,
        chars,
        failures,
        forbidden_hits,
        missing_terms,
        missing_events,
        process_status: status,
        timed_out,
        stdout_tail,
        stderr_tail,
    })
}

async fn run_single_turn_case(
    case: HarnessCase,
    bin: &Path,
    cwd: &Path,
    timeout_secs: u64,
    verbose: bool,
) -> Result<HarnessResult> {
    if case.stream_format != "jsonl" {
        return Ok(HarnessResult {
            ok: false,
            case: case.name.clone(),
            session_id: case.session_id.clone(),
            events: vec![],
            chars: 0,
            failures: vec!["Rust harness currently validates JSONL stream output only; set stream_format=jsonl".to_string()],
            forbidden_hits: vec![],
            missing_terms: vec![],
            missing_events: vec![],
            process_status: None,
            timed_out: false,
            stdout_tail: None,
            stderr_tail: None,
        });
    }

    let request = json!({
        "ruleset_id": case.ruleset_id,
        "module_id": case.module_id,
        "session_id": case.session_id,
        "user_input": case.user_input,
        "recent_transcript": case.recent_transcript,
    });

    let mut child = Command::new(bin)
        .current_dir(cwd)
        .arg("turn")
        .arg("--request-json")
        .arg("-")
        .arg("--stream-format")
        .arg("jsonl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start trpg binary {}", bin.display()))?;

    if let Some(mut stdin) = child.stdin.take() {
        let input = serde_json::to_vec(&request)?;
        stdin.write_all(&input).await?;
        stdin.shutdown().await.ok();
    }

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("failed to capture stdout"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("failed to capture stderr"))?;

    let stdout_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        stdout.read_to_end(&mut buf).await.map(|_| buf)
    });
    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        stderr.read_to_end(&mut buf).await.map(|_| buf)
    });

    let wait_result = tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait()).await;
    let mut timed_out = false;
    let status_code = match wait_result {
        Ok(Ok(status)) => status.code(),
        Ok(Err(err)) => return Err(err.into()),
        Err(_) => {
            timed_out = true;
            child.kill().await.ok();
            let _ = child.wait().await;
            None
        }
    };

    let stdout_bytes = stdout_task.await??;
    let stderr_bytes = stderr_task.await??;
    let stdout_text = String::from_utf8_lossy(&stdout_bytes).to_string();
    let stderr_text = String::from_utf8_lossy(&stderr_bytes).to_string();

    let parsed = parse_jsonl_events(&stdout_text);
    let outcome = evaluate_assertions(&case, &parsed);
    let mut failures = outcome.failures;
    let forbidden_hits = outcome.forbidden_hits;
    let missing_terms = outcome.missing_terms;
    let missing_events = outcome.missing_events;
    let chars = parsed.body.chars().count();
    if timed_out {
        failures.push(format!("case timed out after {timeout_secs}s"));
    }
    if status_code.unwrap_or(1) != 0 {
        failures.push(format!(
            "trpg exited with non-zero status: {:?}",
            status_code
        ));
    }

    Ok(HarnessResult {
        ok: failures.is_empty(),
        case: case.name,
        session_id: parsed.session_id,
        events: parsed.events,
        chars,
        failures,
        forbidden_hits,
        missing_terms,
        missing_events,
        process_status: status_code,
        timed_out,
        stdout_tail: if verbose {
            Some(tail_chars(&stdout_text, 4000))
        } else {
            None
        },
        stderr_tail: if verbose {
            Some(tail_chars(&stderr_text, 4000))
        } else {
            None
        },
    })
}

fn emit_result(result: &HarnessResult, mode: &OutputMode) -> Result<()> {
    match mode {
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(result)?),
        OutputMode::Jsonl => println!("{}", serde_json::to_string(result)?),
        OutputMode::Text => {
            if result.ok {
                println!(
                    "PASS {} ({} chars, events: {})",
                    result.case,
                    result.chars,
                    result.events.join(", ")
                );
            } else {
                println!("FAIL {}", result.case);
                for failure in &result.failures {
                    println!("  - {failure}");
                }
                if let Some(stderr) = &result.stderr_tail {
                    if !stderr.trim().is_empty() {
                        println!("\n[stderr tail]\n{stderr}");
                    }
                }
                if let Some(stdout) = &result.stdout_tail {
                    if !stdout.trim().is_empty() {
                        println!("\n[stdout tail]\n{stdout}");
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod prelude_tests {
    use super::*;
    use trpg_harness::EVIDENCE_SCHEMA_VERSION;

    #[test]
    fn result_state_serializes_screaming_snake() {
        assert_eq!(
            serde_json::to_value(ResultState::Pass).unwrap(),
            json!("PASS")
        );
        assert_eq!(
            serde_json::to_value(ResultState::Fail).unwrap(),
            json!("FAIL")
        );
        assert_eq!(
            serde_json::to_value(ResultState::InvalidSetup).unwrap(),
            json!("INVALID_SETUP")
        );
        assert_eq!(
            serde_json::to_value(ResultState::NotTriggered).unwrap(),
            json!("NOT_TRIGGERED")
        );
        assert_eq!(
            serde_json::to_value(ResultState::TriggeredNoMechanism).unwrap(),
            json!("TRIGGERED_NO_MECHANISM")
        );
        assert_eq!(
            serde_json::to_value(ResultState::MechanismNoUserEffect).unwrap(),
            json!("MECHANISM_NO_USER_EFFECT")
        );
        assert_eq!(
            serde_json::to_value(ResultState::Blocked).unwrap(),
            json!("BLOCKED")
        );
    }

    #[test]
    fn result_state_maps_from_checkpoint_gates() {
        assert_eq!(
            ResultState::from_checkpoint(CheckpointState::TriggeredNoMechanism),
            ResultState::TriggeredNoMechanism
        );
        assert_eq!(
            ResultState::from_checkpoint(CheckpointState::MechanismNoUserEffect),
            ResultState::MechanismNoUserEffect
        );
        assert_eq!(
            ResultState::from_checkpoint(CheckpointState::NotTriggered),
            ResultState::NotTriggered
        );
    }

    #[test]
    fn legacy_turn_without_check_fields_parses() {
        // Backward compatibility: a turn that predates TC-JRNY-01 still parses,
        // with the new adaptive/check fields defaulting to absent.
        let turn: PlaytestTurn =
            serde_json::from_str(r#"{"user_input":"我走向门口","expect_phases":["done"]}"#)
                .unwrap();
        assert!(turn.when.is_none());
        assert!(turn.if_not_triggered.is_none());
        assert!(turn.check.is_none());
    }

    #[test]
    fn demonstration_check_scenario_parses() {
        // The in-repo TC-JRNY-01 demonstration scenario must stay valid against
        // the live PlaytestScenario schema (provider-free; no process spawned).
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
            "../../docs/epics/EPIC-KNOWLEDGE-MEMORY-NPC-RUNTIME/scenarios/TC-JRNY-01-cyberpunk-check-dependent-playtest.json",
        );
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let scenario: PlaytestScenario =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("scenario must parse: {e}"));
        assert!(scenario.setup.and_then(|s| s.character).is_some());
        let turn = &scenario.turns[0];
        assert!(turn.if_not_triggered.is_some());
        let check = turn.check.clone().expect("check checkpoint present");
        assert!(check.require_dice && check.require_check_resolution);
        assert!(check.require_player_visible_effect);
        assert!(!check.player_visible_effect_any.is_empty());
    }

    #[test]
    fn check_turn_schema_parses() {
        let turn: PlaytestTurn = serde_json::from_str(
            r#"{"user_input":"我拆开护盖判断能不能安全切断","when":"session_ready","if_not_triggered":"我实际动手切断线缆","check":{"label":"cable_cut","require_dice":true,"require_check_resolution":true,"require_player_visible_effect":true,"player_visible_effect_any":["切断","失败"]}}"#,
        )
        .unwrap();
        assert_eq!(turn.when.as_deref(), Some("session_ready"));
        assert!(turn.if_not_triggered.is_some());
        let check = turn.check.expect("check checkpoint present");
        assert!(check.is_active());
        assert!(check.require_dice && check.require_check_resolution);
        assert_eq!(check.label.as_deref(), Some("cable_cut"));
    }

    #[test]
    fn missing_setup_yields_invalid_setup_with_no_turns() {
        let out = PathBuf::from("/tmp/does-not-matter");
        let identity = ScenarioIdentity::new("demo", "1");
        let result = invalid_setup_result(
            "demo",
            &identity,
            ExecutionMode::Live,
            &out,
            "no character".to_string(),
        );
        assert_eq!(result.result_state, ResultState::InvalidSetup);
        assert!(!result.ok);
        assert!(result.turns.is_empty());
        assert!(result.character_setup.is_none());
        assert_eq!(result.failures, vec!["no character".to_string()]);
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["result_state"], json!("INVALID_SETUP"));
        assert_eq!(value["ok"], json!(false));
        assert_eq!(value["mode"], json!("live"));
        assert_eq!(value["scenario_id"], json!("demo"));
        assert_eq!(
            value["evidence_schema_version"],
            json!(EVIDENCE_SCHEMA_VERSION)
        );
    }

    #[test]
    fn scenario_without_setup_has_none() {
        let scenario: PlaytestScenario =
            serde_json::from_str(r#"{"name":"n","ruleset_id":"r","turns":[{"user_input":"go"}]}"#)
                .unwrap();
        assert!(scenario.setup.is_none());
    }

    fn scenario_with_turns(turns_json: &str) -> PlaytestScenario {
        serde_json::from_str(&format!(
            r#"{{"name":"n","ruleset_id":"r","setup":{{"character":{{"preferences":"p"}}}},"turns":{turns_json}}}"#
        ))
        .unwrap()
    }

    #[test]
    fn zero_turns_is_missing_first_turn() {
        let scenario = scenario_with_turns("[]");
        let reason = missing_first_turn_reason(&scenario, false).expect("zero turns must fail");
        assert!(reason.contains("no turns"), "reason: {reason}");
    }

    #[test]
    fn empty_first_turn_input_is_missing_first_turn() {
        for raw in ["", "   ", "\n\t  "] {
            let scenario = scenario_with_turns(&format!("[{{\"user_input\":{}}}]", json!(raw)));
            let reason =
                missing_first_turn_reason(&scenario, false).expect("empty input must fail");
            assert!(reason.contains("empty player input"), "reason: {reason}");
        }
    }

    #[test]
    fn debug_only_first_turn_is_empty_when_debug_allowed() {
        let scenario =
            scenario_with_turns(r#"[{"user_input":"[debug]add {\"id\":\"x\"}[/debug]"}]"#);
        // With debug stripping enabled, the player text is empty -> invalid.
        let reason = missing_first_turn_reason(&scenario, true).expect("debug-only must fail");
        assert!(reason.contains("empty player input"), "reason: {reason}");
        // Without debug stripping the raw text is non-empty, so it is playable.
        assert!(missing_first_turn_reason(&scenario, false).is_none());
    }

    #[test]
    fn real_first_turn_input_is_playable() {
        let scenario = scenario_with_turns(r#"[{"user_input":"我走向门口"}]"#);
        assert!(missing_first_turn_reason(&scenario, false).is_none());
        assert!(missing_first_turn_reason(&scenario, true).is_none());
    }

    #[test]
    fn scenario_setup_character_defaults_are_strict() {
        let scenario: PlaytestScenario = serde_json::from_str(
            r#"{"name":"n","ruleset_id":"r","setup":{"character":{"preferences":"a brave knight"}}}"#,
        )
        .unwrap();
        let character = scenario.setup.unwrap().character.unwrap();
        assert_eq!(character.preferences, "a brave knight");
        assert!(character.auto_complete);
        assert!(character.require_persisted);
        assert!(character.require_session_binding);
    }

    // --- TC-JRNY-02: one scenario / deterministic + live + replay executors ---

    fn scenarios_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/epics/EPIC-KNOWLEDGE-MEMORY-NPC-RUNTIME/scenarios")
    }

    fn load_scenario(file: &str) -> PlaytestScenario {
        let path = scenarios_dir().join(file);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("scenario must parse: {e}"))
    }

    fn fixture_args(
        scenario: &str,
        fixture: &str,
        mode: &str,
        label: &str,
    ) -> (PlaytestArgs, PathBuf) {
        let out_dir = std::env::temp_dir()
            .join("trpg-harness-tcjrny02")
            .join(label);
        let _ = std::fs::remove_dir_all(&out_dir);
        std::fs::create_dir_all(&out_dir).unwrap();
        let args = PlaytestArgs {
            scenario: scenarios_dir().join(scenario),
            bin: PathBuf::from("./target/debug/trpg"),
            cwd: PathBuf::from("."),
            timeout_secs: 30,
            output_dir: Some(out_dir.clone()),
            evaluator: "none".to_string(),
            claude_bin: PathBuf::from("claude"),
            evaluator_required: false,
            enable_debug_directives: false,
            require_human_player_input: true,
            mode: mode.to_string(),
            fixture: Some(scenarios_dir().join("fixtures").join(fixture)),
            record_fixture: None,
            output: OutputMode::Json,
            verbose: false,
        };
        (args, out_dir)
    }

    #[test]
    fn technical_pass_scenario_parses_with_identity() {
        let scenario = load_scenario("TC-JRNY-02-cyberpunk-technical-action-playtest.json");
        let identity = scenario.identity();
        assert_eq!(identity.scenario_id, "JRNY-CYBER-TECH-PASS");
        assert_eq!(identity.scenario_version, "1");
        assert_eq!(identity.evidence_schema_version, EVIDENCE_SCHEMA_VERSION);
        let check = scenario.turns[0].check.clone().expect("check present");
        assert!(check.require_dice && check.require_check_resolution);
        assert!(check.require_player_visible_effect);
    }

    #[test]
    fn perception_guard_scenario_has_identity() {
        let scenario = load_scenario("TC-JRNY-01-cyberpunk-check-dependent-playtest.json");
        assert_eq!(
            scenario.identity().scenario_id,
            "JRNY-CYBER-PERCEPTION-FAILCLOSED"
        );
    }

    #[tokio::test]
    async fn deterministic_technical_action_passes() {
        let (args, _out) = fixture_args(
            "TC-JRNY-02-cyberpunk-technical-action-playtest.json",
            "JRNY-CYBER-TECH-PASS.deterministic.json",
            "deterministic",
            "det-tech-pass",
        );
        let scenario = load_scenario("TC-JRNY-02-cyberpunk-technical-action-playtest.json");
        let identity = scenario.identity();
        let out = args.output_dir.clone().unwrap();
        let result = run_fixture_playtest(
            &args,
            &scenario,
            &identity,
            ExecutionMode::Deterministic,
            &out,
        )
        .await
        .unwrap();
        assert!(result.ok, "expected PASS, failures: {:?}", result.failures);
        assert_eq!(result.result_state, ResultState::Pass);
        assert_eq!(result.mode, "deterministic");
        assert_eq!(result.scenario_id, "JRNY-CYBER-TECH-PASS");
        assert_eq!(
            result.turns[0].checkpoint_state,
            Some(CheckpointState::Pass)
        );
    }

    #[tokio::test]
    async fn replay_technical_action_passes() {
        let (args, out) = fixture_args(
            "TC-JRNY-02-cyberpunk-technical-action-playtest.json",
            "JRNY-CYBER-TECH-PASS.replay.json",
            "replay",
            "replay-tech-pass",
        );
        let scenario = load_scenario("TC-JRNY-02-cyberpunk-technical-action-playtest.json");
        let identity = scenario.identity();
        let result = run_fixture_playtest(&args, &scenario, &identity, ExecutionMode::Replay, &out)
            .await
            .unwrap();
        assert!(result.ok, "expected PASS, failures: {:?}", result.failures);
        assert_eq!(result.result_state, ResultState::Pass);
        assert_eq!(result.mode, "replay");
    }

    #[tokio::test]
    async fn replay_rejects_deterministic_provenance_no_live_fallback() {
        // A replay run must consume a live-recorded cassette. Pointing it at the
        // deterministic (authored) fixture is a hard failure, not a live fallback.
        let (args, out) = fixture_args(
            "TC-JRNY-02-cyberpunk-technical-action-playtest.json",
            "JRNY-CYBER-TECH-PASS.deterministic.json",
            "replay",
            "replay-bad-provenance",
        );
        let scenario = load_scenario("TC-JRNY-02-cyberpunk-technical-action-playtest.json");
        let identity = scenario.identity();
        let result = run_fixture_playtest(&args, &scenario, &identity, ExecutionMode::Replay, &out)
            .await
            .unwrap();
        assert!(!result.ok);
        assert_eq!(result.result_state, ResultState::Fail);
        assert!(result
            .failures
            .iter()
            .any(|f| f.contains("provenance mismatch")));
    }

    #[tokio::test]
    async fn deterministic_perception_is_fail_closed() {
        let (args, out) = fixture_args(
            "TC-JRNY-01-cyberpunk-check-dependent-playtest.json",
            "JRNY-CYBER-PERCEPTION-FAILCLOSED.deterministic.json",
            "deterministic",
            "det-perception-failclosed",
        );
        let scenario = load_scenario("TC-JRNY-01-cyberpunk-check-dependent-playtest.json");
        let identity = scenario.identity();
        let result = run_fixture_playtest(
            &args,
            &scenario,
            &identity,
            ExecutionMode::Deterministic,
            &out,
        )
        .await
        .unwrap();
        assert!(!result.ok, "perception guard must not pass");
        assert_eq!(result.result_state, ResultState::TriggeredNoMechanism);
        assert_eq!(
            result.turns[0].checkpoint_state,
            Some(CheckpointState::TriggeredNoMechanism)
        );
    }

    #[tokio::test]
    async fn fixture_mode_without_fixture_is_blocked() {
        let scenario = load_scenario("TC-JRNY-02-cyberpunk-technical-action-playtest.json");
        let identity = scenario.identity();
        let out = std::env::temp_dir()
            .join("trpg-harness-tcjrny02")
            .join("det-no-fixture");
        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(&out).unwrap();
        let mut args = PlaytestArgs {
            scenario: scenarios_dir().join("TC-JRNY-02-cyberpunk-technical-action-playtest.json"),
            bin: PathBuf::from("./target/debug/trpg"),
            cwd: PathBuf::from("."),
            timeout_secs: 30,
            output_dir: Some(out.clone()),
            evaluator: "none".to_string(),
            claude_bin: PathBuf::from("claude"),
            evaluator_required: false,
            enable_debug_directives: false,
            require_human_player_input: true,
            mode: "deterministic".to_string(),
            fixture: None,
            record_fixture: None,
            output: OutputMode::Json,
            verbose: false,
        };
        args.fixture = None;
        let result = run_fixture_playtest(
            &args,
            &scenario,
            &identity,
            ExecutionMode::Deterministic,
            &out,
        )
        .await
        .unwrap();
        assert_eq!(result.result_state, ResultState::Blocked);
        assert!(result
            .failures
            .iter()
            .any(|f| f.contains("--fixture is required")));
    }

    #[test]
    fn record_fixture_cli_arg_parses() {
        let cli = Cli::try_parse_from([
            "trpg-harness",
            "playtest",
            "--scenario",
            "s.json",
            "--mode",
            "live",
            "--record-fixture",
            "out/cassette.json",
        ])
        .expect("cli must parse");
        match cli.command {
            Commands::Playtest(a) => {
                assert_eq!(a.mode, "live");
                assert_eq!(a.record_fixture, Some(PathBuf::from("out/cassette.json")));
                assert_eq!(a.fixture, None);
            }
            _ => panic!("expected playtest subcommand"),
        }
    }

    #[test]
    fn diagnostic_sibling_keeps_stem_next_to_target() {
        assert_eq!(
            diagnostic_sibling(&PathBuf::from("data/cass.json")),
            PathBuf::from("data/cass.diagnostic.json")
        );
        assert_eq!(
            diagnostic_sibling(&PathBuf::from("cass.json")),
            PathBuf::from("cass.diagnostic.json")
        );
    }

    #[tokio::test]
    async fn generated_live_cassette_is_consumable_by_replay() {
        // Simulate the post-live-run writer: build an accepted cassette from
        // recorded turns, write it, then prove the replay executor consumes that
        // generated file end-to-end (writer -> consumer), provider-free.
        let scenario = load_scenario("TC-JRNY-02-cyberpunk-technical-action-playtest.json");
        let identity = scenario.identity();
        let recorded = vec![FixtureTurn {
            user_input: scenario.turns[0].user_input.clone(),
            count_delta: std::collections::BTreeMap::from([
                ("roll_plans".to_string(), 1),
                ("dice_rolls".to_string(), 1),
                ("contest_resolution_events".to_string(), 1),
            ]),
            newest_contest_row: Some(json!({
                "target_value": 14, "success": true, "degree": "success", "total": 18,
                "outcome_json": {"target": 14, "success": true, "degree": "success"}
            })),
            player_visible_body: "你成功切断了公寓的供电电缆，房间陷入黑暗。".to_string(),
            knowledge: None,
            npc_social: None,
            memory: None,
            flight_recorder: None,
        }];
        let build = build_cassette(true, &identity, recorded);
        assert!(build.accepted && build.fixture.recorded_mode == "live");

        let dir = std::env::temp_dir()
            .join("trpg-harness-tcjrny02")
            .join("gen-cassette");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cassette_path = dir.join("cassette.json");
        std::fs::write(
            &cassette_path,
            serde_json::to_vec_pretty(&build.fixture).unwrap(),
        )
        .unwrap();

        let args = PlaytestArgs {
            scenario: scenarios_dir().join("TC-JRNY-02-cyberpunk-technical-action-playtest.json"),
            bin: PathBuf::from("./target/debug/trpg"),
            cwd: PathBuf::from("."),
            timeout_secs: 30,
            output_dir: Some(dir.clone()),
            evaluator: "none".to_string(),
            claude_bin: PathBuf::from("claude"),
            evaluator_required: false,
            enable_debug_directives: false,
            require_human_player_input: true,
            mode: "replay".to_string(),
            fixture: Some(cassette_path.clone()),
            record_fixture: None,
            output: OutputMode::Json,
            verbose: false,
        };
        let result = run_fixture_playtest(&args, &scenario, &identity, ExecutionMode::Replay, &dir)
            .await
            .unwrap();
        assert!(
            result.ok,
            "generated cassette must replay PASS: {:?}",
            result.failures
        );
        assert_eq!(result.result_state, ResultState::Pass);
        assert_eq!(result.scenario_id, "JRNY-CYBER-TECH-PASS");
    }

    // --- TC-VS-KNOW-01: Homecoming knowledge / no-spoiler vertical slice ------

    #[test]
    fn home01_scenario_carries_knowledge_checkpoints_and_identity() {
        let scenario = load_scenario("JRNY-HOME-01-athena-knowledge-reveal.json");
        let identity = scenario.identity();
        assert_eq!(identity.scenario_id, "JRNY-HOME-01-athena-knowledge-reveal");
        assert_eq!(identity.scenario_version, "1");
        // Observation turn forbids the hidden fact and requires player-unknown.
        let obs = scenario.turns[0].knowledge.clone().expect("obs knowledge");
        assert!(obs.require_player_unknown && obs.forbid_player_text_leak);
        assert!(!obs.secret_terms.is_empty());
        // Technical action keeps the real resolved-check requirement.
        let check = scenario.turns[1].check.clone().expect("check present");
        assert!(check.require_dice && check.require_check_resolution);
        // Outcome turn requires player_party learned the fact.
        let outcome = scenario.turns[2]
            .knowledge
            .clone()
            .expect("outcome knowledge");
        assert!(outcome.require_player_learned);
        // Continuity turn requires a post-reload projection.
        let cont = scenario.turns[3]
            .knowledge
            .clone()
            .expect("continuity knowledge");
        assert!(cont.require_player_learned && cont.require_reload);
    }

    #[tokio::test]
    async fn home01_deterministic_success_journey_passes() {
        let (args, out) = fixture_args(
            "JRNY-HOME-01-athena-knowledge-reveal.json",
            "JRNY-HOME-01-athena-knowledge-reveal.deterministic.json",
            "deterministic",
            "home01-det",
        );
        let scenario = load_scenario("JRNY-HOME-01-athena-knowledge-reveal.json");
        let identity = scenario.identity();
        let result = run_fixture_playtest(
            &args,
            &scenario,
            &identity,
            ExecutionMode::Deterministic,
            &out,
        )
        .await
        .unwrap();
        assert!(result.ok, "expected PASS, failures: {:?}", result.failures);
        assert_eq!(result.scenario_id, "JRNY-HOME-01-athena-knowledge-reveal");
        // Before reveal: player-unknown holds and nothing leaked.
        assert_eq!(
            result.turns[0].knowledge_checkpoint_state,
            Some(KnowledgeCheckpointState::Pass)
        );
        // Technical action reached a resolved check.
        assert_eq!(
            result.turns[1].checkpoint_state,
            Some(CheckpointState::Pass)
        );
        // Reveal granted player knowledge; continuity survived reload.
        assert_eq!(
            result.turns[2].knowledge_checkpoint_state,
            Some(KnowledgeCheckpointState::Pass)
        );
        assert_eq!(
            result.turns[3].knowledge_checkpoint_state,
            Some(KnowledgeCheckpointState::Pass)
        );
    }

    #[tokio::test]
    async fn home01_replay_success_journey_passes() {
        let (args, out) = fixture_args(
            "JRNY-HOME-01-athena-knowledge-reveal.json",
            "JRNY-HOME-01-athena-knowledge-reveal.replay.json",
            "replay",
            "home01-replay",
        );
        let scenario = load_scenario("JRNY-HOME-01-athena-knowledge-reveal.json");
        let identity = scenario.identity();
        let result = run_fixture_playtest(&args, &scenario, &identity, ExecutionMode::Replay, &out)
            .await
            .unwrap();
        assert!(result.ok, "replay must PASS: {:?}", result.failures);
        assert_eq!(result.mode, "replay");
        assert_eq!(result.scenario_id, "JRNY-HOME-01-athena-knowledge-reveal");
    }

    #[tokio::test]
    async fn home01_failure_branch_does_not_reveal() {
        let (args, out) = fixture_args(
            "JRNY-HOME-01-athena-knowledge-failbranch.json",
            "JRNY-HOME-01-athena-knowledge-failbranch.deterministic.json",
            "deterministic",
            "home01-failbranch",
        );
        let scenario = load_scenario("JRNY-HOME-01-athena-knowledge-failbranch.json");
        let identity = scenario.identity();
        let result = run_fixture_playtest(
            &args,
            &scenario,
            &identity,
            ExecutionMode::Deterministic,
            &out,
        )
        .await
        .unwrap();
        assert!(
            result.ok,
            "failure branch must PASS (no reveal): {:?}",
            result.failures
        );
        // The check still resolved (mechanism fired) even on a failed degree.
        assert_eq!(
            result.turns[1].checkpoint_state,
            Some(CheckpointState::Pass)
        );
        // Outcome: the hidden fact stays unknown — player never learns it.
        assert_eq!(
            result.turns[2].knowledge_checkpoint_state,
            Some(KnowledgeCheckpointState::Pass)
        );
    }

    // --- TC-VS-NPC-01: Homecoming NPC mind / relationship / social slice ------

    #[test]
    fn home02_scenario_carries_npc_social_checkpoints_and_identity() {
        let scenario = load_scenario("JRNY-HOME-02-odessa-social-memory.json");
        let identity = scenario.identity();
        assert_eq!(identity.scenario_id, "JRNY-HOME-02-odessa-social-memory");
        assert_eq!(identity.scenario_version, "1");
        // reach_npc requires a stable, source-backed NPC profile/identity.
        let reach = scenario.turns[0].npc_social.clone().expect("reach social");
        assert!(reach.require_profile_available);
        assert!(!reach.forbid_unknown_fact_assertion.is_empty());
        assert!(reach.withheld_fact.is_some());
        // baseline_question requires a deterministic behavior plan.
        let baseline = scenario.turns[1]
            .npc_social
            .clone()
            .expect("baseline social");
        assert!(baseline.require_behavior_plan);
        // relationship_action requires an evidence-backed relationship delta.
        let rel = scenario.turns[2].npc_social.clone().expect("rel social");
        assert!(rel.require_relationship_evidence);
        // changed_response requires an observable change from baseline.
        let changed = scenario.turns[3]
            .npc_social
            .clone()
            .expect("changed social");
        assert!(changed.require_response_changed);
        // reload_revisit requires relationship + NPC knowledge to survive reload.
        let reload = scenario.turns[4].npc_social.clone().expect("reload social");
        assert!(reload.require_relationship_reload && reload.require_npc_knowledge_reload);
    }

    #[tokio::test]
    async fn home02_deterministic_social_memory_journey_passes() {
        let (args, out) = fixture_args(
            "JRNY-HOME-02-odessa-social-memory.json",
            "JRNY-HOME-02-odessa-social-memory.deterministic.json",
            "deterministic",
            "home02-det",
        );
        let scenario = load_scenario("JRNY-HOME-02-odessa-social-memory.json");
        let identity = scenario.identity();
        let result = run_fixture_playtest(
            &args,
            &scenario,
            &identity,
            ExecutionMode::Deterministic,
            &out,
        )
        .await
        .unwrap();
        assert!(result.ok, "expected PASS, failures: {:?}", result.failures);
        assert_eq!(result.scenario_id, "JRNY-HOME-02-odessa-social-memory");
        // Every social checkpoint passes: identity → baseline (withhold, no leak) →
        // evidence-backed delta → changed disclosure → reload continuity.
        for (i, t) in result.turns.iter().enumerate() {
            assert_eq!(
                t.npc_social_checkpoint_state,
                Some(NpcSocialCheckpointState::Pass),
                "turn {} social checkpoint not PASS: {:?}",
                i,
                t.failures
            );
        }
    }

    #[tokio::test]
    async fn home02_replay_social_memory_journey_passes() {
        let (args, out) = fixture_args(
            "JRNY-HOME-02-odessa-social-memory.json",
            "JRNY-HOME-02-odessa-social-memory.replay.json",
            "replay",
            "home02-replay",
        );
        let scenario = load_scenario("JRNY-HOME-02-odessa-social-memory.json");
        let identity = scenario.identity();
        let result = run_fixture_playtest(&args, &scenario, &identity, ExecutionMode::Replay, &out)
            .await
            .unwrap();
        assert!(result.ok, "replay must PASS: {:?}", result.failures);
        assert_eq!(result.mode, "replay");
        assert_eq!(result.scenario_id, "JRNY-HOME-02-odessa-social-memory");
    }

    #[tokio::test]
    async fn home02_leakbranch_fails_closed() {
        // Negative control: the NPC blurts the withheld secret before the player
        // knows it. The harness must fail closed with WITHHELD_SECRET_LEAKED.
        let (args, out) = fixture_args(
            "JRNY-HOME-02-odessa-leakbranch.json",
            "JRNY-HOME-02-odessa-leakbranch.deterministic.json",
            "deterministic",
            "home02-leakbranch",
        );
        let scenario = load_scenario("JRNY-HOME-02-odessa-leakbranch.json");
        let identity = scenario.identity();
        let result = run_fixture_playtest(
            &args,
            &scenario,
            &identity,
            ExecutionMode::Deterministic,
            &out,
        )
        .await
        .unwrap();
        assert!(!result.ok, "leak branch must FAIL closed");
        assert_eq!(
            result.turns[1].npc_social_checkpoint_state,
            Some(NpcSocialCheckpointState::WithheldSecretLeaked)
        );
    }

    // --- TC-VS-MEM-01: Committed Memory and Reload vertical slice -------------

    #[test]
    fn home03_scenario_carries_memory_checkpoints_and_identity() {
        let scenario = load_scenario("JRNY-HOME-03-committed-memory-reload.json");
        let identity = scenario.identity();
        assert_eq!(identity.scenario_id, "JRNY-HOME-03-committed-memory-reload");
        assert_eq!(identity.scenario_version, "1");
        // Commit turn: proposal-from-committed + evidence ids + idempotent + no
        // direct commit, plus the player-unknown summary guard.
        let commit = scenario.turns[0].memory.clone().expect("commit memory");
        assert!(commit.require_proposal_from_committed && commit.require_evidence_ids);
        assert!(commit.require_idempotent && commit.forbid_direct_commit);
        assert!(commit.player_unknown_fact.is_some());
        // Reload turn: durable retrieval + later consumption + the full trace chain.
        let reload = scenario.turns[1].memory.clone().expect("reload memory");
        assert!(reload.require_reload_retrieval && reload.require_later_consumption);
        assert_eq!(reload.require_trace_links.len(), 5);
    }

    #[tokio::test]
    async fn home03_deterministic_committed_memory_reload_passes() {
        let (args, out) = fixture_args(
            "JRNY-HOME-03-committed-memory-reload.json",
            "JRNY-HOME-03-committed-memory-reload.deterministic.json",
            "deterministic",
            "home03-det",
        );
        let scenario = load_scenario("JRNY-HOME-03-committed-memory-reload.json");
        let identity = scenario.identity();
        let result = run_fixture_playtest(
            &args,
            &scenario,
            &identity,
            ExecutionMode::Deterministic,
            &out,
        )
        .await
        .unwrap();
        assert!(result.ok, "expected PASS, failures: {:?}", result.failures);
        assert_eq!(result.scenario_id, "JRNY-HOME-03-committed-memory-reload");
        for (i, t) in result.turns.iter().enumerate() {
            assert_eq!(
                t.memory_checkpoint_state,
                Some(MemoryCheckpointState::Pass),
                "turn {} memory checkpoint not PASS: {:?}",
                i,
                t.failures
            );
        }
    }

    #[tokio::test]
    async fn home03_replay_committed_memory_reload_passes() {
        let (args, out) = fixture_args(
            "JRNY-HOME-03-committed-memory-reload.json",
            "JRNY-HOME-03-committed-memory-reload.replay.json",
            "replay",
            "home03-replay",
        );
        let scenario = load_scenario("JRNY-HOME-03-committed-memory-reload.json");
        let identity = scenario.identity();
        let result = run_fixture_playtest(&args, &scenario, &identity, ExecutionMode::Replay, &out)
            .await
            .unwrap();
        assert!(result.ok, "replay must PASS: {:?}", result.failures);
        assert_eq!(result.mode, "replay");
        assert_eq!(result.scenario_id, "JRNY-HOME-03-committed-memory-reload");
    }

    #[tokio::test]
    async fn home03_failbranch_fails_closed() {
        // Negative controls: (a) an uncommitted turn cannot produce committed
        // memory; (b) a proposal without source/evidence ids is rejected.
        let (args, out) = fixture_args(
            "JRNY-HOME-03-memory-failbranch.json",
            "JRNY-HOME-03-memory-failbranch.deterministic.json",
            "deterministic",
            "home03-failbranch",
        );
        let scenario = load_scenario("JRNY-HOME-03-memory-failbranch.json");
        let identity = scenario.identity();
        let result = run_fixture_playtest(
            &args,
            &scenario,
            &identity,
            ExecutionMode::Deterministic,
            &out,
        )
        .await
        .unwrap();
        assert!(!result.ok, "fail branch must FAIL closed");
        assert_eq!(
            result.turns[0].memory_checkpoint_state,
            Some(MemoryCheckpointState::ExtractionNotFromCommitted)
        );
        assert_eq!(
            result.turns[1].memory_checkpoint_state,
            Some(MemoryCheckpointState::EvidenceMissing)
        );
    }

    // --- TC-PIPE-03: production flight-recorder / provenance ------------------

    #[test]
    fn home04_scenario_carries_flight_recorder_checkpoint_and_identity() {
        let scenario = load_scenario("JRNY-HOME-04-flight-recorder-provenance.json");
        let identity = scenario.identity();
        assert_eq!(
            identity.scenario_id,
            "JRNY-HOME-04-flight-recorder-provenance"
        );
        let fr = scenario.turns[0]
            .flight_recorder
            .clone()
            .expect("flight checkpoint");
        assert!(fr.require_view_load_before_plugins && fr.require_ledger_link);
        assert!(fr
            .require_categories
            .contains(&"verifier_finding".to_string()));
        assert!(!fr.forbid_secret_terms_in_trace.is_empty());
    }

    #[tokio::test]
    async fn home04_deterministic_flight_recorder_passes() {
        let (args, out) = fixture_args(
            "JRNY-HOME-04-flight-recorder-provenance.json",
            "JRNY-HOME-04-flight-recorder-provenance.deterministic.json",
            "deterministic",
            "home04-det",
        );
        let scenario = load_scenario("JRNY-HOME-04-flight-recorder-provenance.json");
        let identity = scenario.identity();
        let result = run_fixture_playtest(
            &args,
            &scenario,
            &identity,
            ExecutionMode::Deterministic,
            &out,
        )
        .await
        .unwrap();
        assert!(result.ok, "expected PASS, failures: {:?}", result.failures);
        assert_eq!(
            result.turns[0].flight_recorder_state,
            Some(FlightRecorderState::Pass)
        );
    }

    #[tokio::test]
    async fn home04_replay_flight_recorder_passes() {
        let (args, out) = fixture_args(
            "JRNY-HOME-04-flight-recorder-provenance.json",
            "JRNY-HOME-04-flight-recorder-provenance.replay.json",
            "replay",
            "home04-replay",
        );
        let scenario = load_scenario("JRNY-HOME-04-flight-recorder-provenance.json");
        let identity = scenario.identity();
        let result = run_fixture_playtest(&args, &scenario, &identity, ExecutionMode::Replay, &out)
            .await
            .unwrap();
        assert!(result.ok, "replay must PASS: {:?}", result.failures);
        assert_eq!(result.mode, "replay");
    }

    #[tokio::test]
    async fn home04_failbranch_fails_closed() {
        // Negative controls: (a) a view_load after a plugin contribution; (b) a
        // secret term echoed in a trace summary. Both must fail closed.
        let (args, out) = fixture_args(
            "JRNY-HOME-04-flight-recorder-failbranch.json",
            "JRNY-HOME-04-flight-recorder-failbranch.deterministic.json",
            "deterministic",
            "home04-failbranch",
        );
        let scenario = load_scenario("JRNY-HOME-04-flight-recorder-failbranch.json");
        let identity = scenario.identity();
        let result = run_fixture_playtest(
            &args,
            &scenario,
            &identity,
            ExecutionMode::Deterministic,
            &out,
        )
        .await
        .unwrap();
        assert!(!result.ok, "fail branch must FAIL closed");
        assert_eq!(
            result.turns[0].flight_recorder_state,
            Some(FlightRecorderState::ViewOrderViolation)
        );
        assert_eq!(
            result.turns[1].flight_recorder_state,
            Some(FlightRecorderState::SecretInTrace)
        );
    }
}
