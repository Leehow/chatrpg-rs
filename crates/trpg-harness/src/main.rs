use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use chrono::Utc;
use sqlx::{PgPool, Row};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

#[derive(Debug, Parser)]
#[command(name = "trpg-harness", version, about = "Rust-native regression harness for ChatRPG CLI/API streams")]
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

#[derive(Debug, Clone, Deserialize)]
struct HarnessCase {
    name: String,
    ruleset_id: String,
    #[serde(default)]
    module_id: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    user_input: String,
    #[serde(default)]
    recent_transcript: Option<String>,
    #[serde(default)]
    turns: Vec<HarnessTurn>,
    #[serde(default)]
    forbidden_terms: Vec<String>,
    #[serde(default)]
    required_terms: Vec<String>,
    #[serde(default)]
    required_events: Vec<String>,
    #[serde(default)]
    forbidden_events: Vec<String>,
    #[serde(default)]
    required_event_contains: Vec<EventContainsAssertion>,
    #[serde(default)]
    required_done_reason: Option<String>,
    #[serde(default)]
    require_no_llm_stream_start: bool,
    #[serde(default = "default_stream_format")]
    stream_format: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct HarnessTurn {
    user_input: String,
    #[serde(default)]
    forbidden_terms: Vec<String>,
    #[serde(default)]
    required_terms: Vec<String>,
    #[serde(default)]
    required_events: Vec<String>,
    #[serde(default)]
    forbidden_events: Vec<String>,
    #[serde(default)]
    required_event_contains: Vec<EventContainsAssertion>,
    #[serde(default)]
    required_done_reason: Option<String>,
    #[serde(default)]
    require_no_llm_stream_start: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct EventContainsAssertion {
    event: String,
    contains: String,
}

fn default_stream_format() -> String { "jsonl".to_string() }

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
            let result = run_case_path(&args.case, &args.bin, &args.cwd, args.timeout_secs, args.stream_format.as_deref(), args.verbose).await?;
            emit_result(&result, &args.output)?;
            if result.ok { Ok(()) } else { Err(anyhow!("harness case failed: {}", result.case)) }
        }
        Commands::Playtest(args) => {
            let result = run_playtest(&args).await?;
            emit_playtest_result(&result, &args.output)?;
            if result.ok { Ok(()) } else { Err(anyhow!("playtest failed: {}", result.scenario)) }
        }
        Commands::Suite(args) => {
            let mut paths = collect_case_paths(&args.dir)?;
            paths.sort();
            if paths.is_empty() {
                return Err(anyhow!("no harness case JSON files found in {}", args.dir.display()));
            }
            let mut results = Vec::new();
            let mut failed = false;
            for path in paths {
                let result = run_case_path(&path, &args.bin, &args.cwd, args.timeout_secs, None, args.verbose).await?;
                failed |= !result.ok;
                emit_result(&result, &args.output)?;
                let stop = args.fail_fast && !result.ok;
                results.push(result);
                if stop { break; }
            }
            if matches!(args.output, OutputMode::Text) {
                let passed = results.iter().filter(|r| r.ok).count();
                let failed_count = results.len() - passed;
                eprintln!("suite complete: {passed} passed, {failed_count} failed");
            }
            if failed { Err(anyhow!("harness suite failed")) } else { Ok(()) }
        }
    }
}


#[derive(Debug, Clone, Serialize, Deserialize)]
struct PlaytestScenario {
    name: String,
    ruleset_id: String,
    #[serde(default)]
    module_id: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
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
}

#[derive(Debug, Clone, Serialize)]
struct PlaytestResult {
    ok: bool,
    scenario: String,
    session_id: Option<String>,
    output_dir: String,
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
    let mut scenario: PlaytestScenario = serde_json::from_str(&scenario_text)
        .with_context(|| format!("invalid playtest scenario JSON: {}", args.scenario.display()))?;
    if scenario.turns.is_empty() {
        return Err(anyhow!("playtest scenario has no turns: {}", args.scenario.display()));
    }

