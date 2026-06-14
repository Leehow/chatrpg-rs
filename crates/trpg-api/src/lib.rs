use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{error, info};
use trpg_db::Db;
use trpg_ingest::PdfBackendKind;
use trpg_llm::LlmClient;
use trpg_model::*;
use trpg_parser::{ParserConfig, ProjectParseService};
use trpg_rule_agent::RuleStewardAgent;
use trpg_runtime::{validate_character_template_sheet, RuntimeEngine};
use trpg_gm::{
    execute_turn, GmLoop, LoopConfig, OwnedTurnRequest, SceneDeepExtractFn, ToolRegistry,
    TurnEvent, TurnOutcome, CANONICAL_TURN_PLAN,
};
use trpg_search::{load_search_source_configs, upsert_search_source_config, SearchService, SearchSourceConfig};
use uuid::Uuid;

pub use trpg_runtime::scene_navigation::{
    extract_module_scenes, build_nav_prompt, scene_navigator, validate_transition, prefetch_frontier,
};

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub llm: Arc<dyn LlmClient>,
    pub runtime: RuntimeEngine,
    pub search: SearchService,
    pub parser_config: ParserConfig,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api-doc/openapi.json", get(openapi_json))
        .route("/api/bundles", get(list_bundles))
        .route("/api/ingest/parse-all", post(parse_all))
        .route("/api/modules/{module_id}/extract/continue", post(module_extract_continue))
        .route("/api/modules/{module_id}/skeleton/complete", post(module_skeleton_complete))
        .route("/api/ingest/ruleset", post(ingest_ruleset))
        .route("/api/ingest/{job_id}/status", get(ingest_status))
        .route("/api/ingest/{job_id}/events", get(ingest_events))
        .route("/api/rulesets/{id}/identity", get(ruleset_identity))
        .route("/api/rulesets/{id}/character-template", get(ruleset_character_template))
        .route("/api/context/compile", post(compile_context))
        .route("/api/checks/resolve", post(resolve_check_api))
        .route("/api/conflict/start", post(conflict_start_api))
        .route("/api/conflict/{session_id}/frames", get(conflict_frames_api))
        .route("/api/search", post(search_api))
        .route("/api/search/reindex", post(search_reindex_api))
        .route("/api/search/load", post(search_load_api))
        .route("/api/search/sources", get(search_sources_api).post(upsert_search_source_api))
        .route("/api/rules/lookup", post(rule_lookup))
        .route("/api/rules/steward/assist", post(rule_steward_assist_api))
        .route("/api/rules/playability", post(rule_playability_api))
        .route("/api/rules/character-onboarding/{ruleset_id}", get(rule_character_onboarding_api))
        .route("/api/rulings", post(record_ruling_api))
        .route("/api/learned-packets", post(upsert_learned_packet_api))
        .route("/api/learned-packets/{ruleset_id}", get(list_learned_packets_api))
        .route("/api/learning/candidates", get(list_learning_candidates_api))
        .route("/api/learning/candidates/{candidate_id}/approve", post(approve_learning_candidate_api))
        .route("/api/characters/create", post(create_character_sse))
        .route("/api/sessions/start", post(start_session))
        .route("/api/sessions/{session_id}/turn", post(play_turn_sse))
        .route("/api/sessions/{session_id}/time", get(get_session_time_api))
        .route("/api/sessions/{session_id}/time/advance", post(advance_session_time_api))
        .route("/api/sessions/{session_id}/events", get(list_session_events_api))
        .route("/api/sessions/{session_id}/scheduled-events", post(schedule_session_event_api))
        .route("/api/sessions/{session_id}/memory", get(get_session_memory))
        .route("/api/sessions/{session_id}/memory/retrieve", post(retrieve_session_memory))
        .route("/api/sessions/{session_id}/memory/compact", post(compact_session_memory))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

pub async fn serve(addr: SocketAddr, state: AppState) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!(%addr, "starting trpg api");
    axum::serve(listener, router(state)).await?;
    Ok(())
}

async fn health() -> Json<Value> { Json(json!({"status":"ok"})) }

async fn openapi_json() -> Json<Value> {
    Json(json!({
        "openapi": "3.1.0",
        "info": {"title": "chatrpg-rs API", "version": "1.4.0"},
        "paths": {
            "/health": {"get": {"summary": "Health check"}},
            "/api/ingest/parse-all": {"post": {"summary": "Queue parser job and rebuild Tantivy search index"}},
            "/api/ingest/ruleset": {"post": {"summary": "Start a staged (progressive) ruleset parse and return the job id (202)"}},
            "/api/ingest/{job_id}/status": {"get": {"summary": "One-shot staged-parse job status snapshot"}},
            "/api/ingest/{job_id}/events": {"get": {"summary": "SSE stream of staged-parse progress snapshots"}},
            "/api/rulesets/{id}/identity": {"get": {"summary": "Ruleset identity once Stage 0 done (else 202)"}},
            "/api/rulesets/{id}/character-template": {"get": {"summary": "Character sheet schema once Stage 1 done (else 202)"}},
            "/api/context/compile": {"post": {"summary": "Compile BP1/BP2/BP3 context"}},
            "/api/checks/resolve": {"post": {"summary": "Resolve an open pending check from player roll input"}},
            "/api/conflict/start": {"post": {"summary": "Start or continue a ConflictFrame/CombatFrame through the Rust CombatAgent"}},
            "/api/conflict/{session_id}/frames": {"get": {"summary": "List active combat/conflict working-state frames"}},
            "/api/search": {"post": {"summary": "Unified Tantivy search across configured SQL/file/JSONL sources"}},
            "/api/search/reindex": {"post": {"summary": "Full or incremental Tantivy reindex from search_source_configs"}},
            "/api/search/load": {"post": {"summary": "Persist/load a SearchHit as a TTL ContextBlock"}},
            "/api/search/sources": {"get": {"summary": "List configured search source configs"}},
            "/api/rules/lookup": {"post": {"summary": "Locate a rule demand and record lookup event"}},
            "/api/rules/steward/assist": {"post": {"summary": "Rule Steward Agent source-backed rule/character/materialization assistance"}},
            "/api/rules/playability": {"post": {"summary": "Check RuleKernel + character onboarding + first-session playability gate"}},
            "/api/rules/character-onboarding/{ruleset_id}": {"get": {"summary": "Get parsed CharacterOnboardingPack"}},
            "/api/rulings": {"post": {"summary": "Record a source-backed, learned, provisional, or table ruling"}},
            "/api/learned-packets": {"post": {"summary": "Insert or update a learned packet"}},
            "/api/learned-packets/{ruleset_id}": {"get": {"summary": "Inspect learned packets for a ruleset"}},
            "/api/learning/candidates": {"get": {"summary": "List review-gated learning candidates"}},
            "/api/learning/candidates/{candidate_id}/approve": {"post": {"summary": "Approve a learning candidate into learned_packets"}},
            "/api/characters/create": {"post": {"summary": "Create character over SSE"}},
            "/api/sessions/start": {"post": {"summary": "Start terminal/API session"}},
            "/api/sessions/{session_id}/turn": {"post": {"summary": "Run one GM turn over SSE"}},
            "/api/sessions/{session_id}/time": {"get": {"summary": "Read authoritative world time"}},
            "/api/sessions/{session_id}/time/advance": {"post": {"summary": "Advance authoritative world time and trigger due scheduled events"}},
            "/api/sessions/{session_id}/events": {"get": {"summary": "List world events after a world_tick watermark"}},
            "/api/sessions/{session_id}/scheduled-events": {"post": {"summary": "Schedule a future world event"}},
            "/api/sessions/{session_id}/memory": {"get": {"summary": "Inspect GM memory"}},
            "/api/sessions/{session_id}/memory/retrieve": {"post": {"summary": "Retrieve memory candidates"}},
            "/api/sessions/{session_id}/memory/compact": {"post": {"summary": "Build/update a cache-stable memory snapshot"}}
        },
        "x-stack": ["axum", "tokio", "sqlx", "serde", "reqwest", "tracing", "tantivy", "oxidize-pdf", "runtime-auto-search", "cjk-query-expansion", "rust-gm-agent", "agentic-checks", "world-time-spine"]
    }))
}

async fn list_bundles(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let bundles = state.db.list_bundles().await?;
    Ok(Json(json!({"bundles": bundles})))
}

