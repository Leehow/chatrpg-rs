use anyhow::{anyhow, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use dialoguer::{Input, Password, Select};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::future::Future;
use std::io::{self, IsTerminal, Read, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use trpg_api::{serve, AppState};
use trpg_db::Db;
use trpg_gm::{
    execute_turn, GmLoop, LoopConfig, OwnedTurnRequest, SceneDeepExtractFn, ToolRegistry,
    TurnEvent, TurnOutcome, CANONICAL_TURN_PLAN,
};
use trpg_llm::{LlmClient, LlmConfig, OpenAiCompatibleClient};
use trpg_model::*;
use trpg_parser::{ParserConfig, ProjectParseService};
use trpg_rule_agent::RuleStewardAgent;
use trpg_runtime::scene_navigation::extract_module_scenes;
use trpg_runtime::{roll_dice, validate_character_template_sheet, RuntimeEngine};
use trpg_search::{load_search_source_configs, SearchConfig, SearchService};

mod agent_play;

#[derive(Debug, Parser)]
#[command(name = "trpg", version, about = "Rust TRPG rulebook/module parser and terminal runtime")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Init,
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    Migrate,
    ParseAll {
        #[arg(long, default_value_t = false)]
        force: bool,
        /// Opt into the old expensive full chunk extraction path. Default is GM onboarding + locators only.
        #[arg(long, default_value_t = false)]
        full_parse: bool,
        /// PDF extraction backend. duotext (DEFAULT): two pdftotext views — reading-order prose
        /// (indexed) + a -layout table sidecar (param grep); fast, pure-CPU, no ligature loss.
        /// mineru: vision model — native 2-col order but ~500x slower and row-shifts dense tables.
        /// oxidize/auto: rag_chunks collapses tables — avoid for table-heavy books. (pdftotext=duotext alias)
        #[arg(long, value_parser = ["auto", "oxidize", "pdftotext", "duotext", "mineru"], default_value = "duotext")]
        pdf_backend: String,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    Api {
        #[arg(long, env = "TRPG_API_ADDR", default_value = "127.0.0.1:8787")]
        addr: SocketAddr,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    Inspect {
        #[command(subcommand)]
        command: InspectCommand,
    },
    CreateCharacter(CreateCharacterArgs),
    Play {
        #[arg(long)]
        ruleset: String,
        #[arg(long)]
        module: Option<String>,
    },
    /// Run exactly one GM turn without opening the interactive play shell.
    /// Useful for pipes, regression tests, scripts, and LLM-driven debugging.
    Turn(TurnArgs),
    /// Unified Tantivy search. Short alias inspired by ripgrep.
    Rg(SearchArgs),
    /// Grep the duotext `.layout.md` table sidecars for exact aligned rows (e.g. weapon params).
    /// DB-free: `trpg grep-table "Glock 17"` -> the full stat row. AND-of-tokens, table rows first.
    GrepTable {
        /// term(s) to match on a table row, e.g. "Glock 17" or "revolver 38"
        query: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// Apply a character-growth change: set/add a track (class level, pool, rank) or a
    /// base stat, then re-derive. Engine only moves the value (A0). e.g.
    /// `trpg grow --session S --actor pc.current --bucket tracks --id fighter --op add --amount 1 --kind class_level`
    Grow {
        #[arg(long)] session: String,
        #[arg(long, default_value = "pc.current")] actor: String,
        #[arg(long, value_parser = ["tracks", "stats", "skills", "field"], default_value = "tracks")] bucket: String,
        #[arg(long)] id: String,
        #[arg(long, value_parser = ["set", "add"], default_value = "add")] op: String,
        /// Amount to set/add. Negative allowed (e.g. spend a pool): `--amount -60`.
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)] amount: f64,
        /// A categorical/string value to SET (e.g. a race/ancestry/class choice). Use instead of --amount for non-numeric inputs.
        #[arg(long)] value: Option<String>,
        #[arg(long)] kind: Option<String>,
        #[arg(long)] category: Option<String>,
    },
    Search {
        #[command(subcommand)]
        command: SearchCommand,
    },
    /// Rule Steward Agent: source-backed rule query, character onboarding, and playability audit.
    Rules {
        #[command(subcommand)]
        command: RulesCommand,
    },
    Learn {
        #[command(subcommand)]
        command: LearnCommand,
    },
    Time {
        #[command(subcommand)]
        command: TimeCommand,
    },
    Roll {
        expression: String,
    },
    /// Run the STAGED ruleset parse in-process and stream stage progress to the terminal.
    ParseStaged {
        #[arg(long)]
        ruleset: String,
        #[arg(long)]
        data_dir: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        stage1_only: bool,
        #[arg(long, default_value_t = false)]
        json: bool,
        #[arg(long, default_value_t = 9)]
        budget: usize,
    },
}

#[derive(Debug, Args)]
struct CreateCharacterArgs {
    /// Ruleset id. May also be supplied by --request-json.
    #[arg(long)]
    ruleset: Option<String>,
    /// Module id. May also be supplied by --request-json.
    #[arg(long)]
    module: Option<String>,
    /// User preferences / partial character choices. Skips interactive prompt.
    #[arg(long)]
    preferences: Option<String>,
    /// Read user preferences from a file. Skips interactive prompt.
    #[arg(long)]
    preferences_file: Option<PathBuf>,
    /// Read user preferences from stdin as plain text. Skips interactive prompt.
    #[arg(long, default_value_t = false)]
    stdin: bool,
    /// Read a JSON request from this path, or '-' for stdin.
    /// Shape: {"ruleset_id":"...","module_id":"...","user_preferences":"..."}
    #[arg(long)]
    request_json: Option<String>,
    /// Output stream format for automation.
    #[arg(long, value_enum, default_value = "text")]
    stream_format: StreamFormat,
    /// Do not write the streamed character draft into data/exports/characters.
    #[arg(long, default_value_t = false)]
    no_save: bool,
    /// Auto-generate a COMPLETE starter character from the parsed onboarding
    /// pack, persist it, and bind it into a session so `turn` plays AS it.
    #[arg(long, default_value_t = false)]
    auto: bool,
    /// Session to bind the created character into. If omitted (with --auto) a
    /// new session is started and its id is printed for use with `turn`.
    #[arg(long)]
    session_id: Option<String>,
    /// Actor id to bind the created character as (default pc.current).
    #[arg(long, default_value = "pc.current")]
    actor_id: String,
}

#[derive(Debug, Args)]
struct TurnArgs {
    /// Ruleset id. May also be supplied by --request-json.
    #[arg(long)]
    ruleset: Option<String>,
    /// Module id. May also be supplied by --request-json.
    #[arg(long)]
    module: Option<String>,
    /// Existing session id. If omitted, a new session is created.
    #[arg(long)]
    session_id: Option<String>,
    /// User input for this turn. Skips interactive prompt.
    #[arg(long)]
    input: Option<String>,
    /// Read user input from file. Skips interactive prompt.
    #[arg(long)]
    input_file: Option<PathBuf>,
    /// Read user input from stdin as plain text. Skips interactive prompt.
    #[arg(long, default_value_t = false)]
    stdin: bool,
    /// Optional recent transcript text for BP3 dynamic context.
    #[arg(long)]
    recent_transcript: Option<String>,
    /// Optional recent transcript file.
    #[arg(long)]
    recent_transcript_file: Option<PathBuf>,
    /// Read a JSON request from this path, or '-' for stdin.
    /// Shape: {"ruleset_id":"...","module_id":"...","session_id":"...","user_input":"...","recent_transcript":"..."}
    #[arg(long)]
    request_json: Option<String>,
    /// Output stream format for automation.
    #[arg(long, value_enum, default_value = "text")]
    stream_format: StreamFormat,
}



#[derive(Debug, Subcommand)]
enum TimeCommand {
    Show {
        #[arg(long)] session: String,
    },
    Advance {
        #[arg(long)] session: String,
        #[arg(long, default_value_t = 0)] seconds: i64,
        #[arg(long, default_value_t = 0)] minutes: i64,
        #[arg(long, default_value_t = 0)] hours: i64,
        #[arg(long, default_value_t = 0)] days: i64,
        #[arg(long = "rounds", default_value_t = 0)] combat_rounds: i64,
        #[arg(long = "scene-beats", default_value_t = 0)] scene_beats: i64,
        #[arg(long, default_value = "scene_beat")] scale: String,
        #[arg(long, default_value = "manual CLI time advance")] reason: String,
    },
    Schedule {
        #[arg(long)] session: String,
        #[arg(long = "in-minutes", default_value_t = 0)] in_minutes: i64,
        #[arg(long = "in-seconds", default_value_t = 0)] in_seconds: i64,
        #[arg(long, default_value = "system_event")] kind: String,
        #[arg(long, default_value = "{}")] payload_json: String,
    },
    Events {
        #[arg(long)] session: String,
        #[arg(long = "since-tick", default_value_t = 0)] since_tick: i64,
        #[arg(long = "since-event-seq", default_value_t = 0)] since_event_seq: i64,
        #[arg(long, default_value_t = 50)] limit: i64,
    },
}

#[derive(Debug, Args, Clone)]
struct SearchArgs {
    /// Search query. If omitted, use '*' to inspect the currently indexed corpus.
    #[arg(default_value = "*")]
    query: String,
    /// Restrict by high-level domain. Repeatable: --domain rules --domain modules.
    #[arg(long = "domain", value_delimiter = ',')]
    domains: Vec<String>,
    /// Restrict by logical kind. Repeatable: --kind scene_node --kind procedure.
    #[arg(long = "kind", value_delimiter = ',')]
    kinds: Vec<String>,
    /// Restrict by tag. Repeatable: --tag athena --tag combat.
    #[arg(long = "tag", value_delimiter = ',')]
    tags: Vec<String>,
    /// Convenience scope filter.
    #[arg(long)]
    ruleset: Option<String>,
    /// Convenience scope filter.
    #[arg(long)]
    module: Option<String>,
    /// Convenience scope filter.
    #[arg(long)]
    session: Option<String>,
    /// Convenience scope filter.
    #[arg(long)]
    scene: Option<String>,
    /// Generic scope filter in key=value form. Not tied to any DB schema.
    #[arg(long = "scope")]
    scopes: Vec<String>,
    /// Generic exact filter in key=value form. It matches scopes, facets, or string metadata.
    #[arg(long = "filter")]
    filters: Vec<String>,
    #[arg(long, default_value_t = 10)]
    limit: u32,
    #[arg(long, default_value_t = false)]
    explain: bool,
    #[arg(long, default_value_t = false)]
    jsonl: bool,
    /// Reindex before searching.
    #[arg(long, default_value_t = false)]
    reindex: bool,
    /// Use incremental reindex when --reindex is supplied.
    #[arg(long, default_value_t = false)]
    incremental_reindex: bool,
    #[arg(long)]
    data_dir: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum SearchCommand {
    Query(SearchArgs),
    Reindex {
        #[arg(long)]
        data_dir: Option<PathBuf>,
        /// Only index sources with documents newer than the last per-source watermark.
        #[arg(long, default_value_t = false)]
        incremental: bool,
    },
    Sources,
}


#[derive(Debug, Subcommand)]
enum RulesCommand {
    /// Ask the Rule Steward Agent for a source-backed rule answer/context pack.
    Query {
        #[arg(long)]
        ruleset: String,
        #[arg(long)]
        module: Option<String>,
        /// Natural-language rule/materialization question.
        query: String,
        #[arg(long, value_enum, default_value = "general-rule-query")]
        kind: RuleNeedKindArg,
        #[arg(long = "missing-facet", value_delimiter = ',')]
        missing_facets: Vec<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Show the parsed CharacterOnboardingPack for a ruleset.
    CharacterPack {
        #[arg(long)]
        ruleset: String,
    },
    /// Verify that RuleKernel + character creation + first-session prep can start play.
    Playability {
        #[arg(long)]
        ruleset: String,
        #[arg(long)]
        module: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// List RuleKernel patch proposals for BP1 maintenance.
    Bp1Patches {
        #[arg(long)]
        ruleset: String,
        #[arg(long)]
        status: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum RuleNeedKindArg {
    CoreResolutionModel,
    CheckProcedure,
    AttackProcedure,
    DamageProcedure,
    DefenseOrArmorProcedure,
    SaveOrResistanceProcedure,
    AbilityOrSpellUse,
    ObjectInteraction,
    ResourceOrCondition,
    CharacterSheetField,
    CharacterCreation,
    NpcOrMonsterStatBlock,
    SceneOrModuleRule,
    VisibilityOrSpoilerDecision,
    LearningAudit,
    Bp1KernelReview,
    GeneralRuleQuery,
}

impl From<RuleNeedKindArg> for RuleNeedKind {
    fn from(value: RuleNeedKindArg) -> Self {
        match value {
            RuleNeedKindArg::CoreResolutionModel => RuleNeedKind::CoreResolutionModel,
            RuleNeedKindArg::CheckProcedure => RuleNeedKind::CheckProcedure,
            RuleNeedKindArg::AttackProcedure => RuleNeedKind::AttackProcedure,
            RuleNeedKindArg::DamageProcedure => RuleNeedKind::DamageProcedure,
            RuleNeedKindArg::DefenseOrArmorProcedure => RuleNeedKind::DefenseOrArmorProcedure,
            RuleNeedKindArg::SaveOrResistanceProcedure => RuleNeedKind::SaveOrResistanceProcedure,
            RuleNeedKindArg::AbilityOrSpellUse => RuleNeedKind::AbilityOrSpellUse,
            RuleNeedKindArg::ObjectInteraction => RuleNeedKind::ObjectInteraction,
            RuleNeedKindArg::ResourceOrCondition => RuleNeedKind::ResourceOrCondition,
            RuleNeedKindArg::CharacterSheetField => RuleNeedKind::CharacterSheetField,
            RuleNeedKindArg::CharacterCreation => RuleNeedKind::CharacterCreation,
            RuleNeedKindArg::NpcOrMonsterStatBlock => RuleNeedKind::NpcOrMonsterStatBlock,
            RuleNeedKindArg::SceneOrModuleRule => RuleNeedKind::SceneOrModuleRule,
            RuleNeedKindArg::VisibilityOrSpoilerDecision => RuleNeedKind::VisibilityOrSpoilerDecision,
            RuleNeedKindArg::LearningAudit => RuleNeedKind::LearningAudit,
            RuleNeedKindArg::Bp1KernelReview => RuleNeedKind::Bp1KernelReview,
            RuleNeedKindArg::GeneralRuleQuery => RuleNeedKind::GeneralRuleQuery,
        }
    }
}

#[derive(Debug, Subcommand)]
enum LearnCommand {
    /// List review-gated learning candidates generated by learning_audit.
    Candidates {
        #[arg(long)]
        ruleset: Option<String>,
        #[arg(long, default_value = "pending_review")]
        status: String,
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    /// Approve a learning candidate into learned_packets.
    Approve {
        candidate_id: String,
        #[arg(long, default_value = "used_once")]
        stage: String,
        #[arg(long)]
        notes: Option<String>,
    },
}


#[derive(Debug, Clone, Copy, ValueEnum)]
enum StreamFormat {
    /// Human-readable text stream. Metadata goes to stderr; deltas go to stdout.
    Text,
    /// Newline-delimited JSON events. Good for tests and LLM tooling.
    Jsonl,
    /// Server-Sent Events written to stdout, mirroring the Axum API event shape.
    Sse,
}

#[derive(Debug, Subcommand)]
enum AuthCommand {
    Set,
}

#[derive(Debug, Subcommand)]
enum InspectCommand {
    Bundles,
    CharacterTemplate { #[arg(long)] ruleset: String },
    CharacterOnboarding { #[arg(long)] ruleset: String },
    ProjectJson,
}

#[derive(Debug, Default, Deserialize)]
struct CharacterCreateJsonRequest {
    #[serde(default, alias = "ruleset")]
    ruleset_id: Option<String>,
    #[serde(default, alias = "module")]
    module_id: Option<String>,
    #[serde(default, alias = "preferences")]
    user_preferences: String,
}

#[derive(Debug, Default, Deserialize)]
struct TurnJsonRequest {
    #[serde(default, alias = "ruleset")]
    ruleset_id: Option<String>,
    #[serde(default, alias = "module")]
    module_id: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default, alias = "input")]
    user_input: String,
    #[serde(default)]
    recent_transcript: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string()))
        .init();

    let cli = Cli::parse();
    match cli.command {
        Commands::Init => init_project().await,
        Commands::Auth { command } => match command { AuthCommand::Set => auth_set().await },
        Commands::Migrate => {
            let db = connect_db().await?;
            db.migrate().await?;
            println!("migration complete");
            Ok(())
        }
        Commands::ParseAll { force, full_parse, pdf_backend, data_dir } => {
            let db = connect_db().await?;
            db.migrate().await?;
            let llm = make_llm()?;
            std::env::set_var("TRPG_PDF_BACKEND", &pdf_backend);
            let mut config = ParserConfig::new(data_dir.unwrap_or_else(default_data_dir), force);
            if full_parse {
                config.parse_full_chunks = true;
                let rule_steward_first_pass = std::env::var("TRPG_RULE_STEWARD_FIRST_PASS")
                    .ok()
                    .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
                    .unwrap_or(true);
                config.parse_config_hash = sha256_hex(format!("parser=v1.16.2;schema=v1;prompt=rule_steward_react_first_pass_v1;full_chunks=true;pdf_backend={};clean_mode={};oxidize_chunk_target_chars={};llm_clean={};semantic_units={};semantic_unit_max_chars={};llm_semantic_wash={};rule_steward_first_pass={}", config.pdf_backend.as_str(), config.oxidize_clean_mode.as_str(), config.oxidize_chunk_target_chars, config.llm_clean_extraction, config.semantic_unit_conditioning, config.semantic_unit_max_chars, config.llm_semantic_wash, rule_steward_first_pass));
            }
            let search_data_dir = config.data_dir.clone();
            let pdf_backend_label = config.pdf_backend.as_str().to_string();
            let service = ProjectParseService::new(db.clone(), llm, config);
            let project = service.parse_all().await?;
            let search = make_search(&db, search_data_dir)?;
            let stats = search.reindex_all().await?;
            println!("onboarded/indexed: {} ruleset bundle(s), {} module bundle(s)", project.rulesets.len(), project.modules.len());
            println!("pdf backend: {}", pdf_backend_label);
            let rule_steward_first_pass = std::env::var("TRPG_RULE_STEWARD_FIRST_PASS")
                .ok()
                .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
                .unwrap_or(true);
            println!("rule steward first-pass skills: {}", if rule_steward_first_pass { "enabled" } else { "disabled" });
            if full_parse { println!("full chunk extraction was enabled"); } else { println!("full chunk extraction skipped; cold data will be learned on demand"); }
            println!("wrote data/parsed/project.bundle.json, context_blocks.jsonl, material_index.jsonl");
            println!("tantivy search index updated: {} document(s) from {} source config(s)", stats.indexed_documents, stats.source_count);
            if !stats.errors.is_empty() { eprintln!("search index warnings: {}", serde_json::to_string(&stats.errors)?); }
            Ok(())
        }
        Commands::Api { addr, data_dir } => {
            let db = connect_db().await?;
            db.migrate().await?;
            let llm = make_llm()?;
            let parser_config = ParserConfig::new(data_dir.unwrap_or_else(default_data_dir), false);
            let search = make_search(&db, parser_config.data_dir.clone())?;
            let runtime = RuntimeEngine::new(db.clone()).with_search(search.clone());
            let state = AppState { db, llm, runtime, search, parser_config };
            serve(addr, state).await
        }
        Commands::Inspect { command } => inspect(command).await,
        Commands::CreateCharacter(args) => create_character_cli(args).await,
        Commands::Play { ruleset, module } => {
            agent_play::play_cli_agent(&ruleset, module.as_deref()).await
        }
        Commands::Turn(args) => turn_cli(args).await,
        Commands::Rg(args) => search_query_cli(args).await,
        Commands::GrepTable { query, limit, data_dir } => {
            let dir = data_dir.unwrap_or_else(default_data_dir);
            let hits = trpg_search::grep_layout_tables_in(&dir, &query, limit);
            for h in &hits {
                println!("{}", serde_json::json!({"source_id": h.source_id, "kind": h.source_kind, "page": h.page, "column_score": h.column_score, "context": h.context, "row": h.row}));
            }
            eprintln!("{} hit(s) for {:?}", hits.len(), query);
            Ok(())
        }
        Commands::Grow { session, actor, bucket, id, op, amount, value, kind, category } => {
            grow_cli(&session, &actor, &bucket, &id, &op, amount, value.as_deref(), kind.as_deref(), category.as_deref()).await
        }
        Commands::Search { command } => search_command_cli(command).await,
        Commands::Rules { command } => rules_command_cli(command).await,
        Commands::Learn { command } => learning_command_cli(command).await,
        Commands::Time { command } => time_command_cli(command).await,
        Commands::Roll { expression } => {
            let roll = roll_dice(&expression)?;
            println!("{} => {:?} {:+} = {}", roll.expression, roll.rolls, roll.modifier, roll.total);
            Ok(())
        }
        Commands::ParseStaged { ruleset, data_dir, stage1_only, json, budget } => {
            parse_staged_cli(ruleset, data_dir, stage1_only, json, budget).await
        }
    }
}

async fn init_project() -> Result<()> {
    for dir in [
        "data/rulebooks",
        "data/modules",
        "data/markdown/rulebooks",
        "data/markdown/modules",
        "data/parsed",
        "data/exports/characters",
        "data/exports/sessions",
        "data/exports/traces",
        "data/search/tantivy_v3",
        "data/agent/advice",
    ] {
        tokio::fs::create_dir_all(dir).await?;
    }
    if !PathBuf::from(".env").exists() {
        tokio::fs::write(".env", include_str!("../../../.env.example")).await?;
        println!("created .env from .env.example");
    }
    println!("initialized data folders. Put rulebook PDFs in data/rulebooks and module PDFs in data/modules.");
    println!("start PostgreSQL with: docker compose up -d postgres");
    Ok(())
}

async fn auth_set() -> Result<()> {
    let providers = vec!["openai", "openai_compatible"];
    let provider_idx = Select::new().with_prompt("LLM provider").items(&providers).default(0).interact()?;
    let provider = providers[provider_idx];
    let default_base = if provider == "openai" { "https://api.openai.com/v1" } else { "http://localhost:8000/v1" };
    let base_url: String = Input::new().with_prompt("Base URL").default(default_base.to_string()).interact_text()?;
    let model: String = Input::new().with_prompt("Model").default("gpt-4.1".to_string()).interact_text()?;
    let api_key = Password::new().with_prompt("API key").allow_empty_password(false).interact()?;
    let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| "postgres://chatrpg:chatrpg@localhost:54323/chatrpg".to_string());
    let data_dir = std::env::var("TRPG_DATA_DIR").unwrap_or_else(|_| "./data".to_string());
    let env = format!(
        "DATABASE_URL={database_url}\nTRPG_LLM_PROVIDER={provider}\nTRPG_LLM_BASE_URL={base_url}\nTRPG_LLM_API_KEY={api_key}\nTRPG_LLM_MODEL={model}\nTRPG_LLM_SEND_TEMPERATURE=false\nTRPG_DATA_DIR={data_dir}\nTRPG_API_ADDR=127.0.0.1:8787\nTRPG_MEMORY_RETRIEVAL_LIMIT=8\nTRPG_MEMORY_SNAPSHOT_EVERY_TURNS=8\nTRPG_SEARCH_INDEX_DIR=./data/search/tantivy_v3\nTRPG_SEARCH_WRITER_MEMORY_BYTES=96000000\nTRPG_RUNTIME_AUTO_SEARCH=true\nTRPG_RUNTIME_AUTO_SEARCH_LIMIT=5\nTRPG_RUNTIME_AUTO_SEARCH_MAX_SCENE_PINS=1\nTRPG_RUNTIME_AUTOPIN_SCENE_RULES=true\nTRPG_RUNTIME_AUTO_SEARCH_DOMAINS=learned,rules,modules,rulings,source,parsed\nTRPG_PDF_BACKEND=duotext\nTRPG_PDF_ALLOW_PDFTOTEXT_FALLBACK=true\nTRPG_OXIDIZE_CLEAN_MODE=heuristic\nTRPG_OXIDIZE_CHUNK_TARGET_CHARS=4000\nTRPG_OXIDIZE_WRITE_RAW_CHUNKS=true\nTRPG_INGEST_LLM_CLEAN=false\nTRPG_INGEST_LLM_CLEAN_MAX_CHUNKS=8\nTRPG_INGEST_SEMANTIC_UNITS=true\nTRPG_SEMANTIC_UNIT_MAX_CHARS=6000\nTRPG_INGEST_LLM_SEMANTIC_WASH=false\nTRPG_INGEST_LLM_SEMANTIC_WASH_MAX_UNITS=12\nTRPG_LEARNING_AUDIT=true\nTRPG_LEARNING_AUTO_PROMOTE=false\nTRPG_CONFLICT_AGENT_ENABLE_V10=true\nTRPG_SITUATION_ORCHESTRATOR_ENABLE_V11=true\nTRPG_SITUATION_STALEMATE_TURNS=3\nTRPG_ACTIONABLE_DIRECTOR_ENABLE_V12=true\nTRPG_ACTIONABLE_DIRECTOR_ENABLE_V13=true\nTRPG_ACTIONABLE_DIRECTOR_MAX_EXAMPLES=4\nTRPG_ACTIONABLE_DIRECTOR_MENU_COOLDOWN=2\nTRPG_NOVELTY_DIRECTOR_ENABLE_V13=true\nTRPG_NPC_MAX_REPEAT_TACTIC=1\nTRPG_OUTPUT_NO_REPEAT_VALIDATOR=true\nTRPG_DIRECTION_GATE_AUTO_DEFAULT_AFTER_REPROMPTS=2\nTRPG_WORLD_TIME_ENABLE_V14=true\nTRPG_WORLD_TIME_START_DISPLAY=Day 1, 00:00\nTRPG_WORLD_TIME_CALENDAR_ID=relative_default\nTRPG_WORLD_TIME_CONTEXT_EVENT_LIMIT=24\nTRPG_WORLD_TIME_AUTO_ADVANCE_PER_TURN_SECONDS=0\nTRPG_INTERACTION_KERNEL_ENABLE_V15=true\nTRPG_INTERACTION_RECONCILE_EVERY_TURN=true\nTRPG_INTERACTION_CASCADE_CLOSE_FRAME=true\nTRPG_INTERACTION_GENERATION_GUARD=true\nTRPG_RULESET_ADVICE_DIR=./data/ruleset_advice\nTRPG_COMBAT_AUTO_FRAME=true\nTRPG_COMBAT_COMPACTION=true\nTRPG_OBJECT_KERNEL_ENABLE_V16=true\nTRPG_OBJECT_INTERACTION_AUTOCONTRACT=true\nTRPG_OBJECT_CONTEXT_BP3=true\nTRPG_OBJECT_COMPACTION_ENABLE=true\nTRPG_OBJECT_DEFAULT_DISARM_TARGET=15\nTRPG_TURN_ORCHESTRATOR_ENABLE_V17=true\nTRPG_TURN_ORCHESTRATOR_GATE_RELEVANCE=true\nTRPG_TURN_ORCHESTRATOR_FRAME_FIRST=true\nTRPG_TURN_ORCHESTRATOR_OBJECT_AS_CHILD=true\nTRPG_TURN_ORCHESTRATOR_DISABLE_GENERIC_CHECK_FOR_FRAME_ACTION=true\nTRPG_RUNTIME_PARAM_HYDRATION_ENABLE_V18=true\nTRPG_OBJECT_BIND_NARRATION_TO_RULES_V18=true\nTRPG_OBJECT_SESSION_SCOPED_IDS=true\nTRPG_ORCHESTRATOR_OBJECT_SELF_SELECT=false\nTRPG_REQUIRED_REACTION_BLOCKS_OBJECT_INTENT=true\nTRPG_SEMANTIC_PRIMARY=true\nTRPG_SEMANTIC_CLASSIFIER_ENABLE_V19=true\nTRPG_SEMANTIC_EXTRACTOR_ENABLE_V19=true\nTRPG_LEXICAL_FALLBACK_ENABLE=false\nTRPG_LEXICAL_FALLBACK_AUDIT_ONLY=true\nTRPG_ABILITY_KERNEL_ENABLE_V19=true\nTRPG_RULE_BINDING_ENABLE_V19=true\nTRPG_RULE_BINDING_REQUIRE_RUNTIME_WRITEBACK=true\nTRPG_ABILITY_CONTEXT_BP3=true\nTRPG_SEMANTIC_ROUTE_REDUCER_STRICT=true\nTRPG_SEMANTIC_ROUTE_CACHE_ENABLE=true\nTRPG_SEMANTIC_RECORD_REPLAY_ENABLE=false\nTRPG_REAL_MATERIALIZATION_ENABLE_V110=true\nTRPG_REAL_MATERIALIZATION_EXTRACTOR_ENABLE_V110=true\nTRPG_MATERIALIZATION_REQUIRE_SOURCE_OR_PROVISIONAL_REASON=true\nTRPG_MATERIALIZATION_BLOCKING_FOR_MECHANICS=true\nTRPG_MATERIALIZATION_CONTEXT_BP3=true\nTRPG_MATERIALIZATION_PREFER_MODULE_CARDS=true\nTRPG_SIMULATION_GUIDED_HOTFIX_V1101=true\nTRPG_FORCE_TECH_ASSESSMENT_CHECK=true\nTRPG_REACTION_REPROMPT_EMITS_WINDOW=true\nTRPG_PLAYER_VALUE_REFEREE_ENABLE_V1102=true\nTRPG_PLAYER_VALUE_ALLOW_TABLE_OVERRIDE=true\nTRPG_PLAYER_VALUE_REQUIRE_RULE_OR_TABLE_CHECK=true\nTRPG_REFEREE_COMBAT_SLICE_ENABLE_V1102=true\nTRPG_MECHANICAL_LEDGER_CONTEXT_BP3=true\nTRPG_CONTEST_KERNEL_ENABLE_V111=true\n# TRPG_CONTEST_DEFAULT_PERCENTILE_SKILL=50  # optional table override only; default unset\nTRPG_CONTEST_CONTEXT_BP3=true\nTRPG_CONTEST_REQUIRE_MODEL_FOR_CHECK=true\nTRPG_MECHANICS_SEARCH_SKILLS_ENABLE_V112=true\nTRPG_MECHANICS_SEARCH_MULTI_STEP=true\nTRPG_MECHANICS_SEARCH_USE_RULESET_LOCATORS=true\nTRPG_MECHANICS_SEARCH_WRITE_FACETS=true\nTRPG_MECHANICS_SEARCH_CONTEXT_BP3=true\nTRPG_MECHANICS_SEARCH_GREP_CANDIDATES_ONLY=true\nTRPG_UNIFIED_ROLL_EFFECT_EXECUTOR_ENABLE_V1121=true\nTRPG_AGENT_TABLE_DICE_POLICY=system_rolls_visible\nTRPG_EFFECT_CONTEXT_BP3=true\nTRPG_EFFECT_ALLOW_PROVISIONAL_TARGET_PARAMETER=false\nTRPG_STRICT_SOURCE_BACKED_MATERIALIZATION=true\nTRPG_FAIL_ON_MISSING_SOURCE_BACKED_PARAMS=true\nTRPG_ALLOW_SYNTHETIC_ACTOR_SEEDS=false\nTRPG_RUNTIME_PARAM_ALLOW_SYNTHETIC_NPC_SEEDS=false\nTRPG_MATERIALIZATION_WRITE_PARTIAL=false\nTRPG_RULE_STEWARD_ENABLE_V116=true\nTRPG_RULE_STEWARD_AUTO_QUERY=true\nTRPG_CHARACTER_ONBOARDING_REQUIRED=true\nTRPG_PLAYABILITY_GATE_BLOCKING=false\nTRPG_RULE_KERNEL_PATCH_AUTO_APPLY=false\nTRPG_RULE_STEWARD_STRICT_SOURCE_BACKED=true\nTRPG_EFFECT_REQUIRE_PARAMETER_IMPACT=true\nTRPG_ROLL_BINDING_HOTFIX_ENABLE_V1122=true\nTRPG_ROLL_BINDING_USE_LATEST_UNRESOLVED_CHECK=true\nTRPG_DIRECTION_GATE_IS_ADVISORY=true\nTRPG_AUTO_RESOLVE_SYSTEM_EFFECT_ROLLS=true\nTRPG_PARAMETER_FACET_EXECUTOR_ENABLE_V113=true\nTRPG_PARAMETER_FACET_EXECUTOR_USE_BOUND_FACETS_FIRST=true\nTRPG_PARAMETER_FACET_EXECUTOR_USE_RULESET_STARTER_PROFILES=false\nTRPG_PARAMETER_FACET_EXECUTOR_WRITE_GENERIC_STATES=true\nTRPG_PARAMETER_FACET_EXECUTOR_CONTEXT_BP3=true\nTRPG_PARAMETER_FACET_EXECUTOR_AUDIT_PROVISIONAL=true\nTRPG_COMBAT_ROUTE_SOURCE_OBJECTS_AS_ATTACKS=true\nTRPG_COMBAT_COMMIT_FRAME_START_ACTION=true\nTRPG_ORCHESTRATOR_ATTACK_OVER_OBJECT_MATERIALIZATION=true
TRPG_SEMANTIC_COMBAT_INTENT_ENABLE_V1133=true
TRPG_SEMANTIC_COMBAT_LEXICAL_FALLBACK_AUDIT=true
TRPG_SUSTAINED_COMBAT_LOOP_POLICY_V1133=true
TRPG_DETERMINISTIC_ROLL_AUTHORITY_V1133=true
TRPG_DIRECTION_GATE_ADVISORY_FOR_FRAME_ACTIONS=true
TRPG_EXTERNAL_PLAYTEST_EVALUATOR_ENABLE_V114=true
TRPG_PLAYTEST_EVALUATOR_BACKEND=claude-code
TRPG_PLAYTEST_CLAUDE_BIN=claude
TRPG_PLAYTEST_EXPORT_DB_DIFF=true
TRPG_PLAYTEST_FORBID_RULE_TABLE_ASKS=true
TRPG_RULE_STEWARD_ENABLE_V116=true
TRPG_RULE_STEWARD_AUTO_QUERY=true
TRPG_CHARACTER_ONBOARDING_REQUIRED=true
TRPG_PLAYABILITY_GATE_BLOCKING=false
TRPG_RULE_KERNEL_PATCH_AUTO_APPLY=false
TRPG_RULE_STEWARD_STRICT_SOURCE_BACKED=true
# NPC persona-judge synthesis (flagged provisional, drives resolution when no source). Set false for strict fail-closed.
TRPG_NPC_PERSONA_SYNTHESIS=true
\n"
    );
    tokio::fs::write(".env", env).await?;
    println!("saved .env");
    Ok(())
}

async fn inspect(command: InspectCommand) -> Result<()> {
    let db = connect_db().await?;
    match command {
        InspectCommand::Bundles => {
            let bundles = db.list_bundles().await?;
            println!("{}", serde_json::to_string_pretty(&bundles)?);
        }
        InspectCommand::CharacterTemplate { ruleset } => {
            let template = db.load_character_template(&ruleset).await?.ok_or_else(|| anyhow!("template not found for {ruleset}"))?;
            println!("{}", serde_json::to_string_pretty(&template)?);
        }
        InspectCommand::CharacterOnboarding { ruleset } => {
            let pack = db.load_character_onboarding_pack(&ruleset).await?.ok_or_else(|| anyhow!("character onboarding pack not found for {ruleset}"))?;
            println!("{}", serde_json::to_string_pretty(&pack)?);
        }
        InspectCommand::ProjectJson => {
            let project = db.load_latest_project_bundle().await?.ok_or_else(|| anyhow!("project bundle not found"))?;
            println!("{}", serde_json::to_string_pretty(&project)?);
        }
    }
    Ok(())
}

async fn create_character_cli(args: CreateCharacterArgs) -> Result<()> {
    let mut req = if let Some(path) = args.request_json.as_deref() {
        let text = read_path_or_stdin(path).await.context("failed to read --request-json")?;
        serde_json::from_str::<CharacterCreateJsonRequest>(&text).context("invalid create-character JSON request")?
    } else {
        CharacterCreateJsonRequest::default()
    };

    if let Some(ruleset) = args.ruleset {
        req.ruleset_id = Some(ruleset);
    }
    if let Some(module) = args.module {
        req.module_id = Some(module);
    }
    if let Some(preferences) = resolve_text_input(
        args.preferences,
        args.preferences_file,
        args.stdin,
        args.request_json.is_none(),
    ).await? {
        req.user_preferences = preferences;
    } else if args.request_json.is_none() && io::stdin().is_terminal() {
        req.user_preferences = Input::new()
            .with_prompt("角色概念 / 已确定选择（可以只写一部分，留空会让 AI 提案）")
            .allow_empty(true)
            .interact_text()?;
    }

    let ruleset_id = req.ruleset_id.ok_or_else(|| anyhow!("missing --ruleset or ruleset_id in --request-json"))?;

    let db = connect_db().await?;
    let llm = make_llm()?;
    let runtime = RuntimeEngine::new(db.clone());

    // F9 auto path: generate a complete starter character from the parsed
    // onboarding pack, persist it, and bind it into a session so `turn` plays
    // AS it (writes runtime_actor_parameters, not just a markdown draft).
    if args.auto {
        emit_phase(args.stream_format, "start", json!({"kind":"character_create_auto", "ruleset_id": ruleset_id, "module_id": req.module_id}))?;
        let created = runtime
            .create_and_bind_character(&*llm, &ruleset_id, args.session_id.as_deref(), &args.actor_id, &req.user_preferences)
            .await?;
        emit_phase(args.stream_format, "character_created", json!({
            "character_id": created.character_id,
            "name": created.name,
            "status": created.status,
            "session_id": created.session_id,
            "actor_id": created.actor_id,
            "validation": created.validation,
            "sheet": created.sheet,
        }))?;
        emit_phase(args.stream_format, "bound", json!({
            "session_id": created.session_id,
            "actor_id": created.actor_id,
            "play_hint": format!("trpg turn --ruleset {ruleset_id} --session-id {} --input \"...\"", created.session_id),
        }))?;
        emit_phase(args.stream_format, "done", json!({}))?;
        return Ok(());
    }

    let messages = runtime.character_creation_messages(&ruleset_id, req.module_id.as_deref(), &req.user_preferences).await?;

    emit_phase(args.stream_format, "start", json!({"kind":"character_create", "ruleset_id": ruleset_id, "module_id": req.module_id}))?;
    emit_phase(args.stream_format, "llm_stream_start", json!({}))?;

    let mut stream = match llm.stream_chat(messages, 0.7).await {
        Ok(stream) => stream,
        Err(err) => {
            emit_error(args.stream_format, &err.to_string())?;
            return Err(err);
        }
    };
    let mut full = String::new();
    while let Some(item) = stream.next().await {
        match item {
            Ok(delta) => {
                emit_delta(args.stream_format, &delta)?;
                full.push_str(&delta);
            }
            Err(err) => {
                emit_error(args.stream_format, &err.to_string())?;
                return Err(err);
            }
        }
    }

    if !args.no_save {
        tokio::fs::create_dir_all("data/exports/characters").await.ok();
        let path = format!("data/exports/characters/character_draft_{}.md", uuid::Uuid::new_v4().simple());
        tokio::fs::write(&path, &full).await?;
        emit_phase(args.stream_format, "draft_saved", json!({"path": path}))?;

        let pack = db.load_character_onboarding_pack(&ruleset_id).await?;
        let template = match pack.as_ref() {
            Some(pack) => pack.sheet_template.clone(),
            None => db.load_character_template(&ruleset_id).await?.ok_or_else(|| anyhow!("template not found for {ruleset_id}"))?,
        };
        let draft = extract_json_cli(&full).unwrap_or_else(|| json!({"raw_response": full.clone()}));
        let name = draft.get("name")
            .or_else(|| draft.get("character_name"))
            .and_then(Value::as_str)
            .unwrap_or("Unnamed Character")
            .to_string();
        let validation = validate_character_template_sheet(&template, &draft);
        let pack_mechanically_ready = pack.as_ref().map(|p| {
            p.validation_report.status == "ok"
                && !p.derived_formula_pack.formulas.is_empty()
                && !p.runtime_bindings.is_empty()
                && !p.creation_flows.is_empty()
        }).unwrap_or(false);
        let status = if validation.status == "ok" && pack_mechanically_ready { "ready" } else { "draft_needs_rules_source" };
        let character = CharacterSheet {
            character_id: format!("character_{}", uuid::Uuid::new_v4().simple()),
            ruleset_id: ruleset_id.clone(),
            template_id: template.template_id.clone(),
            name,
            sheet: draft,
            validation_report: validation.clone(),
        };
        db.save_character(&character, status).await?;
        emit_phase(args.stream_format, "character_saved", json!({"character_id": character.character_id, "status": status, "validation": validation, "used_character_onboarding_pack": pack.is_some()}))?;
    }
    emit_phase(args.stream_format, "done", json!({}))?;
    Ok(())
}

async fn grow_cli(
    session: &str, actor: &str, bucket: &str, id: &str, op: &str, amount: f64,
    text: Option<&str>, kind: Option<&str>, category: Option<&str>,
) -> anyhow::Result<()> {
    let db = connect_db().await?;
    db.migrate().await?;
    let runtime = RuntimeEngine::new(db.clone());
    let changed = runtime.apply_track_change(session, actor, bucket, id, op, amount, text, kind, category).await?;
    println!("{}", serde_json::json!({
        "ok": changed, "session": session, "actor": actor,
        "bucket": bucket, "id": id, "op": op, "amount": amount, "value": text
    }));
    Ok(())
}

async fn turn_cli(args: TurnArgs) -> Result<()> {
    let mut req = if let Some(path) = args.request_json.as_deref() {
        let text = read_path_or_stdin(path).await.context("failed to read --request-json")?;
        serde_json::from_str::<TurnJsonRequest>(&text).context("invalid turn JSON request")?
    } else {
        TurnJsonRequest::default()
    };

    if let Some(ruleset) = args.ruleset {
        req.ruleset_id = Some(ruleset);
    }
    if let Some(module) = args.module {
        req.module_id = Some(module);
    }
    if let Some(session_id) = args.session_id {
        req.session_id = Some(session_id);
    }
    if let Some(input) = resolve_text_input(args.input, args.input_file, args.stdin, args.request_json.is_none()).await? {
        req.user_input = input;
    }
    if let Some(recent) = resolve_text_input(args.recent_transcript, args.recent_transcript_file, false, false).await? {
        req.recent_transcript = Some(recent);
    }

    let ruleset_id = req.ruleset_id.ok_or_else(|| anyhow!("missing --ruleset or ruleset_id in --request-json"))?;
    if req.user_input.trim().is_empty() {
        return Err(anyhow!("missing --input, --input-file, --stdin, or user_input in --request-json"));
    }

    let db = connect_db().await?;
    db.migrate().await?;
    let llm = make_llm()?;
    let data_dir = default_data_dir();
    let search = make_search(&db, data_dir.clone())?;
    let runtime = RuntimeEngine::new(db.clone()).with_search(search);
    let module_id = req.module_id.clone();
    let session_id = match req.session_id {
        Some(id) => id,
        None => runtime.start_session(&ruleset_id, module_id.as_deref()).await?,
    };

    emit_phase(args.stream_format, "session", json!({"session_id": session_id.clone()}))?;

    // 一次性单回合（无交互循环）：建 GmLoop + OwnedTurnRequest → drain execute_turn 流。
    // GmLoop 含不可 Clone 字段，单回合即用即弃；scene_extractor 仅模组在场时装配。
    let engine = RuntimeEngine::new(db.clone()).with_search(make_search(&db, data_dir.clone())?);
    let mut gm = GmLoop::new(engine, llm.clone(), ToolRegistry::standard(), LoopConfig::default(), data_dir.clone());
    if let Some(mid) = module_id.clone() {
        let db2 = db.clone();
        let llm2 = llm.clone();
        let rs2 = ruleset_id.clone();
        let dir2 = data_dir.clone();
        gm.scene_extractor = Some(Arc::new(move |node_id: String| {
            let db3 = db2.clone();
            let llm3 = llm2.clone();
            let mid3 = mid.clone();
            let rs3 = rs2.clone();
            let dir3 = dir2.clone();
            Box::pin(async move {
                extract_module_scenes(&db3, llm3.as_ref(), &mid3, None, Some(&rs3), &dir3, 12, Some(&node_id)).await
            }) as Pin<Box<dyn Future<Output = Result<usize>> + Send>>
        }) as SceneDeepExtractFn);
    }

    let turn_id = format!("turn_{}", uuid::Uuid::new_v4().simple());
    let request = ContextRequest {
        ruleset_id: ruleset_id.clone(),
        module_id: module_id.clone(),
        session_id: session_id.clone(),
        turn_id,
        viewer: VisibilityProfile::gm(),
        token_budget: TokenBudget::default(),
    };
    let scene_id = db.load_session_scene(&session_id).await.ok().flatten();
    let state = RuntimeState {
        ruleset_id: ruleset_id.clone(),
        module_id: module_id.clone(),
        scene_id,
        ..Default::default()
    };
    let owned = OwnedTurnRequest {
        request,
        state,
        user_input: req.user_input.clone(),
        history: Vec::new(),
        recent_transcript: req.recent_transcript.clone(),
        module_id: module_id.clone(),
        data_dir: data_dir.clone(),
    };

    let mut stream = Box::pin(execute_turn(gm, owned, CANONICAL_TURN_PLAN));
    while let Some(event) = stream.next().await {
        match event {
            TurnEvent::Delta(delta) => emit_delta(args.stream_format, &delta)?,
            TurnEvent::AwaitingPlayerRoll { check_id, prompt_public } => {
                emit_phase(args.stream_format, "pending_check_created", json!({"check_id": check_id}))?;
                emit_delta(args.stream_format, &prompt_public)?;
                emit_phase(args.stream_format, "done", json!({"reason":"awaiting_player_roll"}))?;
            }
            TurnEvent::SceneTransition { from, to, reason } => {
                emit_named_event(args.stream_format, "scene_transition", json!({"from": from, "to": to, "reason": reason}))?;
            }
            TurnEvent::Errata(entry) => {
                emit_phase(args.stream_format, "errata", serde_json::to_value(&entry).unwrap_or_else(|_| json!({})))?;
            }
            TurnEvent::PostprocessScheduled => {
                emit_phase(args.stream_format, "postprocess_scheduled", json!({}))?;
            }
            TurnEvent::TurnComplete { outcome } => {
                if let TurnOutcome::Narration(_) = outcome {
                    emit_phase(args.stream_format, "done", json!({}))?;
                }
            }
        }
    }
    Ok(())
}

async fn resolve_text_input(
    inline: Option<String>,
    file: Option<PathBuf>,
    read_stdin_flag: bool,
    auto_stdin_when_piped: bool,
) -> Result<Option<String>> {
    if let Some(value) = inline {
        return Ok(Some(value));
    }
    if let Some(path) = file {
        return Ok(Some(tokio::fs::read_to_string(path).await?));
    }
    if read_stdin_flag || (auto_stdin_when_piped && !io::stdin().is_terminal()) {
        let text = read_stdin_to_string()?;
        return Ok(Some(text));
    }
    Ok(None)
}

async fn read_path_or_stdin(path_or_dash: &str) -> Result<String> {
    if path_or_dash == "-" {
        read_stdin_to_string()
    } else {
        Ok(tokio::fs::read_to_string(path_or_dash).await?)
    }
}

fn read_stdin_to_string() -> Result<String> {
    let mut buf = String::new();
    io::stdin().read_to_string(&mut buf)?;
    Ok(buf)
}

fn emit_phase(format: StreamFormat, phase: &str, data: Value) -> Result<()> {
    match format {
        StreamFormat::Text => {
            eprintln!("[phase] {} {}", phase, serde_json::to_string(&data)?);
        }
        StreamFormat::Jsonl => {
            println!("{}", json!({"event":"phase", "phase": phase, "data": data}));
            io::stdout().flush().ok();
        }
        StreamFormat::Sse => {
            emit_sse("phase", &json!({"phase": phase, "data": data}).to_string());
        }
    }
    Ok(())
}

fn emit_delta(format: StreamFormat, delta: &str) -> Result<()> {
    match format {
        StreamFormat::Text => {
            print!("{delta}");
            io::stdout().flush().ok();
        }
        StreamFormat::Jsonl => {
            println!("{}", json!({"event":"delta", "data": delta}));
            io::stdout().flush().ok();
        }
        StreamFormat::Sse => {
            emit_sse("delta", delta);
        }
    }
    Ok(())
}


fn emit_named_event(format: StreamFormat, event: &str, data: Value) -> Result<()> {
    match format {
        StreamFormat::Text => eprintln!("[event] {} {}", event, serde_json::to_string(&data)?),
        StreamFormat::Jsonl => println!("{}", json!({"event": event, "data": data})),
        StreamFormat::Sse => emit_sse(event, &data.to_string()),
    }
    io::stdout().flush().ok();
    Ok(())
}

fn emit_error(format: StreamFormat, message: &str) -> Result<()> {
    match format {
        StreamFormat::Text => eprintln!("[error] {message}"),
        StreamFormat::Jsonl => println!("{}", json!({"event":"error", "message": message})),
        StreamFormat::Sse => emit_sse("error", &json!({"message": message}).to_string()),
    }
    io::stdout().flush().ok();
    Ok(())
}

fn emit_sse(event: &str, data: &str) {
    println!("event: {event}");
    for line in data.lines() {
        println!("data: {line}");
    }
    if data.is_empty() {
        println!("data:");
    }
    println!();
    io::stdout().flush().ok();
}

async fn time_command_cli(command: TimeCommand) -> Result<()> {
    let db = connect_db().await?;
    db.migrate().await?;
    let runtime = RuntimeEngine::new(db.clone());
    match command {
        TimeCommand::Show { session } => {
            let state = runtime.current_world_time(&session).await?;
            println!("{}", serde_json::to_string_pretty(&state)?);
        }
        TimeCommand::Advance { session, seconds, minutes, hours, days, combat_rounds, scene_beats, scale, reason } => {
            let amount = TimeAmount { seconds, minutes, hours, days, combat_rounds, scene_beats, label: String::new() };
            let result = runtime.advance_world_time(TimeAdvanceRequest {
                session_id: session,
                campaign_id: None,
                reason,
                amount,
                scale: parse_time_scale(&scale),
                mutation_kind: TimeMutationKind::Advance,
                visibility: Visibility::PlayerVisible,
                caused_by_turn_id: None,
                caused_by_event_id: None,
                scene_epoch: None,
            }).await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        TimeCommand::Schedule { session, in_minutes, in_seconds, kind, payload_json } => {
            let service = trpg_time::WorldTimeService::new(db.clone());
            let payload: Value = serde_json::from_str(&payload_json).unwrap_or_else(|_| json!({"raw": payload_json}));
            let scheduled = service.schedule_in(&session, TimeAmount { seconds: in_seconds, minutes: in_minutes, ..Default::default() }, parse_world_event_kind(&kind), payload, Visibility::GmOnly, None).await?;
            println!("{}", serde_json::to_string_pretty(&scheduled)?);
        }
        TimeCommand::Events { session, since_tick, since_event_seq, limit } => {
            let events = db.list_world_events_since(&session, since_tick, since_event_seq, limit).await?;
            println!("{}", serde_json::to_string_pretty(&events)?);
        }
    }
    Ok(())
}

fn parse_time_scale(scale: &str) -> TimeScale {
    match scale.to_ascii_lowercase().as_str() {
        "instant" => TimeScale::Instant,
        "combat_round" | "combat" | "round" => TimeScale::CombatRound,
        "exploration" => TimeScale::Exploration,
        "travel" => TimeScale::Travel,
        "downtime" => TimeScale::Downtime,
        "flashback" => TimeScale::Flashback,
        _ => TimeScale::SceneBeat,
    }
}

fn parse_world_event_kind(kind: &str) -> WorldEventKind {
    match kind.to_ascii_lowercase().as_str() {
        "player_action" => WorldEventKind::PlayerAction,
        "npc_action" => WorldEventKind::NpcAction,
        "clock_tick" => WorldEventKind::ClockTick,
        "scene_changed" => WorldEventKind::SceneChanged,
        "scheduled_event_due" => WorldEventKind::ScheduledEventDue,
        "frame_closed" => WorldEventKind::FrameClosed,
        "frame_opened" => WorldEventKind::FrameOpened,
        _ => WorldEventKind::SystemEvent,
    }
}

async fn rules_command_cli(command: RulesCommand) -> Result<()> {
    let db = connect_db().await?;
    db.migrate().await?;
    let search = make_search(&db, default_data_dir())?;
    let steward = RuleStewardAgent::new(db.clone(), search, default_data_dir());
    match command {
        RulesCommand::Query { ruleset, module, query, kind, missing_facets, json: as_json } => {
            let need = RuleNeed {
                need_id: format!("cli_need_{}", uuid::Uuid::new_v4().simple()),
                ruleset_id: ruleset,
                module_id: module,
                need_kind: kind.into(),
                query: query.clone(),
                player_action_summary: query,
                missing_facets,
                visibility: Visibility::GmOnly,
                urgency: RuleUrgency::ImmediateTurn,
                allowed_outputs: vec![RuleAssistOutputKind::ContextBlock, RuleAssistOutputKind::MaterializationPatch, RuleAssistOutputKind::Bp1PatchProposal],
                ..Default::default()
            };
            let assist = steward.assist(need).await?;
            if as_json {
                println!("{}", serde_json::to_string_pretty(&assist)?);
            } else {
                println!("status: {} confidence: {:.2}", assist.status.as_str(), assist.confidence);
                println!("scope: {:?}", assist.answer_scope);
                println!("{}", assist.gm_brief);
                if let Some(summary) = &assist.player_safe_summary { println!("player-safe: {}", summary); }
                if assist.source_refs.is_empty() {
                    println!("source refs: none");
                } else {
                    println!("source refs: {}", assist.source_refs.len());
                    for src in assist.source_refs.iter().take(8) {
                        println!("- {} page={:?} anchor={:?}", src.source_id, src.page, src.anchor_id);
                    }
                }
                if !assist.unresolved_questions.is_empty() {
                    println!("unresolved:");
                    for gap in &assist.unresolved_questions {
                        println!("- {}{}", gap.description, if gap.blocking { " [blocking]" } else { "" });
                    }
                }
                println!("context blocks: {}", assist.context_blocks.len());
            }
            Ok(())
        }
        RulesCommand::CharacterPack { ruleset } => {
            let pack = steward.character_onboarding_pack(&ruleset).await?
                .ok_or_else(|| anyhow!("character onboarding pack not found for {ruleset}; run parse-all first"))?;
            println!("{}", serde_json::to_string_pretty(&pack)?);
            Ok(())
        }
        RulesCommand::Playability { ruleset, module, json: as_json } => {
            let report = steward.playability_gate(&ruleset, module.as_deref()).await?;
            if as_json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("ready: {}", report.ready);
                println!("rule_kernel: {}", report.has_rule_kernel);
                println!("character_template: {}", report.has_character_sheet_template);
                println!("character_creation_flow: {}", report.has_character_creation_flow);
                println!("derived_formula_pack: {}", report.has_derived_formula_pack);
                println!("starter_character_path: {}", report.has_starter_character_path);
                println!("first_session_packet: {}", report.has_first_session_packet);
                if !report.blocking_gaps.is_empty() {
                    println!("blocking gaps:");
                    for gap in &report.blocking_gaps {
                        println!("- {} ({})", gap.message, gap.repair_skill.as_deref().unwrap_or("no repair skill"));
                    }
                }
                if !report.warnings.is_empty() {
                    println!("warnings:");
                    for warning in &report.warnings { println!("- {}: {}", warning.code, warning.message); }
                }
            }
            Ok(())
        }
        RulesCommand::Bp1Patches { ruleset, status, limit } => {
            let patches = db.list_rule_kernel_patches(&ruleset, status.as_deref(), limit).await?;
            println!("{}", serde_json::to_string_pretty(&json!({"patches": patches}))?);
            Ok(())
        }
    }
}

async fn learning_command_cli(command: LearnCommand) -> Result<()> {
    match command {
        LearnCommand::Candidates { ruleset, status, limit } => {
            let db = connect_db().await?;
            db.migrate().await?;
            let candidates = db.list_learning_candidates(ruleset.as_deref(), Some(&status), limit).await?;
            println!("{}", serde_json::to_string_pretty(&json!({"candidates": candidates}))?);
            Ok(())
        }
        LearnCommand::Approve { candidate_id, stage, notes } => {
            let db = connect_db().await?;
            db.migrate().await?;
            let candidate = db.get_learning_candidate(&candidate_id).await?.ok_or_else(|| anyhow!("unknown candidate_id: {candidate_id}"))?;
            let packet = candidate.to_learned_packet(parse_learning_stage(&stage)?);
            db.upsert_learned_packet(&packet).await?;
            db.update_learning_candidate_status(&candidate_id, LearningCandidateStatus::Approved, notes.as_deref()).await?;
            println!("{}", serde_json::to_string_pretty(&json!({"candidate_id": candidate_id, "learned_packet": packet}))?);
            Ok(())
        }
    }
}

fn parse_learning_stage(input: &str) -> Result<LearningStage> {
    match input {
        "unseen" => Ok(LearningStage::Unseen),
        "located" => Ok(LearningStage::Located),
        "looked_up" => Ok(LearningStage::LookedUp),
        "used_once" => Ok(LearningStage::UsedOnce),
        "stable" => Ok(LearningStage::Stable),
        "memorized" => Ok(LearningStage::Memorized),
        other => Err(anyhow!("unknown learning stage: {other}")),
    }
}

async fn search_command_cli(command: SearchCommand) -> Result<()> {
    match command {
        SearchCommand::Query(args) => search_query_cli(args).await,
        SearchCommand::Reindex { data_dir, incremental } => {
            let db = connect_db().await?;
            db.migrate().await?;
            let search = make_search(&db, data_dir.unwrap_or_else(default_data_dir))?;
            let stats = if incremental { search.reindex_incremental().await? } else { search.reindex_all().await? };
            println!("{}", serde_json::to_string_pretty(&stats)?);
            Ok(())
        }
        SearchCommand::Sources => {
            let db = connect_db().await?;
            db.migrate().await?;
            let sources = load_search_source_configs(&db.pool).await?;
            println!("{}", serde_json::to_string_pretty(&sources)?);
            Ok(())
        }
    }
}

async fn search_query_cli(args: SearchArgs) -> Result<()> {
    let jsonl_output = args.jsonl;
    let db = connect_db().await?;
    db.migrate().await?;
    let search = make_search(&db, args.data_dir.clone().unwrap_or_else(default_data_dir))?;
    if args.reindex {
        let stats = if args.incremental_reindex { search.reindex_incremental().await? } else { search.reindex_all().await? };
        if jsonl_output {
            println!("{}", json!({"event":"reindex", "stats": stats}));
        } else {
            eprintln!("search index updated: {} document(s) from {} source config(s)", stats.indexed_documents, stats.source_count);
        }
    }
    let request = build_search_request(args)?;
    let response = search.search_async(&request).await?;
    if let Some(session_id) = request.scopes.get("session_id") {
        let event = LookupEvent {
            event_id: format!("lookup.cli.{}", uuid::Uuid::new_v4().simple()),
            session_id: Some(session_id.clone()),
            ruleset_id: request.scopes.get("ruleset_id").cloned(),
            module_id: request.scopes.get("module_id").cloned(),
            demand_id: None,
            query_text: request.query.clone(),
            search_terms: vec![request.query.clone()],
            source_hits: serde_json::to_value(&response.hits)?,
            result_status: if response.hits.is_empty() { "no_hits".into() } else { "hit".into() },
            created_at: chrono::Utc::now(),
        };
        db.insert_lookup_event(&event).await.ok();
    }
    if request.explain || response.hits.is_empty() || !request.query.is_empty() {
        // no-op: keep variables consumed clearly for future extension
    }
    if response.hits.is_empty() {
        if jsonl_output {
            println!("{}", json!({"event":"done", "hits": 0, "query_id": response.query_id}));
        } else {
            println!("no hits");
        }
        return Ok(());
    }
    if jsonl_output {
        for hit in response.hits {
            println!("{}", json!({"event":"hit", "hit": hit}));
        }
        println!("{}", json!({"event":"done", "query_id": response.query_id, "considered": response.total_considered}));
    } else {
        for (idx, hit) in response.hits.iter().enumerate() {
            println!("{}. [{:.3}] {} / {} / {}", idx + 1, hit.score, hit.domain, hit.logical_kind, hit.title);
            if !hit.scopes.is_empty() { println!("   scopes: {}", serde_json::to_string(&hit.scopes)?); }
            if !hit.source_refs.is_empty() { println!("   sources: {}", serde_json::to_string(&hit.source_refs)?); }
            println!("   {}", hit.snippet.replace('\n', "\n   "));
            if request.explain { println!("   explain: {}", serde_json::to_string(&hit.explain)?); }
        }
    }
    Ok(())
}

fn build_search_request(args: SearchArgs) -> Result<SearchRequest> {
    let mut scopes = BTreeMap::new();
    if let Some(v) = args.ruleset { scopes.insert("ruleset_id".to_string(), v); }
    if let Some(v) = args.module { scopes.insert("module_id".to_string(), v); }
    if let Some(v) = args.session { scopes.insert("session_id".to_string(), v); }
    if let Some(v) = args.scene { scopes.insert("scene_id".to_string(), v); }
    for raw in args.scopes {
        let (key, value) = split_key_value(&raw)?;
        scopes.insert(key, value);
    }
    let mut filters: BTreeMap<String, Vec<String>> = Default::default();
    for raw in args.filters {
        let (key, value) = split_key_value(&raw)?;
        filters.entry(key).or_default().push(value);
    }
    Ok(SearchRequest {
        query: args.query,
        mode: SearchMode::Auto,
        domains: args.domains,
        kinds: args.kinds,
        tags: args.tags,
        scopes,
        filters,
        rewrite_query: true,
        intent: Some("cli_search".into()),
        limit: args.limit.min(100),
        explain: args.explain,
        viewer: VisibilityProfile::gm(),
    })
}

fn split_key_value(input: &str) -> Result<(String, String)> {
    let Some((key, value)) = input.split_once('=') else {
        return Err(anyhow!("expected key=value, got `{input}`"));
    };
    Ok((key.trim().to_string(), value.trim().to_string()))
}

fn search_config_for_data_dir(data_dir: PathBuf) -> SearchConfig {
    SearchConfig::from_env_or_defaults(data_dir)
}

fn make_search(db: &Db, data_dir: PathBuf) -> Result<SearchService> {
    let config = search_config_for_data_dir(data_dir);
    SearchService::open(db.pool.clone(), config)
}


async fn connect_db() -> Result<Db> {
    let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| "postgres://chatrpg:chatrpg@localhost:54323/chatrpg".to_string());
    Db::connect(&url).await
}

fn make_llm() -> Result<Arc<dyn LlmClient>> {
    let config = LlmConfig::from_env()?;
    Ok(Arc::new(OpenAiCompatibleClient::new(config)?))
}

fn default_data_dir() -> PathBuf {
    std::env::var("TRPG_DATA_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("./data"))
}

/// In-process driver for the STAGED ruleset parse — the quick test harness (no
/// server, no browser). Loads the rulebook units + duotext sidecar for one
/// ruleset, inserts a `background_jobs` row, runs `StagedParse::run`, and streams
/// each stage transition (with elapsed time) to stderr. With `--json` it dumps
/// the raw `JobStatus` snapshot; otherwise it prints the resolved
/// `character_sheet_schema` from the partial kernel for eyeballing the fast path.
async fn parse_staged_cli(ruleset: String, data_dir: Option<PathBuf>, stage1_only: bool, json: bool, budget: usize) -> Result<()> {
    let db = connect_db().await?;
    db.migrate().await?;
    let llm = make_llm()?;
    let dir = data_dir.unwrap_or_else(default_data_dir);
    // Locate the rulebook source by picking the largest semantic_units.jsonl.
    let (units_path, source_id) = largest_units_file(&dir.join("parsed/source_units"))?;
    let units = trpg_rule_agent::reader::load_units(&units_path)?;
    let sidecar = std::fs::read_to_string(dir.join(format!("markdown/rulebooks/{source_id}.md"))).ok();
    let job_id = format!("staged_{}", uuid::Uuid::new_v4().simple());
    db.insert_background_job(&job_id, "ruleset_parse_staged", json!({"ruleset": ruleset})).await?;
    let sp = trpg_parser::staged::StagedParse {
        db: db.clone(),
        llm,
        ruleset_id: ruleset.clone(),
        units,
        sidecar_text: sidecar,
        title: ruleset.clone(),
        job_id,
        data_dir: dir,
        stage1_only,
    };
    let t0 = std::time::Instant::now();
    let st = sp.run(budget).await;
    for s in &st.stages {
        let dur = s.duration_secs().map(|d| format!("{d:>6.1}s")).unwrap_or_else(|| "    -- ".into());
        eprintln!("[{dur}] {:<10} {:<7} {}", s.name, s.status, s.detail);
    }
    eprintln!("[{:>6.1}s] TOTAL", t0.elapsed().as_secs_f32());
    if json {
        println!("{}", serde_json::to_string_pretty(&st.to_value())?);
    } else if let Some(k) = db.load_rule_kernel(&ruleset).await? {
        println!("{}", serde_json::to_string_pretty(&k.character_sheet_schema)?);
    }
    Ok(())
}

/// Pick the largest `*.semantic_units.jsonl` under `dir` and derive its
/// `source_id` from the filename (strip the `.semantic_units.jsonl` suffix).
fn largest_units_file(dir: &std::path::Path) -> Result<(PathBuf, String)> {
    let mut best: Option<(PathBuf, String, u64)> = None;
    let entries = std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        let source_id = match name.strip_suffix(".semantic_units.jsonl") {
            Some(id) => id.to_string(),
            None => continue,
        };
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        if best.as_ref().map(|(_, _, s)| size > *s).unwrap_or(true) {
            best = Some((path, source_id, size));
        }
    }
    best.map(|(p, id, _)| (p, id))
        .ok_or_else(|| anyhow!("no *.semantic_units.jsonl under {}", dir.display()))
}



fn extract_json_cli(text: &str) -> Option<Value> {
    if let Some(v) = extract_fenced_cli(text, "json").and_then(|s| serde_json::from_str::<Value>(&s).ok()) { return Some(v); }
    if let Some(v) = extract_fenced_cli(text, "character_draft").and_then(|s| serde_json::from_str::<Value>(&s).ok()) { return Some(v); }
    serde_json::from_str::<Value>(text).ok()
}

fn extract_fenced_cli(text: &str, lang: &str) -> Option<String> {
    let fence = format!("```{lang}");
    let start = text.find(&fence)? + fence.len();
    let rest = &text[start..];
    let end = rest.find("```")?;
    Some(rest[..end].trim().to_string())
}