    let run_id = format!("{}-{}", sanitize_name(&scenario.name), Utc::now().format("%Y%m%dT%H%M%SZ"));
    let out_dir = args.output_dir.clone().unwrap_or_else(|| args.cwd.join("data").join("playtests").join(run_id));
    tokio::fs::create_dir_all(&out_dir).await?;
    tokio::fs::write(out_dir.join("scenario.json"), serde_json::to_vec_pretty(&scenario)?).await?;

    let pool = maybe_connect_db(&args.cwd).await;
    let mut results = Vec::new();
    let mut all_failures = Vec::new();
    let mut recent_transcript = String::new();
    let mut debug_state: BTreeMap<String, Value> = BTreeMap::new();

    for idx in 0..scenario.turns.len() {
        let turn = scenario.turns[idx].clone();
        let turn_dir = out_dir.join(format!("turn_{:03}", idx + 1));
        tokio::fs::create_dir_all(&turn_dir).await?;
        tokio::fs::write(turn_dir.join("player_input_raw.txt"), &turn.user_input).await?;

        let debug_allowed = args.enable_debug_directives || scenario.allow_debug_directives;
        let mut debug_result = if debug_allowed {
            apply_debug_directives(&turn.user_input, &mut debug_state)
        } else {
            DebugDirectiveResult { stripped_input: turn.user_input.clone(), state: debug_state.clone(), ..Default::default() }
        };
        let clean_input = normalize_player_input(&debug_result.stripped_input);
        debug_result.stripped_input = clean_input.clone();
        debug_result.state = debug_state.clone();
        write_json(turn_dir.join("debug_directives.json"), &debug_result).await?;
        tokio::fs::write(turn_dir.join("player_input.txt"), &clean_input).await?;

        let before = db_snapshot(pool.as_ref(), scenario.session_id.as_deref()).await;
        write_json(turn_dir.join("db_snapshot_before.json"), &before).await?;

        let execution = execute_turn_process(
            &args.bin,
            &args.cwd,
            args.timeout_secs,
            &scenario.ruleset_id,
            scenario.module_id.clone(),
            scenario.session_id.clone(),
            &clean_input,
            build_recent_transcript_context(&recent_transcript, &debug_state),
        ).await?;

        if let Some(session_id) = execution.parsed.session_id.clone() {
            scenario.session_id = Some(session_id);
        }

        let after = db_snapshot(pool.as_ref(), scenario.session_id.as_deref()).await;
        let diff = db_diff(&before, &after);
        write_json(turn_dir.join("db_snapshot_after.json"), &after).await?;
        write_json(turn_dir.join("db_diff.json"), &diff).await?;
        tokio::fs::write(turn_dir.join("events.jsonl"), &execution.stdout_text).await?;
        tokio::fs::write(turn_dir.join("stderr.txt"), &execution.stderr_text).await?;
        tokio::fs::write(turn_dir.join("player_visible_output.txt"), &execution.parsed.body).await?;

        let mut turn_failures = execution.parsed.failures.clone();
        turn_failures.extend(debug_result.failures.iter().map(|f| format!("debug directive error: {f}")));
        let enforce_human_input = args.require_human_player_input && !scenario.allow_nonhuman_player_input && !turn.allow_nonhuman_player_input;
        if enforce_human_input {
            let allow_manual_roll = scenario.allow_manual_roll_input || turn.allow_manual_roll_input;
            for finding in human_player_input_findings(&clean_input, allow_manual_roll) {
                turn_failures.push(format!("non-human player input: {finding}"));
            }
        }
        if execution.timed_out { turn_failures.push(format!("turn timed out after {}s", args.timeout_secs)); }
        if execution.status_code.unwrap_or(1) != 0 { turn_failures.push(format!("trpg exited with non-zero status: {:?}", execution.status_code)); }
        for phase in &turn.expect_phases {
            let name = if phase.starts_with("phase:") || phase.starts_with("event:") { phase.clone() } else { format!("phase:{phase}") };
            if !execution.parsed.events.iter().any(|e| e == &name) {
                turn_failures.push(format!("missing expected phase/event: {name}"));
            }
        }
        for phase in &turn.forbid_phases {
            let name = if phase.starts_with("phase:") || phase.starts_with("event:") { phase.clone() } else { format!("phase:{phase}") };
            if execution.parsed.events.iter().any(|e| e == &name) {
                turn_failures.push(format!("forbidden phase/event appeared: {name}"));
            }
        }
        for forbidden in scenario.forbidden_player_text.iter().chain(turn.forbid_player_text.iter()) {
            if !forbidden.is_empty() && execution.parsed.body.contains(forbidden) {
                turn_failures.push(format!("forbidden player-visible text appeared: {forbidden}"));
            }
        }
        if !(scenario.allow_manual_roll_input || turn.allow_manual_roll_input) {
            for finding in player_visible_roll_request_findings(&execution.parsed.body) {
                turn_failures.push(format!("player-visible manual roll request: {finding}"));
            }
        }
        for (table, min_delta) in &turn.require_db_deltas {
            let actual = diff.get("count_delta").and_then(|d| d.get(table)).and_then(Value::as_i64).unwrap_or(0);
            if actual < *min_delta {
                turn_failures.push(format!("DB delta for {table} was {actual}, expected >= {min_delta}"));
            }
        }

        let eval_prompt = build_eval_prompt(&scenario, &turn, &clean_input, &debug_result, &execution, &before, &after, &diff);
        tokio::fs::write(turn_dir.join("eval_prompt.md"), &eval_prompt).await?;
        let mut evaluator_verdict = None;
        if args.evaluator == "claude-code" {
            match run_claude_evaluator(&args.claude_bin, &eval_prompt).await {
                Ok(value) => {
                    evaluator_verdict = value.get("verdict").and_then(Value::as_str).map(str::to_string);
                    write_json(turn_dir.join("eval_result.json"), &value).await?;
                }
                Err(err) => {
                    let value = json!({"verdict":"evaluator_error","error": err.to_string()});
                    write_json(turn_dir.join("eval_result.json"), &value).await?;
                    if args.evaluator_required { turn_failures.push(format!("evaluator failed: {err}")); }
                }
            }
        } else {
            write_json(turn_dir.join("eval_result.json"), &json!({"verdict":"not_run","evaluator":"none"})).await?;
        }

        if args.verbose && !turn_failures.is_empty() {
            tokio::fs::write(turn_dir.join("stdout_tail.txt"), tail_chars(&execution.stdout_text, 4000)).await?;
            tokio::fs::write(turn_dir.join("stderr_tail.txt"), tail_chars(&execution.stderr_text, 4000)).await?;
        }

        all_failures.extend(turn_failures.iter().map(|f| format!("turn {}: {f}", idx + 1)));
        recent_transcript.push_str(&format!("\nPlayer: {}\nGM: {}\n", clean_input.trim(), execution.parsed.body.trim()));
        recent_transcript = tail_chars(&recent_transcript, 12000);
        results.push(PlaytestTurnResult {
            turn: idx + 1,
            user_input: clean_input.clone(),
            events: execution.parsed.events,
            chars: execution.parsed.body.chars().count(),
            db_diff: diff,
            evaluator_verdict,
            failures: turn_failures,
        });
    }