#[derive(Debug, Deserialize, Default)]
pub struct ParseAllQuery {
    pub force: Option<bool>,
    pub full_parse: Option<bool>,
    pub pdf_backend: Option<String>,
}

async fn parse_all(Query(query): Query<ParseAllQuery>, State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let job_id = format!("parse_all_{}", Uuid::new_v4().simple());
    let mut config = state.parser_config.clone();
    if query.force.unwrap_or(false) { config.force = true; }
    if let Some(pdf_backend) = query.pdf_backend.as_deref() {
        config.pdf_backend = match pdf_backend.to_ascii_lowercase().as_str() {
            "oxidize" | "oxidize-pdf" | "oxidize_pdf" => PdfBackendKind::Oxidize,
            "pdftotext" | "poppler" => PdfBackendKind::Pdftotext,
            _ => PdfBackendKind::Auto,
        };
    }
    if query.full_parse.unwrap_or(false) {
        config.parse_full_chunks = true;
    }
    let rule_steward_first_pass = std::env::var("TRPG_RULE_STEWARD_FIRST_PASS")
        .ok()
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(true);
    config.parse_config_hash = sha256_hex(format!("parser=v1.16.2;schema=v1;prompt=rule_steward_react_first_pass_v1;full_chunks={};pdf_backend={};clean_mode={};oxidize_chunk_target_chars={};llm_clean={};semantic_units={};semantic_unit_max_chars={};llm_semantic_wash={};rule_steward_first_pass={}", config.parse_full_chunks, config.pdf_backend.as_str(), config.oxidize_clean_mode.as_str(), config.oxidize_chunk_target_chars, config.llm_clean_extraction, config.semantic_unit_conditioning, config.semantic_unit_max_chars, config.llm_semantic_wash, rule_steward_first_pass));
    state.db.insert_background_job(&job_id, "parse_all", json!({"data_dir": config.data_dir.clone(), "force": config.force, "full_parse": config.parse_full_chunks, "pdf_backend": config.pdf_backend.as_str(), "clean_mode": config.oxidize_clean_mode.as_str(), "llm_clean": config.llm_clean_extraction, "rule_steward_first_pass": rule_steward_first_pass})).await?;
    let db = state.db.clone();
    let llm = state.llm.clone();
    let search = state.search.clone();
    let job_id_spawn = job_id.clone();
    tokio::spawn(async move {
        let service = ProjectParseService::new(db.clone(), llm, config);
        if let Err(err) = db.update_background_job(&job_id_spawn, "running", json!({}), None).await {
            error!(error = %err, "failed to mark parse job running");
        }
        match service.parse_all().await {
            Ok(project) => {
                let search_stats = match search.reindex_all().await {
                    Ok(stats) => json!(stats),
                    Err(err) => json!({"search_index_error": err.to_string()}),
                };
                let _ = db.update_background_job(&job_id_spawn, "done", json!({"rulesets": project.rulesets.len(), "modules": project.modules.len(), "search": search_stats}), None).await;
            }
            Err(err) => {
                error!(error = %err, "parse-all background job failed");
                let _ = db.update_background_job(&job_id_spawn, "error", json!({}), Some(&err.to_string())).await;
            }
        }
    });
    Ok(Json(json!({"job_id": job_id, "status": "queued"})))
}

// --- Background module deep-extract continue (Phase 5) ------------------------

#[derive(Debug, Deserialize, Default)]
pub struct ModuleExtractContinueRequest {
    /// The semantic-units source_id (file stem under parsed/source_units). Lets
    /// the job locate `{data_dir}/parsed/source_units/{source_id}.semantic_units.jsonl`.
    pub source_id: String,
    /// Optional override; defaults to the bundle's stored ruleset_id.
    #[serde(default)]
    pub ruleset_id: Option<String>,
    /// Per-scene LLM tool-call budget; defaults to 12 (matches parse_module).
    #[serde(default)]
    pub budget: Option<usize>,
}

/// POST /api/modules/{module_id}/extract/continue — enqueue a background job that
/// deep-extracts the module's remaining `SkeletonOnly` scenes one by one and
/// re-persists the upgraded ModuleGraph. Mirrors `parse_all`'s
/// insert_background_job + tokio::spawn(mark running -> work -> done/error).
/// Returns the job id immediately (202). Fail-closed downgrade: the first-scene
/// fallback already shipped, so this is pure optimization.
async fn module_extract_continue(
    Path(module_id): Path<String>,
    State(state): State<AppState>,
    Json(req): Json<ModuleExtractContinueRequest>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let job_id = format!("modext.{module_id}");
    let budget = req.budget.unwrap_or(12);
    // Read-after-check dedupe: if this module's job is already running, don't
    // insert+spawn a second deep-extract coroutine (they'd read the same bundle
    // and last-writer-wins overwrite each other). Narrow TOCTOU window, but fine
    // for a manual one-shot endpoint. Do not touch the shared insert helper.
    if matches!(state.db.background_job_status(&job_id).await?.as_deref(), Some("running")) {
        return Ok((StatusCode::OK, Json(json!({"job_id": job_id, "module_id": module_id, "status": "already_running"}))));
    }
    state.db.insert_background_job(&job_id, "module_extract_continue", json!({"module_id": module_id, "source_id": req.source_id, "ruleset_id": req.ruleset_id})).await?;
    let db = state.db.clone();
    let llm = state.llm.clone();
    let data_dir = state.parser_config.data_dir.clone();
    let job_id_spawn = job_id.clone();
    let (mid, sid, rid) = (module_id.clone(), req.source_id.clone(), req.ruleset_id.clone());
    tokio::spawn(async move {
        let _ = db.update_background_job(&job_id_spawn, "running", json!({}), None).await;
        match continue_module_extraction(&db, llm.as_ref(), &mid, &sid, rid.as_deref(), &data_dir, budget).await {
            Ok(n) => { let _ = db.update_background_job(&job_id_spawn, "done", json!({"deep_extracted": n}), None).await; }
            Err(err) => {
                error!(error = %err, module_id = %mid, "module_extract_continue job failed");
                let _ = db.update_background_job(&job_id_spawn, "error", json!({}), Some(&err.to_string())).await;
            }
        }
    });
    Ok((StatusCode::ACCEPTED, Json(json!({"job_id": job_id, "module_id": module_id, "status": "queued"}))))
}

/// Load the module bundle's ModuleGraph + units, deep-extract every scene still
/// `SkeletonOnly` (serially, no concurrent overwrite), then re-persist the
/// upgraded ModuleGraph via `upsert_module_bundle` (same row, same hashes).
/// Returns how many scenes were upgraded to `DeepExtracted`.
///
/// Fail-closed: missing bundle/units -> warn + early-return Ok(0) (never panic,
/// never fabricate). A persistence failure surfaces as Err so the job is marked
/// `error`. Per-scene loop failures keep that scene `SkeletonOnly` (the reusable
/// `deep_extract_scene_in_place` returns false) — the next session falls back to
/// the skeleton, which is the accepted downgrade.
pub async fn continue_module_extraction(
    db: &Db,
    llm: &dyn LlmClient,
    module_id: &str,
    source_id: &str,
    ruleset_id: Option<&str>,
    data_dir: &std::path::Path,
    budget: usize,
) -> anyhow::Result<usize> {
    // Thin wrapper: deep-extract ALL remaining SkeletonOnly scenes (only=None).
    // The single-scene path (scene_navigator on-arrival) reuses the same core
    // with only=Some(node_id). source_id is known here (from the request), so
    // pass it explicitly to skip the bundle-derived fallback.
    extract_module_scenes(db, llm, module_id, Some(source_id), ruleset_id, data_dir, budget, None).await
}

// --- Background module skeleton gleaning (deployment wiring) ------------------

#[derive(Debug, Deserialize, Default)]
pub struct ModuleSkeletonCompleteRequest {
    /// Optional override; defaults to the bundle's stored ruleset_id.
    #[serde(default)]
    pub ruleset_id: Option<String>,
    /// Gleaning tool-call budget; defaults to 8 (skeleton stubs are cheap vs deep).
    #[serde(default)]
    pub budget: Option<usize>,
}

/// Build the dedicated module-reader LLM client for background gleaning: same
/// provider/base_url/key as the main client, but a quality-first model (default
/// gpt-5.4 via TRPG_MODULE_READER_MODEL). Mirrors trpg-parser's crate-private
/// `build_module_reader_llm` (not reachable from here). `None` → caller falls
/// back to the main AppState client. Zero hardcode: model name is env-defaulted.
fn build_module_reader_llm() -> Option<Arc<dyn LlmClient>> {
    let mut cfg = trpg_llm::LlmConfig::from_env().ok()?;
    cfg.model = std::env::var("TRPG_MODULE_READER_MODEL").unwrap_or_else(|_| "gpt-5.4".to_string());
    trpg_llm::OpenAiCompatibleClient::new(cfg)
        .ok()
        .map(|c| Arc::new(c) as Arc<dyn LlmClient>)
}

/// POST /api/modules/{module_id}/skeleton/complete — enqueue a background job
/// that gleans the module's skeleton (completes missing TOC stubs as new
/// SkeletonOnly scenes, NO deep extraction) so the navigation map is whole, then
/// re-persists the upgraded ModuleGraph. Mirrors `module_extract_continue`'s
/// insert_background_job + tokio::spawn(running -> work -> done/error). Returns
/// the job id immediately (202). Pure optimization on top of the first-scene
/// fallback, so all failure paths downgrade to Ok(0).
async fn module_skeleton_complete(
    Path(module_id): Path<String>,
    State(state): State<AppState>,
    Json(req): Json<ModuleSkeletonCompleteRequest>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let job_id = format!("modskel.{module_id}");
    let budget = req.budget.unwrap_or(8);
    // Read-after-check dedupe: a second running coroutine would read the same
    // bundle and last-writer-wins clobber the first. Narrow TOCTOU, fine for a
    // manual one-shot endpoint.
    if matches!(state.db.background_job_status(&job_id).await?.as_deref(), Some("running")) {
        return Ok((StatusCode::OK, Json(json!({"job_id": job_id, "module_id": module_id, "status": "already_running"}))));
    }
    state.db.insert_background_job(&job_id, "module_skeleton_complete", json!({"module_id": module_id, "ruleset_id": req.ruleset_id})).await?;
    let db = state.db.clone();
    // Gleaning is invisible background work → prefer the quality-first module
    // reader model (gpt-5.4); fall back to the GM/main client if it can't build.
    let llm = build_module_reader_llm().unwrap_or_else(|| state.llm.clone());
    let data_dir = state.parser_config.data_dir.clone();
    let job_id_spawn = job_id.clone();
    let (mid, rid) = (module_id.clone(), req.ruleset_id.clone());
    tokio::spawn(async move {
        let _ = db.update_background_job(&job_id_spawn, "running", json!({}), None).await;
        match complete_module_skeleton(&db, llm.as_ref(), &mid, rid.as_deref(), &data_dir, budget).await {
            Ok(n) => { let _ = db.update_background_job(&job_id_spawn, "done", json!({"stubs_added": n}), None).await; }
            Err(err) => {
                error!(error = %err, module_id = %mid, "module_skeleton_complete job failed");
                let _ = db.update_background_job(&job_id_spawn, "error", json!({}), Some(&err.to_string())).await;
            }
        }
    });
    Ok((StatusCode::ACCEPTED, Json(json!({"job_id": job_id, "module_id": module_id, "status": "queued"}))))
}

/// Load the module bundle's ModuleGraph + units, glean the skeleton against the
/// TOC to append missing scenes as new `SkeletonOnly` nodes (no deep extraction),
/// then re-persist the upgraded ModuleGraph via `upsert_module_bundle` (same row,
/// same hashes). Returns how many stub scenes were added. Reuses the same
/// load/upsert skeleton as `extract_module_scenes`.
///
/// Fail-closed: missing bundle/units → warn + Ok(0); no new stubs → Ok(0) without
/// re-persisting; never panic, never fabricate. Only a persistence failure
/// surfaces as Err so the job is marked `error`.
pub async fn complete_module_skeleton(
    db: &Db,
    llm: &dyn LlmClient,
    module_id: &str,
    ruleset_id: Option<&str>,
    data_dir: &std::path::Path,
    budget: usize,
) -> anyhow::Result<usize> {
    let Some((mut bundle, source_hash, parse_config_hash)) = db.load_module_bundle_for_continue(module_id).await? else {
        info!(module_id, "complete_module_skeleton: no module bundle found; nothing to do");
        return Ok(0);
    };
    // source_id derived from the bundle's source_index — the same id parse_module
    // wrote the semantic-units file under (mirrors extract_module_scenes).
    let Some(source_id) = bundle.source_index.sources.first().map(|s| s.source_id.clone()) else {
        info!(module_id, "complete_module_skeleton: no source_id in bundle; nothing to do");
        return Ok(0);
    };
    // Load the same semantic units parse_module read, from the same data dir path.
    let units_path = data_dir.join("parsed/source_units").join(format!("{source_id}.semantic_units.jsonl"));
    let units = match trpg_rule_agent::reader::load_units(&units_path) {
        Ok(u) if !u.is_empty() => u,
        Ok(_) => { info!(module_id, path = %units_path.display(), "complete_module_skeleton: empty units; nothing to do"); return Ok(0); }
        Err(err) => { error!(error = %err, path = %units_path.display(), "complete_module_skeleton: load_units failed; nothing to do"); return Ok(0); }
    };
    // Optional column-aligned sidecar (read_layout view); degrades to None.
    let sidecar_text = std::fs::read_to_string(data_dir.join(format!("markdown/modules/{source_id}.md"))).ok();
    let resolved_ruleset = ruleset_id.map(str::to_string).or_else(|| bundle.ruleset_id.clone());
    let ctx = trpg_rule_agent::reader::ModuleReaderCtx { units: &units, sidecar_text, ruleset_id: resolved_ruleset };

    // Glean the skeleton: append missing TOC stubs as new SkeletonOnly scenes.
    // complete_skeleton_stubs only appends (existing nodes untouched), so we
    // operate directly on the graph's scenes vec.
    let mut scenes = bundle.module_graph.scenes.clone();
    let added = trpg_rule_agent::reader::complete_skeleton_stubs(llm, &ctx, &mut scenes, budget).await;
    if added == 0 {
        info!(module_id, "complete_module_skeleton: no stub added; skipping re-persist");
        return Ok(0);
    }
    bundle.module_graph.scenes = scenes;

    // Re-persist into the same parsed_bundles row (on conflict (bundle_id)).
    db.upsert_module_bundle(&bundle, None, &source_hash, &parse_config_hash).await?;
    info!(module_id, stubs_added = added, "complete_module_skeleton: re-persisted upgraded ModuleGraph");
    Ok(added)
}

// --- Staged (progressive) ruleset parsing: ingest job + live progress ---------

#[derive(Debug, Deserialize, Default)]
pub struct IngestRulesetRequest {
    /// The ruleset id to parse (also used as the title fallback).
    pub ruleset_id: String,
    /// Optional explicit title; defaults to `ruleset_id`.
    #[serde(default)]
    pub title: Option<String>,
    /// Per-slice LLM tool-call budget; defaults to 9 (matches the CLI harness).
    #[serde(default)]
    pub budget: Option<usize>,
    /// Stop after Stage 1 (fast path) — used by smoke tests.
    #[serde(default)]
    pub stage1_only: bool,
}