    let result = PlaytestResult {
        ok: all_failures.is_empty(),
        scenario: scenario.name,
        session_id: scenario.session_id,
        output_dir: out_dir.to_string_lossy().to_string(),
        turns: results,
        failures: all_failures,
    };
    write_json(out_dir.join("playtest_result.json"), &result).await?;
    Ok(result)
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

    let mut stdout = child.stdout.take().ok_or_else(|| anyhow!("failed to capture stdout"))?;
    let mut stderr = child.stderr.take().ok_or_else(|| anyhow!("failed to capture stderr"))?;
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
    Ok(TurnExecution { stdout_text, stderr_text, parsed, status_code, timed_out })
}

async fn maybe_connect_db(cwd: &Path) -> Option<PgPool> {
    let from_env = std::env::var("DATABASE_URL").ok();
    let from_dotenv = read_database_url_from_dotenv(cwd).ok().flatten();
    let url = from_env.or(from_dotenv)?;
    PgPool::connect(&url).await.ok()
}

fn read_database_url_from_dotenv(cwd: &Path) -> Result<Option<String>> {
    let path = cwd.join(".env");
    if !path.exists() { return Ok(None); }
    let iter = dotenvy::from_path_iter(&path)?;
    for item in iter {
        let (key, value) = item?;
        if key == "DATABASE_URL" { return Ok(Some(value)); }
    }
    Ok(None)
}