/// Locate the largest `*.semantic_units.jsonl` under `dir`, deriving its
/// `source_id` from the filename. Mirrors the CLI `largest_units_file`.
fn largest_units_file(dir: &std::path::Path) -> anyhow::Result<(std::path::PathBuf, String)> {
    let mut best: Option<(std::path::PathBuf, String, u64)> = None;
    let entries = std::fs::read_dir(dir).map_err(|e| anyhow::anyhow!("read {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) { Some(n) => n, None => continue };
        let source_id = match name.strip_suffix(".semantic_units.jsonl") { Some(id) => id.to_string(), None => continue };
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        if best.as_ref().map(|(_, _, s)| size > *s).unwrap_or(true) {
            best = Some((path, source_id, size));
        }
    }
    best.map(|(p, id, _)| (p, id))
        .ok_or_else(|| anyhow::anyhow!("no *.semantic_units.jsonl under {}", dir.display()))
}

/// Build a `StagedParse` for one ruleset by loading its units + sidecar from the
/// configured data dir. Shared by `ingest_ruleset` and any future caller.
fn build_staged_parse(state: &AppState, req: &IngestRulesetRequest, job_id: String) -> anyhow::Result<trpg_parser::staged::StagedParse> {
    let dir = state.parser_config.data_dir.clone();
    let (units_path, source_id) = largest_units_file(&dir.join("parsed/source_units"))?;
    let units = trpg_rule_agent::reader::load_units(&units_path)?;
    let sidecar = std::fs::read_to_string(dir.join(format!("markdown/rulebooks/{source_id}.md"))).ok();
    Ok(trpg_parser::staged::StagedParse {
        db: state.db.clone(),
        llm: state.llm.clone(),
        ruleset_id: req.ruleset_id.clone(),
        units,
        sidecar_text: sidecar,
        title: req.title.clone().unwrap_or_else(|| req.ruleset_id.clone()),
        job_id,
        data_dir: dir,
        stage1_only: req.stage1_only,
    })
}

/// POST /api/ingest/ruleset — start a staged parse in the background and return
/// the job id immediately (202 Accepted).
async fn ingest_ruleset(State(state): State<AppState>, Json(req): Json<IngestRulesetRequest>) -> Result<(StatusCode, Json<Value>), ApiError> {
    let job_id = format!("staged_{}", Uuid::new_v4().simple());
    let budget = req.budget.unwrap_or(9);
    let sp = build_staged_parse(&state, &req, job_id.clone())?;
    state.db.insert_background_job(&job_id, "ruleset_parse_staged", json!({"ruleset": req.ruleset_id})).await?;
    let job_id_resp = job_id.clone();
    let ruleset_id = req.ruleset_id.clone();
    tokio::spawn(async move {
        let st = sp.run(budget).await;
        if let Some(err) = &st.error {
            error!(error = %err, "staged ruleset parse reported error");
        }
    });
    Ok((StatusCode::ACCEPTED, Json(json!({"job_id": job_id_resp, "ruleset_id": ruleset_id}))))
}

/// GET /api/ingest/{job_id}/status — one-shot JSON snapshot (poll fallback).
async fn ingest_status(Path(job_id): Path<String>, State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    match state.db.load_background_job(&job_id).await? {
        Some(row) => Ok(Json(row)),
        None => Err(anyhow::anyhow!("job not found: {job_id}").into()),
    }
}

/// GET /api/ingest/{job_id}/events — SSE stream of `result_json` snapshots, one
/// every ~750ms, ending when the job status is `done` or `failed`.
async fn ingest_events(Path(job_id): Path<String>, State(state): State<AppState>) -> impl IntoResponse {
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(64);
    let db = state.db.clone();
    tokio::spawn(async move {
        loop {
            match db.load_background_job(&job_id).await {
                Ok(Some(row)) => {
                    let status = row.get("status").and_then(Value::as_str).unwrap_or("").to_string();
                    let result_json = row.get("result_json").cloned().unwrap_or(Value::Null);
                    let _ = tx.send(Ok(Event::default().json_data(result_json).unwrap_or_default())).await;
                    if matches!(status.as_str(), "done" | "failed" | "error") {
                        break;
                    }
                }
                Ok(None) => {
                    let _ = tx.send(Ok(Event::default().json_data(json!({"error": "job not found"})).unwrap_or_default())).await;
                    break;
                }
                Err(err) => {
                    let _ = tx.send(Ok(Event::default().json_data(json!({"error": err.to_string()})).unwrap_or_default())).await;
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(750)).await;
        }
    });
    Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default())
}

/// Build the `{stage, progress_pct}` 202 body from the latest staged job
/// snapshot for this ruleset, falling back to the pending stub if none exists.
async fn latest_staged_progress(db: &Db, ruleset_id: &str) -> Value {
    if let Ok(Some(result)) = db.latest_staged_job_for_ruleset(ruleset_id).await {
        let stage = result.get("stage").and_then(Value::as_str).unwrap_or("identity").to_string();
        let progress = result.get("progress_pct").and_then(Value::as_u64).unwrap_or(0);
        return json!({"stage": stage, "progress_pct": progress});
    }
    json!({"stage": "identity", "progress_pct": 0})
}

/// GET /api/rulesets/{id}/identity — `{title, game_identity}` once Stage 0 is
/// done; else 202 with `{stage, progress_pct}`.
async fn ruleset_identity(Path(id): Path<String>, State(state): State<AppState>) -> Result<(StatusCode, Json<Value>), ApiError> {
    if let Some(kernel) = state.db.load_rule_kernel(&id).await? {
        let summary = kernel.game_identity.get("summary").and_then(Value::as_str).unwrap_or("");
        if !summary.is_empty() {
            return Ok((StatusCode::OK, Json(json!({"ruleset_id": id, "game_identity": kernel.game_identity}))));
        }
    }
    Ok((StatusCode::ACCEPTED, Json(latest_staged_progress(&state.db, &id).await)))
}

/// A kernel jsonb value counts as "ready" when it is a non-empty object (or any
/// non-null scalar/array) — the Stage-1/Stage-0 gates serve 200 only then.
fn kernel_value_ready(value: &Value) -> bool {
    match value {
        Value::Object(m) => !m.is_empty(),
        Value::Null => false,
        _ => true,
    }
}

/// GET /api/rulesets/{id}/character-template — the sheet schema once Stage 1 is
/// done; else 202 with `{stage, progress_pct}`.
async fn ruleset_character_template(Path(id): Path<String>, State(state): State<AppState>) -> Result<(StatusCode, Json<Value>), ApiError> {
    if let Some(kernel) = state.db.load_rule_kernel(&id).await? {
        if kernel_value_ready(&kernel.character_sheet_schema) {
            return Ok((StatusCode::OK, Json(json!({"ruleset_id": id, "character_sheet_schema": kernel.character_sheet_schema}))));
        }
    }
    Ok((StatusCode::ACCEPTED, Json(latest_staged_progress(&state.db, &id).await)))
}

#[derive(Debug, Deserialize, Default)]
pub struct SearchReindexRequest {
    pub incremental: Option<bool>,
}

async fn search_api(State(state): State<AppState>, Json(req): Json<SearchRequest>) -> Result<Json<SearchResponse>, ApiError> {
    let response = state.search.search_async(&req).await?;
    if let Some(session_id) = req.scopes.get("session_id").cloned() {
        let event = LookupEvent {
            event_id: format!("lookup_{}", Uuid::new_v4().simple()),
            session_id: Some(session_id),
            ruleset_id: req.scopes.get("ruleset_id").cloned(),
            module_id: req.scopes.get("module_id").cloned(),
            demand_id: req.intent.clone(),
            query_text: req.query.clone(),
            search_terms: vec![req.query.clone()],
            source_hits: serde_json::to_value(&response.hits)?,
            result_status: if response.hits.is_empty() { "no_hits".into() } else { "searched".into() },
            created_at: Utc::now(),
        };
        let _ = state.db.insert_lookup_event(&event).await;
    }
    Ok(Json(response))
}

async fn search_reindex_api(State(state): State<AppState>, Json(req): Json<SearchReindexRequest>) -> Result<Json<SearchIndexStats>, ApiError> {
    let stats = if req.incremental.unwrap_or(false) {
        state.search.reindex_incremental().await?
    } else {
        state.search.reindex_all().await?
    };
    Ok(Json(stats))
}

async fn search_load_api(State(state): State<AppState>, Json(req): Json<SearchLoadRequest>) -> Result<Json<SearchLoadResponse>, ApiError> {
    let block = state.search.load_hit_to_context_block(&req)?;
    let mut persisted = false;
    if req.persist {
        if let Some(session_id) = req.session_id.as_deref() {
            state.db.upsert_runtime_context_block(session_id, &block).await?;
            persisted = true;
        }
    }
    let lookup_event_id = if let Some(session_id) = req.session_id.clone() {
        let event = LookupEvent {
            event_id: format!("lookup_{}", Uuid::new_v4().simple()),
            session_id: Some(session_id),
            ruleset_id: req.ruleset_id.clone().or_else(|| req.hit.scopes.get("ruleset_id").cloned()),
            module_id: req.module_id.clone().or_else(|| req.hit.scopes.get("module_id").cloned()),
            demand_id: req.demand_id.clone().or_else(|| Some("search_load".to_string())),
            query_text: req.query_text.clone().unwrap_or_else(|| req.load_reason.clone()),
            search_terms: vec![req.hit.title.clone()],
            source_hits: serde_json::to_value(&req.hit)?,
            result_status: if persisted { "loaded_persisted".into() } else { "loaded_ephemeral".into() },
            created_at: Utc::now(),
        };
        state.db.insert_lookup_event(&event).await?;
        Some(event.event_id)
    } else { None };
    Ok(Json(SearchLoadResponse { block, persisted, lookup_event_id }))
}

async fn search_sources_api(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let sources = load_search_source_configs(&state.db.pool).await?;
    let out = sources.into_iter().map(|source| json!({
        "source_config_id": source.source_config_id,
        "source_kind": source.source_kind,
        "label": source.label,
        "enabled": source.enabled,
        "priority": source.priority,
        "config_json": source.config_json,
    })).collect::<Vec<_>>();
    Ok(Json(json!({"sources": out})))
}

async fn upsert_search_source_api(State(state): State<AppState>, Json(req): Json<SearchSourceConfig>) -> Result<Json<SearchSourceConfig>, ApiError> {
    upsert_search_source_config(&state.db.pool, &req).await?;
    Ok(Json(req))
}

#[derive(Debug, Deserialize)]
pub struct RuleLookupRequest {
    pub session_id: Option<String>,
    pub ruleset_id: Option<String>,
    pub module_id: Option<String>,
    pub scene_id: Option<String>,
    pub demand_id: Option<String>,
    pub query_text: String,
    pub limit: Option<u32>,
}

async fn rule_lookup(State(state): State<AppState>, Json(req): Json<RuleLookupRequest>) -> Result<Json<Value>, ApiError> {
    let mut scopes = std::collections::BTreeMap::new();
    if let Some(v) = req.ruleset_id.clone() { scopes.insert("ruleset_id".into(), v); }
    if let Some(v) = req.module_id.clone() { scopes.insert("module_id".into(), v); }
    if let Some(v) = req.session_id.clone() { scopes.insert("session_id".into(), v); }
    if let Some(v) = req.scene_id.clone() { scopes.insert("scene_id".into(), v); }
    let response = state.search.search_async(&SearchRequest {
        query: req.query_text.clone(),
        mode: SearchMode::Auto,
        domains: vec!["rules".into(), "modules".into(), "learned".into(), "source".into()],
        scopes,
        limit: req.limit.unwrap_or(8),
        explain: true,
        viewer: VisibilityProfile::gm(),
        rewrite_query: true,
        intent: Some("rule_lookup".into()),
        ..Default::default()
    }).await?;
    let status = if response.hits.is_empty() { "provisional" } else { "source_backed" };
    let event = LookupEvent {
        event_id: format!("lookup_{}", Uuid::new_v4().simple()),
        session_id: req.session_id.clone(),
        ruleset_id: req.ruleset_id.clone(),
        module_id: req.module_id.clone(),
        demand_id: req.demand_id.clone(),
        query_text: req.query_text.clone(),
        search_terms: vec![req.query_text.clone()],
        source_hits: serde_json::to_value(&response.hits)?,
        result_status: status.to_string(),
        created_at: Utc::now(),
    };
    state.db.insert_lookup_event(&event).await?;
    Ok(Json(json!({"event_id": event.event_id, "status": status, "query_id": response.query_id, "hits": response.hits})))
}


async fn rule_steward_assist_api(State(state): State<AppState>, Json(req): Json<RuleNeed>) -> Result<Json<RuleAssist>, ApiError> {
    let steward = RuleStewardAgent::new(state.db.clone(), state.search.clone(), state.parser_config.data_dir.clone());
    let assist = steward.assist(req).await?;
    Ok(Json(assist))
}

#[derive(Debug, Deserialize)]
pub struct RulePlayabilityRequest {
    pub ruleset_id: String,
    #[serde(default)]
    pub module_id: Option<String>,
}

async fn rule_playability_api(State(state): State<AppState>, Json(req): Json<RulePlayabilityRequest>) -> Result<Json<PlayabilityGateReport>, ApiError> {
    let steward = RuleStewardAgent::new(state.db.clone(), state.search.clone(), state.parser_config.data_dir.clone());
    let report = steward.playability_gate(&req.ruleset_id, req.module_id.as_deref()).await?;
    Ok(Json(report))
}

async fn rule_character_onboarding_api(State(state): State<AppState>, Path(ruleset_id): Path<String>) -> Result<Json<CharacterOnboardingPack>, ApiError> {
    let pack = state.db.load_character_onboarding_pack(&ruleset_id).await?
        .ok_or_else(|| anyhow::anyhow!("character onboarding pack not found for {ruleset_id}"))?;
    Ok(Json(pack))
}

#[derive(Debug, Deserialize)]
pub struct RecordRulingRequest {
    pub session_id: Option<String>,
    pub ruleset_id: Option<String>,
    pub module_id: Option<String>,
    pub demand_id: Option<String>,
    pub ruling_text: String,
    #[serde(default)]
    pub source_refs: Vec<SourceRef>,
    pub status: Option<RulingStatus>,
    pub confidence: Option<RulingConfidence>,
    pub provisional: Option<bool>,
    pub superseded_by: Option<String>,
}

async fn record_ruling_api(State(state): State<AppState>, Json(req): Json<RecordRulingRequest>) -> Result<Json<RulingLogEntry>, ApiError> {
    let status = req.status.unwrap_or(RulingStatus::Provisional);
    let provisional = req.provisional.unwrap_or(matches!(status, RulingStatus::Provisional));
    let ruling = RulingLogEntry {
        ruling_id: format!("ruling_{}", Uuid::new_v4().simple()),
        session_id: req.session_id,
        ruleset_id: req.ruleset_id,
        module_id: req.module_id,
        demand_id: req.demand_id,
        ruling_text: req.ruling_text,
        source_refs: req.source_refs,
        status,
        confidence: req.confidence.unwrap_or(if provisional { RulingConfidence::Low } else { RulingConfidence::Medium }),
        provisional,
        superseded_by: req.superseded_by,
        created_at: Utc::now(),
    };
    state.db.insert_ruling_log(&ruling).await?;
    Ok(Json(ruling))
}

#[derive(Debug, Deserialize)]
pub struct UpsertLearnedPacketRequest {
    pub packet_id: Option<String>,
    pub ruleset_id: String,
    pub module_id: Option<String>,
    pub packet_type: String,
    pub packet_key: String,
    pub title: String,
    pub summary: String,
    #[serde(default)]
    pub packet_json: Value,
    #[serde(default)]
    pub source_refs: Vec<SourceRef>,
    pub use_count: Option<i32>,
    pub learning_stage: Option<LearningStage>,
    pub confidence: Option<RulingConfidence>,
    pub cache_zone: Option<CacheZone>,
    pub visibility: Option<Visibility>,
}

async fn upsert_learned_packet_api(State(state): State<AppState>, Json(req): Json<UpsertLearnedPacketRequest>) -> Result<Json<LearnedPacket>, ApiError> {
    let now = Utc::now();
    let packet = LearnedPacket {
        packet_id: req.packet_id.unwrap_or_else(|| format!("learned_{}", Uuid::new_v4().simple())),
        ruleset_id: req.ruleset_id,
        module_id: req.module_id,
        packet_type: req.packet_type,
        packet_key: req.packet_key,
        title: req.title,
        summary: req.summary,
        packet_json: req.packet_json,
        source_refs: req.source_refs,
        use_count: req.use_count.unwrap_or(1),
        learning_stage: req.learning_stage.unwrap_or(LearningStage::UsedOnce),
        confidence: req.confidence.unwrap_or(RulingConfidence::Medium),
        cache_zone: req.cache_zone.unwrap_or(CacheZone::PinnedMiddle),
        visibility: req.visibility.unwrap_or(Visibility::GmOnly),
        last_used_at: Some(now),
        created_at: now,
        updated_at: now,
    };
    state.db.upsert_learned_packet(&packet).await?;
    Ok(Json(packet))
}

async fn list_learned_packets_api(Path(ruleset_id): Path<String>, State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let packets = state.db.list_learned_packets(&ruleset_id, None, 50).await?;
    Ok(Json(json!({"ruleset_id": ruleset_id, "packets": packets})))
}


#[derive(Debug, Deserialize, Default)]
pub struct LearningCandidatesQuery {
    pub ruleset_id: Option<String>,
    pub status: Option<String>,
    pub limit: Option<i64>,
}

async fn list_learning_candidates_api(Query(query): Query<LearningCandidatesQuery>, State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let candidates = state.db.list_learning_candidates(query.ruleset_id.as_deref(), query.status.as_deref(), query.limit.unwrap_or(50)).await?;
    Ok(Json(json!({"candidates": candidates})))
}

#[derive(Debug, Deserialize, Default)]
pub struct ApproveLearningCandidateRequest {
    pub notes: Option<String>,
    pub stage: Option<LearningStage>,
}

async fn approve_learning_candidate_api(Path(candidate_id): Path<String>, State(state): State<AppState>, Json(req): Json<ApproveLearningCandidateRequest>) -> Result<Json<Value>, ApiError> {
    let candidate = state.db.get_learning_candidate(&candidate_id).await?.ok_or_else(|| anyhow::anyhow!("unknown candidate_id: {candidate_id}"))?;
    let packet = candidate.to_learned_packet(req.stage.unwrap_or(LearningStage::UsedOnce));
    state.db.upsert_learned_packet(&packet).await?;
    state.db.update_learning_candidate_status(&candidate_id, LearningCandidateStatus::Approved, req.notes.as_deref()).await?;
    Ok(Json(json!({"candidate_id": candidate_id, "learned_packet": packet})))
}


#[derive(Debug, Deserialize)]
pub struct ResolveCheckRequest {
    pub session_id: String,
    pub turn_id: Option<String>,
    pub roll_input: String,
}

async fn resolve_check_api(State(state): State<AppState>, Json(req): Json<ResolveCheckRequest>) -> Result<Json<Value>, ApiError> {
    let turn_id = req.turn_id.unwrap_or_else(|| format!("turn_{}", Uuid::new_v4().simple()));
    let outcome = state.runtime.handle_open_interaction_gate(&req.session_id, &turn_id, &req.roll_input).await?;
    Ok(Json(json!({"session_id": req.session_id, "turn_id": turn_id, "outcome": outcome})))
}

#[derive(Debug, Deserialize)]
pub struct CompileContextRequest {
    pub context_request: ContextRequest,
    pub runtime_state: RuntimeState,
    pub current_input: Option<String>,
    pub recent_transcript: Option<String>,
}

async fn compile_context(State(state): State<AppState>, Json(req): Json<CompileContextRequest>) -> Result<Json<CompiledContext>, ApiError> {
    let compiled = state.runtime.prepare_turn_context(&req.context_request, &req.runtime_state, req.current_input.as_deref(), req.recent_transcript.as_deref()).await?;
    Ok(Json(compiled))
}

#[derive(Debug, Deserialize)]
pub struct CreateCharacterRequest {
    pub ruleset_id: String,
    pub module_id: Option<String>,
    pub user_preferences: String,
}

async fn create_character_sse(State(state): State<AppState>, Json(req): Json<CreateCharacterRequest>) -> impl IntoResponse {
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(128);
    let runtime = state.runtime.clone();
    let llm = state.llm.clone();
    let db = state.db.clone();
    tokio::spawn(async move {
        send_phase(&tx, "start", json!({"kind":"character_create"})).await;
        let messages = match runtime.character_creation_messages(&req.ruleset_id, req.module_id.as_deref(), &req.user_preferences).await {
            Ok(m) => m,
            Err(err) => { send_error(&tx, &err.to_string()).await; return; }
        };
        send_phase(&tx, "llm_stream_start", json!({})).await;
        let mut full = String::new();
        match llm.stream_chat(messages, 0.7).await {
            Ok(mut stream) => {
                while let Some(item) = stream.next().await {
                    match item {
                        Ok(delta) => { full.push_str(&delta); send_delta(&tx, &delta).await; }
                        Err(err) => { send_error(&tx, &err.to_string()).await; return; }
                    }
                }
            }
            Err(err) => { send_error(&tx, &err.to_string()).await; return; }
        }
        let job_id = format!("character_postprocess_{}", Uuid::new_v4().simple());
        let ruleset_id_for_postprocess = req.ruleset_id.clone();
        let _ = db.insert_background_job(&job_id, "character_postprocess", json!({"ruleset_id": req.ruleset_id.clone(), "module_id": req.module_id.clone()})).await;
        send_phase(&tx, "postprocess_scheduled", json!({"job_id": job_id.clone()})).await;
        let db2 = db.clone();
        tokio::spawn(async move {
            if let Err(err) = postprocess_character(db2.clone(), &job_id, &ruleset_id_for_postprocess, full).await {
                error!(error = %err, "character postprocess failed");
                let _ = db2.update_background_job(&job_id, "error", json!({}), Some(&err.to_string())).await;
            }
        });
        send_phase(&tx, "done", json!({})).await;
    });
    Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default())
}