async fn db_snapshot(pool: Option<&PgPool>, session_id: Option<&str>) -> DbSnapshot {
    let Some(pool) = pool else {
        return DbSnapshot { ok: false, error: Some("DATABASE_URL not configured or connection failed".into()), ..Default::default() };
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
    ];
    let mut snap = DbSnapshot { ok: true, ..Default::default() };
    for table in tables {
        if !table_exists(pool, table).await.unwrap_or(false) { continue; }
        let count = count_table(pool, table, session_id).await.unwrap_or(0);
        snap.counts.insert(table.to_string(), count);
        let latest = latest_rows(pool, table, session_id, 5).await.unwrap_or_default();
        snap.latest.insert(table.to_string(), latest);
    }
    snap
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
    if let Some(id) = session_id.filter(|_| sql.contains("$1")) { query = query.bind(id); }
    Ok(query.fetch_one(pool).await?)
}

async fn latest_rows(pool: &PgPool, table: &str, session_id: Option<&str>, limit: i64) -> Result<Vec<Value>> {
    let has_session = session_id.is_some() && table_has_session_id(pool, table).await.unwrap_or(false);
    let sql = if has_session {
        format!("select to_jsonb(t) as row_json from (select * from {table} where session_id = $1 order by created_at desc nulls last limit $2) t")
    } else {
        format!("select to_jsonb(t) as row_json from (select * from {table} order by created_at desc nulls last limit $1) t")
    };
    let rows = if has_session {
        sqlx::query(&sql).bind(session_id.unwrap()).bind(limit).fetch_all(pool).await?
    } else {
        sqlx::query(&sql).bind(limit).fetch_all(pool).await?
    };
    Ok(rows.into_iter().filter_map(|row| row.try_get::<Value, _>("row_json").ok()).collect())
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
        debug_directives = serde_json::to_string_pretty(&debug_result.directives).unwrap_or_default(),
        debug_state = serde_json::to_string_pretty(&debug_result.state).unwrap_or_default(),
        body = execution.parsed.body,
        events = execution.parsed.events.join(", "),
        diff = serde_json::to_string_pretty(diff).unwrap_or_default(),
        before = serde_json::to_string_pretty(before).unwrap_or_default(),
        after = serde_json::to_string_pretty(after).unwrap_or_default(),
    )
}


fn normalize_player_input(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ").trim().to_string()
}

fn apply_debug_directives(input: &str, state: &mut BTreeMap<String, Value>) -> DebugDirectiveResult {
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
                let record = DebugDirectiveRecord { action: action.clone(), payload: payload.clone(), raw: raw.clone() };
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
    DebugDirectiveResult { stripped_input: stripped, directives, changes, failures, state: state.clone() }
}