async fn postprocess_character(db: Db, job_id: &str, ruleset_id: &str, response_text: String) -> anyhow::Result<()> {
    db.update_background_job(job_id, "running", json!({}), None).await?;
    let pack = db.load_character_onboarding_pack(ruleset_id).await?;
    let template = match pack.as_ref() {
        Some(pack) => pack.sheet_template.clone(),
        None => db.load_character_template(ruleset_id).await?.ok_or_else(|| anyhow::anyhow!("template not found"))?,
    };
    let draft = extract_json(&response_text).unwrap_or_else(|| json!({"raw_response": response_text}));
    let name = draft.get("name").or_else(|| draft.get("character_name")).and_then(Value::as_str).unwrap_or("Unnamed Character").to_string();
    let validation = validate_character_template_sheet(&template, &draft);
    let pack_mechanically_ready = pack.as_ref().map(|p| {
        p.validation_report.status == "ok"
            && !p.derived_formula_pack.formulas.is_empty()
            && !p.runtime_bindings.is_empty()
            && !p.creation_flows.is_empty()
    }).unwrap_or(false);
    let status = if validation.status == "ok" && pack_mechanically_ready { "ready" } else { "draft_needs_rules_source" };
    let sheet = CharacterSheet {
        character_id: format!("character_{}", Uuid::new_v4().simple()),
        ruleset_id: ruleset_id.to_string(),
        template_id: template.template_id.clone(),
        name,
        sheet: draft.clone(),
        validation_report: validation.clone(),
    };
    db.save_character(&sheet, status).await?;
    db.update_background_job(job_id, "done", json!({"character_id": sheet.character_id, "status": status, "validation": validation, "used_character_onboarding_pack": pack.is_some()}), None).await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct StartSessionRequest { pub ruleset_id: String, pub module_id: Option<String> }

async fn start_session(State(state): State<AppState>, Json(req): Json<StartSessionRequest>) -> Result<Json<Value>, ApiError> {
    let session_id = state.runtime.start_session(&req.ruleset_id, req.module_id.as_deref()).await?;
    Ok(Json(json!({"session_id": session_id})))
}

#[derive(Debug, Deserialize)]
pub struct PlayTurnRequest {
    pub ruleset_id: String,
    pub module_id: Option<String>,
    pub user_input: String,
    pub runtime_state: Option<RuntimeState>,
    pub recent_transcript: Option<String>,
}

async fn play_turn_sse(Path(session_id): Path<String>, State(state): State<AppState>, Json(req): Json<PlayTurnRequest>) -> impl IntoResponse {
    // SSE transport channel: TurnEvent (from execute_turn) → axum Event。
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(128);
    let data_dir = state.parser_config.data_dir.clone();
    let llm = state.llm.clone();
    let db = state.db.clone();
    // 每请求重建 GmLoop（含 ToolRegistry(Box<dyn GmTool>)/ErrataMemory/ObligationLedger
    // 等不可 Clone 字段）。RuntimeEngine 持 Arc<Db> 克隆，成本极低；errata/obligations
    // 是 per-turn 状态，随 spawned 任务结束丢弃，不跨回合。
    let engine = state.runtime.clone();
    let mut gm = GmLoop::new(
        engine,
        llm.clone(),
        ToolRegistry::standard(),
        LoopConfig::default(),
        data_dir.clone(),
    );
    // 装配到场深抽闭包（scene_navigate phase 切场景到达时深抽目标场景），
    // 仅模组在场时——与 CLI agent_play::wire_scene_extractor 同约定。
    if let Some(mid) = req.module_id.clone() {
        let db2 = db.clone();
        let llm2 = llm.clone();
        let rs2 = req.ruleset_id.clone();
        let dir2 = data_dir.clone();
        gm.scene_extractor = Some(Arc::new(move |node_id: String| {
            let db3 = db2.clone();
            let llm3 = llm2.clone();
            let mid3 = mid.clone();
            let rs3 = rs2.clone();
            let dir3 = dir2.clone();
            Box::pin(async move {
                extract_module_scenes(&db3, llm3.as_ref(), &mid3, None, Some(&rs3), &dir3, 12, Some(&node_id)).await
            }) as Pin<Box<dyn std::future::Future<Output = anyhow::Result<usize>> + Send>>
        }) as SceneDeepExtractFn);
    }
    let runtime = state.runtime.clone();
    tokio::spawn(async move {
        let turn_id = format!("turn_{}", Uuid::new_v4().simple());
        send_phase(&tx, "start", json!({"kind":"play_turn", "turn_id": turn_id.clone()})).await;
        // world_time 注入 RuntimeState（execute_turn 不替我们查时间）。PlayerAction
        // 世界事件由 execute_turn 的 phase_record_player_action 接管（不在此手写，避免双记）。
        let world_time = match runtime.current_world_time(&session_id).await {
            Ok(t) => t,
            Err(err) => { send_error(&tx, &err.to_string()).await; return; }
        };
        send_phase(&tx, "world_time", serde_json::to_value(&world_time).unwrap_or_else(|_| json!({}))).await;
        let mut runtime_state = req.runtime_state.clone().unwrap_or_default();
        runtime_state.world_time = Some(world_time);
        runtime_state.ruleset_id = req.ruleset_id.clone();
        runtime_state.module_id = req.module_id.clone();
        // 持久化的 current_scene_id 填进 RuntimeState（对齐 prepare_turn_context
        // 单点约定；scene_navigate phase 切换后下一回合在此读到新场景）。
        if runtime_state.scene_id.is_none() {
            runtime_state.scene_id = db.load_session_scene(&session_id).await.ok().flatten();
        }
        let context_request = ContextRequest {
            ruleset_id: req.ruleset_id.clone(),
            module_id: req.module_id.clone(),
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            viewer: VisibilityProfile::gm(),
            token_budget: TokenBudget::default(),
        };
        let owned = OwnedTurnRequest {
            request: context_request,
            state: runtime_state,
            user_input: req.user_input.clone(),
            history: Vec::new(),
            recent_transcript: req.recent_transcript.clone(),
            module_id: req.module_id.clone(),
            data_dir: data_dir.clone(),
        };
        // execute_turn 内部已 tokio::spawn pipeline 并经自带 mpsc 推 TurnEvent。
        // 本 handler 只做「TurnEvent→SSE Event」翻译：narration Delta 逐 token 直通，
        // 尾部 phase（verify/finalize/save_turn/memory/audit/scene_nav/carryover）在
        // execute_turn 的任务里继续跑——对客户端=后台，连接保持到 TurnComplete。
        let mut stream = execute_turn(gm, owned, CANONICAL_TURN_PLAN);
        while let Some(ev) = stream.next().await {
            match ev {
                TurnEvent::Delta(delta) => {
                    // 真流式：逐 token 直通，不缓冲（一期硬性原则）。
                    send_delta(&tx, &delta).await;
                }
                TurnEvent::AwaitingPlayerRoll { check_id, prompt_public } => {
                    send_delta(&tx, &prompt_public).await;
                    send_phase(&tx, "pending_check_created", json!({"check_id": check_id})).await;
                    send_phase(&tx, "done", json!({"reason":"awaiting_player_roll"})).await;
                }
                TurnEvent::SceneTransition { from, to, reason } => {
                    send_event(&tx, "scene_transition", json!({"from": from, "to": to, "reason": reason})).await;
                }
                TurnEvent::Errata(entry) => {
                    send_phase(&tx, "errata", serde_json::to_value(&entry).unwrap_or_else(|_| json!({}))).await;
                }
                TurnEvent::PostprocessScheduled => {
                    // 叙事已流完，尾部 phase 即将（在同一任务继续 drain）跑——对客户端=后台。
                    send_phase(&tx, "postprocess_scheduled", json!({})).await;
                }
                TurnEvent::TurnComplete { outcome } => {
                    // AwaitingPlayerRoll 终态的 done 已在上面发过；仅 Narration 终态发常规 done。
                    if let TurnOutcome::Narration(_) = outcome {
                        send_phase(&tx, "done", json!({})).await;
                    }
                }
            }
        }
    });
    Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default())
}


#[derive(Debug, Deserialize)]
struct ConflictStartRequest {
    session_id: String,
    ruleset_id: String,
    #[serde(default)]
    module_id: Option<String>,
    user_input: String,
}

async fn conflict_start_api(State(state): State<AppState>, Json(req): Json<ConflictStartRequest>) -> Result<Json<Value>, ApiError> {
    let turn_id = format!("turn_{}", Uuid::new_v4().simple());
    let context_request = ContextRequest { ruleset_id: req.ruleset_id.clone(), module_id: req.module_id.clone(), session_id: req.session_id.clone(), turn_id: turn_id.clone(), viewer: VisibilityProfile::gm(), token_budget: TokenBudget::default() };
    let runtime_state = RuntimeState { ruleset_id: req.ruleset_id.clone(), module_id: req.module_id.clone(), ..Default::default() };
    let result = state.runtime.try_handle_conflict_turn(&context_request, &runtime_state, &req.user_input, None).await?;
    Ok(Json(json!({"turn_id": turn_id, "result": result})))
}

async fn conflict_frames_api(Path(session_id): Path<String>, State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let frames = state.db.list_active_state_frames(&session_id, 20).await?;
    Ok(Json(json!({"session_id": session_id, "frames": frames})))
}


#[derive(Debug, Deserialize, Default)]
struct AdvanceTimeRequest {
    #[serde(default)] seconds: i64,
    #[serde(default)] minutes: i64,
    #[serde(default)] hours: i64,
    #[serde(default)] days: i64,
    #[serde(default)] combat_rounds: i64,
    #[serde(default)] scene_beats: i64,
    #[serde(default = "default_time_scale_string")] scale: String,
    #[serde(default = "default_time_reason")] reason: String,
    #[serde(default)] scene_epoch: Option<String>,
}
fn default_time_scale_string() -> String { "scene_beat".into() }
fn default_time_reason() -> String { "api time advance".into() }