fn parse_debug_directive(raw: &str) -> Result<(String, Value)> {
    let trimmed = raw.trim();
    if trimmed.is_empty() { return Err(anyhow!("empty debug directive")); }
    if trimmed.starts_with('{') {
        let value: Value = serde_json::from_str(trimmed)?;
        let action = value.get("action").or_else(|| value.get("op")).and_then(Value::as_str).unwrap_or("add").to_string();
        return Ok((action, value));
    }
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let action = parts.next().unwrap_or("add").trim().to_ascii_lowercase();
    let payload_text = parts.next().unwrap_or("{}").trim();
    let payload = if payload_text.is_empty() { json!({}) } else { serde_json::from_str(payload_text)? };
    Ok((action, payload))
}

fn apply_debug_directive_to_state(action: &str, payload: Value, state: &mut BTreeMap<String, Value>, counter: &mut usize) -> Result<Value> {
    match action {
        "add" | "insert" | "create" => {
            *counter += 1;
            let id = debug_payload_id(&payload).unwrap_or_else(|| format!("debug_{}", counter));
            state.insert(id.clone(), payload.clone());
            Ok(json!({"action":"add", "id": id, "payload": payload}))
        }
        "set" | "replace" => {
            let id = debug_payload_id(&payload).ok_or_else(|| anyhow!("set/replace requires id"))?;
            let value = payload.get("value").or_else(|| payload.get("data")).cloned().unwrap_or_else(|| payload.clone());
            state.insert(id.clone(), value.clone());
            Ok(json!({"action":"set", "id": id, "value": value}))
        }
        "patch" | "update" | "modify" => {
            let id = debug_payload_id(&payload).ok_or_else(|| anyhow!("patch/update requires id"))?;
            let patch = payload.get("patch").or_else(|| payload.get("data")).cloned().unwrap_or_else(|| payload.clone());
            let entry = state.entry(id.clone()).or_insert_with(|| json!({"id": id.clone()}));
            merge_json_value(entry, &patch);
            Ok(json!({"action":"patch", "id": id, "patch": patch, "after": entry.clone()}))
        }
        "delete" | "remove" => {
            let id = debug_payload_id(&payload).or_else(|| payload.as_str().map(str::to_string)).ok_or_else(|| anyhow!("delete/remove requires id"))?;
            let removed = state.remove(&id);
            Ok(json!({"action":"delete", "id": id, "removed": removed}))
        }
        "clear" => {
            let previous = state.len();
            state.clear();
            Ok(json!({"action":"clear", "removed_count": previous}))
        }
        other => Err(anyhow!("unsupported debug action `{other}`; supported: add, set, patch, delete, clear")),
    }
}

fn debug_payload_id(payload: &Value) -> Option<String> {
    payload.get("id").or_else(|| payload.get("debug_id")).or_else(|| payload.get("entity_id")).and_then(Value::as_str).map(str::to_string)
        .or_else(|| payload.get("data").and_then(|v| v.get("id")).and_then(Value::as_str).map(str::to_string))
        .or_else(|| payload.get("row").and_then(|v| v.get("id")).and_then(Value::as_str).map(str::to_string))
        .or_else(|| payload.get("name").and_then(Value::as_str).map(|s| format!("name:{}", sanitize_name(s))))
}

fn merge_json_value(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(target_map), Value::Object(patch_map)) => {
            for (key, patch_value) in patch_map {
                if patch_value.is_null() {
                    target_map.remove(key);
                } else {
                    merge_json_value(target_map.entry(key.clone()).or_insert(Value::Null), patch_value);
                }
            }
        }
        (target_slot, patch_value) => *target_slot = patch_value.clone(),
    }
}

fn build_recent_transcript_context(recent_transcript: &str, debug_state: &BTreeMap<String, Value>) -> Option<String> {
    let mut parts = Vec::new();
    if !recent_transcript.trim().is_empty() { parts.push(recent_transcript.to_string()); }
    if !debug_state.is_empty() {
        parts.push(format!(
            "[GM-ONLY TEST DEBUG STATE — do not reveal this label to the player. Treat these JSON entities/items/actors/clocks as present for this test unless later debug directives modify/delete them.]\n{}",
            serde_json::to_string_pretty(debug_state).unwrap_or_default()
        ));
    }
    if parts.is_empty() { None } else { Some(parts.join("\n\n")) }
}

fn human_player_input_findings(input: &str, allow_manual_roll_input: bool) -> Vec<String> {
    let trimmed = input.trim();
    let mut findings = Vec::new();
    if trimmed.is_empty() { return findings; }
    let lower = trimmed.to_ascii_lowercase();
    if trimmed.contains('{') || trimmed.contains('}') || trimmed.contains("```") {
        findings.push("contains JSON/code-like syntax; normal players should describe fictional actions in natural language".into());
    }
    let internal_terms = [
        "phase:", "event:", "checkcontract", "effectcontract", "parameterimpact", "actor_mechanical_states", "damage_packets",
        "db", "database", "sql", "json", "schema", "function", "cargo", "harness", "expected", "assert", "test case",
        "roll_plan", "rule_binding", "materialization", "semantic route", "api", "sse"
    ];
    for term in internal_terms {
        if lower.contains(term) {
            findings.push(format!("contains internal/test-engineering term `{term}`"));
        }
    }
    if !allow_manual_roll_input {
        for term in ["/roll", "掷骰", "骰", "d6", "d10", "d20", "1d", "2d", "3d", "4d", "5d", "roll result", "i rolled", "total", "合计", "总共", "掷出来"] {
            if lower.contains(term) || trimmed.contains(term) {
                findings.push(format!("contains manual dice/result language `{term}`; product-mode players should describe only fictional actions and let the system roll/resolve automatically"));
            }
        }
    }
    let comma_count = trimmed.matches('，').count() + trimmed.matches(',').count() + trimmed.matches(';').count() + trimmed.matches('；').count();
    if comma_count >= 5 && lower.contains("测试") {
        findings.push("looks like a bundled QA probe rather than one human player action".into());
    }
    findings
}


fn player_visible_roll_request_findings(output: &str) -> Vec<String> {
    let visible = strip_tagged_sections(output, &["roll", "gm", "system"]);
    let lower = visible.to_ascii_lowercase();
    let mut findings = Vec::new();
    let patterns = [
        "请掷", "请投", "你来掷", "你来投", "告诉我点数", "告诉我结果", "给出总值", "给我总值",
        "掷骰结果", "投骰结果", "roll and tell", "roll the dice", "make a roll", "give me the roll",
        "tell me your total", "report the total", "what did you roll", "/roll ", "d10+", "d20+", "d6+",
    ];
    for pat in patterns {
        if lower.contains(pat) || visible.contains(pat) {
            findings.push(format!("contains `{pat}` outside allowed [roll]/[system]/[gm] tags"));
        }
    }
    findings
}

fn strip_tagged_sections(input: &str, tags: &[&str]) -> String {
    let mut out = input.to_string();
    for tag in tags {
        loop {
            let open = format!("[{}]", tag);
            let close = format!("[/{}]", tag);
            let Some(start) = out.to_ascii_lowercase().find(&open) else { break; };
            let rest_lower = out[start + open.len()..].to_ascii_lowercase();
            let Some(rel_end) = rest_lower.find(&close) else { break; };
            let end = start + open.len() + rel_end + close.len();
            out.replace_range(start..end, "");
        }
    }
    out
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
    let schema_file = std::env::temp_dir().join(format!("trpg-eval-schema-{}.json", Utc::now().timestamp_nanos_opt().unwrap_or_default()));
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
        return Err(anyhow!("claude evaluator exited {:?}: {}", output.status.code(), tail_chars(&stderr, 1200)));
    }
    serde_json::from_str::<Value>(&stdout).with_context(|| format!("claude evaluator did not return JSON: {}", tail_chars(&stdout, 1200)))
}

async fn write_json(path: PathBuf, value: &impl Serialize) -> Result<()> {
    tokio::fs::write(path, serde_json::to_vec_pretty(value)?).await?;
    Ok(())
}