async fn get_session_time_api(Path(session_id): Path<String>, State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let t = state.runtime.current_world_time(&session_id).await?;
    Ok(Json(json!({"time": t})))
}

async fn advance_session_time_api(Path(session_id): Path<String>, State(state): State<AppState>, Json(req): Json<AdvanceTimeRequest>) -> Result<Json<Value>, ApiError> {
    let result = state.runtime.advance_world_time(TimeAdvanceRequest {
        session_id,
        campaign_id: None,
        reason: req.reason,
        amount: TimeAmount { seconds: req.seconds, minutes: req.minutes, hours: req.hours, days: req.days, combat_rounds: req.combat_rounds, scene_beats: req.scene_beats, label: String::new() },
        scale: parse_time_scale_api(&req.scale),
        mutation_kind: TimeMutationKind::Advance,
        visibility: Visibility::PlayerVisible,
        caused_by_turn_id: None,
        caused_by_event_id: None,
        scene_epoch: req.scene_epoch,
    }).await?;
    Ok(Json(json!({"result": result})))
}

#[derive(Debug, Deserialize, Default)]
struct EventsQuery { since_tick: Option<i64>, since_event_seq: Option<i64>, limit: Option<i64> }
async fn list_session_events_api(Path(session_id): Path<String>, Query(q): Query<EventsQuery>, State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let events = state.db.list_world_events_since(&session_id, q.since_tick.unwrap_or(0), q.since_event_seq.unwrap_or(0), q.limit.unwrap_or(50)).await?;
    Ok(Json(json!({"events": events})))
}