fn sanitize_name(input: &str) -> String {
    let out: String = input.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    out.trim_matches('-').chars().take(80).collect()
}

fn emit_playtest_result(result: &PlaytestResult, mode: &OutputMode) -> Result<()> {
    match mode {
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(result)?),
        OutputMode::Jsonl => println!("{}", serde_json::to_string(result)?),
        OutputMode::Text => {
            if result.ok {
                println!("PASS playtest {} ({} turns) -> {}", result.scenario, result.turns.len(), result.output_dir);
            } else {
                println!("FAIL playtest {} -> {}", result.scenario, result.output_dir);
                for failure in &result.failures { println!("  - {failure}"); }
            }
        }
    }
    Ok(())
}

fn collect_case_paths(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("failed to read harness dir {}", dir.display()))? {
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

async fn run_turn_case(mut case: HarnessCase, bin: &Path, cwd: &Path, timeout_secs: u64, verbose: bool) -> Result<HarnessResult> {
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
        if !turn.forbidden_terms.is_empty() { single.forbidden_terms = turn.forbidden_terms; }
        if !turn.required_terms.is_empty() { single.required_terms = turn.required_terms; }
        if !turn.required_events.is_empty() { single.required_events = turn.required_events; }
        if !turn.forbidden_events.is_empty() { single.forbidden_events = turn.forbidden_events; }
        if !turn.required_event_contains.is_empty() { single.required_event_contains = turn.required_event_contains; }
        if turn.required_done_reason.is_some() { single.required_done_reason = turn.required_done_reason; }
        if turn.require_no_llm_stream_start { single.require_no_llm_stream_start = true; }

        let result = run_single_turn_case(single, bin, cwd, timeout_secs, verbose).await?;
        if let Some(session_id) = result.session_id.clone() {
            case.session_id = Some(session_id);
        }
        chars += result.chars;
        all_events.extend(result.events.iter().map(|e| format!("turn{}:{}", idx + 1, e)));
        failures.extend(result.failures);
        forbidden_hits.extend(result.forbidden_hits);
        missing_terms.extend(result.missing_terms);
        missing_events.extend(result.missing_events);
        if result.process_status != Some(0) { status = result.process_status; }
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

async fn run_single_turn_case(case: HarnessCase, bin: &Path, cwd: &Path, timeout_secs: u64, verbose: bool) -> Result<HarnessResult> {
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

    let mut stdout = child.stdout.take().ok_or_else(|| anyhow!("failed to capture stdout"))?;
    let mut stderr = child.stderr.take().ok_or_else(|| anyhow!("failed to capture stderr"))?;

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
    let body = parsed.body;
    let events = parsed.events;
    let mut failures = parsed.failures;
    let mut forbidden_hits = Vec::new();
    for term in &case.forbidden_terms {
        if !term.is_empty() && body.contains(term) {
            forbidden_hits.push(term.clone());
            failures.push(format!("forbidden term appeared in player-facing delta stream: {term}"));
        }
    }
    let mut missing_terms = Vec::new();
    for term in &case.required_terms {
        if !term.is_empty() && !body.contains(term) {
            missing_terms.push(term.clone());
            failures.push(format!("required term did not appear in player-facing delta stream: {term}"));
        }
    }
    let mut missing_events = Vec::new();
    for expected in &case.required_events {
        if !events.iter().any(|actual| actual == expected) {
            missing_events.push(expected.clone());
            failures.push(format!("missing required event: {expected}"));
        }
    }
    for forbidden in &case.forbidden_events {
        if events.iter().any(|actual| actual == forbidden) {
            failures.push(format!("forbidden event appeared: {forbidden}"));
        }
    }
    if case.require_no_llm_stream_start && events.iter().any(|e| e == "phase:llm_stream_start") {
        failures.push("llm_stream_start appeared but this case expected the Rust Agent to stop before narration".into());
    }
    if let Some(reason) = &case.required_done_reason {
        let ok = parsed.raw_events.iter().any(|event| {
            event.get("event").and_then(Value::as_str) == Some("phase")
                && event.get("phase").and_then(Value::as_str) == Some("done")
                && event.get("data").and_then(|d| d.get("reason")).and_then(Value::as_str) == Some(reason.as_str())
        });
        if !ok { failures.push(format!("missing done reason: {reason}")); }
    }
    for assertion in &case.required_event_contains {
        let ok = parsed.raw_events.iter().any(|event| {
            let name = event.get("event").and_then(Value::as_str).unwrap_or_default();
            let phase_name = if name == "phase" { event.get("phase").and_then(Value::as_str).map(|p| format!("phase:{p}")) } else { Some(format!("event:{name}")) };
            phase_name.as_deref() == Some(assertion.event.as_str()) && event.to_string().contains(&assertion.contains)
        });
        if !ok { failures.push(format!("event {} did not contain `{}`", assertion.event, assertion.contains)); }
    }
    if timed_out {
        failures.push(format!("case timed out after {timeout_secs}s"));
    }
    if status_code.unwrap_or(1) != 0 {
        failures.push(format!("trpg exited with non-zero status: {:?}", status_code));
    }

    Ok(HarnessResult {
        ok: failures.is_empty(),
        case: case.name,
        session_id: parsed.session_id,
        events,
        chars: body.chars().count(),
        failures,
        forbidden_hits,
        missing_terms,
        missing_events,
        process_status: status_code,
        timed_out,
        stdout_tail: if verbose { Some(tail_chars(&stdout_text, 4000)) } else { None },
        stderr_tail: if verbose { Some(tail_chars(&stderr_text, 4000)) } else { None },
    })
}

#[derive(Debug)]
struct ParsedJsonlStream {
    events: Vec<String>,
    raw_events: Vec<Value>,
    body: String,
    failures: Vec<String>,
    session_id: Option<String>,
}

fn parse_jsonl_events(stdout_text: &str) -> ParsedJsonlStream {
    let mut events = Vec::new();
    let mut raw_events = Vec::new();
    let mut body = String::new();
    let mut failures = Vec::new();
    let mut session_id = None;
    for (line_no, raw) in stdout_text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() { continue; }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            failures.push(format!("stdout line {} is not valid JSONL: {}", line_no + 1, tail_chars(line, 240)));
            continue;
        };
        raw_events.push(value.clone());
        match value.get("event").and_then(Value::as_str) {
            Some("phase") => {
                if let Some(phase) = value.get("phase").and_then(Value::as_str) {
                    if phase == "session" {
                        if let Some(id) = value.get("data").and_then(|d| d.get("session_id")).and_then(Value::as_str) {
                            session_id = Some(id.to_string());
                        }
                    }
                    events.push(format!("phase:{phase}"));
                }
            }
            Some("delta") => {
                if let Some(delta) = value.get("data").and_then(Value::as_str) {
                    body.push_str(delta);
                }
            }
            Some("error") => {
                failures.push(format!("trpg emitted error event: {}", value));
            }
            Some(other) => events.push(format!("event:{other}")),
            None => failures.push(format!("stdout line {} missing event field: {}", line_no + 1, value)),
        }
    }
    ParsedJsonlStream { events, raw_events, body, failures, session_id }
}

fn emit_result(result: &HarnessResult, mode: &OutputMode) -> Result<()> {
    match mode {
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(result)?),
        OutputMode::Jsonl => println!("{}", serde_json::to_string(result)?),
        OutputMode::Text => {
            if result.ok {
                println!("PASS {} ({} chars, events: {})", result.case, result.chars, result.events.join(", "));
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

fn tail_chars(input: &str, max_chars: usize) -> String {
    let mut chars: Vec<char> = input.chars().rev().take(max_chars).collect();
    chars.reverse();
    chars.into_iter().collect()
}