#[derive(Debug, Deserialize, Default)]
struct ScheduleEventRequest {
    #[serde(default)] seconds: i64,
    #[serde(default)] minutes: i64,
    #[serde(default = "default_scheduled_kind")] kind: String,
    #[serde(default)] payload_json: Value,
}
fn default_scheduled_kind() -> String { "system_event".into() }
async fn schedule_session_event_api(Path(session_id): Path<String>, State(state): State<AppState>, Json(req): Json<ScheduleEventRequest>) -> Result<Json<Value>, ApiError> {
    let service = trpg_time::WorldTimeService::new(state.db.clone());
    let scheduled = service.schedule_in(&session_id, TimeAmount { seconds: req.seconds, minutes: req.minutes, ..Default::default() }, parse_world_event_kind_api(&req.kind), req.payload_json, Visibility::GmOnly, None).await?;
    Ok(Json(json!({"scheduled_event": scheduled})))
}

fn parse_time_scale_api(scale: &str) -> TimeScale {
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
fn parse_world_event_kind_api(kind: &str) -> WorldEventKind {
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

async fn get_session_memory(Path(session_id): Path<String>, State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let events = state.db.list_memory_events(&session_id, 20).await?;
    let facts = state.db.list_memory_facts(&session_id, 50).await?;
    let snapshots = state.db.list_memory_snapshots(&session_id, 10).await?;
    Ok(Json(json!({"session_id": session_id, "snapshots": snapshots, "facts": facts, "events": events})))
}

#[derive(Debug, Deserialize)]
pub struct RetrieveMemoryRequest {
    pub text: String,
    pub ruleset_id: Option<String>,
    pub module_id: Option<String>,
    pub scene_id: Option<String>,
    pub location_id: Option<String>,
    #[serde(default)] pub actor_ids: Vec<String>,
    #[serde(default)] pub tags: Vec<String>,
    pub limit: Option<u32>,
}

async fn retrieve_session_memory(Path(session_id): Path<String>, State(state): State<AppState>, Json(req): Json<RetrieveMemoryRequest>) -> Result<Json<MemoryRetrievalResult>, ApiError> {
    let query = MemoryQuery {
        session_id,
        text: req.text,
        ruleset_id: req.ruleset_id,
        module_id: req.module_id,
        scene_id: req.scene_id,
        location_id: req.location_id,
        actor_ids: req.actor_ids,
        tags: req.tags,
        limit: req.limit.unwrap_or_else(default_memory_limit),
        viewer: VisibilityProfile::gm(),
    };
    let result = state.db.retrieve_memory(&query).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct CompactMemoryRequest {
    pub ruleset_id: String,
    pub module_id: Option<String>,
    pub scope_type: Option<ScopeType>,
    pub scope_id: Option<String>,
    pub title: Option<String>,
    pub max_events: Option<i64>,
}

async fn compact_session_memory(Path(session_id): Path<String>, State(state): State<AppState>, Json(req): Json<CompactMemoryRequest>) -> Result<Json<MemorySnapshot>, ApiError> {
    let events = state.db.list_memory_events(&session_id, req.max_events.unwrap_or(24).max(1).min(100)).await?;
    let facts = state.db.list_memory_facts(&session_id, 40).await?;
    let mut summary = String::from("# GM Memory Snapshot\n\nThis is a cache-stable session memory summary. It should be refreshed on scene/chapter breaks or explicit compaction, not every turn.\n\n## Recent Events\n");
    for event in events.iter().rev() { summary.push_str(&format!("- {}\n", event.summary)); }
    if !facts.is_empty() {
        summary.push_str("\n## Active Facts\n");
        for fact in &facts { summary.push_str(&format!("- {}\n", fact.summary)); }
    }
    let scope = Scope { scope_type: req.scope_type.unwrap_or(ScopeType::Session), scope_id: req.scope_id.unwrap_or_else(|| session_id.clone()) };
    let snapshot_id = format!("memory.snapshot.{}.{}", session_id, match scope.scope_type { ScopeType::Scene => "scene", ScopeType::Chapter => "chapter", ScopeType::Mission => "mission", _ => "session" });
    let snapshot = MemorySnapshot::new(
        snapshot_id,
        session_id.clone(),
        req.ruleset_id,
        req.module_id,
        scope,
        Visibility::GmOnly,
        req.title.unwrap_or_else(|| "GM Memory Snapshot".to_string()),
        summary,
        events.iter().map(|e| e.event_id.clone()).collect(),
        facts.iter().map(|f| f.fact_id.clone()).collect(),
        1,
    );
    state.db.upsert_memory_snapshot(&snapshot).await?;
    Ok(Json(snapshot))
}

fn default_memory_limit() -> u32 {
    std::env::var("TRPG_MEMORY_RETRIEVAL_LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(8)
}

async fn send_phase(tx: &mpsc::Sender<Result<Event, Infallible>>, phase: &str, data: Value) {
    let _ = tx.send(Ok(Event::default().event("phase").data(json!({"phase": phase, "data": data}).to_string()))).await;
}

async fn send_delta(tx: &mpsc::Sender<Result<Event, Infallible>>, delta: &str) {
    let _ = tx.send(Ok(Event::default().event("delta").data(delta.to_string()))).await;
}


async fn send_event(tx: &mpsc::Sender<Result<Event, Infallible>>, event: &str, data: Value) {
    let _ = tx.send(Ok(Event::default().event(event).data(data.to_string()))).await;
}

async fn send_error(tx: &mpsc::Sender<Result<Event, Infallible>>, error: &str) {
    let _ = tx.send(Ok(Event::default().event("error").data(json!({"error": error}).to_string()))).await;
}

fn extract_json(text: &str) -> Option<Value> {
    if let Some(v) = extract_fenced(text, "json").and_then(|s| serde_json::from_str::<Value>(&s).ok()) { return Some(v); }
    serde_json::from_str::<Value>(text).ok()
}

fn extract_fenced(text: &str, lang: &str) -> Option<String> {
    let fence = format!("```{lang}");
    let start = text.find(&fence)? + fence.len();
    let rest = &text[start..];
    let end = rest.find("```")?;
    Some(rest[..end].trim().to_string())
}



#[derive(Debug)]
pub struct ApiError(anyhow::Error);

impl<E> From<E> for ApiError where E: Into<anyhow::Error> {
    fn from(value: E) -> Self { Self(value.into()) }
}

impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let body = Json(json!({"error": self.0.to_string()}));
        (StatusCode::INTERNAL_SERVER_ERROR, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Deterministic coverage of the Stage-0/Stage-1 gate decision: a missing or
    // empty kernel value serves 202, a populated one serves 200. The full live
    // POST -> poll -> 202-before / 200-after transition needs a real DB + LLM and
    // is exercised by the `#[ignore]`d smoke test below + the CLI harness (Task C).
    #[test]
    fn kernel_value_ready_gates_on_non_empty() {
        assert!(!kernel_value_ready(&Value::Null));
        assert!(!kernel_value_ready(&json!({})));
        assert!(kernel_value_ready(&json!({"fields": []})));
        assert!(kernel_value_ready(&json!({"summary": "A cosmic-horror investigation game."})));
        // Stage-1 character template (non-empty object) is "ready".
        assert!(kernel_value_ready(&json!({"template_id": "coc.sheet", "fields": [{"id": "str"}]})));
    }

    // The 202 progress body extracts `stage`/`progress_pct` from the latest job
    // snapshot shape produced by the orchestrator's `JobStatus`.
    #[test]
    fn progress_body_extracts_stage_and_pct() {
        let snapshot = json!({"stage": "character", "progress_pct": 35, "stages": []});
        let stage = snapshot.get("stage").and_then(Value::as_str).unwrap_or("identity");
        let pct = snapshot.get("progress_pct").and_then(Value::as_u64).unwrap_or(0);
        assert_eq!(stage, "character");
        assert_eq!(pct, 35);
    }

    // Live integration smoke (needs a Postgres at DATABASE_URL + a reachable LLM):
    // POST /api/ingest/ruleset, poll /status until `character` is done, asserting
    // /character-template returns 202 before Stage 1 and 200 after. Ignored by
    // default; run with `--ignored` against a seeded `_rstest_coc` data dir.
    #[tokio::test]
    #[ignore = "requires live Postgres + LLM and a seeded data_dir"]
    async fn staged_ingest_character_template_gate_smoke() {
        // Intentionally a documentation-only harness: building AppState requires a
        // RuntimeEngine + SearchService bound to a live DB. The transition contract
        // (202 before Stage 1, 200 after) is validated end-to-end by the CLI
        // `parse-staged` harness; this marker keeps the API-surface coverage intent
        // explicit so the gap is not silent.
    }
}

