use anyhow::{Context, Result};
use chrono::Utc;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tracing::{info, warn};
use trpg_db::Db;
use trpg_ingest::{default_pdf_extractor, find_pdfs, render_oxidize_markdown, source_index_from_book, write_chunks_jsonl, ExtractionCleanMode, IngestConfig, PdfBackendKind, PdfMarkdownExtractor};
use trpg_llm::{system, user, LlmClient, LlmConfig, OpenAiCompatibleClient};
use trpg_model::*;
use trpg_rule_agent::reader;
use trpg_rule_agent::RuleStewardFirstPassAgent;

pub mod staged;
pub mod staged_status;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParserConfig {
    pub project_id: String,
    pub data_dir: PathBuf,
    pub force: bool,
    pub chunk_pages: usize,
    pub max_chunk_chars: usize,
    pub parse_full_chunks: bool,
    pub parse_config_hash: String,
    pub pdf_backend: PdfBackendKind,
    pub oxidize_clean_mode: ExtractionCleanMode,
    pub oxidize_chunk_target_chars: usize,
    /// v1.15.4: high-leverage pre-parser conditioning.  The old name is kept
    /// only for env compatibility; the pipeline now builds semantic units for
    /// retrieval/materialization instead of cosmetically reflowing fixed chunks.
    pub llm_clean_extraction: bool,
    pub llm_clean_max_chunks: usize,
    pub semantic_unit_conditioning: bool,
    pub semantic_unit_max_chars: usize,
    pub llm_semantic_wash: bool,
    pub llm_semantic_wash_max_units: usize,
}

fn env_bool_default(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(default)
}

fn rule_steward_first_pass_enabled() -> bool {
    env_bool_default("TRPG_RULE_STEWARD_FIRST_PASS", true)
}

impl ParserConfig {
    pub fn new(data_dir: impl Into<PathBuf>, force: bool) -> Self {
        let parse_full_chunks = std::env::var("TRPG_PARSE_FULL_CHUNKS").map(|v| v == "1" || v.eq_ignore_ascii_case("true")).unwrap_or(false);
        let pdf_backend = PdfBackendKind::from_env();
        let oxidize_clean_mode = ExtractionCleanMode::from_env();
        let oxidize_chunk_target_chars = std::env::var("TRPG_OXIDIZE_CHUNK_TARGET_CHARS").ok().and_then(|v| v.parse().ok()).unwrap_or(4_000);
        let llm_clean_extraction = std::env::var("TRPG_INGEST_LLM_CLEAN")
            .ok()
            .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);
        let llm_clean_max_chunks = std::env::var("TRPG_INGEST_LLM_CLEAN_MAX_CHUNKS").ok().and_then(|v| v.parse().ok()).unwrap_or(8);
        let semantic_unit_conditioning = std::env::var("TRPG_INGEST_SEMANTIC_UNITS")
            .ok()
            .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(true);
        let semantic_unit_max_chars = std::env::var("TRPG_SEMANTIC_UNIT_MAX_CHARS").ok().and_then(|v| v.parse().ok()).unwrap_or(6_000);
        let llm_semantic_wash = std::env::var("TRPG_INGEST_LLM_SEMANTIC_WASH")
            .ok()
            .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);
        let llm_semantic_wash_max_units = std::env::var("TRPG_INGEST_LLM_SEMANTIC_WASH_MAX_UNITS").ok().and_then(|v| v.parse().ok()).unwrap_or(12);
        let rule_steward_first_pass = rule_steward_first_pass_enabled();
        let data_dir: PathBuf = data_dir.into();
        // Distinct project id PER DATA DIR — multiple rulesets sharing one DB used
        // to all upsert "local_project" and overwrite each other's project bundle,
        // so a turn could only ever load the last-parsed ruleset's project.
        let project_id = format!(
            "project.{}",
            data_dir.file_name().and_then(|s| s.to_str()).map(sanitize_id).unwrap_or_else(|| "local".to_string())
        );
        Self {
            project_id,
            data_dir,
            force,
            chunk_pages: 10,
            max_chunk_chars: 18_000,
            parse_full_chunks,
            parse_config_hash: sha256_hex(format!("parser=v1.16.2;schema=v1;prompt=rulebook_reader_agent_v1;chargen_compiler=v11_categorical_keyed;full_chunks={parse_full_chunks};pdf_backend={};clean_mode={};oxidize_chunk_target_chars={oxidize_chunk_target_chars};llm_clean={llm_clean_extraction};semantic_units={semantic_unit_conditioning};semantic_unit_max_chars={semantic_unit_max_chars};llm_semantic_wash={llm_semantic_wash};rule_steward_first_pass={rule_steward_first_pass}", pdf_backend.as_str(), oxidize_clean_mode.as_str())),
            pdf_backend,
            oxidize_clean_mode,
            oxidize_chunk_target_chars,
            llm_clean_extraction,
            llm_clean_max_chunks,
            semantic_unit_conditioning,
            semantic_unit_max_chars,
            llm_semantic_wash,
            llm_semantic_wash_max_units,
        }
    }
}

pub struct ProjectParseService {
    pub db: Db,
    pub llm: Arc<dyn LlmClient>,
    pub extractor: Arc<dyn PdfMarkdownExtractor>,
    pub config: ParserConfig,
}

impl ProjectParseService {
    pub fn new(db: Db, llm: Arc<dyn LlmClient>, config: ParserConfig) -> Self {
        Self { db, llm, extractor: default_pdf_extractor(), config }
    }

    pub async fn parse_all(&self) -> Result<ProjectBundle> {
        fs::create_dir_all(self.config.data_dir.join("parsed")).await?;
        let mut ingest_config = IngestConfig::new(self.config.data_dir.clone(), self.config.parse_config_hash.clone());
        ingest_config.force_markdown = self.config.force;
        ingest_config.pdf_backend = self.config.pdf_backend;
        ingest_config.clean_mode = self.config.oxidize_clean_mode;
        ingest_config.chunk_target_chars = self.config.oxidize_chunk_target_chars;

        let mut project = ProjectBundle::empty(&self.config.project_id);
        project.conversion_trace.push(ConversionTraceEvent::new("onboard_and_index_started", format!("Scanning rulebook and module folders with pdf_backend={} clean_mode={} ; building GM onboarding, locators, and first-session packets.", self.config.pdf_backend.as_str(), self.config.oxidize_clean_mode.as_str())));

        for pdf in find_pdfs(&self.config.data_dir, SourceKind::Rulebook) {
            info!(path = %pdf.display(), "ingesting rulebook");
            let extracted = self.extractor.extract(&pdf, SourceKind::Rulebook, &ingest_config).await?;
            let book = self.maybe_llm_clean_extraction(&extracted.book).await?;
            self.db.upsert_source_document(&extracted.source_document).await?;
            project.source_documents.push(SourceDocumentRef {
                source_id: extracted.source_document.source_id.clone(),
                title: extracted.source_document.title.clone(),
                source_kind: SourceKind::Rulebook,
                source_hash: extracted.source_document.source_hash.clone(),
                file_path: Some(extracted.source_document.file_path.clone()),
            });

            let bundle = if !self.config.force {
                match self.db.load_rule_bundle_by_source(&extracted.source_document.source_hash, &self.config.parse_config_hash).await? {
                    Some(cached) => {
                        info!(bundle_id = %cached.bundle_id, "using cached rule bundle");
                        cached
                    }
                    None => self.parse_rulebook(&extracted.source_document, &book).await?,
                }
            } else {
                self.parse_rulebook(&extracted.source_document, &book).await?
            };
            self.db.upsert_rule_bundle(&bundle, None, &extracted.source_document.source_hash, &self.config.parse_config_hash).await?;
            project.rulesets.push(bundle);
        }

        // Ruleset(s) parsed in this project (rulebook loop ran above) — used to
        // bind modules whose own text doesn't name the system (§module metadata fix).
        let project_ruleset_ids: Vec<String> = project.rulesets.iter().map(|r| r.ruleset_id.clone()).collect();
        for pdf in find_pdfs(&self.config.data_dir, SourceKind::Module) {
            info!(path = %pdf.display(), "ingesting module");
            let extracted = self.extractor.extract(&pdf, SourceKind::Module, &ingest_config).await?;
            let book = self.maybe_llm_clean_extraction(&extracted.book).await?;
            self.db.upsert_source_document(&extracted.source_document).await?;
            project.source_documents.push(SourceDocumentRef {
                source_id: extracted.source_document.source_id.clone(),
                title: extracted.source_document.title.clone(),
                source_kind: SourceKind::Module,
                source_hash: extracted.source_document.source_hash.clone(),
                file_path: Some(extracted.source_document.file_path.clone()),
            });

            let bundle = if !self.config.force {
                match self.db.load_module_bundle_by_source(&extracted.source_document.source_hash, &self.config.parse_config_hash).await? {
                    Some(cached) if cached_module_bundle_reusable(&cached.module_graph, module_reader_enabled()) => {
                        info!(bundle_id = %cached.bundle_id, "using cached module bundle");
                        cached
                    }
                    Some(cached) => {
                        info!(bundle_id = %cached.bundle_id, "cached module bundle has an empty scene graph (pre-reader artifact); re-extracting with the module reader");
                        self.parse_module(&extracted.source_document, &book, &project_ruleset_ids).await?
                    }
                    None => self.parse_module(&extracted.source_document, &book, &project_ruleset_ids).await?,
                }
            } else {
                self.parse_module(&extracted.source_document, &book, &project_ruleset_ids).await?
            };
            self.db.upsert_module_bundle(&bundle, None, &extracted.source_document.source_hash, &self.config.parse_config_hash).await?;
            project.modules.push(bundle);
        }

        project.generated_at = Utc::now();
        project.conversion_trace.push(ConversionTraceEvent::new("onboard_and_index_finished", "Project onboarding bundle assembled. Cold data remains locator-backed and on-demand."));
        let json_path = self.config.data_dir.join("parsed/project.bundle.json");
        let json_text = serde_json::to_string_pretty(&project)?;
        fs::write(&json_path, json_text).await?;
        self.export_project_jsonl(&project).await?;
        let project_hash = stable_json_hash(&project);
        self.db.upsert_project_bundle(&project, Some(&json_path.to_string_lossy()), &project_hash, &self.config.parse_config_hash).await?;
        Ok(project)
    }

    async fn export_project_jsonl(&self, project: &ProjectBundle) -> Result<()> {
        let parsed_dir = self.config.data_dir.join("parsed");
        fs::create_dir_all(&parsed_dir).await?;

        let mut blocks = String::new();
        let mut material = String::new();

        for ruleset in &project.rulesets {
            for block in &ruleset.context_blocks {
                blocks.push_str(&serde_json::to_string(block)?);
                blocks.push('\n');
            }
            for entry in &ruleset.material_index {
                material.push_str(&serde_json::to_string(entry)?);
                material.push('\n');
            }
        }
        for module in &project.modules {
            for block in &module.context_blocks {
                blocks.push_str(&serde_json::to_string(block)?);
                blocks.push('\n');
            }
            for entry in &module.material_index {
                material.push_str(&serde_json::to_string(entry)?);
                material.push('\n');
            }
        }

        fs::write(parsed_dir.join("context_blocks.jsonl"), blocks).await?;
        fs::write(parsed_dir.join("material_index.jsonl"), material).await?;
        Ok(())
    }

    async fn maybe_llm_clean_extraction(&self, book: &PlainTextBook) -> Result<PlainTextBook> {
        // v1.15.4: this is no longer a cosmetic layout-cleaning pass.  The
        // expensive part of ingestion now conditions the source for retrieval
        // and materialization: structural repair only for genuinely broken text,
        // then deterministic/optional-LLM semantic units with category/entity/
        // secrecy/mechanics tags.  The original function name is retained for
        // compatibility with older callers and env vars.
        let mut conditioned = book.clone();

        if self.config.llm_clean_extraction && !conditioned.chunks.is_empty() {
            let mut repaired_count = 0usize;
            for chunk in conditioned.chunks.iter_mut() {
                if repaired_count >= self.config.llm_clean_max_chunks { break; }
                if !chunk_needs_structural_llm_repair(chunk) { continue; }
                let prompt = format!(
                    "Repair only broken extraction artifacts in this TRPG PDF chunk. Do not summarize, improve prose, or reflow cosmetic line breaks. Only fix: (1) encoding/mojibake/invalid characters, (2) table columns collapsed into unreadable rows. Preserve every rule number, NPC/stat value, clue, label, and GM-only marker. Return JSON only with keys: text, heading_context, element_types, quality_flags.\n\nsource pages: {:?}\ncurrent heading_context: {:?}\nraw chunk:\n{}",
                    chunk.page_numbers, chunk.heading_context, chunk.text
                );
                match self.llm.complete_json(vec![system("You repair structurally broken TRPG source extraction. Cosmetic formatting is out of scope; semantic boundaries are handled later."), user(prompt)], 0.0).await {
                    Ok(value) => {
                        if let Some(text) = value.get("text").and_then(Value::as_str) {
                            chunk.text = text.trim().to_string();
                            chunk.full_text = chunk.text.clone();
                            chunk.text_hash = Some(sha256_hex(&chunk.text));
                            chunk.clean_status = Some("llm_structural_repair".to_string());
                        }
                        if let Some(arr) = value.get("heading_context").and_then(Value::as_array) {
                            chunk.heading_context = arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
                        }
                        if let Some(arr) = value.get("element_types").and_then(Value::as_array) {
                            chunk.element_types = arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
                        }
                        if let Some(flags) = value.get("quality_flags") {
                            let mut meta = conditioned_chunk_metadata(chunk);
                            meta["quality_flags"] = flags.clone();
                            chunk.metadata = meta;
                        }
                        repaired_count += 1;
                    }
                    Err(err) => warn!(chunk_id = %chunk.chunk_id, error = %err, "LLM structural repair failed; retaining source chunk"),
                }
            }
            if repaired_count > 0 {
                conditioned = self.write_conditioned_chunks(&conditioned, "structural_repaired").await?;
            }
        }

        if self.config.semantic_unit_conditioning {
            let source_kind = infer_source_kind_for_book(book);
            let mut semantic_units = semantic_units_from_book(&conditioned, source_kind, self.config.semantic_unit_max_chars);
            if self.config.llm_semantic_wash {
                semantic_units = self.maybe_llm_refine_semantic_units(&conditioned, semantic_units).await?;
            }
            let signal_count = semantic_units.iter().filter(|u| semantic_signal_class(u) == "signal").count();
            let noise_count = semantic_units.len().saturating_sub(signal_count);
            self.write_semantic_units(&conditioned, &semantic_units, signal_count, noise_count).await?;
            if signal_count > 0 {
                conditioned.chunks = semantic_units.into_iter().filter(|u| semantic_signal_class(u) == "signal").collect();
            }
        }
        Ok(conditioned)
    }

    async fn write_conditioned_chunks(&self, book: &PlainTextBook, suffix: &str) -> Result<PlainTextBook> {
        let chunk_dir = self.config.data_dir.join("parsed/source_chunks");
        fs::create_dir_all(&chunk_dir).await?;
        let chunk_path = chunk_dir.join(format!("{}.{}.jsonl", book.source_id, suffix));
        write_chunks_jsonl(&chunk_path, &book.chunks).await?;
        let markdown_dir = self.config.data_dir.join("markdown/conditioned");
        fs::create_dir_all(&markdown_dir).await?;
        let markdown_path = markdown_dir.join(format!("{}.{}.md", book.source_id, suffix));
        let markdown = render_oxidize_markdown(&book.source_id, &book.title, &book.source_hash, &book.chunks, None, None, self.config.pdf_backend.as_str());
        fs::write(&markdown_path, markdown).await?;
        Ok(book.clone())
    }

    async fn write_semantic_units(&self, book: &PlainTextBook, units: &[DocumentChunk], signal_count: usize, noise_count: usize) -> Result<()> {
        let unit_dir = self.config.data_dir.join("parsed/source_units");
        fs::create_dir_all(&unit_dir).await?;
        let unit_path = unit_dir.join(format!("{}.semantic_units.jsonl", book.source_id));
        let mut lines = String::new();
        for unit in units {
            let category = semantic_category(unit);
            let title = defined_entity(unit).unwrap_or_else(|| unit.heading_context.last().cloned().unwrap_or_else(|| unit.chunk_id.clone()));
            let source_refs = source_refs_for_semantic_chunk(unit);
            let mut tags = vec!["semantic_unit".to_string(), category.clone(), semantic_signal_class(unit)];
            if let Some(arr) = unit.metadata.get("mechanics_tags").and_then(Value::as_array) {
                tags.extend(arr.iter().filter_map(Value::as_str).map(str::to_string));
            }
            if unit.metadata.get("gm_secret").and_then(Value::as_bool).unwrap_or(false) { tags.push("gm_secret".into()); }
            tags.sort(); tags.dedup();
            let record = json!({
                "schema_version": "chatrpg.semantic_source_unit.v1",
                "kind": "semantic_unit",
                "unit_id": unit.chunk_id,
                "title": title,
                "category": category,
                "signal_class": semantic_signal_class(unit),
                "content_text": unit.text,
                "heading_context": unit.heading_context,
                "page_numbers": unit.page_numbers,
                "tags": tags,
                "source_refs": source_refs,
                "metadata": unit.metadata,
                "text_hash": unit.text_hash,
            });
            lines.push_str(&serde_json::to_string(&record)?);
            lines.push('\n');
        }
        fs::write(&unit_path, lines).await?;
        let report = json!({
            "schema_version": "chatrpg.semantic_source_units.v1",
            "source_id": book.source_id,
            "source_hash": book.source_hash,
            "title": book.title,
            "total_units": units.len(),
            "signal_units": signal_count,
            "noise_units": noise_count,
            "policy": "semantic units feed retrieval/materialization; noise is retained for audit but dropped from parser chunks; cosmetic line-break reflow is not a goal"
        });
        fs::write(unit_dir.join(format!("{}.semantic_unit_report.json", book.source_id)), serde_json::to_string_pretty(&report)?).await?;
        let markdown_dir = self.config.data_dir.join("markdown/semantic_units");
        fs::create_dir_all(&markdown_dir).await?;
        let markdown_path = markdown_dir.join(format!("{}.semantic_units.md", book.source_id));
        fs::write(markdown_path, render_oxidize_markdown(&book.source_id, &book.title, &book.source_hash, units, None, None, self.config.pdf_backend.as_str())).await?;
        Ok(())
    }

    async fn maybe_llm_refine_semantic_units(&self, book: &PlainTextBook, units: Vec<DocumentChunk>) -> Result<Vec<DocumentChunk>> {
        let mut refined = Vec::with_capacity(units.len());
        let mut used = 0usize;
        for unit in units {
            if used >= self.config.llm_semantic_wash_max_units || semantic_signal_class(&unit) == "noise" || !unit_needs_llm_semantic_wash(&unit) {
                refined.push(unit);
                continue;
            }
            let body = if unit.full_text.trim().is_empty() { &unit.text } else { &unit.full_text };
            let prompt = format!(
                "Classify and, only if necessary, split this TRPG source unit for downstream retrieval/materialization. The goal is one complete independent semantic unit per output item, not pretty formatting. Preserve exact numbers and names. Mark noise. Mark GM-only/secret content. Return JSON only: {{semantic_units:[{{title, text, category, signal_class, defined_entity, gm_secret, mechanics_tags, visibility, split_reason}}]}}.\n\nsource_id: {}\npage_numbers: {:?}\ncurrent_heading_context: {:?}\ncurrent_metadata: {}\ntext:\n{}",
                book.source_id,
                unit.page_numbers,
                unit.heading_context,
                serde_json::to_string(&unit.metadata).unwrap_or_default(),
                body
            );
            match self.llm.complete_json(vec![system("You prepare TRPG semantic source units for exact retrieval, source-backed materialization, and spoiler redaction. Do not invent data."), user(prompt)], 0.0).await {
                Ok(value) => {
                    let mut out = semantic_units_from_llm_value(&unit, &value);
                    if out.is_empty() { refined.push(unit); } else { refined.append(&mut out); }
                    used += 1;
                }
                Err(err) => {
                    warn!(unit_id = %unit.chunk_id, error = %err, "LLM semantic unit wash failed; retaining deterministic unit");
                    refined.push(unit);
                }
            }
        }
        Ok(refined)
    }

    async fn parse_rulebook(&self, doc: &SourceDocument, book: &PlainTextBook) -> Result<RuleBundle> {
        let ruleset_id = infer_ruleset_id(&doc.title);
        let bundle_id = format!("rulebook.{ruleset_id}.{}", doc.source_id);
        let source_index = source_index_from_book(doc, book);
        let document_type = classify_rulebook(&doc.title, book);
        let mut context_blocks = Vec::new();
        let mut material_index = Vec::new();
        let mut conversion_trace = vec![ConversionTraceEvent::new("rulebook_parse_started", format!("Parsing {}", doc.title))];

        let procedures = infer_procedures(&ruleset_id, &doc.title, book);
        let procedure_registry = ProcedureRegistry { procedures: procedures.clone(), ..Default::default() };

        // The rulebook-reader agent (below) replaces the old first-pass for the
        // kernel + derived formulas, so skip the (expensive, formula-less) old
        // first-pass when the reader is enabled to avoid double LLM work.
        let first_pass = if rule_steward_first_pass_enabled() && !reader_agent_enabled() {
            let steward = RuleStewardFirstPassAgent::new(self.db.clone(), self.llm.clone(), self.config.data_dir.clone());
            match steward.run_rulebook_first_pass(doc, book, &ruleset_id, &procedures).await {
                Ok(first_pass) => {
                    conversion_trace.push(ConversionTraceEvent::new(
                        "rule_steward_first_pass_used",
                        format!("Rulebook first pass routed through Rule Steward skills: {}", first_pass.skill_sequence.join(" -> ")),
                    ));
                    Some(first_pass)
                }
                Err(err) => {
                    warn!(error = %err, "Rule Steward first-pass failed; falling back to legacy parser extraction path");
                    conversion_trace.push(ConversionTraceEvent::new(
                        "rule_steward_first_pass_failed",
                        format!("Rule Steward first-pass failed and parser fallback was used: {err}"),
                    ));
                    None
                }
            }
        } else {
            conversion_trace.push(ConversionTraceEvent::new(
                "rule_steward_first_pass_disabled",
                "TRPG_RULE_STEWARD_FIRST_PASS=false; using legacy parser extraction path.",
            ));
            None
        };

        let book_map = first_pass.as_ref().map(|fp| fp.book_map.clone()).unwrap_or_else(|| build_book_map(book));
        // When the reader is on, the real identity/style/director-policy live in
        // the RuleKernel (BP1, built below from the reader). Skip the expensive
        // whole-book overview extract; these resident blocks fall back to generic
        // defaults (the kernel carries the source-backed content).
        let resident_json = if reader_agent_enabled() {
            json!({})
        } else {
            match first_pass.as_ref().and_then(|fp| fp.resident_json.clone()) {
                Some(value) => value,
                None => self.extract_rulebook_overview(&ruleset_id, &doc.title, &book_map, book).await.unwrap_or_else(|err| {
                    warn!(error = %err, "rulebook overview extraction failed; using fallback");
                    json!({})
                }),
            }
        };

        let resident_core = resident_json.get("resident_core_markdown").and_then(Value::as_str).unwrap_or("Core rules are indexed in the material catalog. Load exact procedures and assets as needed.");
        let world_style = resident_json.get("world_style_markdown").and_then(Value::as_str).unwrap_or("Use the tone, setting assumptions, terminology, and genre conventions extracted from the rulebook.");
        let director_policy = resident_json.get("director_policy_markdown").and_then(Value::as_str).unwrap_or("Run the game as referee, narrator, and state proposal generator. Never commit hidden or mechanical state without validator approval.");

        context_blocks.push(tagged_block(
            format!("ruleset.{ruleset_id}.resident_core"),
            BlockKind::RulesetResidentCore,
            "Resident Core",
            resident_core,
            Visibility::GmOnly,
            Stability::RarelyChanged,
            CacheZone::Prefix,
            Scope::ruleset(&ruleset_id),
            100,
            vec!["resident", "ruleset", "core"],
            first_source_ref(book),
        ));
        context_blocks.push(tagged_block(
            format!("ruleset.{ruleset_id}.world_style"),
            BlockKind::RulesetWorldStyle,
            "World Style",
            world_style,
            Visibility::GmOnly,
            Stability::RarelyChanged,
            CacheZone::Prefix,
            Scope::ruleset(&ruleset_id),
            90,
            vec!["resident", "style"],
            first_source_ref(book),
        ));
        context_blocks.push(tagged_block(
            format!("ruleset.{ruleset_id}.director_policy.core"),
            BlockKind::RulesetDirectorPolicy,
            "Core Director Policy",
            director_policy,
            Visibility::GmOnly,
            Stability::RarelyChanged,
            CacheZone::Prefix,
            Scope::ruleset(&ruleset_id),
            95,
            vec!["resident", "director"],
            first_source_ref(book),
        ));

        // Rulebook-reader agent (replaces the old first-pass AND the expensive
        // legacy character/onboarding/kernel extraction): one cheap pass reading
        // the book like a GM -> GmRunKit (BP1 kernel + structured core + COMPLETE
        // character template/flow/options). Falls back to legacy extraction on
        // failure / when TRPG_READER_AGENT is off.
        let reader_run_kit: Option<reader::GmRunKit> = if reader_agent_enabled() {
            let units_path = self.config.data_dir.join("parsed/source_units").join(format!("{}.semantic_units.jsonl", doc.source_id));
            match reader::load_units(&units_path) {
                Ok(units) if !units.is_empty() => {
                    let budget = std::env::var("TRPG_READER_BUDGET").ok().and_then(|v| v.parse().ok()).unwrap_or(9);
                    match reader::run_reader_parallel(self.llm.as_ref(), &units, &ruleset_id, budget).await {
                        Ok(res) => {
                            tracing::info!(tool_calls = res.tool_calls, llm_calls = res.llm_calls, tokens = res.prompt_tokens + res.completion_tokens, "rulebook reader produced GmRunKit");
                            Some(res.run_kit)
                        }
                        Err(err) => {
                            warn!(error = %err, "rulebook reader failed; using legacy extraction");
                            None
                        }
                    }
                }
                _ => None,
            }
        } else {
            None
        };
        let reader_char_template = reader_run_kit.as_ref().map(|rk| rk.character_template.clone()).filter(|t| {
            t.get("fields").and_then(|f| f.as_array()).map(|a| !a.is_empty()).unwrap_or(false)
        });

        let mut character_template = match reader_char_template {
            Some(t) => coerce_character_template(t, &ruleset_id, &doc.title),
            None => match first_pass.as_ref().and_then(|fp| fp.character_template_json.clone()) {
                Some(value) => coerce_character_template(value, &ruleset_id, &doc.title),
                None => self.extract_character_template(&ruleset_id, &doc.title, book).await.unwrap_or_else(|err| {
                    warn!(error = %err, "character template extraction failed; using fallback");
                    fallback_character_template(&ruleset_id, &doc.title)
                }),
            },
        };
        // §4 chargen compiler (focused rule-agent second pass): upgrade the
        // reader's PROSE derived_values to machine-evaluable form (expr + inlined
        // duotext tables + characteristic-derived skills), round-trip-validated.
        // Only when the reader produced a template; failure leaves prose intact.
        let mut object_schemas: Vec<serde_json::Value> = Vec::new();
        // The mechanics third pass (at the kernel production point below) REUSES
        // the units/sidecar/skills already loaded here — never re-reads disk.
        let mut mech_units: Vec<reader::Unit> = Vec::new();
        let mut mech_sidecar: Option<String> = None;
        let mut mech_skills: Vec<String> = Vec::new();
        if reader_run_kit.is_some() {
            let units_path = self.config.data_dir.join("parsed/source_units").join(format!("{}.semantic_units.jsonl", doc.source_id));
            if let Ok(units) = reader::load_units(&units_path) {
                // duotext2 ingest folds the column-aligned layout (table-heavy
                // pages keep -layout COLUMN ALIGNMENT) into the single merged
                // rulebook `.md`; it never writes `layout_sidecar_path`, so reading
                // that key always yielded None and the read_layout view silently
                // degraded — chargen compile lost the column-aligned table view
                // (lookup tables especially). Read the merged `.md` directly (same
                // single source the Tantivy index / param grep use; same pattern as
                // parse_module's `markdown/modules/{id}.md`). fail-closed: missing
                // file -> None.
                let sidecar_text = std::fs::read_to_string(
                    self.config.data_dir.join(format!("markdown/rulebooks/{}.md", doc.source_id))
                ).ok();
                let located_pages = character_template.source_refs.iter()
                    .filter_map(|r| r.page.map(|p| p.to_string()))
                    .collect::<Vec<_>>().join(",");
                let skill_names: Vec<String> = reader_run_kit.as_ref()
                    .and_then(|rk| rk.option_catalogs.as_array())
                    .map(|cats| cats.iter()
                        .filter(|c| c.get("category").and_then(|v| v.as_str()).map(|s| s.to_ascii_lowercase().contains("skill")).unwrap_or(false))
                        .flat_map(|c| c.get("options").and_then(|o| o.as_array()).cloned().unwrap_or_default())
                        .filter_map(|o| o.get("title").and_then(|v| v.as_str()).map(String::from))
                        .collect())
                    .unwrap_or_default();
                mech_sidecar = sidecar_text.clone();
                mech_skills = skill_names.clone();
                let ctx = reader::CompileCtx { units: &units, sidecar_text: sidecar_text.clone(), located_pages, skill_names: skill_names.clone() };
                // The chargen compile pass is LOW-VOLUME (once per ruleset) but needs
                // RELIABLE full-sheet extraction, so it runs on a stronger model
                // (TRPG_CHARGEN_COMPILER_MODEL, default gpt-5.4) while the bulk
                // reader/GM passes stay on mini. Falls back to the main client.
                let compiler_llm = build_compiler_llm().unwrap_or_else(|| self.llm.clone());
                let gaps = reader::compile_chargen_formulas(compiler_llm.as_ref(), &mut character_template, ctx, 14).await;
                if !gaps.is_empty() { tracing::info!(?gaps, "chargen compiler gaps (provisional/uncoerced)"); }

                // §obj object/ability SCHEMA compiler (discover -> extract): per
                // category a typed schema + 2 source-backed examples whose formula
                // slots HOOK into the engine (damage->dice, attack->skill,
                // ammo/cost->resource). Couples to the SAME skill field_ids +
                // resource-track ids the chargen pass uses, so item params share
                // the character's vocabulary. Lands in the kernel; play-time
                // materialization fills these slots instead of free-form guessing.
                let mut obj_skills: Vec<String> = character_template.fields.iter()
                    .filter(|f| f.field_type == "skill").map(|f| f.field_id.clone()).collect();
                obj_skills.extend(skill_names);
                obj_skills.sort();
                obj_skills.dedup();
                let resource_tracks: Vec<String> = reader_run_kit.as_ref()
                    .map(|rk| rk.core.resource_tracks.iter()
                        .filter_map(|t| t.get("id").or_else(|| t.get("name")).and_then(|v| v.as_str()).map(String::from))
                        .collect())
                    .unwrap_or_default();
                let obj_ctx = reader::ObjectCtx { units: &units, sidecar_text, skills: obj_skills, resource_tracks };
                object_schemas = reader::compile_object_schemas(compiler_llm.as_ref(), &obj_ctx, 14).await;
                if !object_schemas.is_empty() {
                    tracing::info!(categories = object_schemas.len(), "object/ability schemas compiled");
                }
                mech_units = units;
            }
        }
        normalize_character_sheet_template_schema(&mut character_template, &ruleset_id, &doc.title, book);
        let character_kernel = serde_json::to_string_pretty(&character_template).unwrap_or_default();
        context_blocks.push(tagged_block(
            format!("ruleset.{ruleset_id}.character_template"),
            BlockKind::RulesetCharacterKernel,
            "Character Template Kernel",
            &character_kernel,
            Visibility::GmOnly,
            Stability::RarelyChanged,
            CacheZone::Prefix,
            Scope::ruleset(&ruleset_id),
            90,
            vec!["character", "template", "chargen"],
            first_source_ref(book),
        ));

        let mut character_onboarding_pack = if let Some(rk) = &reader_run_kit {
            // Build the pack deterministically from the reader's COMPLETE template
            // (sheet + creation flow), then layer in the reader's option catalogs.
            // No whole-book extract LLM call.
            let mut pack = fallback_character_onboarding_pack(&ruleset_id, &doc.title, book, &book_map, &character_template, &procedures);
            if let Ok(cats) = serde_json::from_value::<Vec<CharacterOptionCatalog>>(rk.option_catalogs.clone()) {
                if !cats.is_empty() {
                    pack.option_catalogs = cats;
                }
            }
            pack
        } else {
            match first_pass.as_ref().and_then(|fp| fp.character_onboarding_pack_json.clone()) {
                Some(value) => coerce_character_onboarding_pack(value, &ruleset_id, &doc.title, book, &book_map, &character_template, &procedures),
                None => self.extract_character_onboarding_pack(&ruleset_id, &doc.title, book, &book_map, &character_template, &procedures).await.unwrap_or_else(|err| {
                    warn!(error = %err, "character onboarding pack extraction failed; using deterministic fallback");
                    fallback_character_onboarding_pack(&ruleset_id, &doc.title, book, &book_map, &character_template, &procedures)
                }),
            }
        };

        if let Some(rk) = &reader_run_kit {
            let formulas = derived_values_from_run_kit(rk);
            if !formulas.is_empty() {
                if character_onboarding_pack.derived_formula_pack.pack_id.is_empty() {
                    character_onboarding_pack.derived_formula_pack.pack_id = format!("{ruleset_id}.derived_formula_pack.reader_v1");
                    character_onboarding_pack.derived_formula_pack.ruleset_id = ruleset_id.clone();
                }
                character_onboarding_pack.derived_formula_pack.formulas = formulas;
            }
        }
        self.write_character_onboarding_pack_artifact(&character_onboarding_pack).await.ok();
        let character_pack_kernel = serde_json::to_string_pretty(&character_onboarding_pack).unwrap_or_default();
        context_blocks.push(tagged_block(
            format!("ruleset.{ruleset_id}.character_onboarding_pack"),
            BlockKind::CharacterOnboardingPack,
            "Character Onboarding Pack",
            &character_pack_kernel,
            Visibility::GmOnly,
            Stability::RarelyChanged,
            CacheZone::Prefix,
            Scope::ruleset(&ruleset_id),
            92,
            vec!["character", "creation", "onboarding", "playability_gate"],
            first_source_ref(book),
        ));

        for procedure in &procedures {
            let mut block = ContextBlock::new(
                format!("ruleset.{ruleset_id}.proc.{}", sanitize_id(&procedure.procedure_id)),
                BlockKind::ProcedureDetail,
                &procedure.title,
                BlockContent::Procedure(procedure.clone()),
                Visibility::GmOnly,
                Stability::RarelyChanged,
                CacheZone::Prefix,
                Scope::ruleset(&ruleset_id),
                85,
            );
            block.tags = vec!["procedure".to_string(), "resident".to_string(), "starter_procedure".to_string()];
            context_blocks.push(block);
        }

        // When the reader is on, the real game identity/loop is already in the
        // RuleKernel (BP1); synthesize gm_onboarding deterministically from the
        // reader instead of the expensive whole-book extract.
        let mut gm_onboarding = if let Some(rk) = &reader_run_kit {
            let mut gm = fallback_gm_onboarding(&ruleset_id, &doc.title, doc, book, &book_map, &procedures);
            gm.game_identity = json!({"summary": rk.game_identity});
            gm.play_loop = json!({"gm_procedures": rk.gm_procedures, "subsystem_map": rk.subsystem_map});
            gm
        } else {
            match first_pass.as_ref().and_then(|fp| fp.gm_onboarding_json.clone()) {
                Some(value) => coerce_gm_onboarding(value, &ruleset_id, &doc.title, book, &book_map, &procedures),
                None => self.extract_gm_onboarding(&ruleset_id, &doc.title, &book_map, book, &procedures).await.unwrap_or_else(|err| {
                    warn!(error = %err, "GM onboarding extraction failed; using deterministic fallback");
                    fallback_gm_onboarding(&ruleset_id, &doc.title, doc, book, &book_map, &procedures)
                }),
            }
        };
        if gm_onboarding.book_locator.is_empty() {
            gm_onboarding.book_locator = book_locator_entries_from_book_map(&ruleset_id, "ruleset", &doc.source_id, &book_map, "located");
        }
        if gm_onboarding.cold_data_locator.is_empty() {
            gm_onboarding.cold_data_locator = cold_data_locators_from_book_map(&ruleset_id, "ruleset", &doc.source_id, &book_map);
        }
        for block in onboarding_blocks(&gm_onboarding) {
            context_blocks.push(block);
        }
        let mut rule_kernel = match &reader_run_kit {
            Some(rk) => rule_kernel_from_run_kit(&ruleset_id, &doc.title, rk, &character_onboarding_pack, object_schemas),
            None => rule_kernel_from_onboarding(&ruleset_id, &doc.title, &gm_onboarding, &character_onboarding_pack, &procedures),
        };
        // Rule agent's THIRD pass (mechanics catalog): compile BEFORE the
        // artifact write and the BP1 context block so the catalog rides the
        // kernel JSON everywhere it lands. Gated + fail-closed inside.
        compile_mechanics_into_kernel(&self.llm, &mut rule_kernel, &mech_units, mech_sidecar, mech_skills).await;
        let _ = self.write_rule_kernel_artifact(&rule_kernel).await;
        context_blocks.push(rule_kernel_context_block(&rule_kernel));
        for locator in gm_onboarding.book_locator.iter().chain(gm_onboarding.cold_data_locator.iter()) {
            material_index.push(material_from_locator(&bundle_id, locator));
        }
        material_index.extend(materials_from_semantic_units(&bundle_id, &ruleset_id, "ruleset", book));

        if self.config.parse_full_chunks {
            conversion_trace.push(ConversionTraceEvent::new("full_chunk_parse_enabled", "TRPG_PARSE_FULL_CHUNKS enabled: extracting rulebook chunks."));
            for chunk in chunk_book(book, self.config.chunk_pages, self.config.max_chunk_chars) {
                let extracted = self.extract_rulebook_chunk(&ruleset_id, &doc.title, &chunk).await;
                match extracted {
                    Ok((mut blocks, mut materials)) => {
                        context_blocks.append(&mut blocks);
                        material_index.append(&mut materials);
                    }
                    Err(err) => {
                        warn!(error = %err, start = chunk.start_page, end = chunk.end_page, "chunk extraction failed");
                        let fallback = fallback_chunk_block(&ruleset_id, &chunk, SourceKind::Rulebook);
                        context_blocks.push(fallback);
                    }
                }
            }
        } else {
            conversion_trace.push(ConversionTraceEvent::new("full_chunk_parse_skipped", "Rulebook cold data was indexed as locators; detailed packets will be learned on demand."));
        }

        material_index.extend(context_blocks.iter().map(|block| material_from_block(&bundle_id, block)));

        let parsed_ruleset = ParsedRuleset {
            ruleset_id: ruleset_id.clone(),
            title: doc.title.clone(),
            document_type: Some(document_type),
            ruleset_family: Some(RulesetFamily {
                family_id: ruleset_id.clone(),
                title: doc.title.clone(),
                core_books: vec![doc.source_id.clone()],
                supplements: vec![],
            }),
            ..Default::default()
        };

        conversion_trace.push(ConversionTraceEvent::new("rulebook_parse_finished", format!("Parsed {} blocks", context_blocks.len())));
        Ok(RuleBundle {
            schema_version: RULE_SCHEMA_VERSION.to_string(),
            bundle_id,
            ruleset_id,
            title: doc.title.clone(),
            source_index,
            parsed_ruleset,
            procedure_registry,
            rule_kernel: Some(rule_kernel),
            gm_onboarding: Some(gm_onboarding),
            character_templates: vec![character_template],
            character_onboarding_packs: vec![character_onboarding_pack],
            material_index,
            context_blocks,
            validation_report: ValidationReport { status: "ok".to_string(), ..Default::default() },
            conversion_trace,
        })
    }

    async fn parse_module(&self, doc: &SourceDocument, book: &PlainTextBook, project_ruleset_ids: &[String]) -> Result<ModuleBundle> {
        // Bind the module to a ruleset: keyword-infer from its text, else (e.g. a
        // non-English module whose text doesn't name the system) fall back to the
        // project's sole ruleset — modules share a data dir with their ruleset.
        let ruleset_id = infer_module_ruleset(&doc.title, book)
            .or_else(|| if project_ruleset_ids.len() == 1 { project_ruleset_ids.first().cloned() } else { None });
        // A non-ASCII (e.g. Chinese) title sanitizes to the junk stub "id"; fall
        // back to a stable ruleset-scoped id from the source filename instead.
        let raw_module_id = infer_module_id(&doc.title);
        let module_id = if raw_module_id == "id" || raw_module_id.len() < 3 {
            let base = ruleset_id.clone().unwrap_or_else(|| "module".to_string());
            format!("{base}.{}", sanitize_id(&doc.source_id))
        } else {
            raw_module_id
        };
        let bundle_id = format!("module.{module_id}.{}", doc.source_id);
        let source_index = source_index_from_book(doc, book);
        let module_type = classify_module(&doc.title, book);
        let book_map = build_book_map(book);
        let mut context_blocks = Vec::new();
        let mut material_index = Vec::new();
        let mut conversion_trace = vec![ConversionTraceEvent::new("module_parse_started", format!("Parsing {}", doc.title))];

        let spine_json = self.extract_module_spine(&module_id, &doc.title, &book_map, book).await.unwrap_or_else(|err| {
            warn!(error = %err, "module spine extraction failed; using fallback");
            json!({"summary":"Module spine extraction failed; see indexed chunks.", "book_map": book_map.clone()})
        });

        let spine_md = serde_json::to_string_pretty(&spine_json).unwrap_or_default();
        context_blocks.push(tagged_block(
            format!("module.{module_id}.spine"),
            BlockKind::ModuleSpine,
            "Module Spine",
            &spine_md,
            Visibility::GmOnly,
            Stability::RarelyChanged,
            CacheZone::PinnedMiddle,
            Scope::module(&module_id),
            100,
            vec!["module", "spine"],
            first_source_ref(book),
        ));

        let prep_packet = self.extract_module_prep_packet(&module_id, ruleset_id.as_deref(), &doc.title, &book_map, book).await.unwrap_or_else(|err| {
            warn!(error = %err, "module first-session prep failed; using deterministic fallback");
            fallback_module_prep_packet(&module_id, ruleset_id.as_deref(), &doc.title, book, &book_map)
        });
        for block in module_prep_blocks(&prep_packet) {
            context_blocks.push(block);
        }
        for demand in &prep_packet.required_rule_demands {
            material_index.push(MaterialIndexEntry {
                material_id: format!("{}.rule_demand.{}", module_id, sanitize_id(demand)),
                material_type: MaterialType::LookupRecipe,
                title: format!("Rule demand: {demand}"),
                summary: "A rule demand identified during module preparation. Resolve on demand into a learned packet.".to_string(),
                default_cache_zone: CacheZone::DynamicTail,
                visibility: Visibility::GmOnly,
                stability: Stability::TurnDynamic,
                source_refs: prep_packet.source_refs.clone(),
                dependencies: vec![],
                load_when: vec![LoadPredicate::ActiveModule { module_id: module_id.clone() }],
                extracted_block_id: None,
                estimated_tokens: None,
                tags: vec!["rule_demand".into(), "on_demand".into(), sanitize_id(demand)],
            });
        }

        let module_locators = book_locator_entries_from_book_map(&module_id, "module", &doc.source_id, &book_map, "on_demand");
        for locator in &module_locators {
            material_index.push(material_from_locator(&bundle_id, locator));
        }
        material_index.extend(materials_from_semantic_units(&bundle_id, &module_id, "module", book));

        if self.config.parse_full_chunks {
            conversion_trace.push(ConversionTraceEvent::new("full_module_chunk_parse_enabled", "TRPG_PARSE_FULL_CHUNKS enabled: extracting module chunks."));
            for chunk in chunk_book(book, self.config.chunk_pages, self.config.max_chunk_chars) {
                let extracted = self.extract_module_chunk(&module_id, ruleset_id.as_deref(), &doc.title, &chunk).await;
                match extracted {
                    Ok((mut blocks, mut materials)) => {
                        context_blocks.append(&mut blocks);
                        material_index.append(&mut materials);
                    }
                    Err(err) => {
                        warn!(error = %err, start = chunk.start_page, end = chunk.end_page, "module chunk extraction failed");
                        context_blocks.push(fallback_chunk_block(&module_id, &chunk, SourceKind::Module));
                    }
                }
            }
        } else {
            conversion_trace.push(ConversionTraceEvent::new("full_module_chunk_parse_skipped", "Module was prepared as overview + first-session packet; later chapters remain locator-backed."));
        }
        material_index.extend(context_blocks.iter().map(|block| material_from_block(&bundle_id, block)));

        // Module-reader agent (structured BP2/BP3 extraction): one gated pass that
        // reads the semantic units like a GM -> scenes/npcs/clues/.../custom-rules.
        // Fail-open: any failure (units load OR reader) -> warn + None -> the legacy
        // empty graph below, never panicking or aborting parse_module. `readout` is
        // borrowed (not moved) by the ModuleGraph construction so Phase 3 can reuse it.
        let readout = if module_reader_enabled() {
            let units_path = self.config.data_dir.join("parsed/source_units").join(format!("{}.semantic_units.jsonl", doc.source_id));
            match reader::load_units(&units_path) {
                Ok(units) if !units.is_empty() => {
                    // duotext2 ingest folds the column-aligned layout (and Phase 0
                    // column removal) into the single merged module `.md`; it never
                    // writes `layout_sidecar_path`, so reading that key always
                    // yielded None and the read_layout view silently degraded. Read
                    // the merged `.md` directly (same path P5 continue uses).
                    // fail-closed: missing file -> None.
                    let sidecar_text = std::fs::read_to_string(
                        self.config.data_dir.join(format!("markdown/modules/{}.md", doc.source_id))
                    ).ok();
                    let ctx = reader::ModuleReaderCtx { units: &units, sidecar_text, ruleset_id: ruleset_id.clone() };
                    let budget = std::env::var("TRPG_MODULE_READER_BUDGET").ok().and_then(|s| s.parse().ok()).unwrap_or(12);
                    // Module extraction defaults to gpt-5.4 (TRPG_MODULE_READER_MODEL),
                    // quality-first since it runs in the background; decoupled from the
                    // GM model. fail-closed: build failure → fall back to main client.
                    let reader_llm = build_module_reader_llm().unwrap_or_else(|| self.llm.clone());
                    match reader::run_module_reader(reader_llm.as_ref(), ctx, budget).await {
                        Ok(r) => Some(r),
                        Err(err) => {
                            warn!(error = %err, "module_reader failed; falling back to empty graph");
                            None
                        }
                    }
                }
                Ok(_) => {
                    warn!("load_units for module returned empty units; empty graph");
                    None
                }
                Err(err) => {
                    warn!(error = %err, "load_units for module failed; empty graph");
                    None
                }
            }
        } else {
            None
        };

        // Phase 3: classified BP2/BP3 static content -> cache-zoned blocks.
        // `readout` is borrowed (not moved) so the ModuleGraph below can reuse it.
        if let Some(r) = &readout {
            context_blocks.extend(module_static_blocks(&module_id, r));
        }

        let module_graph = ModuleGraph {
            module_id: module_id.clone(),
            ruleset_id: ruleset_id.clone(),
            title: doc.title.clone(),
            module_type: module_type.clone(),
            spine: readout.as_ref()
                .map(|r| r.spine.clone())
                .filter(|s| !s.is_null())
                .unwrap_or(spine_json),
            chapters: vec![],
            missions: vec![],
            scenes: readout.as_ref().map(|r| r.scenes.clone()).unwrap_or_default(),
            locations: readout.as_ref().map(|r| r.locations.clone()).unwrap_or_default(),
            npcs: readout.as_ref().map(|r| r.npcs.clone()).unwrap_or_default(),
            factions: readout.as_ref().map(|r| r.factions.clone()).unwrap_or_default(),
            clues: readout.as_ref().map(|r| r.clues.clone()).unwrap_or_default(),
            handouts: readout.as_ref().map(|r| r.handouts.clone()).unwrap_or_default(),
            encounters: readout.as_ref().map(|r| r.encounters.clone()).unwrap_or_default(),
            module_specific_rules: readout.as_ref().map(|r| r.module_specific_rules.clone()).unwrap_or_default(),
            // 自动抽取的模组级引导事实(无 reader/未抽到 → None,director 回退通用兜底)。
            director_facilitation: readout.as_ref().and_then(|r| r.facilitation_facts.clone()),
        };

        conversion_trace.push(ConversionTraceEvent::new("module_parse_finished", format!("Parsed {} blocks", context_blocks.len())));
        Ok(ModuleBundle {
            schema_version: MODULE_SCHEMA_VERSION.to_string(),
            bundle_id,
            module_id,
            ruleset_id,
            title: doc.title.clone(),
            source_index,
            module_graph,
            module_prep_packets: vec![prep_packet],
            module_locators,
            material_index,
            context_blocks,
            validation_report: ValidationReport { status: "ok".to_string(), ..Default::default() },
            conversion_trace,
        })
    }


    async fn extract_gm_onboarding(&self, ruleset_id: &str, title: &str, book_map: &Value, book: &PlainTextBook, procedures: &[ProcedureDef]) -> Result<GmOnboardingBundle> {
        let sample = onboarding_relevant_pages(book, 36_000);
        let value = self.llm.complete_json(vec![
            system(gm_onboarding_prompt()),
            user(format!(
                "Ruleset id: {ruleset_id}\nRulebook title: {title}\nBook map:\n{}\n\nRepresentative onboarding pages:\n{}\n\nStarter procedures already inferred:\n{}\n\nReturn one GM onboarding JSON object. Focus on operational familiarity, not full data extraction.",
                serde_json::to_string_pretty(book_map)?,
                sample,
                serde_json::to_string_pretty(procedures)?,
            )),
        ], 0.1).await?;
        Ok(coerce_gm_onboarding(value, ruleset_id, title, book, book_map, procedures))
    }

    async fn extract_module_prep_packet(&self, module_id: &str, ruleset_id: Option<&str>, title: &str, book_map: &Value, book: &PlainTextBook) -> Result<ModulePrepPacket> {
        let sample = module_first_session_pages(book, 36_000);
        let value = self.llm.complete_json(vec![
            system(module_prep_prompt()),
            user(format!(
                "Module id: {module_id}\nRuleset hint: {}\nModule title: {title}\nBook map:\n{}\n\nSynopsis / first-session candidate pages:\n{}\n\nReturn one ModulePrepPacket JSON object. Prepare only module overview and current/first session packet. Later chapters remain cold-located.",
                ruleset_id.unwrap_or("unknown"),
                serde_json::to_string_pretty(book_map)?,
                sample,
            )),
        ], 0.1).await?;
        Ok(coerce_module_prep_packet(value, module_id, ruleset_id, title, book, book_map))
    }

    async fn extract_rulebook_overview(&self, ruleset_id: &str, title: &str, book_map: &Value, book: &PlainTextBook) -> Result<Value> {
        let sample = sample_pages(book, 20_000);
        self.llm.complete_json(vec![
            system(rulebook_system_prompt()),
            user(format!(
                "Extract a high-level resident overview for ruleset `{ruleset_id}` from rulebook `{title}`.\n\nBook map:\n{}\n\nRepresentative pages:\n{}\n\nReturn JSON with keys: resident_core_markdown, world_style_markdown, director_policy_markdown, action_router_markdown. Keep it concise and operational; do not reproduce long source passages.",
                serde_json::to_string_pretty(book_map)?, sample
            )),
        ], 0.1).await
    }

    async fn extract_character_template(&self, ruleset_id: &str, title: &str, book: &PlainTextBook) -> Result<CharacterTemplate> {
        let excerpt = character_relevant_pages(book, 30_000);
        let value = self.llm.complete_json(vec![
            system(character_template_prompt()),
            user(format!(
                "Ruleset id: {ruleset_id}\nRulebook title: {title}\nRelevant character creation / sheet text:\n{excerpt}\n\nReturn a CharacterTemplate JSON exactly matching the requested shape."
            )),
        ], 0.1).await?;
        let template = coerce_character_template(value, ruleset_id, title);
        Ok(template)
    }

    async fn extract_character_onboarding_pack(&self, ruleset_id: &str, title: &str, book: &PlainTextBook, book_map: &Value, template: &CharacterTemplate, procedures: &[ProcedureDef]) -> Result<CharacterOnboardingPack> {
        let excerpt = character_onboarding_relevant_pages(book, 42_000);
        let value = self.llm.complete_json(vec![
            system(character_onboarding_pack_prompt()),
            user(format!(
                "Ruleset id: {ruleset_id}\nRulebook title: {title}\nBook map:\n{}\n\nExisting CharacterTemplate draft:\n{}\n\nStarter procedures:\n{}\n\nRelevant character creation / sheet / derived formula text:\n{excerpt}\n\nReturn one CharacterOnboardingPack JSON. Focus on fast playability: sheet template, creation flow, options locator, formulas, starter/pregen route, runtime bindings. Do not invent source values.",
                serde_json::to_string_pretty(book_map)?,
                serde_json::to_string_pretty(template)?,
                serde_json::to_string_pretty(procedures)?,
            )),
        ], 0.1).await?;
        Ok(coerce_character_onboarding_pack(value, ruleset_id, title, book, book_map, template, procedures))
    }

    async fn write_character_onboarding_pack_artifact(&self, pack: &CharacterOnboardingPack) -> Result<()> {
        let dir = self.config.data_dir.join("parsed/characters");
        fs::create_dir_all(&dir).await?;
        fs::write(dir.join(format!("{}.character_onboarding_pack.json", pack.ruleset_id)), serde_json::to_string_pretty(pack)?).await?;
        fs::write(dir.join(format!("{}.character_sheet_template.json", pack.ruleset_id)), serde_json::to_string_pretty(&pack.sheet_template)?).await?;
        fs::write(dir.join(format!("{}.character_creation_flow.json", pack.ruleset_id)), serde_json::to_string_pretty(&pack.creation_flows)?).await?;
        fs::write(dir.join(format!("{}.character_option_catalogs.json", pack.ruleset_id)), serde_json::to_string_pretty(&pack.option_catalogs)?).await?;
        fs::write(dir.join(format!("{}.derived_formulas.json", pack.ruleset_id)), serde_json::to_string_pretty(&pack.derived_formula_pack)?).await?;
        fs::write(dir.join(format!("{}.starter_character_pack.json", pack.ruleset_id)), serde_json::to_string_pretty(&pack.starter_character_pack)?).await?;
        Ok(())
    }

    async fn write_rule_kernel_artifact(&self, kernel: &RuleKernel) -> Result<()> {
        let dir = self.config.data_dir.join("parsed/rules");
        fs::create_dir_all(&dir).await?;
        fs::write(dir.join(format!("{}.rule_kernel.json", kernel.ruleset_id)), serde_json::to_string_pretty(kernel)?).await?;
        Ok(())
    }

    async fn extract_rulebook_chunk(&self, ruleset_id: &str, title: &str, chunk: &BookChunk) -> Result<(Vec<ContextBlock>, Vec<MaterialIndexEntry>)> {
        let value = self.llm.complete_json(vec![
            system(rulebook_chunk_prompt()),
            user(format!(
                "Ruleset id: {ruleset_id}\nRulebook title: {title}\nPage range: {}-{}\nSource text:\n{}\n\nExtract full structured playable materials for this chunk.",
                chunk.start_page, chunk.end_page, chunk.text
            )),
        ], 0.1).await?;
        Ok(blocks_and_materials_from_llm(&value, ruleset_id, Scope::ruleset(ruleset_id), SourceKind::Rulebook, chunk))
    }

    async fn extract_module_spine(&self, module_id: &str, title: &str, book_map: &Value, book: &PlainTextBook) -> Result<Value> {
        let sample = sample_pages(book, 24_000);
        self.llm.complete_json(vec![
            system(module_system_prompt()),
            user(format!(
                "Extract a module spine for module `{module_id}` from `{title}`.\n\nBook map:\n{}\n\nRepresentative pages:\n{}\n\nReturn JSON with keys: module_type, ruleset_hint, synopsis, background, tone, entry_hooks, campaign_or_mission_structure, spoiler_policy, current_playable_units.",
                serde_json::to_string_pretty(book_map)?, sample
            )),
        ], 0.1).await
    }

    async fn extract_module_chunk(&self, module_id: &str, ruleset_id: Option<&str>, title: &str, chunk: &BookChunk) -> Result<(Vec<ContextBlock>, Vec<MaterialIndexEntry>)> {
        let value = self.llm.complete_json(vec![
            system(module_chunk_prompt()),
            user(format!(
                "Module id: {module_id}\nRuleset hint: {}\nModule title: {title}\nPage range: {}-{}\nSource text:\n{}\n\nExtract full structured playable materials for this chunk.",
                ruleset_id.unwrap_or("unknown"), chunk.start_page, chunk.end_page, chunk.text
            )),
        ], 0.1).await?;
        Ok(blocks_and_materials_from_llm(&value, module_id, Scope::module(module_id), SourceKind::Module, chunk))
    }
}

#[derive(Debug, Clone)]
pub struct BookChunk {
    pub source_id: String,
    pub start_page: u32,
    pub end_page: u32,
    pub text: String,
}

pub fn chunk_book(book: &PlainTextBook, pages_per_chunk: usize, max_chars: usize) -> Vec<BookChunk> {
    if !book.chunks.is_empty() {
        let mut out = Vec::new();
        for c in &book.chunks {
            if semantic_signal_class(c) == "noise" { continue; }
            let start_page = c.page_numbers.first().copied().unwrap_or(1);
            let end_page = c.page_numbers.last().copied().unwrap_or(start_page);
            let body = if c.full_text.trim().is_empty() { c.text.clone() } else { c.full_text.clone() };
            let category = semantic_category(c);
            let entity = defined_entity(c).unwrap_or_else(|| c.heading_context.last().cloned().unwrap_or_else(|| "unnamed_source_unit".into()));
            let tags = c.metadata.get("mechanics_tags").cloned().unwrap_or_else(|| json!([]));
            let preamble = format!("[SEMANTIC_SOURCE_UNIT category={} entity={} gm_secret={} mechanics_tags={}]\n", category, entity, c.metadata.get("gm_secret").and_then(Value::as_bool).unwrap_or(false), tags);
            out.push(BookChunk { source_id: book.source_id.clone(), start_page, end_page, text: format!("{}{}", preamble, body) });
        }
        return out;
    }
    let mut chunks = Vec::new();
    let mut buf = String::new();
    let mut start_page = None;
    let mut end_page = 0;
    let mut page_count = 0usize;
    for page in &book.pages {
        if page_is_noise(&page.text) { continue; }
        if start_page.is_none() { start_page = Some(page.page); }
        let page_text = format!("\n\n[PAGE {}]\n{}", page.page, page.text);
        if (!buf.is_empty() && buf.len() + page_text.len() > max_chars) || page_count >= pages_per_chunk {
            chunks.push(BookChunk { source_id: book.source_id.clone(), start_page: start_page.unwrap_or(end_page), end_page, text: buf.clone() });
            buf.clear();
            start_page = Some(page.page);
            page_count = 0;
        }
        buf.push_str(&page_text);
        end_page = page.page;
        page_count += 1;
    }
    if !buf.trim().is_empty() {
        chunks.push(BookChunk { source_id: book.source_id.clone(), start_page: start_page.unwrap_or(1), end_page, text: buf });
    }
    chunks
}

fn chunk_needs_structural_llm_repair(chunk: &DocumentChunk) -> bool {
    let text = &chunk.text;
    let full = if chunk.full_text.trim().is_empty() { &chunk.text } else { &chunk.full_text };
    text.contains("þÿ")
        || text.contains('\u{fffd}')
        || text.contains('\u{0000}')
        || full.contains("þÿ")
        || full.contains('\u{fffd}')
        || full.contains('\u{0000}')
        || likely_table_columns_collapsed(full)
}

fn conditioned_chunk_metadata(chunk: &DocumentChunk) -> Value {
    if chunk.metadata.is_object() { chunk.metadata.clone() } else { json!({}) }
}

fn infer_source_kind_for_book(book: &PlainTextBook) -> SourceKind {
    let id_title = format!("{} {}", book.source_id, book.title).to_ascii_lowercase();
    if id_title.contains("homecoming") || id_title.contains("vault") || id_title.contains("masks") || id_title.contains("mission") || id_title.contains("adventure") { SourceKind::Module }
    else if id_title.contains("rulebook") || id_title.contains("handbook") || id_title.contains("guide") || id_title.contains("manual") || id_title.contains("orc") { SourceKind::Rulebook }
    else { SourceKind::Unknown }
}

fn semantic_units_from_book(book: &PlainTextBook, source_kind: SourceKind, max_chars: usize) -> Vec<DocumentChunk> {
    let mut units = Vec::new();
    let mut current_title = String::new();
    let mut current_lines: Vec<String> = Vec::new();
    let mut current_pages: Vec<u32> = Vec::new();
    let mut current_heading_path: Vec<String> = Vec::new();
    let mut major_path: Vec<String> = Vec::new();
    let mut unit_idx = 1usize;

    let flush = |units: &mut Vec<DocumentChunk>,
                 unit_idx: &mut usize,
                 title: &mut String,
                 lines: &mut Vec<String>,
                 pages: &mut Vec<u32>,
                 heading_path: &mut Vec<String>,
                 book: &PlainTextBook,
                 source_kind: SourceKind| {
        let text = lines.join("\n").trim().to_string();
        if text.trim().is_empty() {
            lines.clear();
            pages.clear();
            return;
        }
        let title_value = if title.trim().is_empty() { infer_title_from_text(&text) } else { title.trim().to_string() };
        let mut heading = heading_path.clone();
        if heading.is_empty() { heading.push(title_value.clone()); }
        let metadata = classify_semantic_unit(&title_value, &text, source_kind.clone());
        let source_ref = SourceRef {
            source_id: book.source_id.clone(),
            page: pages.first().copied(),
            anchor_id: Some(format!("{}.unit_{:05}", book.source_id, *unit_idx)),
            section_path: heading.clone(),
            text_hash: Some(sha256_hex(&text)),
            note: Some(format!("semantic_unit pages {:?}", pages)),
            ..Default::default()
        };
        let chunk = DocumentChunk {
            chunk_id: format!("{}.unit_{:05}", book.source_id, *unit_idx),
            text: text.clone(),
            full_text: text.clone(),
            page_numbers: pages.clone(),
            element_types: semantic_element_types(&metadata),
            heading_context: heading,
            token_estimate: Some((text.chars().count() as u32 / 4).max(1)),
            is_oversized: text.chars().count() > 8_000,
            bounding_boxes: vec![],
            text_hash: Some(sha256_hex(&text)),
            clean_status: Some("semantic_unit_v1".into()),
            metadata: merge_semantic_source_ref(metadata, source_ref),
        };
        units.push(chunk);
        *unit_idx += 1;
        title.clear();
        lines.clear();
        pages.clear();
        heading_path.clear();
    };

    for page in &book.pages {
        let cleaned_lines = filter_page_noise_lines(&page.text);
        if cleaned_lines.is_empty() { continue; }
        let page_text = cleaned_lines.join("\n");
        if page_is_noise(&page_text) {
            let title = if looks_like_toc(&page_text) { "Table of Contents".to_string() } else { infer_title_from_text(&page_text) };
            let metadata = classify_semantic_unit(&title, &page_text, source_kind.clone());
            let mut meta = metadata;
            meta["signal_class"] = json!("noise");
            let text_hash = sha256_hex(&page_text);
            units.push(DocumentChunk {
                chunk_id: format!("{}.unit_{:05}", book.source_id, unit_idx),
                text: page_text.clone(),
                full_text: page_text,
                page_numbers: vec![page.page],
                element_types: vec![semantic_category_from_meta(&meta), "noise".into()],
                heading_context: vec![title],
                token_estimate: Some(1),
                is_oversized: false,
                bounding_boxes: vec![],
                text_hash: Some(text_hash),
                clean_status: Some("semantic_noise_v1".into()),
                metadata: meta,
            });
            unit_idx += 1;
            continue;
        }
        for raw_line in cleaned_lines {
            let line = raw_line.trim_end().to_string();
            if line.trim().is_empty() {
                if !current_lines.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
                    current_lines.push(String::new());
                }
                continue;
            }
            let heading_like = is_semantic_heading_line(&line);
            let accumulated = current_lines.iter().map(|l| l.chars().count()).sum::<usize>();
            if heading_like && !current_lines.is_empty() {
                flush(&mut units, &mut unit_idx, &mut current_title, &mut current_lines, &mut current_pages, &mut current_heading_path, book, source_kind.clone());
            } else if accumulated > max_chars && line_ends_unit(&line) && !current_lines.is_empty() {
                flush(&mut units, &mut unit_idx, &mut current_title, &mut current_lines, &mut current_pages, &mut current_heading_path, book, source_kind.clone());
            }
            if heading_like {
                let heading = line.trim().trim_end_matches(':').to_string();
                if is_major_heading(&heading) {
                    major_path.clear();
                    major_path.push(heading.clone());
                }
                current_title = heading.clone();
                current_heading_path = major_path.clone();
                if current_heading_path.last() != Some(&heading) { current_heading_path.push(heading); }
            } else {
                if current_title.is_empty() {
                    current_title = infer_title_from_text(&line);
                    current_heading_path = major_path.clone();
                    if !current_title.is_empty() { current_heading_path.push(current_title.clone()); }
                }
                if !current_pages.contains(&page.page) { current_pages.push(page.page); }
                current_lines.push(line);
            }
        }
        if !current_lines.is_empty() && !current_pages.contains(&page.page) { current_pages.push(page.page); }
    }
    flush(&mut units, &mut unit_idx, &mut current_title, &mut current_lines, &mut current_pages, &mut current_heading_path, book, source_kind);
    units
}

fn filter_page_noise_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim_end().to_string())
        .filter(|l| {
            let t = l.trim();
            if t.is_empty() { return true; }
            if Regex::new(r"(?i)^page\s+\d+\s*$").unwrap().is_match(t) { return false; }
            if Regex::new(r"^\d+\s*$").unwrap().is_match(t) { return false; }
            if t.len() <= 2 && !t.chars().any(char::is_alphanumeric) { return false; }
            true
        })
        .collect()
}

fn page_is_noise(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let stripped = lower.trim();
    if stripped.len() < 20 { return true; }
    looks_like_toc(text)
        || lower.contains("copyright") && lower.contains("all rights reserved")
        || lower.contains("isbn") && (lower.contains("printed") || lower.contains("publisher"))
        || lower.contains("credits") && (lower.contains("illustrator") || lower.contains("layout") || lower.contains("editor"))
        || lower.contains("special thanks")
        || lower.contains("acknowledgements")
        || lower.contains("cover illustrator")
        || lower.contains("backers & supporters")
}

fn looks_like_toc(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if lower.contains("table of contents") || lower.trim() == "contents" { return true; }
    let dot_leader_lines = text.lines().filter(|l| l.matches('.').count() >= 4 && Regex::new(r"\d+\s*$").unwrap().is_match(l.trim())).count();
    dot_leader_lines >= 4
}

fn is_semantic_heading_line(line: &str) -> bool {
    let t = line.trim();
    if t.len() < 3 || t.len() > 100 { return false; }
    if t.starts_with('•') || t.starts_with('-') || t.starts_with('o') && t.contains(" DV") { return false; }
    if t.ends_with('.') || t.ends_with(',') || t.contains(" | ") { return false; }
    let lower = t.to_ascii_lowercase();
    if Regex::new(r"(?i)^(chapter|part|appendix|section|floor\s+#?\d+|step\s+\w+|scenario|encounter|investigation|aftermath|pre-investigation|chaos effects|anomaly profile)\b").unwrap().is_match(t) { return true; }
    if t.chars().any(char::is_alphabetic) {
        let alpha = t.chars().filter(|c| c.is_alphabetic()).count();
        let upper = t.chars().filter(|c| c.is_alphabetic() && c.is_uppercase()).count();
        if alpha >= 4 && upper * 100 / alpha >= 72 { return true; }
    }
    let words = t.split_whitespace().collect::<Vec<_>>();
    words.len() <= 6 && words.iter().filter(|w| w.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)).count() >= words.len().saturating_sub(1) && !lower.contains("from")
}

fn is_major_heading(heading: &str) -> bool {
    Regex::new(r"(?i)^(chapter|part|appendix|introduction|prologue|book structure)\b").unwrap().is_match(heading.trim())
}

fn line_ends_unit(line: &str) -> bool {
    line.trim().ends_with('.') || line.trim().ends_with('。') || line.trim().ends_with('!') || line.trim().ends_with('！')
}

fn infer_title_from_text(text: &str) -> String {
    text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("source unit").chars().take(80).collect()
}

fn classify_semantic_unit(title: &str, text: &str, source_kind: SourceKind) -> Value {
    let hay = format!("{}\n{}", title, text).to_ascii_lowercase();
    let signal_class = if page_is_noise(&hay) || hay.contains("general index") || hay.contains("monster index") || hay.contains("spells index") { "noise" } else { "signal" };
    let category = if signal_class == "noise" {
        if looks_like_toc(text) { "toc" } else if hay.contains("index") { "index" } else if hay.contains("credit") || hay.contains("copyright") || hay.contains("isbn") { "front_matter" } else { "noise" }
    } else if looks_like_stat_block(text) { "stat_block" }
    else if looks_like_table(text) { "table" }
    else if matches!(source_kind, SourceKind::Module) && contains_any(&hay, &["npc", "dramatis personae", "athena", "scav", "lawman", "foxwell", "hisako", "quil", "anomaly", "minor anomaly", "influencer"]) { "npc_or_anomaly" }
    else if matches!(source_kind, SourceKind::Module) && contains_any(&hay, &["clue", "handout", "file", "recording", "message", "questions for", "briefing"]) { "clue_or_handout" }
    else if matches!(source_kind, SourceKind::Module) && contains_any(&hay, &["warehouse", "room", "street", "avenue", "domain", "location", "map", "sewer", "apartment"]) { "location_or_scene" }
    else if matches!(source_kind, SourceKind::Module) && contains_any(&hay, &["chapter", "scene", "encounter", "investigation", "pre-investigation", "aftermath", "lawmen in trouble"]) { "scene" }
    else if contains_any(&hay, &["how to", "step", "method", "flow", "procedure", "resolving", "resolution", "difficulty", "check", "attack", "damage", "saving throw", "skill roll", "contest"]) { "rule_or_procedure" }
    else if contains_any(&hay, &["weapon", "armor", "equipment", "spell", "ability", "requisition", "cyberware", "program", "power"]) { "data_entry" }
    else { "lore_or_guidance" };
    let mechanics_tags = mechanics_tags_for(&hay);
    let gm_secret = matches!(source_kind, SourceKind::Module) && contains_any(&hay, &["background", "secret", "keeper", "gm", "don't read", "dont read", "make sure the pcs only know", "unbeknownst", "cult in residence", "current situation", "impulse", "focus", "domain", "the secret version", "spoiler"]);
    let visibility = if gm_secret || matches!(source_kind, SourceKind::Module) { "gm_only" } else { "gm_only" };
    let defined_entity = title.trim().to_string();
    let extraction_priority = if signal_class == "noise" { 0.05 } else if category == "stat_block" || category == "table" { 0.95 } else if !mechanics_tags.is_empty() { 0.85 } else { 0.6 };
    json!({
        "unit_kind": "semantic_unit",
        "signal_class": signal_class,
        "semantic_category": category,
        "defined_entity": defined_entity,
        "gm_secret": gm_secret,
        "visibility_hint": visibility,
        "mechanics_tags": mechanics_tags,
        "quality_flags": quality_flags_for(text),
        "index_weight": extraction_priority,
        "drop_from_materialization": signal_class == "noise",
        "source_kind": source_kind.as_str(),
    })
}

fn contains_any(hay: &str, needles: &[&str]) -> bool { needles.iter().any(|n| hay.contains(n)) }

fn mechanics_tags_for(hay: &str) -> Vec<String> {
    let mut tags = Vec::new();
    for (tag, needles) in [
        ("check_target", vec!["dv", "dc", "difficulty", "target number", "target value"]),
        ("dice", vec!["d20", "d10", "d100", "2d6", "6d4", "1d", "roll"]),
        ("damage", vec!["damage", "伤害", "hp reaches 0", "hit points", "hp"]),
        ("armor_defense", vec!["armor", "sp", "ac", "defense", "evasion"]),
        ("skill", vec!["skill", "basic tech", "interface", "brawling", "stealth", "athletics"]),
        ("resource", vec!["sanity", "san", "chaos", "harm", "mp", "humanity", "commendation", "demerit"]),
        ("netrunning", vec!["net architecture", "password", "control node", "file dv", "netrun", "cyberdeck"]),
        ("stat_block", vec!["stat block", "npc card", "hp", "move", "rof", "ammo"]),
        ("scene_state", vec!["round", "turn", "clock", "countdown", "4 rounds", "current situation"]),
        ("spoiler", vec!["secret", "background", "unbeknownst", "keeper", "gm only"]),
    ] {
        if needles.iter().any(|n| hay.contains(n)) { tags.push(tag.to_string()); }
    }
    tags.sort(); tags.dedup(); tags
}

fn quality_flags_for(text: &str) -> Vec<String> {
    let mut flags = Vec::new();
    if text.contains("þÿ") || text.contains('\u{fffd}') || text.contains('\u{0000}') { flags.push("encoding_artifact".to_string()); }
    if likely_table_columns_collapsed(text) { flags.push("table_columns_collapsed".to_string()); }
    if text.chars().count() > 8_000 { flags.push("oversized_semantic_unit".to_string()); }
    flags
}

fn looks_like_table(text: &str) -> bool {
    let lines = text.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>();
    if lines.iter().filter(|l| l.matches('|').count() >= 2).count() >= 2 { return true; }
    if lines.iter().filter(|l| Regex::new(r"\s{2,}\S+\s{2,}\S+").unwrap().is_match(l)).count() >= 3 { return true; }
    false
}

fn looks_like_stat_block(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let signals = ["hp", "dv", "sp", "ac", "damage", "move", "rof", "ammo", "skill", "armor", "stability", "threat", "floor #", "control node"];
    signals.iter().filter(|s| lower.contains(**s)).count() >= 3
}

fn likely_table_columns_collapsed(text: &str) -> bool {
    if text.matches('|').count() > 20 || text.contains("| |") { return true; }
    let dense_numeric_rows = text.lines().filter(|l| {
        let digit_groups = Regex::new(r"\b\d+[dD]?\d*\b").unwrap().find_iter(l).count();
        l.chars().count() > 160 && digit_groups >= 6
    }).count();
    dense_numeric_rows >= 2
}

fn semantic_element_types(metadata: &Value) -> Vec<String> {
    let mut out = vec![semantic_category_from_meta(metadata), semantic_signal_from_meta(metadata)];
    if let Some(arr) = metadata.get("mechanics_tags").and_then(Value::as_array) {
        out.extend(arr.iter().filter_map(Value::as_str).map(str::to_string));
    }
    out.sort(); out.dedup(); out
}

fn semantic_category_from_meta(metadata: &Value) -> String { metadata.get("semantic_category").and_then(Value::as_str).unwrap_or("unknown").to_string() }
fn semantic_signal_from_meta(metadata: &Value) -> String { metadata.get("signal_class").and_then(Value::as_str).unwrap_or("signal").to_string() }
fn semantic_signal_class(chunk: &DocumentChunk) -> String { semantic_signal_from_meta(&chunk.metadata) }
fn semantic_category(chunk: &DocumentChunk) -> String { semantic_category_from_meta(&chunk.metadata) }
fn defined_entity(chunk: &DocumentChunk) -> Option<String> { chunk.metadata.get("defined_entity").and_then(Value::as_str).map(str::to_string).filter(|s| !s.trim().is_empty()) }

fn merge_semantic_source_ref(mut metadata: Value, source_ref: SourceRef) -> Value {
    metadata["source_refs"] = serde_json::to_value(vec![source_ref]).unwrap_or_else(|_| json!([]));
    metadata
}

fn unit_needs_llm_semantic_wash(unit: &DocumentChunk) -> bool {
    let flags = unit.metadata.get("quality_flags").and_then(Value::as_array).cloned().unwrap_or_default();
    flags.iter().any(|f| matches!(f.as_str(), Some("encoding_artifact" | "table_columns_collapsed" | "oversized_semantic_unit")))
        || unit.text.chars().count() > 5_000 && unit.text.matches('\n').count() > 50
}

fn semantic_units_from_llm_value(base: &DocumentChunk, value: &Value) -> Vec<DocumentChunk> {
    let Some(arr) = value.get("semantic_units").and_then(Value::as_array) else { return vec![]; };
    let mut out = Vec::new();
    for (idx, item) in arr.iter().enumerate() {
        let text = item.get("text").and_then(Value::as_str).unwrap_or("").trim();
        if text.is_empty() { continue; }
        let title = item.get("title").and_then(Value::as_str).unwrap_or_else(|| base.heading_context.last().map(String::as_str).unwrap_or("semantic unit"));
        let mut meta = base.metadata.clone();
        meta["unit_kind"] = json!("semantic_unit");
        meta["semantic_category"] = json!(item.get("category").and_then(Value::as_str).unwrap_or_else(|| meta.get("semantic_category").and_then(Value::as_str).unwrap_or("unknown")));
        meta["signal_class"] = json!(item.get("signal_class").and_then(Value::as_str).unwrap_or_else(|| meta.get("signal_class").and_then(Value::as_str).unwrap_or("signal")));
        meta["defined_entity"] = json!(item.get("defined_entity").and_then(Value::as_str).unwrap_or(title));
        meta["gm_secret"] = json!(item.get("gm_secret").and_then(Value::as_bool).unwrap_or_else(|| meta.get("gm_secret").and_then(Value::as_bool).unwrap_or(false)));
        if let Some(tags) = item.get("mechanics_tags") { meta["mechanics_tags"] = tags.clone(); }
        meta["llm_semantic_wash"] = json!({"from_unit_id": base.chunk_id, "split_index": idx + 1, "split_reason": item.get("split_reason").cloned().unwrap_or_else(|| json!(null))});
        let mut heading = base.heading_context.clone();
        if heading.last().map(|h| h.as_str()) != Some(title) { heading.push(title.to_string()); }
        out.push(DocumentChunk {
            chunk_id: format!("{}.llm_split_{:02}", base.chunk_id, idx + 1),
            text: text.to_string(),
            full_text: text.to_string(),
            page_numbers: base.page_numbers.clone(),
            element_types: semantic_element_types(&meta),
            heading_context: heading,
            token_estimate: Some((text.chars().count() as u32 / 4).max(1)),
            is_oversized: text.chars().count() > 8_000,
            bounding_boxes: base.bounding_boxes.clone(),
            text_hash: Some(sha256_hex(text)),
            clean_status: Some("llm_semantic_unit_v1".into()),
            metadata: meta,
        });
    }
    out
}

fn materials_from_semantic_units(_bundle_id: &str, owner_id: &str, owner_kind: &str, book: &PlainTextBook) -> Vec<MaterialIndexEntry> {
    book.chunks.iter()
        .filter(|chunk| chunk.metadata.get("unit_kind").and_then(Value::as_str) == Some("semantic_unit"))
        .filter(|chunk| semantic_signal_class(chunk) != "noise")
        .map(|chunk| {
            let category = semantic_category(chunk);
            let title = defined_entity(chunk).unwrap_or_else(|| chunk.heading_context.last().cloned().unwrap_or_else(|| chunk.chunk_id.clone()));
            let summary = chunk.text.chars().take(500).collect::<String>();
            let visibility = parse_visibility(chunk.metadata.get("visibility_hint").and_then(Value::as_str).unwrap_or("gm_only"));
            let mut tags = vec!["semantic_unit".into(), category.clone(), owner_kind.to_string()];
            if let Some(arr) = chunk.metadata.get("mechanics_tags").and_then(Value::as_array) {
                tags.extend(arr.iter().filter_map(Value::as_str).map(str::to_string));
            }
            if chunk.metadata.get("gm_secret").and_then(Value::as_bool).unwrap_or(false) { tags.push("gm_secret".into()); }
            tags.sort(); tags.dedup();
            let load_when = if owner_kind == "module" { vec![LoadPredicate::ActiveModule { module_id: owner_id.to_string() }] } else { vec![LoadPredicate::ActiveRuleset { ruleset_id: owner_id.to_string() }] };
            MaterialIndexEntry {
                material_id: format!("material.semantic.{}", chunk.chunk_id),
                material_type: material_type_from_semantic_category(&category),
                title,
                summary,
                default_cache_zone: CacheZone::NeverPrompt,
                visibility,
                stability: Stability::RarelyChanged,
                source_refs: source_refs_for_semantic_chunk(chunk),
                dependencies: vec![],
                load_when,
                extracted_block_id: None,
                estimated_tokens: chunk.token_estimate,
                tags,
            }
        })
        .collect()
}

fn source_refs_for_semantic_chunk(chunk: &DocumentChunk) -> Vec<SourceRef> {
    if let Some(arr) = chunk.metadata.get("source_refs").cloned() {
        if let Ok(refs) = serde_json::from_value::<Vec<SourceRef>>(arr) { if !refs.is_empty() { return refs; } }
    }
    vec![SourceRef { source_id: chunk.chunk_id.split('.').next().unwrap_or_default().to_string(), page: chunk.page_numbers.first().copied(), anchor_id: Some(chunk.chunk_id.clone()), section_path: chunk.heading_context.clone(), text_hash: chunk.text_hash.clone(), ..Default::default() }]
}

fn material_type_from_semantic_category(category: &str) -> MaterialType {
    match category {
        "stat_block" => MaterialType::Npc,
        "table" | "data_entry" => MaterialType::AssetIndex,
        "rule_or_procedure" => MaterialType::Procedure,
        "npc_or_anomaly" => MaterialType::Npc,
        "location_or_scene" => MaterialType::Location,
        "scene" => MaterialType::Scene,
        "clue_or_handout" => MaterialType::Clue,
        _ => MaterialType::Other,
    }
}


fn rulebook_system_prompt() -> &'static str {
    "You are a TRPG rulebook compiler. Extract operational rules as structured, concise material. Preserve source page references. Do not quote long passages. Output valid JSON only."
}


/// Build the dedicated LLM client for the chargen compile pass: same provider/
/// base_url/key as the main client, but a stronger model (default gpt-5.4) for
/// reliable full-sheet extraction. `None` → caller falls back to the main client.
pub(crate) fn build_compiler_llm() -> Option<Arc<dyn LlmClient>> {
    let mut cfg = LlmConfig::from_env().ok()?;
    // gpt-5.4 (full, non-fast): reliable full-sheet extraction + fills tool-call
    // arguments. NOT a `-fast` variant (those bill more) and NOT codex-spark
    // (emits EMPTY tool args via this relay → unusable). Env-overridable.
    cfg.model = std::env::var("TRPG_CHARGEN_COMPILER_MODEL").unwrap_or_else(|_| "gpt-5.4".to_string());
    OpenAiCompatibleClient::new(cfg).ok().map(|c| Arc::new(c) as Arc<dyn LlmClient>)
}

/// Build the dedicated LLM client for the module reader (Pass A skeleton +
/// gleaning + Pass B deep extraction): same provider/base_url/key as the main
/// client, but quality-first model (default gpt-5.4). Module extraction runs in
/// the background (invisible to players), so quality is prioritised over speed
/// and is decoupled from the GM model. `None` → caller falls back to main client.
pub(crate) fn build_module_reader_llm() -> Option<Arc<dyn LlmClient>> {
    let mut cfg = LlmConfig::from_env().ok()?;
    cfg.model = std::env::var("TRPG_MODULE_READER_MODEL").unwrap_or_else(|_| "gpt-5.4".to_string());
    OpenAiCompatibleClient::new(cfg).ok().map(|c| Arc::new(c) as Arc<dyn LlmClient>)
}

/// Build the dedicated LLM client for the mechanics-catalog compile pass (the
/// rule agent's third pass): quality-first model (default gpt-5.4 — runs in the
/// background, once per ruleset; NOT a `-fast` variant — bills more — and NOT
/// codex-spark — empty tool args via this relay). `None` → caller falls back
/// to the main client.
pub(crate) fn build_mechanics_compiler_llm() -> Option<Arc<dyn LlmClient>> {
    let mut cfg = LlmConfig::from_env().ok()?;
    cfg.model = std::env::var("TRPG_MECHANICS_COMPILE_MODEL").unwrap_or_else(|_| "gpt-5.4".to_string());
    OpenAiCompatibleClient::new(cfg).ok().map(|c| Arc::new(c) as Arc<dyn LlmClient>)
}

/// The ONE wiring primitive both parse paths (non-staged `parse_rulebook`,
/// staged `persist_stage2_kernel`) share: gate + fail-closed wrapper around the
/// rule agent's third pass. Gate OFF → return untouched; client build failure →
/// fallback main client; the pass itself is fail-closed (no submit → kernel
/// untouched; round-trip guard reverts in full) and this layer adds a final
/// serialization belt: any non-panic failure leaves the kernel exactly as it
/// entered. Gaps land in tracing (observable, never interrupts the parse).
pub(crate) async fn compile_mechanics_into_kernel(
    fallback: &Arc<dyn LlmClient>,
    kernel: &mut RuleKernel,
    units: &[reader::Unit],
    sidecar_text: Option<String>,
    skill_names: Vec<String>,
) {
    if !mechanics_compile_enabled() {
        tracing::info!(ruleset = %kernel.ruleset_id, "mechanics catalog compile pass gated OFF (TRPG_MECHANICS_COMPILE)");
        return;
    }
    if units.is_empty() {
        tracing::info!(ruleset = %kernel.ruleset_id, "mechanics catalog compile pass skipped: no semantic units available");
        return;
    }
    let client = build_mechanics_compiler_llm().unwrap_or_else(|| fallback.clone());
    let before = kernel.clone();
    let ctx = reader::MechCompileCtx {
        units,
        sidecar_text,
        located_pages: String::new(),
        skill_names,
    };
    let gaps = reader::compile_mechanics_catalog(client.as_ref(), kernel, ctx, 14).await;
    if !gaps.is_empty() {
        tracing::info!(ruleset = %kernel.ruleset_id, ?gaps, "mechanics compiler gaps (kernel kept consistent; see validation_report)");
    }
    // Belt over the pass's own round-trip guard: a kernel this layer cannot
    // re-serialize must never reach upsert — restore the pre-pass state.
    if serde_json::to_value(&*kernel).is_err() {
        *kernel = before;
        tracing::warn!(ruleset = %kernel.ruleset_id, "mechanics compile left kernel unserializable; reverted to pre-pass state");
    } else {
        tracing::info!(ruleset = %kernel.ruleset_id, entries = kernel.mechanics_catalog.len(), "mechanics catalog compile pass finished");
    }
}

pub(crate) fn coerce_character_template(value: Value, ruleset_id: &str, title: &str) -> CharacterTemplate {
    let candidate = value.get("character_template")
        .or_else(|| value.get("template"))
        .or_else(|| value.get("data"))
        .cloned()
        .unwrap_or(value);

    if let Ok(mut template) = serde_json::from_value::<CharacterTemplate>(candidate.clone()) {
        if template.template_id.is_empty() {
            template.template_id = format!("{ruleset_id}.character_template.v1");
        }
        if template.ruleset_id.is_empty() {
            template.ruleset_id = ruleset_id.to_string();
        }
        if template.title.is_empty() {
            template.title = format!("{title} Character Template");
        }
        if template.fields.is_empty() {
            let fallback = fallback_character_template(ruleset_id, title);
            template.fields = fallback.fields;
            if template.sections.is_empty() || template.sections.iter().all(|s| s.field_ids.is_empty()) {
                template.sections = fallback.sections;
            }
            if template.creation_flow.is_empty() { template.creation_flow = fallback.creation_flow; }
            if template.validation_rules.is_empty() { template.validation_rules = fallback.validation_rules; }
        }
        return template;
    }

    let mut template = fallback_character_template(ruleset_id, title);
    if let Some(obj) = candidate.as_object() {
        if let Some(v) = obj.get("template_id").or_else(|| obj.get("id")).and_then(Value::as_str) {
            template.template_id = v.to_string();
        }
        if let Some(v) = obj.get("ruleset_id").and_then(Value::as_str) {
            template.ruleset_id = v.to_string();
        }
        if let Some(v) = obj.get("title").or_else(|| obj.get("name")).and_then(Value::as_str) {
            template.title = v.to_string();
        }
        if let Some(fields) = obj.get("fields").or_else(|| obj.get("character_fields")) {
            let parsed = parse_character_fields_flexible(fields);
            if !parsed.is_empty() {
                template.fields = parsed;
            }
        }
        if let Some(sections) = obj.get("sections").or_else(|| obj.get("character_sections")) {
            let parsed = parse_character_sections_flexible(sections);
            if !parsed.is_empty() {
                template.sections = parsed;
            }
        }
        if let Some(flow) = obj.get("creation_flow").or_else(|| obj.get("steps")) {
            let parsed = parse_creation_steps_flexible(flow);
            if !parsed.is_empty() {
                template.creation_flow = parsed;
            }
        }
        if let Some(derived) = obj.get("derived_values") {
            if let Ok(parsed) = serde_json::from_value::<Vec<DerivedValue>>(derived.clone()) {
                template.derived_values = parsed;
            }
        }
        if let Some(validation) = obj.get("validation_rules") {
            if let Ok(parsed) = serde_json::from_value::<Vec<CharacterValidationRule>>(validation.clone()) {
                template.validation_rules = parsed;
            }
        }
        if let Some(policy) = obj.get("llm_creation_policy") {
            template.llm_creation_policy = policy.clone();
        }
    }

    if template.sections.is_empty() && !template.fields.is_empty() {
        template.sections = vec![CharacterSection {
            section_id: "main".to_string(),
            title: "Main".to_string(),
            field_ids: template.fields.iter().map(|f| f.field_id.clone()).collect(),
        }];
    }
    template
}

fn reader_agent_enabled() -> bool {
    std::env::var("TRPG_READER_AGENT")
        .map(|v| !matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no"))
        .unwrap_or(true)
}

/// Mirror of `reader_agent_enabled` for the module reader (structured BP2/BP3
/// extraction). Defaults ON since the Phase 6 cross-structure e2e passed
/// (2026-06-09); opt out via TRPG_MODULE_READER=0.
fn module_reader_enabled() -> bool {
    std::env::var("TRPG_MODULE_READER")
        .map(|v| !matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no"))
        .unwrap_or(true)
}

/// Whether a cached module bundle can be reused as-is. With the module reader
/// enabled, an empty scene graph is a pre-reader (or failed-extraction)
/// artifact — `module_entry_scene_id` keys off `scenes`, so such a bundle can
/// never activate a session's entry scene and must be re-extracted. With the
/// reader disabled the old behavior stands (empty graphs are the norm there;
/// re-parsing would never improve them).
fn cached_module_bundle_reusable(graph: &ModuleGraph, reader_enabled: bool) -> bool {
    !reader_enabled || !graph.scenes.is_empty()
}

/// Gate for the mechanics-catalog compile pass (rule agent's third pass).
/// Same template as `module_reader_enabled` but the default is INVERTED:
/// unset / anything but "0"/"false"-family -> true (ON by default).
pub(crate) fn mechanics_compile_enabled() -> bool {
    std::env::var("TRPG_MECHANICS_COMPILE")
        .map(|v| !matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no"))
        .unwrap_or(true)
}

#[cfg(test)]
mod module_reader_gate_tests {
    use super::*;

    #[test]
    fn module_reader_gate_defaults_on_and_reads_env() {
        // All states run sequentially inside ONE test (no other test in this
        // binary touches TRPG_MODULE_READER); set then restore.
        let saved = std::env::var("TRPG_MODULE_READER").ok();
        std::env::remove_var("TRPG_MODULE_READER");
        assert!(module_reader_enabled(), "unset -> default ON (post Phase 6 e2e)");
        std::env::set_var("TRPG_MODULE_READER", "0");
        assert!(!module_reader_enabled(), "\"0\" -> OFF");
        std::env::set_var("TRPG_MODULE_READER", "1");
        assert!(module_reader_enabled(), "\"1\" -> ON");
        match saved {
            Some(v) => std::env::set_var("TRPG_MODULE_READER", v),
            None => std::env::remove_var("TRPG_MODULE_READER"),
        }
    }

    #[test]
    fn empty_scene_graph_cache_is_not_reusable_when_reader_enabled() {
        let empty = ModuleGraph::default();
        let mut populated = ModuleGraph::default();
        populated.scenes.push(ScenarioNode { node_id: "sc01".to_string(), ..Default::default() });
        assert!(!cached_module_bundle_reusable(&empty, true), "reader on + empty graph -> re-extract");
        assert!(cached_module_bundle_reusable(&populated, true), "reader on + scenes present -> reuse");
        assert!(cached_module_bundle_reusable(&empty, false), "reader off -> old behavior, reuse");
    }
}

#[cfg(test)]
mod mechanics_wiring_tests {
    use super::*;

    #[test]
    fn mechanics_gate_env_reads_three_states() {
        // All three states run sequentially inside ONE test (no other test in
        // this binary touches TRPG_MECHANICS_COMPILE); set then restore.
        let saved = std::env::var("TRPG_MECHANICS_COMPILE").ok();
        std::env::remove_var("TRPG_MECHANICS_COMPILE");
        assert!(mechanics_compile_enabled(), "unset -> default ON");
        std::env::set_var("TRPG_MECHANICS_COMPILE", "0");
        assert!(!mechanics_compile_enabled(), "\"0\" -> OFF");
        std::env::set_var("TRPG_MECHANICS_COMPILE", "false");
        assert!(!mechanics_compile_enabled(), "\"false\" -> OFF");
        std::env::set_var("TRPG_MECHANICS_COMPILE", "1");
        assert!(mechanics_compile_enabled(), "\"1\" -> ON");
        match saved {
            Some(v) => std::env::set_var("TRPG_MECHANICS_COMPILE", v),
            None => std::env::remove_var("TRPG_MECHANICS_COMPILE"),
        }
    }
}

/// Map the GmRunKit's structured `core.derived_formulas` into engine DerivedValues.
fn derived_values_from_run_kit(rk: &reader::GmRunKit) -> Vec<DerivedValue> {
    rk.core
        .derived_formulas
        .iter()
        .filter_map(|v| {
            let field_id = v.get("field_id").and_then(Value::as_str)?.to_string();
            let formula = v.get("formula").and_then(Value::as_str)?.to_string();
            if field_id.is_empty() || formula.is_empty() {
                return None;
            }
            let depends_on = v
                .get("depends_on")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let evaluator = v.get("evaluator").and_then(Value::as_str).unwrap_or("").to_string();
            let notes = v.get("notes").and_then(Value::as_str).map(String::from);
            Some(DerivedValue { field_id, formula, depends_on, evaluator, notes , ..Default::default() })
        })
        .collect()
}

/// Build the BP1 RuleKernel directly from the reader's GmRunKit. The kernel's
/// fields are the GmRunKit by another name; this is the near-1:1 mapping that
/// makes the reader's output drop straight into the engine's BP1 injection.
pub(crate) fn rule_kernel_from_run_kit(ruleset_id: &str, title: &str, rk: &reader::GmRunKit, character_pack: &CharacterOnboardingPack, object_schemas: Vec<Value>) -> RuleKernel {
    RuleKernel {
        kernel_id: format!("{ruleset_id}.rule_kernel.v1"),
        ruleset_id: ruleset_id.to_string(),
        version: "v1_reader".into(),
        game_identity: json!({"summary": rk.game_identity}),
        play_loop: json!({"gm_procedures": rk.gm_procedures, "subsystem_map": rk.subsystem_map}),
        dice_core: json!({"dice": rk.core.dice, "direction": rk.core.direction, "compare_to": rk.core.compare_to, "success_rule": rk.core.success_rule, "summary": rk.core_resolution, "compare": rk.core.compare, "target_face": rk.core.target_face, "success_threshold": rk.core.success_threshold, "target_number": rk.core.target_number, "success_bands": rk.core.success_bands}),
        check_model: json!({"summary": rk.core_resolution, "direction": rk.core.direction, "success_rule": rk.core.success_rule, "compare_to": rk.core.compare_to, "compare": rk.core.compare, "target_face": rk.core.target_face, "success_threshold": rk.core.success_threshold, "target_number": rk.core.target_number}),
        contest_models: Vec::new(),
        damage_effect_model: json!({"policy":"resolve damage/effects from the source-backed derived_formulas; if a required value is missing, block mechanical execution rather than inventing it","derived_formulas": rk.core.derived_formulas}),
        resource_tracks: rk.core.resource_tracks.clone(),
        object_schemas,
        mechanics_catalog: Vec::new(),
        character_sheet_schema: serde_json::to_value(&character_pack.sheet_template).unwrap_or_else(|_| json!({"title": title})),
        visibility_policy: json!({
            "gm_secret":"never place module secrets in BP1",
            "player_visible":"only rules summaries and player-facing character choices",
            "runtime_only":"rolls/effects/status deltas",
            "resolution_policy":"resolve uncertain actions via the core mechanic (a relevant skill/stat roll using dice_core); NEVER invent mechanical values; when an action implicates a flagged subsystem or a player invokes a named rule, retrieve the rule passage from the book before resolving"
        }),
        source_refs: character_pack.derived_formula_pack.source_refs.clone(),
        validation_report: ValidationReport { status: "ok".into(), ..Default::default() },
        ..Default::default()
    }
}

// --- Staged-orchestrator helpers (shared with `staged.rs`; DRY with parse_source) ---

/// The character skill field_ids the chargen/object passes couple to: the
/// template's own `skill` fields plus any skill titles from the reader's option
/// catalogs. Mirrors the inline extraction in `parse_rulebook` (lines ~547-575).
pub(crate) fn skill_ids(template: &CharacterTemplate, option_catalogs: &Value) -> Vec<String> {
    let mut skills: Vec<String> = template.fields.iter().filter(|f| f.field_type == "skill").map(|f| f.field_id.clone()).collect();
    let from_catalogs: Vec<String> = option_catalogs
        .as_array()
        .map(|cats| {
            cats.iter()
                .filter(|c| c.get("category").and_then(Value::as_str).map(|s| s.to_ascii_lowercase().contains("skill")).unwrap_or(false))
                .flat_map(|c| c.get("options").and_then(Value::as_array).cloned().unwrap_or_default())
                .filter_map(|o| o.get("title").and_then(Value::as_str).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    skills.extend(from_catalogs);
    skills.sort();
    skills.dedup();
    skills
}

/// Resource-track ids from the reader's `core.resource_tracks` values (the same
/// `id`/`name` extraction used inline in `parse_rulebook`, factored out for reuse).
pub(crate) fn track_ids(resource_tracks: &[Value]) -> Vec<String> {
    resource_tracks
        .iter()
        .filter_map(|t| t.get("id").or_else(|| t.get("name")).and_then(Value::as_str).map(String::from))
        .collect()
}

/// Build a minimal onboarding pack from the Stage-1 character slice (template +
/// option catalogs) without a `PlainTextBook` — the staged orchestrator does not
/// re-read the book at this point. Reuses the coerced template + parsed catalogs.
/// Deterministically wrap the template's ordered `creation_flow` (the CreationSteps
/// the reader already produced) into ONE structured `CharacterCreationFlow`, with
/// step order + ids preserved. No LLM, no book_map — usable inside the fast Stage-1
/// pack so even a thin onboarding pack carries a steps-bearing flow. Empty steps ->
/// no flow (the validator still flags it honestly, rather than fabricating one).
pub(crate) fn creation_flows_from_template(template: &CharacterTemplate) -> Vec<CharacterCreationFlow> {
    if template.creation_flow.is_empty() {
        return Vec::new();
    }
    let ruleset_id = template.ruleset_id.clone();
    vec![CharacterCreationFlow {
        flow_id: format!("{ruleset_id}.guided_creation.v1"),
        ruleset_id,
        title: "Guided playable character creation".into(),
        mode: CharacterCreationMode::Guided,
        supported_modes: vec![
            CharacterCreationMode::Guided,
            CharacterCreationMode::QuickStart,
            CharacterCreationMode::ImportExistingSheet,
        ],
        steps: template.creation_flow.clone(),
        decision_graph: vec![],
        required_tools: vec!["character_validator".into(), "source_backed_formula_resolver".into()],
        source_refs: template.source_refs.clone(),
        validation_profile: json!({"source_backed": true}),
    }]
}

pub(crate) fn stage1_onboarding_pack(ruleset_id: &str, title: &str, template: &CharacterTemplate, option_catalogs: &Value) -> CharacterOnboardingPack {
    let cats: Vec<CharacterOptionCatalog> = serde_json::from_value(option_catalogs.clone()).unwrap_or_default();
    let mut pack = CharacterOnboardingPack {
        pack_id: format!("{ruleset_id}.character_onboarding.v1"),
        ruleset_id: ruleset_id.to_string(),
        title: format!("{title} Character Onboarding Pack"),
        sheet_template: template.clone(),
        creation_flows: creation_flows_from_template(template),
        option_catalogs: cats,
        derived_formula_pack: DerivedFormulaPack {
            pack_id: format!("{ruleset_id}.derived_formulas.v1"),
            ruleset_id: ruleset_id.to_string(),
            formulas: template.derived_values.clone(),
            ..Default::default()
        },
        starter_character_pack: StarterCharacterPack::default(),
        import_mapping_profile: json!({"mode":"template_field_mapping"}),
        validation_profile: json!({}),
        runtime_binding_profile: json!({}),
        runtime_bindings: infer_character_runtime_bindings(template),
        source_refs: Vec::new(),
        validation_report: ValidationReport { status: "ok".into(), ..Default::default() },
    };
    pack.validation_report = validate_character_onboarding_pack(&pack);
    pack
}

/// Stage-1 persistence: write the onboarding-pack artifacts (sheet template +
/// option catalogs) under `parsed/characters/` and upsert the onboarding pack row
/// so `/character-template` can serve it before the deep stage finishes.
pub(crate) async fn persist_stage1_character_artifacts(db: &Db, data_dir: &Path, ruleset_id: &str, title: &str, template: &CharacterTemplate, option_catalogs: &Value) -> Result<()> {
    let pack = stage1_onboarding_pack(ruleset_id, title, template, option_catalogs);
    let dir = data_dir.join("parsed/characters");
    fs::create_dir_all(&dir).await?;
    fs::write(dir.join(format!("{ruleset_id}.character_onboarding_pack.json")), serde_json::to_string_pretty(&pack)?).await?;
    fs::write(dir.join(format!("{ruleset_id}.character_sheet_template.json")), serde_json::to_string_pretty(template)?).await?;
    fs::write(dir.join(format!("{ruleset_id}.character_option_catalogs.json")), serde_json::to_string_pretty(&pack.option_catalogs)?).await?;
    db.upsert_character_onboarding_pack(&pack).await?;
    Ok(())
}

/// Stage-2 persistence: assemble the full RuleKernel from the resolution+gm slice,
/// the (formula-compiled) character template, and the discovered object stubs, then
/// upsert it. Reuses `rule_kernel_from_run_kit` by reconstructing a GmRunKit.
pub(crate) async fn persist_stage2_kernel(
    db: &Db, ruleset_id: &str, title: &str, rg: Option<&reader::ResolutionGm>,
    template: &CharacterTemplate, option_catalogs: &Value, object_stubs: Vec<Value>,
    units: &[reader::Unit], sidecar_text: Option<String>, fallback_llm: &Arc<dyn LlmClient>,
) -> Result<()> {
    let pack = stage1_onboarding_pack(ruleset_id, title, template, option_catalogs);
    let run_kit = reader::GmRunKit {
        game_identity: rg.map(|r| r.game_identity.clone()).unwrap_or_default(),
        core_resolution: rg.map(|r| r.core_resolution.clone()).unwrap_or_default(),
        state_tracks: rg.map(|r| r.state_tracks.clone()).unwrap_or_default(),
        character: String::new(),
        subsystem_map: rg.map(|r| r.subsystem_map.clone()).unwrap_or_default(),
        gm_procedures: rg.map(|r| r.gm_procedures.clone()).unwrap_or_default(),
        source_pages: rg.map(|r| r.source_pages.clone()).unwrap_or_default(),
        core: rg.map(|r| r.core.clone()).unwrap_or_default(),
        character_template: serde_json::to_value(template).unwrap_or_default(),
        option_catalogs: option_catalogs.clone(),
    };
    let mut kernel = rule_kernel_from_run_kit(ruleset_id, title, &run_kit, &pack, object_stubs);
    // Stage-2 kernel must carry the formula-compiled template, not just the pack's.
    kernel.character_sheet_schema = serde_json::to_value(template).unwrap_or_default();
    kernel.version = "v1_staged".into();
    // Rule agent's THIRD pass (mechanics catalog) before the upsert — the
    // staged path's twin of the parse_rulebook wiring. Gated + fail-closed.
    let skills = skill_ids(template, option_catalogs);
    compile_mechanics_into_kernel(fallback_llm, &mut kernel, units, sidecar_text, skills).await;
    db.upsert_rule_kernel(&kernel).await?;
    Ok(())
}

/// Build the COMPLETE onboarding pack from a Stage-1 base + the compiled starter
/// pack. Pure (no DB) so the merge is unit-testable. An empty compiled pack `{}`
/// leaves the base starter pack untouched (fail-closed: never fabricate a build).
/// Re-validates so the returned pack carries an honest `validation_report`.
pub(crate) fn merge_starter_into_pack(mut pack: CharacterOnboardingPack, starter_pack: &Value) -> CharacterOnboardingPack {
    // `StarterCharacterPack.pack_id`/`ruleset_id` are NON-default required fields,
    // but the compiler (and `finalize_starter_pack`) emit only archetypes/shortcuts/
    // pregens — so inject stable identity ids before deserializing, else the whole
    // pack silently fails to parse and nothing lands. Empty `{}` stays empty (its
    // archetypes/shortcuts/pregens are all empty -> base starter untouched).
    let mut candidate = starter_pack.clone();
    if let Some(obj) = candidate.as_object_mut() {
        obj.entry("pack_id").or_insert_with(|| json!(format!("{}.starter_characters.v1", pack.ruleset_id)));
        obj.entry("ruleset_id").or_insert_with(|| json!(pack.ruleset_id.clone()));
        // The LLM emits `required_option_refs` as an OBJECT ({"occupation":["occupation"]})
        // or a string, but `RecommendedArchetype.required_option_refs` is `Vec<String>`.
        // `from_value::<StarterCharacterPack>` is all-or-nothing, so that one off-shape
        // field silently empties the WHOLE starter pack (3 archetypes compiled -> 0 landed).
        // Coerce each archetype's refs to a flat Vec<String> before deserializing.
        if let Some(arch) = obj.get_mut("archetypes").and_then(Value::as_array_mut) {
            for a in arch.iter_mut() {
                if let Some(ao) = a.as_object_mut() {
                    if let Some(refs) = ao.get("required_option_refs") {
                        let flat = flatten_str_ids(refs);
                        ao.insert("required_option_refs".into(), json!(flat));
                    }
                }
            }
        }
    }
    if let Ok(starter) = serde_json::from_value::<StarterCharacterPack>(candidate) {
        if !starter.archetypes.is_empty() || !starter.creation_shortcuts.is_empty() || !starter.pregens.is_empty() {
            let mut starter = starter;
            // Preserve the pack's stable identity even when the compiler omits ids.
            if starter.pack_id.is_empty() { starter.pack_id = format!("{}.starter_characters.v1", pack.ruleset_id); }
            if starter.ruleset_id.is_empty() { starter.ruleset_id = pack.ruleset_id.clone(); }
            pack.starter_character_pack = starter;
        }
    }
    pack.validation_report = validate_character_onboarding_pack(&pack);
    pack
}

/// Flatten an LLM-shaped id reference (a string, an array, or an object whose keys
/// AND/OR string leaves are ids) into a deduped, sorted `Vec<String>`. Tolerates the
/// `{"occupation":["occupation"]}` shape the starter-pack compiler emits.
fn flatten_str_ids(v: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    match v {
        Value::String(s) if !s.trim().is_empty() => out.push(s.trim().to_string()),
        Value::Array(a) => out.extend(a.iter().flat_map(flatten_str_ids)),
        Value::Object(o) => {
            for (k, val) in o {
                if !k.trim().is_empty() { out.push(k.trim().to_string()); }
                out.extend(flatten_str_ids(val));
            }
        }
        _ => {}
    }
    out.sort();
    out.dedup();
    out
}

/// Stage-2 onboarding persistence: build the COMPLETE `CharacterOnboardingPack`
/// (Stage-1 base — now carrying deterministic creation_flows — plus the compiled
/// `starter_pack`), re-validate, and re-upsert it. Independent + non-fatal at the
/// call site (mirrors the object-schema sub-step). The DB path is live-validated.
pub(crate) async fn persist_stage2_onboarding(db: &Db, ruleset_id: &str, title: &str, template: &CharacterTemplate, option_catalogs: &Value, starter_pack: &Value) -> Result<()> {
    let base = stage1_onboarding_pack(ruleset_id, title, template, option_catalogs);
    let pack = merge_starter_into_pack(base, starter_pack);
    db.upsert_character_onboarding_pack(&pack).await?;
    Ok(())
}

/// Best-effort Stage-2 module + reindex for one ruleset. The module reader and
/// search index live behind the synchronous `parse_all` path (owned by another
/// worker / the search crate); the staged orchestrator treats them as a black box
/// and this is intentionally a non-fatal no-op hook until that path is wired in.
pub(crate) async fn run_module_and_index_for_ruleset(_db: &Db, ruleset_id: &str) -> Result<()> {
    tracing::info!(ruleset = %ruleset_id, "staged: module+index deferred (best-effort, black-box)");
    Ok(())
}

#[cfg(test)]
mod reader_integration_tests {
    use super::*;

    #[test]
    fn reader_minimal_character_template_plugs_into_engine() {
        // The reader emits minimal CharacterField/CreationStep ({field_id,title,
        // field_type} / {step_id,title,prompt}) without required/repeatable/inputs.
        // Verify coerce_character_template still ingests them (not dropped to the
        // generic fallback) -> i.e. the format really plugs in.
        let v = json!({"character_template": {
            "fields": [
                {"field_id":"ref","title":"REF","field_type":"stat"},
                {"field_id":"handgun","title":"Handgun","field_type":"skill"},
                {"field_id":"hit_points_total","title":"HP","field_type":"derived"},
                {"field_id":"cash","title":"Cash","field_type":"resource"}
            ],
            "creation_flow": [
                {"step_id":"fill_stats","title":"Fill STATs","prompt":"record INT..EMP"},
                {"step_id":"fill_skills","title":"Skills"}
            ]
        }});
        let t = coerce_character_template(v, "cyberpunk_red", "Cyberpunk RED");
        assert!(t.fields.iter().any(|f| f.field_id == "ref" && f.field_type == "stat"), "stat field survived");
        assert!(t.fields.iter().any(|f| f.field_id == "hit_points_total"), "derived field survived");
        assert!(t.fields.iter().any(|f| f.field_id == "cash"), "resource field survived");
        assert!(t.creation_flow.iter().any(|s| s.step_id == "fill_stats"), "creation step survived");
        assert!(!t.template_id.is_empty() && !t.ruleset_id.is_empty());
    }

    #[test]
    fn derived_formulas_map_to_engine_derived_values() {
        let mut rk = reader::GmRunKit::default();
        rk.core.derived_formulas = vec![
            json!({"field_id":"standard_roll","formula":"6d4 count 3s","depends_on":["quality"],"evaluator":"pool_count","notes":"n"}),
            json!({"field_id":"ranged_attack","formula":"1d10+REF+skill"}), // minimal: no depends_on/evaluator
            json!({"formula":"missing field_id -> dropped"}),               // invalid -> filtered
        ];
        let formulas = derived_values_from_run_kit(&rk);
        assert_eq!(formulas.len(), 2);
        assert_eq!(formulas[0].field_id, "standard_roll");
        assert_eq!(formulas[0].depends_on, vec!["quality".to_string()]);
        assert_eq!(formulas[0].evaluator, "pool_count");
        assert_eq!(formulas[1].field_id, "ranged_attack");
        assert!(formulas[1].depends_on.is_empty());
    }
}

fn rule_kernel_from_onboarding(ruleset_id: &str, title: &str, onboarding: &GmOnboardingBundle, character_pack: &CharacterOnboardingPack, procedures: &[ProcedureDef]) -> RuleKernel {
    RuleKernel {
        kernel_id: format!("{ruleset_id}.rule_kernel.v1"),
        ruleset_id: ruleset_id.to_string(),
        version: "v1".into(),
        game_identity: onboarding.game_identity.clone(),
        play_loop: onboarding.play_loop.clone(),
        dice_core: json!({"starter_procedures": procedures.iter().map(|p| json!({"id": &p.procedure_id, "title": &p.title, "roll_model": &p.roll_model})).collect::<Vec<_>>() }),
        check_model: onboarding.ruleset_kernel.clone(),
        contest_models: procedures.iter().map(|p| json!({"procedure_id": &p.procedure_id, "title": &p.title, "inputs": &p.inputs, "outputs": &p.outputs, "roll_model": &p.roll_model})).collect(),
        damage_effect_model: json!({"policy":"resolve damage/effects only from source-backed facets; missing HP/DV/damage/SP/AC/SAN/Chaos/Harm blocks mechanical-ready execution"}),
        resource_tracks: json_value_array_from_character_pack(character_pack),
        object_schemas: Vec::new(),
        mechanics_catalog: Vec::new(),
        character_sheet_schema: serde_json::to_value(&character_pack.sheet_template).unwrap_or_else(|_| json!({"title": title})),
        visibility_policy: json!({"gm_secret":"never place module secrets in BP1", "player_visible":"only rules summaries and player-facing character choices", "runtime_only":"rolls/effects/status deltas"}),
        source_refs: onboarding.source_refs.clone(),
        validation_report: ValidationReport { status: "ok".into(), ..Default::default() },
        ..Default::default()
    }
}

fn json_value_array_from_character_pack(character_pack: &CharacterOnboardingPack) -> Vec<Value> {
    character_pack.sheet_template.fields.iter()
        .filter(|f| {
            let id = f.field_id.to_ascii_lowercase();
            id.contains("hp") || id.contains("san") || id.contains("mp") || id.contains("resource") || id.contains("harm") || id.contains("chaos") || id.contains("humanity")
        })
        .map(|f| json!({"field_id": f.field_id, "title": f.title, "field_type": f.field_type}))
        .collect()
}

fn rule_kernel_context_block(kernel: &RuleKernel) -> ContextBlock {
    let content = serde_json::to_string_pretty(kernel).unwrap_or_default();
    let mut block = ContextBlock::new(
        format!("ruleset.{}.rule_steward_kernel", kernel.ruleset_id),
        BlockKind::RuleStewardKernel,
        "Rule Steward BP1 Kernel",
        BlockContent::Markdown(content),
        Visibility::GmOnly,
        Stability::RarelyChanged,
        CacheZone::Prefix,
        Scope::ruleset(&kernel.ruleset_id),
        110,
    );
    block.tags = vec!["rule_kernel".into(), "bp1".into(), "rule_steward".into(), "source_backed".into()];
    block.source_refs = kernel.source_refs.clone();
    block
}

fn coerce_character_onboarding_pack(value: Value, ruleset_id: &str, title: &str, book: &PlainTextBook, book_map: &Value, template: &CharacterTemplate, procedures: &[ProcedureDef]) -> CharacterOnboardingPack {
    let candidate = value.get("character_onboarding_pack")
        .or_else(|| value.get("pack"))
        .or_else(|| value.get("data"))
        .cloned()
        .unwrap_or(value);

    if let Ok(mut pack) = serde_json::from_value::<CharacterOnboardingPack>(candidate.clone()) {
        normalize_character_onboarding_pack(&mut pack, ruleset_id, title, book, template, procedures);
        return pack;
    }

    let mut pack = fallback_character_onboarding_pack(ruleset_id, title, book, book_map, template, procedures);
    if let Some(obj) = candidate.as_object() {
        if let Some(v) = obj.get("pack_id").or_else(|| obj.get("id")).and_then(Value::as_str) { pack.pack_id = v.to_string(); }
        if let Some(v) = obj.get("ruleset_id").and_then(Value::as_str) { pack.ruleset_id = v.to_string(); }
        if let Some(v) = obj.get("title").or_else(|| obj.get("name")).and_then(Value::as_str) { pack.title = v.to_string(); }
        if let Some(sheet) = obj.get("sheet_template").or_else(|| obj.get("character_template")) {
            pack.sheet_template = coerce_character_template(sheet.clone(), ruleset_id, title);
        }
        if let Some(flows) = obj.get("creation_flows").or_else(|| obj.get("character_creation_flows")) {
            pack.creation_flows = parse_character_creation_flows_flexible(flows, ruleset_id, book);
        } else if let Some(flow) = obj.get("creation_flow").or_else(|| obj.get("steps")) {
            let steps = parse_creation_steps_flexible(flow);
            if !steps.is_empty() {
                pack.creation_flows = vec![CharacterCreationFlow {
                    flow_id: format!("{ruleset_id}.guided_creation"),
                    ruleset_id: ruleset_id.to_string(),
                    title: "Guided character creation".into(),
                    mode: CharacterCreationMode::Guided,
                    supported_modes: vec![CharacterCreationMode::Guided, CharacterCreationMode::QuickStart, CharacterCreationMode::ImportExistingSheet],
                    steps,
                    decision_graph: vec![],
                    required_tools: vec!["dice_tool".into(), "character_validator".into(), "source_backed_formula_resolver".into()],
                    source_refs: first_source_ref(book).into_iter().collect(),
                    validation_profile: json!({"source_backed": true}),
                }];
            }
        }
        if let Some(catalogs) = obj.get("option_catalogs") {
            if let Ok(parsed) = serde_json::from_value::<Vec<CharacterOptionCatalog>>(catalogs.clone()) { if !parsed.is_empty() { pack.option_catalogs = parsed; } }
        }
        if let Some(formulas) = obj.get("derived_formula_pack") {
            if let Ok(parsed) = serde_json::from_value::<DerivedFormulaPack>(formulas.clone()) { pack.derived_formula_pack = parsed; }
        } else if let Some(formulas) = obj.get("derived_values").or_else(|| obj.get("derived_formulas")) {
            if let Ok(parsed) = serde_json::from_value::<Vec<DerivedValue>>(formulas.clone()) {
                pack.derived_formula_pack.formulas = parsed;
            }
        }
        if let Some(starter) = obj.get("starter_character_pack") {
            if let Ok(parsed) = serde_json::from_value::<StarterCharacterPack>(starter.clone()) { pack.starter_character_pack = parsed; }
        }
        if let Some(v) = obj.get("import_mapping_profile") { pack.import_mapping_profile = v.clone(); }
        if let Some(v) = obj.get("validation_profile") { pack.validation_profile = v.clone(); }
        if let Some(v) = obj.get("runtime_binding_profile") { pack.runtime_binding_profile = v.clone(); }
        if let Some(v) = obj.get("runtime_bindings") {
            if let Ok(parsed) = serde_json::from_value::<Vec<CharacterRuntimeBinding>>(v.clone()) { pack.runtime_bindings = parsed; }
        }
    }
    normalize_character_onboarding_pack(&mut pack, ruleset_id, title, book, template, procedures);
    pack
}

fn normalize_character_onboarding_pack(pack: &mut CharacterOnboardingPack, ruleset_id: &str, title: &str, book: &PlainTextBook, template: &CharacterTemplate, procedures: &[ProcedureDef]) {
    if pack.pack_id.is_empty() { pack.pack_id = format!("{ruleset_id}.character_onboarding.v1"); }
    if pack.ruleset_id.is_empty() { pack.ruleset_id = ruleset_id.to_string(); }
    if pack.title.is_empty() { pack.title = format!("{title} Character Onboarding Pack"); }
    if pack.sheet_template.template_id.is_empty() { pack.sheet_template = template.clone(); }
    if pack.source_refs.is_empty() { pack.source_refs = first_source_ref(book).into_iter().collect(); }
    normalize_character_sheet_template_schema(&mut pack.sheet_template, ruleset_id, title, book);
    if pack.sheet_template.fields.is_empty() {
        let fallback = fallback_character_template(ruleset_id, title);
        pack.sheet_template.fields = fallback.fields;
        if pack.sheet_template.sections.is_empty() || pack.sheet_template.sections.iter().all(|s| s.field_ids.is_empty()) {
            pack.sheet_template.sections = fallback.sections;
        }
        if pack.sheet_template.creation_flow.is_empty() { pack.sheet_template.creation_flow = fallback.creation_flow; }
        if pack.sheet_template.validation_rules.is_empty() { pack.sheet_template.validation_rules = fallback.validation_rules; }
    }
    if pack.sheet_template.source_refs.is_empty() { pack.sheet_template.source_refs = pack.source_refs.clone(); }
    if pack.creation_flows.is_empty() {
        pack.creation_flows = default_character_creation_flows(ruleset_id, book, template, procedures);
    }
    if pack.derived_formula_pack.pack_id.is_empty() {
        pack.derived_formula_pack.pack_id = format!("{ruleset_id}.derived_formulas.v1");
    }
    if pack.derived_formula_pack.ruleset_id.is_empty() {
        pack.derived_formula_pack.ruleset_id = ruleset_id.to_string();
    }
    if pack.derived_formula_pack.formulas.is_empty() && !pack.sheet_template.derived_values.is_empty() {
        pack.derived_formula_pack.formulas = pack.sheet_template.derived_values.clone();
    }
    if pack.derived_formula_pack.formulas.is_empty() {
        let seeded = source_backed_first_play_mechanical_formulas(ruleset_id, book);
        if !seeded.is_empty() {
            pack.derived_formula_pack.formulas = seeded;
            if pack.derived_formula_pack.source_refs.is_empty() {
                pack.derived_formula_pack.source_refs = source_refs_for_first_play_mechanical_formulas(ruleset_id, book);
            }
            if pack.derived_formula_pack.precedence_rules.is_empty() {
                pack.derived_formula_pack.precedence_rules = vec![
                    "source-backed first-play mechanical formula seeds are usable for CheckContract/ContestProfile binding".into(),
                    "specific character, weapon, NPC, module, or table formulas override generic ruleset seeds".into(),
                    "missing actor/weapon/target facets remain unresolved_source_required; do not invent numeric values".into(),
                ];
            }
        }
    }
    if pack.derived_formula_pack.source_refs.is_empty() { pack.derived_formula_pack.source_refs = pack.source_refs.clone(); }
    if !pack.derived_formula_pack.formulas.is_empty()
        && (pack.derived_formula_pack.validation_report.status.is_empty()
            || (pack.derived_formula_pack.validation_report.status == "warning" && pack.derived_formula_pack.validation_report.errors.is_empty())) {
        pack.derived_formula_pack.validation_report = ValidationReport { status: "ok".into(), info: vec![ValidationMessage { code: "source_backed_formula_seeded".into(), message: "Rule Steward/parser seeded first-play mechanical formulas from located rulebook source pages.".into(), target: Some("derived_formula_pack.formulas".into()) }], ..Default::default() };
    }
    if pack.starter_character_pack.pack_id.is_empty() { pack.starter_character_pack.pack_id = format!("{ruleset_id}.starter_characters.v1"); }
    if pack.starter_character_pack.ruleset_id.is_empty() { pack.starter_character_pack.ruleset_id = ruleset_id.to_string(); }
    if pack.starter_character_pack.source_refs.is_empty() { pack.starter_character_pack.source_refs = pack.source_refs.clone(); }
    if pack.starter_character_pack.archetypes.is_empty() && pack.starter_character_pack.pregens.is_empty() {
        pack.starter_character_pack.archetypes = default_starter_archetypes(ruleset_id);
    }
    if pack.option_catalogs.is_empty() {
        pack.option_catalogs = vec![default_character_option_catalog(ruleset_id, book)];
    }
    if pack.import_mapping_profile.is_null() {
        pack.import_mapping_profile = json!({"mode":"map_input_fields_to_template", "unknown_fields":"preserve_for_review", "source_backed_mechanics_required": true});
    }
    if pack.validation_profile.is_null() {
        pack.validation_profile = json!({"required_fields": pack.sheet_template.fields.iter().filter(|f| f.required).map(|f| f.field_id.clone()).collect::<Vec<_>>(), "draft_status_when_missing_rules":"draft_needs_rules_source"});
    }
    if pack.runtime_binding_profile.is_null() {
        pack.runtime_binding_profile = json!({"policy":"bind character sheet fields to actor/object/ability parameter facets only when source-backed", "missing_values":"unresolved_source_required"});
    }
    if pack.runtime_bindings.is_empty() {
        pack.runtime_bindings = infer_character_runtime_bindings(&pack.sheet_template);
    }
    pack.validation_report = validate_character_onboarding_pack(pack);
}

fn source_refs_for_first_play_mechanical_formulas(ruleset_id: &str, book: &PlainTextBook) -> Vec<SourceRef> {
    let lower_ruleset = ruleset_id.to_ascii_lowercase();
    let keywords: Vec<&str> = if lower_ruleset.contains("cyberpunk") {
        vec!["resolving actions", "skills", "ranged combat", "damage", "armor", "dv", "friday night firefight"]
    } else if lower_ruleset.contains("dnd") {
        vec!["d20", "difficulty class", "armor class", "ability checks", "making an attack", "damage and healing"]
    } else if lower_ruleset.contains("sword_world") {
        vec!["skill check", "skill check method", "weapon attacks", "damage", "calculation of values"]
    } else if lower_ruleset.contains("cthulhu") || lower_ruleset.contains("coc") || lower_ruleset.contains("brp") {
        vec!["skill roll", "d100", "hit points", "damage", "sanity", "derived characteristics"]
    } else if lower_ruleset.contains("triangle") {
        vec!["four-sided dice", "conflict resolution", "chaos", "harm", "stability"]
    } else {
        vec!["skill check", "attack", "damage", "hit points", "derived", "character sheet"]
    };
    let mut refs = Vec::new();
    for page in &book.pages {
        let hay = page.text.to_ascii_lowercase();
        if keywords.iter().any(|kw| hay.contains(&kw.to_ascii_lowercase())) {
            refs.push(SourceRef {
                source_id: book.source_id.clone(),
                page: Some(page.page),
                anchor_id: Some(format!("{}:page:{}", book.source_id, page.page)),
                section_path: vec!["first_play_mechanical_formulas".into()],
                char_start: None,
                char_end: None,
                text_hash: Some(sha256_hex(&page.text)),
                note: Some("source_backed_first_play_formula_seed".into()),
            });
            if refs.len() >= 8 { break; }
        }
    }
    if refs.is_empty() { refs = first_source_ref(book).into_iter().collect(); }
    refs
}

fn source_backed_first_play_mechanical_formulas(ruleset_id: &str, book: &PlainTextBook) -> Vec<DerivedValue> {
    let ruleset = ruleset_id.to_ascii_lowercase();
    let pages = source_refs_for_first_play_mechanical_formulas(ruleset_id, book)
        .iter()
        .filter_map(|r| r.page.map(|p| p.to_string()))
        .collect::<Vec<_>>()
        .join(",");
    let note = |label: &str| Some(format!("source-backed first-play formula seed; verify exact parameters against source pages [{}]; {}", pages, label));
    // All entries produced here are provisional seeds — deterministic first-play
    // placeholders, not LLM-extracted source-backed formulas. Mark tier so that
    // combat/effect executors skip exact dice binding. (N1 seeded formula tiers)
    let seed = |field_id: &str, formula: &str, depends: Vec<&str>, evaluator: &str, n: Option<String>| DerivedValue {
        field_id: field_id.into(),
        formula: formula.into(),
        depends_on: depends.into_iter().map(|s| s.into()).collect(),
        evaluator: evaluator.into(),
        notes: n,
        tier: Some("provisional_seed".into()),
        ..Default::default()
    };
    if ruleset.contains("cyberpunk") {
        return vec![
            seed("mechanic.skill_check.total", "1d10 + STAT + Skill + modifiers vs Difficulty Value (DV)", vec!["stat","skill","modifiers","dv"], "contest_profile_static_dv_or_source_bound_dv", note("Cyberpunk RED core skill resolution")),
            seed("mechanic.attack.ranged.total", "1d10 + REF + relevant weapon skill + modifiers vs source-bound range DV / target defense", vec!["ref","weapon_skill","range_dv","target_defense","modifiers"], "attack_vs_defense_or_dv", note("ranged attack requires actor, weapon, range, and target facets")),
            seed("mechanic.damage.weapon_effect", "weapon damage expression from equipped weapon; apply armor/SP mitigation and write remaining damage to target HP or source-bound resource", vec!["weapon.damage","target.armor_sp","target.hp"], "effect_resolution_packet", note("damage is source-object/equipment driven; do not invent weapon damage")),
        ];
    }
    if ruleset.contains("dnd") {
        return vec![
            seed("mechanic.d20_check.total", "1d20 + ability modifier + proficiency bonus + modifiers vs DC", vec!["ability_modifier","proficiency_bonus","dc"], "static_dc_contest", note("D&D d20 check")),
            seed("mechanic.attack.total", "1d20 + ability modifier + proficiency bonus + modifiers vs AC", vec!["ability_modifier","proficiency_bonus","target.ac"], "attack_vs_ac", note("D&D attack roll")),
            seed("mechanic.damage.weapon_or_spell", "damage dice from weapon/spell/ability source; apply resistance/immunity/vulnerability then write HP delta", vec!["source.damage","target.hp","mitigation"], "effect_resolution_packet", note("D&D source-bound damage")),
        ];
    }
    if ruleset.contains("sword_world") {
        return vec![
            seed("mechanic.skill_check.total", "2d6 + skill package standard value vs target number", vec!["class_level","ability_modifier","target_number"], "static_target_or_opposed_2d6", note("Sword World skill check")),
            seed("mechanic.damage.power_table", "source-bound Power Table result + additional damage, then apply mitigation/defense as specified by source", vec!["weapon_or_spell_power","additional_damage","target_defense"], "table_driven_effect_resolution", note("Sword World damage remains table-driven; exact table lookup required")),
        ];
    }
    if ruleset.contains("cthulhu") || ruleset.contains("brp") {
        return vec![
            seed("mechanic.percentile_check", "d100 roll-under against skill/characteristic rating", vec!["skill_rating"], "percentile_roll_under", note("BRP/CoC percentile check")),
            seed("mechanic.hit_points", "source-backed HP formula from CON/SIZ or ruleset-specific investigator sheet", vec!["con","siz"], "character_derived_value", note("HP formula must be confirmed per ruleset/version")),
            seed("mechanic.sanity", "source-backed SAN resource; losses write to sanity track, not HP", vec!["sanity","san_loss"], "parameter_facet_executor", note("SAN is a separate parameter/resource")),
        ];
    }
    if ruleset.contains("triangle") {
        return vec![
            seed("mechanic.conflict_roll", "roll 6d4 and evaluate 3s according to Triangle Agency conflict resolution", vec!["six_d4","threes","chaos"], "triangle_conflict_resolution", note("Triangle Agency 6d4 conflict core")),
            seed("mechanic.harm_chaos", "source-bound Harm/Chaos/Stability effects write to their tracks instead of HP", vec!["harm","chaos","stability"], "parameter_facet_executor", note("Triangle resource tracks")),
        ];
    }
    vec![
        seed("mechanic.core_check", "ruleset source-backed dice expression + actor facet + target/opposition facet", vec!["dice","actor_facet","target_facet"], "contest_profile", note("generic source-backed first-play check")),
        seed("mechanic.damage_or_effect", "source-bound effect expression writes to source-bound resource/condition/object state", vec!["effect_source","target_parameter"], "effect_resolution_packet", note("generic source-bound effect")),
    ]
}

fn normalize_character_sheet_template_schema(template: &mut CharacterTemplate, ruleset_id: &str, title: &str, book: &PlainTextBook) {
    if template.template_id.is_empty() { template.template_id = format!("{ruleset_id}.character_template.v1"); }
    if template.ruleset_id.is_empty() { template.ruleset_id = ruleset_id.to_string(); }
    if template.title.is_empty() { template.title = format!("{title} Character Sheet"); }
    if template.source_refs.is_empty() { template.source_refs = character_schema_source_refs(book); }
    if template.fields.is_empty() {
        template.fields = default_character_fields_for_ruleset(ruleset_id);
    }
    let missing_or_empty_sections = template.sections.is_empty() || template.sections.iter().all(|s| s.field_ids.is_empty());
    if missing_or_empty_sections && !template.fields.is_empty() {
        template.sections = default_character_sections_for_fields(ruleset_id, &template.fields);
    }
}

fn character_schema_source_refs(book: &PlainTextBook) -> Vec<SourceRef> {
    let keywords = ["character sheet", "copy character sheet", "how to read character sheet", "what are statistics", "skills", "creating a character", "character creation"];
    source_refs_for_keywords(book, &keywords, 8)
}

fn default_character_fields_for_ruleset(ruleset_id: &str) -> Vec<CharacterField> {
    let mut fields = vec![
        field("character_name", "Character Name", "string", true),
        field("concept", "Concept", "string", false),
    ];
    if ruleset_id.contains("cyberpunk") {
        fields.extend([
            field("role", "Role", "choice", true),
            field("lifepath", "Lifepath", "object", false),
            field("stats", "Statistics", "map:number", true),
            field("skills", "Skills", "map:number", true),
            field("weapons", "Weapons", "list", false),
            field("armor", "Armor / SP", "object", false),
            field("hp", "Hit Points", "resource", false),
            field("humanity", "Humanity", "resource", false),
            field("cyberware", "Cyberware", "list", false),
            field("gear", "Gear", "list", false),
        ]);
    } else if ruleset_id.contains("dnd") {
        fields.extend([
            field("race", "Race", "choice", true),
            field("class", "Class", "choice", true),
            field("background", "Background", "choice", true),
            field("ability_scores", "Ability Scores", "map:number", true),
            field("proficiencies", "Proficiencies", "list", false),
            field("skills", "Skills", "map:number", false),
            field("armor_class", "Armor Class", "number", false),
            field("hit_points", "Hit Points", "resource", false),
            field("equipment", "Equipment", "list", false),
            field("spells", "Spells", "list", false),
        ]);
    } else if ruleset_id.contains("sword_world") {
        fields.extend([
            field("race", "Race", "choice", true),
            field("background", "Background", "choice", true),
            field("ability_scores", "Ability Scores", "map:number", true),
            field("classes", "Classes", "map:number", true),
            field("skills", "Skill Packages", "map:number", false),
            field("combat_feats", "Combat Feats", "list", false),
            field("weapons", "Weapons", "list", false),
            field("armor", "Armor", "object", false),
            field("hp", "HP", "resource", false),
            field("mp", "MP", "resource", false),
            field("languages", "Languages", "list", false),
        ]);
    } else if ruleset_id.contains("coc") || ruleset_id.contains("cthulhu") || ruleset_id.contains("brp") {
        fields.extend([
            field("occupation", "Occupation / Profession", "choice", true),
            field("characteristics", "Characteristics", "map:number", true),
            field("skills", "Skills", "map:percent", true),
            field("hit_points", "Hit Points", "resource", false),
            field("magic_points", "Magic / Power Points", "resource", false),
            field("sanity", "Sanity", "resource", false),
            field("equipment", "Equipment", "list", false),
        ]);
    } else if ruleset_id.contains("triangle") {
        fields.extend([
            field("arc", "ARC", "choice", true),
            field("competency", "Competency", "choice", true),
            field("qualities", "Qualities", "list", false),
            field("chaos", "Chaos", "resource", false),
            field("harm", "Harm", "resource", false),
            field("commendations", "Commendations", "resource", false),
            field("demerits", "Demerits", "resource", false),
            field("requisitions", "Requisitions", "list", false),
        ]);
    }
    fields
}

fn field(field_id: &str, title: &str, field_type: &str, required: bool) -> CharacterField {
    CharacterField { field_id: field_id.into(), title: title.into(), field_type: field_type.into(), required, repeatable: false, choices_material_id: None, default_value: None, visibility: Some(Visibility::Public), notes: Some("source-backed schema fallback; exact legal values remain locator/on-demand until extracted".into()) }
}

fn default_character_sections_for_fields(ruleset_id: &str, fields: &[CharacterField]) -> Vec<CharacterSection> {
    let ids = |needles: &[&str]| fields.iter().filter(|f| needles.iter().any(|n| f.field_id.contains(n))).map(|f| f.field_id.clone()).collect::<Vec<_>>();
    let mut sections = vec![CharacterSection { section_id: "identity".into(), title: "Identity".into(), field_ids: ids(&["name", "concept", "background", "lifepath", "occupation", "role", "race", "class", "arc", "competency"]) }];
    sections.push(CharacterSection { section_id: "mechanics".into(), title: "Core Mechanics".into(), field_ids: ids(&["stat", "ability", "characteristic", "skill", "proficiency", "class"]) });
    sections.push(CharacterSection { section_id: "resources".into(), title: "Resources and State".into(), field_ids: ids(&["hp", "hit_points", "mp", "magic", "power", "sanity", "humanity", "chaos", "harm", "commend", "demerit"]) });
    sections.push(CharacterSection { section_id: "equipment".into(), title: "Equipment and Abilities".into(), field_ids: ids(&["weapon", "armor", "equipment", "gear", "cyberware", "spell", "feat", "quality", "requisition"]) });
    sections.retain(|s| !s.field_ids.is_empty());
    if sections.is_empty() {
        sections.push(CharacterSection { section_id: format!("{ruleset_id}.main"), title: "Character Sheet".into(), field_ids: fields.iter().map(|f| f.field_id.clone()).collect() });
    }
    sections
}

fn source_backed_mechanical_formulas_from_book(ruleset_id: &str, book: &PlainTextBook) -> Vec<DerivedValue> {
    let mut formulas = Vec::new();
    let ruleset = ruleset_id.to_ascii_lowercase();
    if ruleset.contains("cyberpunk") {
        if has_any_source(book, &["resolving actions with skills", "getting it done", "skill check", "difficulty value", "dv"]) {
            formulas.push(derived("cyberpunk_red.core_skill_check", "1d10 + STAT + Skill + situational modifiers vs DV", &["stat", "skill", "dv"], "ruleset_formula", "Source-backed locator formula for Cyberpunk RED skill checks; concrete STAT/Skill/DV must be materialized from character/scene facets before final mechanical resolution."));
        }
        if has_any_source(book, &["ranged combat", "ranged attack", "ref", "handgun", "dv"]) {
            formulas.push(derived("cyberpunk_red.ranged_attack_check", "1d10 + REF + relevant weapon skill + modifiers vs range DV", &["ref", "weapon_skill", "range_dv"], "ruleset_formula", "Attack formula locator. Target-specific range DV and actor weapon skill remain source-backed facets."));
        }
        if has_any_source(book, &["weapons and armor", "heavy pistol", "very heavy pistol", "weapon", "damage"]) {
            formulas.push(derived("cyberpunk_red.weapon_damage_from_table", "weapon damage expression from source-backed weapon table", &["weapon", "damage_table"], "table_lookup", "Damage must be looked up from the exact weapon table or equipped weapon facet; never invent damage dice."));
        }
        if has_any_source(book, &["before you take damage", "when armor doesn't cut it", "armor", "sp", "ablation"]) {
            formulas.push(derived("cyberpunk_red.armor_sp_mitigation", "damage is mitigated by armor/SP before HP/resource impact; armor degradation follows source rules", &["armor_sp", "damage", "hp"], "effect_formula", "Armor/SP interaction locator; exact ablation and damage-through behavior requires source-backed armor facet."));
        }
    } else if ruleset.contains("dnd") {
        if has_any_source(book, &["the d20", "ability checks", "attack rolls", "saving throws", "difficulty class", "armor class"]) {
            formulas.push(derived("dnd5e.d20_check", "1d20 + ability modifier + proficiency/modifiers vs DC or AC", &["ability_modifier", "proficiency", "target_dc_or_ac"], "ruleset_formula", "D&D 5e core d20 resolution formula."));
            formulas.push(derived("dnd5e.attack_vs_ac", "1d20 + attack modifier vs target AC", &["attack_modifier", "ac"], "attack_formula", "Attack formula; weapon/spell damage remains a source-backed facet."));
        }
    } else if ruleset.contains("sword_world") {
        if has_any_source(book, &["skill check method", "skill checks", "2d6", "target number", "standard value"]) {
            formulas.push(derived("sword_world_2_5.skill_check", "2d6 + standard value vs target number", &["class_level", "ability_modifier", "target_number"], "ruleset_formula", "Sword World 2.5 core check formula."));
        }
        if has_any_source(book, &["damage", "power table", "weapon attacks", "armor"]) {
            formulas.push(derived("sword_world_2_5.damage_power_table", "damage uses source-backed weapon/spell power table plus modifiers; armor/defense applied by source rules", &["power_table", "weapon", "armor"], "table_lookup", "Damage is table-driven and must not be invented."));
        }
    } else if ruleset.contains("coc") || ruleset.contains("cthulhu") || ruleset.contains("brp") {
        if has_any_source(book, &["d100", "percentile", "skill roll", "success", "special success", "critical success"]) {
            formulas.push(derived("brp.percentile_roll_under", "1d100 roll-under ability/skill rating; special and critical thresholds from source rules", &["skill_rating", "success_level"], "ruleset_formula", "BRP/CoC percentile resolution."));
        }
        if has_any_source(book, &["hit points", "sanity", "major wound", "damage"]) {
            formulas.push(derived("brp.resource_tracks", "damage/SAN/resource effects apply to source-backed tracks such as HP, SAN, major wound, or power points", &["hp", "sanity", "damage"], "effect_formula", "Resource formulas must be resolved from character sheet/source."));
        }
    } else if ruleset.contains("triangle") {
        if has_any_source(book, &["using four-sided dice", "6 four-sided dice", "conflict resolution", "chaos", "harm"]) {
            formulas.push(derived("triangle_agency.conflict_resolution", "roll 6d4 and interpret 3s with Chaos/Harm rules", &["6d4", "chaos", "harm"], "ruleset_formula", "Triangle Agency conflict formula locator."));
        }
    }
    formulas
}

fn derived(field_id: &str, formula: &str, depends_on: &[&str], evaluator: &str, notes: &str) -> DerivedValue {
    DerivedValue { field_id: field_id.into(), formula: formula.into(), depends_on: depends_on.iter().map(|s| s.to_string()).collect(), evaluator: evaluator.into(), notes: Some(notes.into()) , ..Default::default() }
}

fn merge_derived_formulas(dest: &mut Vec<DerivedValue>, incoming: Vec<DerivedValue>) {
    let mut seen = dest.iter().map(|d| d.field_id.clone()).collect::<std::collections::BTreeSet<_>>();
    for formula in incoming {
        if seen.insert(formula.field_id.clone()) { dest.push(formula); }
    }
}

fn mechanical_formula_source_refs(ruleset_id: &str, book: &PlainTextBook) -> Vec<SourceRef> {
    let ruleset = ruleset_id.to_ascii_lowercase();
    let keywords: Vec<&str> = if ruleset.contains("cyberpunk") {
        vec!["resolving actions with skills", "ranged combat", "weapons and armor", "before you take damage", "armor", "dv", "damage"]
    } else if ruleset.contains("dnd") {
        vec!["the d20", "ability checks", "attack rolls", "difficulty class", "armor class", "damage"]
    } else if ruleset.contains("sword_world") {
        vec!["skill check method", "2d6", "weapon attacks", "damage", "calculation of values"]
    } else if ruleset.contains("coc") || ruleset.contains("cthulhu") || ruleset.contains("brp") {
        vec!["d100", "percentile", "skill roll", "hit points", "sanity", "major wound"]
    } else if ruleset.contains("triangle") {
        vec!["using four-sided dice", "conflict resolution", "chaos", "harm"]
    } else {
        vec!["skill check", "combat", "damage", "character sheet"]
    };
    source_refs_for_keywords(book, &keywords, 12)
}

fn has_any_source(book: &PlainTextBook, keywords: &[&str]) -> bool {
    book.pages.iter().any(|page| {
        let lower = page.text.to_ascii_lowercase();
        keywords.iter().any(|kw| lower.contains(&kw.to_ascii_lowercase()))
    })
}

fn source_refs_for_keywords(book: &PlainTextBook, keywords: &[&str], limit: usize) -> Vec<SourceRef> {
    let mut refs = Vec::new();
    for page in &book.pages {
        let lower = page.text.to_ascii_lowercase();
        if keywords.iter().any(|kw| lower.contains(&kw.to_ascii_lowercase())) {
            refs.push(SourceRef { source_id: book.source_id.clone(), page: Some(page.page), anchor_id: Some(format!("{}:page:{}", book.source_id, page.page)), section_path: vec![], char_start: None, char_end: None, text_hash: Some(sha256_hex(&page.text)), note: Some("source_backed_mechanical_formula_locator".into()) });
            if refs.len() >= limit { break; }
        }
    }
    if refs.is_empty() { first_source_ref(book).into_iter().collect() } else { refs }
}

fn parse_character_creation_flows_flexible(value: &Value, ruleset_id: &str, book: &PlainTextBook) -> Vec<CharacterCreationFlow> {
    if let Ok(flows) = serde_json::from_value::<Vec<CharacterCreationFlow>>(value.clone()) { return flows; }
    let mut out = Vec::new();
    if let Some(arr) = value.as_array() {
        for (idx, item) in arr.iter().enumerate() {
            if let Ok(flow) = serde_json::from_value::<CharacterCreationFlow>(item.clone()) {
                out.push(flow);
                continue;
            }
            let obj = item.as_object();
            let flow_id = obj.and_then(|o| o.get("flow_id").or_else(|| o.get("id")).and_then(Value::as_str)).map(sanitize_id).unwrap_or_else(|| format!("{ruleset_id}.creation_flow_{}", idx + 1));
            let title = obj.and_then(|o| o.get("title").or_else(|| o.get("name")).and_then(Value::as_str)).unwrap_or("Character creation flow").to_string();
            let mode = obj.and_then(|o| o.get("mode").and_then(Value::as_str)).map(parse_character_creation_mode).unwrap_or_default();
            let steps = obj.and_then(|o| o.get("steps").or_else(|| o.get("creation_flow"))).map(parse_creation_steps_flexible).unwrap_or_default();
            out.push(CharacterCreationFlow { flow_id, ruleset_id: ruleset_id.to_string(), title, mode, supported_modes: vec![CharacterCreationMode::Guided, CharacterCreationMode::QuickStart, CharacterCreationMode::ImportExistingSheet], steps, decision_graph: vec![], required_tools: vec!["dice_tool".into(), "character_validator".into()], source_refs: first_source_ref(book).into_iter().collect(), validation_profile: json!({"source_backed": true}) });
        }
    }
    out
}

fn parse_character_creation_mode(input: &str) -> CharacterCreationMode {
    match input.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "pregen" | "pregenerated" | "sample" => CharacterCreationMode::Pregenerated,
        "quick" | "quick_start" | "easy" | "easy_creation" => CharacterCreationMode::QuickStart,
        "detailed" | "full" => CharacterCreationMode::Detailed,
        "import" | "import_existing_sheet" => CharacterCreationMode::ImportExistingSheet,
        "random" | "randomized" => CharacterCreationMode::Randomized,
        "high_level" | "advanced" => CharacterCreationMode::HighLevel,
        _ => CharacterCreationMode::Guided,
    }
}

fn fallback_character_onboarding_pack(ruleset_id: &str, title: &str, book: &PlainTextBook, book_map: &Value, template: &CharacterTemplate, procedures: &[ProcedureDef]) -> CharacterOnboardingPack {
    let source_refs = first_source_ref(book).into_iter().collect::<Vec<_>>();
    let mut pack = CharacterOnboardingPack {
        pack_id: format!("{ruleset_id}.character_onboarding.v1"),
        ruleset_id: ruleset_id.to_string(),
        title: format!("{title} Character Onboarding Pack"),
        sheet_template: template.clone(),
        creation_flows: default_character_creation_flows(ruleset_id, book, template, procedures),
        option_catalogs: vec![default_character_option_catalog_from_book_map(ruleset_id, book, book_map)],
        derived_formula_pack: DerivedFormulaPack {
            pack_id: format!("{ruleset_id}.derived_formulas.v1"),
            ruleset_id: ruleset_id.to_string(),
            formulas: template.derived_values.clone(),
            precedence_rules: vec!["source-backed explicit formula beats inferred field naming".into(), "missing formulas leave draft_needs_rules_source".into()],
            source_refs: source_refs.clone(),
            validation_report: ValidationReport { status: if template.derived_values.is_empty() { "warning".into() } else { "ok".into() }, warnings: if template.derived_values.is_empty() { vec![ValidationMessage { code: "derived_formula_pack_partial".into(), message: "No explicit derived formulas were extracted; character finalization should treat derived mechanics as unresolved until looked up.".into(), target: None }] } else { vec![] }, ..Default::default() },
        },
        starter_character_pack: StarterCharacterPack {
            pack_id: format!("{ruleset_id}.starter_characters.v1"),
            ruleset_id: ruleset_id.to_string(),
            module_id: None,
            pregens: pregens_from_book(book, ruleset_id),
            archetypes: default_starter_archetypes(ruleset_id),
            creation_shortcuts: vec![CreationShortcut { shortcut_id: "quick_start_guided".into(), title: "Quick start guided creation".into(), mode: CharacterCreationMode::QuickStart, description: "Ask for concept/role preference, choose only starter-legal options, then validate source-backed mechanics before play.".into(), source_refs: source_refs.clone() }],
            module_fit_notes: vec![],
            source_refs: source_refs.clone(),
        },
        import_mapping_profile: json!({"mode":"template_field_mapping", "unknown_fields":"preserve_for_review", "mechanical_unknowns":"unresolved_source_required"}),
        validation_profile: json!({"required_fields": template.fields.iter().filter(|f| f.required).map(|f| f.field_id.clone()).collect::<Vec<_>>(), "missing_mechanical_fields":"block_mechanical_ready"}),
        runtime_binding_profile: json!({"policy":"bind source-backed character fields into actor_mechanical_states/generic_parameter_states/object/ability facets"}),
        runtime_bindings: infer_character_runtime_bindings(template),
        source_refs,
        validation_report: ValidationReport { status: "ok".into(), ..Default::default() },
    };
    pack.validation_report = validate_character_onboarding_pack(&pack);
    pack
}

fn default_character_creation_flows(ruleset_id: &str, book: &PlainTextBook, template: &CharacterTemplate, procedures: &[ProcedureDef]) -> Vec<CharacterCreationFlow> {
    let mut steps = if !template.creation_flow.is_empty() { template.creation_flow.clone() } else { fallback_character_template(ruleset_id, &book.title).creation_flow };
    if steps.iter().all(|s| s.step_id != "validate_and_bind") {
        steps.push(CreationStep { step_id: "validate_and_bind".into(), title: "Validate, calculate, and bind runtime state".into(), required: true, prompt: Some("Check required fields, calculate source-backed derived values, then bind HP/resources/equipment/abilities into runtime state.".into()), inputs: vec!["character_draft".into()], outputs: vec!["playable_character".into(), "runtime_bindings".into()], source_refs: first_source_ref(book).into_iter().collect() });
    }
    let mut required_tools = vec!["character_validator".into(), "source_backed_formula_resolver".into(), "parameter_facet_binder".into()];
    if procedures.iter().any(|p| serde_json::to_string(&p.roll_model).unwrap_or_default().contains("D") || p.title.to_ascii_lowercase().contains("roll")) {
        required_tools.push("dice_tool".into());
    }
    vec![CharacterCreationFlow { flow_id: format!("{ruleset_id}.guided_creation.v1"), ruleset_id: ruleset_id.to_string(), title: "Guided playable character creation".into(), mode: CharacterCreationMode::Guided, supported_modes: vec![CharacterCreationMode::Pregenerated, CharacterCreationMode::QuickStart, CharacterCreationMode::Guided, CharacterCreationMode::ImportExistingSheet], steps, decision_graph: vec![], required_tools, source_refs: first_source_ref(book).into_iter().collect(), validation_profile: json!({"mechanical_ready_requires_source_backed_derived_values": true}) }]
}

fn default_character_option_catalog(ruleset_id: &str, book: &PlainTextBook) -> CharacterOptionCatalog {
    let map = build_book_map(book);
    default_character_option_catalog_from_book_map(ruleset_id, book, &map)
}

fn default_character_option_catalog_from_book_map(ruleset_id: &str, book: &PlainTextBook, book_map: &Value) -> CharacterOptionCatalog {
    let mut groups = Vec::new();
    let heading_entries = book_map.get("headings").or_else(|| book_map.get("entries")).and_then(Value::as_array).cloned().unwrap_or_default();
    let categories: Vec<(&str, Vec<&str>)> = vec![
        ("origin", vec!["race", "ancestry", "origin", "species"]),
        ("role_or_class", vec!["class", "role", "profession", "career", "archetype", "arc", "competency"]),
        ("background", vec!["background", "lifepath", "personality"]),
        ("skills", vec!["skill", "proficiency", "quality"]),
        ("equipment", vec!["equipment", "weapon", "armor", "gear", "requisition"]),
        ("abilities", vec!["spell", "power", "ability", "feat", "cyberware", "technique"]),
    ];
    for (category, terms) in categories {
        let mut locators = Vec::new();
        for entry in &heading_entries {
            let label = entry.get("heading").or_else(|| entry.get("label")).and_then(Value::as_str).unwrap_or_default();
            let lower = label.to_ascii_lowercase();
            if terms.iter().any(|term| lower.contains(*term)) {
                let page = entry.get("page").or_else(|| entry.get("page_start")).and_then(Value::as_u64).map(|v| v as u32);
                locators.push(BookLocatorEntry { locator_id: format!("{ruleset_id}.chargen.{}.{}", category, sanitize_id(label)), owner_id: ruleset_id.to_string(), owner_kind: "ruleset".into(), label: label.to_string(), category: category.into(), source_document_id: book.source_id.clone(), page_start: page, page_end: page, heading_path: vec![label.to_string()], search_terms: vec![label.to_string(), category.to_string()], summary: format!("Character option locator for {label}"), confidence: 0.65, parse_policy: "on_demand".into(), tags: vec!["character_option".into(), category.into()], source_refs: first_source_ref(book).into_iter().collect() });
            }
        }
        if !locators.is_empty() {
            groups.push(CharacterOptionGroup { group_id: format!("{ruleset_id}.{category}"), title: category.replace('_', " "), category: category.into(), starter_legal: true, options: vec![], locators, source_refs: first_source_ref(book).into_iter().collect() });
        }
    }
    CharacterOptionCatalog { catalog_id: format!("{ruleset_id}.character_options.v1"), ruleset_id: ruleset_id.to_string(), title: "Character option locator catalog".into(), option_groups: groups, option_locators: vec![], compatibility_rules: vec![], module_recommendations: vec![], source_refs: first_source_ref(book).into_iter().collect() }
}

fn pregens_from_book(book: &PlainTextBook, ruleset_id: &str) -> Vec<PregenCharacterRef> {
    let mut pregens = Vec::new();
    for page in &book.pages {
        let lower = page.text.to_ascii_lowercase();
        if lower.contains("sample character") || lower.contains("pregenerated") || lower.contains("pre-generated") || lower.contains("ready-to-play") || lower.contains("easy creation") {
            let title = page.text.lines().find(|l| {
                let ll = l.to_ascii_lowercase();
                !l.trim().is_empty() && (ll.contains("sample") || ll.contains("character") || ll.contains("creation"))
            }).unwrap_or("Sample / pre-generated character locator").trim().to_string();
            pregens.push(PregenCharacterRef { pregen_id: format!("{ruleset_id}.pregen.page_{}", page.page), title, summary: take_tail_chars(&page.text, 600), sheet_ref: None, source_refs: vec![SourceRef { source_id: book.source_id.clone(), page: Some(page.page), section_path: vec!["character_creation".into()], text_hash: Some(sha256_hex(&page.text)), ..Default::default() }] });
            if pregens.len() >= 8 { break; }
        }
    }
    pregens
}

fn default_starter_archetypes(ruleset_id: &str) -> Vec<RecommendedArchetype> {
    if ruleset_id.contains("cyberpunk") {
        vec![
            RecommendedArchetype { archetype_id: "tech_or_netrunner".into(), title: "Tech / Netrunner investigator".into(), summary: "Good for technical clues, devices, NET architecture, and unusual hardware.".into(), fit_tags: vec!["technical".into(), "investigation".into()], required_option_refs: vec![], source_refs: vec![] },
            RecommendedArchetype { archetype_id: "solo_or_lawman".into(), title: "Solo / Lawman responder".into(), summary: "Good for dangerous scenes, firefights, protecting bystanders, and tactical pressure.".into(), fit_tags: vec!["combat".into(), "street".into()], required_option_refs: vec![], source_refs: vec![] },
            RecommendedArchetype { archetype_id: "fixer_or_media".into(), title: "Fixer / Media connector".into(), summary: "Good for contacts, legwork, negotiation, and street-level complications.".into(), fit_tags: vec!["social".into(), "contacts".into()], required_option_refs: vec![], source_refs: vec![] },
        ]
    } else if ruleset_id.contains("cthulhu") || ruleset_id.contains("brp") {
        vec![
            RecommendedArchetype { archetype_id: "investigator".into(), title: "Investigator".into(), summary: "Built around observation, research, interviewing, and following clues.".into(), fit_tags: vec!["investigation".into()], required_option_refs: vec![], source_refs: vec![] },
            RecommendedArchetype { archetype_id: "doctor_or_academic".into(), title: "Doctor / Academic".into(), summary: "Good for specialized knowledge, analysis, medicine, and slow-burn horror.".into(), fit_tags: vec!["knowledge".into(), "support".into()], required_option_refs: vec![], source_refs: vec![] },
        ]
    } else if ruleset_id.contains("triangle") {
        vec![
            RecommendedArchetype { archetype_id: "field_agent_balanced".into(), title: "Balanced Field Agent".into(), summary: "Choose ARC and Competency to cover investigation, weird powers, and workplace complications.".into(), fit_tags: vec!["fieldwork".into(), "anomaly".into()], required_option_refs: vec![], source_refs: vec![] },
        ]
    } else if ruleset_id.contains("sword_world") || ruleset_id.contains("dnd") {
        vec![
            RecommendedArchetype { archetype_id: "frontline".into(), title: "Frontline defender".into(), summary: "A durable character who can survive direct conflict.".into(), fit_tags: vec!["frontline".into(), "combat".into()], required_option_refs: vec![], source_refs: vec![] },
            RecommendedArchetype { archetype_id: "healer_support".into(), title: "Healer / support".into(), summary: "A support character with recovery or protective abilities.".into(), fit_tags: vec!["support".into(), "healing".into()], required_option_refs: vec![], source_refs: vec![] },
            RecommendedArchetype { archetype_id: "scout_expert".into(), title: "Scout / expert".into(), summary: "Useful for exploration, traps, knowledge, or non-combat obstacles.".into(), fit_tags: vec!["exploration".into(), "skills".into()], required_option_refs: vec![], source_refs: vec![] },
        ]
    } else {
        vec![RecommendedArchetype { archetype_id: "balanced_starter".into(), title: "Balanced starter character".into(), summary: "A concept-first character with enough source-backed mechanics to enter the first scene.".into(), fit_tags: vec!["starter".into()], required_option_refs: vec![], source_refs: vec![] }]
    }
}

fn infer_character_runtime_bindings(template: &CharacterTemplate) -> Vec<CharacterRuntimeBinding> {
    let mut bindings = Vec::new();
    for field in &template.fields {
        let id = field.field_id.to_ascii_lowercase();
        let runtime_path = if id.contains("hp") || id.contains("hit_point") || id == "health" {
            Some("actor.hp".to_string())
        } else if id.contains("san") || id.contains("sanity") {
            Some("actor.resources.sanity".to_string())
        } else if id.contains("mp") || id.contains("magic_point") || id.contains("power_point") {
            Some("actor.resources.magic_or_power_points".to_string())
        } else if id.contains("skill") {
            Some("actor.skills".to_string())
        } else if id.contains("stat") || id.contains("attribute") || id.contains("ability_score") || id.contains("characteristic") {
            Some("actor.attributes".to_string())
        } else if id.contains("weapon") || id.contains("equipment") || id.contains("armor") || id.contains("gear") {
            Some("actor.inventory_or_equipment".to_string())
        } else if id.contains("condition") || id.contains("wound") || id.contains("harm") || id.contains("chaos") {
            Some("generic_parameter_states.actor.resources_or_conditions".to_string())
        } else { None };
        if let Some(runtime_path) = runtime_path {
            bindings.push(CharacterRuntimeBinding { sheet_path: format!("sheet.{}", field.field_id), runtime_path, binding_kind: "source_backed_parameter_facet".into(), source_refs: vec![] });
        }
    }
    bindings
}

fn validate_character_onboarding_pack(pack: &CharacterOnboardingPack) -> ValidationReport {
    let mut report = ValidationReport { status: "ok".into(), ..Default::default() };
    if pack.sheet_template.fields.is_empty() {
        report.status = "error".into();
        report.errors.push(ValidationMessage { code: "missing_character_sheet_template_fields".into(), message: "Character onboarding pack has no sheet fields.".into(), target: Some("sheet_template.fields".into()) });
    }
    if pack.creation_flows.is_empty() || pack.creation_flows.iter().all(|f| f.steps.is_empty()) {
        report.status = "error".into();
        report.errors.push(ValidationMessage { code: "missing_character_creation_flow".into(), message: "No character creation flow with steps was extracted.".into(), target: Some("creation_flows".into()) });
    }
    if pack.derived_formula_pack.formulas.is_empty() {
        if report.status == "ok" { report.status = "warning".into(); }
        report.warnings.push(ValidationMessage { code: "no_derived_formulas".into(), message: "No derived formulas were extracted; finalization should block mechanical-ready status until formulas are found or reviewed.".into(), target: Some("derived_formula_pack.formulas".into()) });
    }
    if pack.starter_character_pack.pregens.is_empty() && pack.starter_character_pack.archetypes.is_empty() && pack.starter_character_pack.creation_shortcuts.is_empty() {
        report.status = "error".into();
        report.errors.push(ValidationMessage { code: "missing_starter_character_path".into(), message: "No pregen, archetype, or quick-start shortcut exists.".into(), target: Some("starter_character_pack".into()) });
    }
    report
}

fn parse_character_fields_flexible(value: &Value) -> Vec<CharacterField> {
    let mut out = Vec::new();
    if let Some(arr) = value.as_array() {
        for item in arr {
            if let Some(field) = parse_character_field_item(None, item) {
                out.push(field);
            }
        }
    } else if let Some(map) = value.as_object() {
        for (id, item) in map {
            if let Some(field) = parse_character_field_item(Some(id), item) {
                out.push(field);
            }
        }
    }
    out
}

fn parse_character_field_item(id_hint: Option<&str>, item: &Value) -> Option<CharacterField> {
    if let Ok(field) = serde_json::from_value::<CharacterField>(item.clone()) {
        return Some(field);
    }
    let obj = item.as_object();
    let field_id = obj
        .and_then(|o| o.get("field_id").or_else(|| o.get("id")).or_else(|| o.get("name")).and_then(Value::as_str))
        .or(id_hint)
        .map(sanitize_id)?;
    let title = obj
        .and_then(|o| o.get("title").or_else(|| o.get("label")).or_else(|| o.get("name")).and_then(Value::as_str))
        .unwrap_or(id_hint.unwrap_or(field_id.as_str()))
        .to_string();
    let field_type = obj
        .and_then(|o| o.get("field_type").or_else(|| o.get("type")).and_then(Value::as_str))
        .unwrap_or("text")
        .to_string();
    let required = obj.and_then(|o| o.get("required")).and_then(Value::as_bool).unwrap_or(false);
    let repeatable = obj.and_then(|o| o.get("repeatable")).and_then(Value::as_bool).unwrap_or(false);
    let choices_material_id = obj.and_then(|o| o.get("choices_material_id").or_else(|| o.get("choices_from")).and_then(Value::as_str)).map(str::to_string);
    let default_value = obj.and_then(|o| o.get("default_value").or_else(|| o.get("default"))).cloned();
    let visibility = obj
        .and_then(|o| o.get("visibility").and_then(Value::as_str))
        .map(parse_visibility);
    let notes = obj.and_then(|o| o.get("notes").or_else(|| o.get("description")).and_then(Value::as_str)).map(str::to_string);
    Some(CharacterField { field_id, title, field_type, required, repeatable, choices_material_id, default_value, visibility, notes })
}

fn parse_character_sections_flexible(value: &Value) -> Vec<CharacterSection> {
    let mut out = Vec::new();
    if let Some(arr) = value.as_array() {
        for item in arr {
            if let Some(section) = parse_character_section_item(None, item) {
                out.push(section);
            }
        }
    } else if let Some(map) = value.as_object() {
        for (id, item) in map {
            if let Some(section) = parse_character_section_item(Some(id), item) {
                out.push(section);
            }
        }
    }
    out
}

fn parse_character_section_item(id_hint: Option<&str>, item: &Value) -> Option<CharacterSection> {
    if let Ok(section) = serde_json::from_value::<CharacterSection>(item.clone()) {
        return Some(section);
    }
    let obj = item.as_object();
    let section_id = obj
        .and_then(|o| o.get("section_id").or_else(|| o.get("id")).or_else(|| o.get("name")).and_then(Value::as_str))
        .or(id_hint)
        .map(sanitize_id)?;
    let title = obj
        .and_then(|o| o.get("title").or_else(|| o.get("label")).or_else(|| o.get("name")).and_then(Value::as_str))
        .unwrap_or(id_hint.unwrap_or(section_id.as_str()))
        .to_string();
    let field_ids = obj
        .and_then(|o| o.get("field_ids").or_else(|| o.get("fields")))
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).map(sanitize_id).collect())
        .unwrap_or_default();
    Some(CharacterSection { section_id, title, field_ids })
}

fn parse_creation_steps_flexible(value: &Value) -> Vec<CreationStep> {
    if let Ok(steps) = serde_json::from_value::<Vec<CreationStep>>(value.clone()) {
        return steps;
    }
    let mut out = Vec::new();
    if let Some(arr) = value.as_array() {
        for (idx, item) in arr.iter().enumerate() {
            let obj = item.as_object();
            let step_id = obj
                .and_then(|o| o.get("step_id").or_else(|| o.get("id")).or_else(|| o.get("name")).and_then(Value::as_str))
                .map(sanitize_id)
                .unwrap_or_else(|| format!("step_{}", idx + 1));
            let title = obj
                .and_then(|o| o.get("title").or_else(|| o.get("label")).or_else(|| o.get("name")).and_then(Value::as_str))
                .or_else(|| item.as_str())
                .unwrap_or("Creation step")
                .to_string();
            let required = obj.and_then(|o| o.get("required")).and_then(Value::as_bool).unwrap_or(true);
            out.push(CreationStep { step_id, title, required, prompt: None, inputs: vec![], outputs: vec![], source_refs: vec![] });
        }
    }
    out
}


fn gm_onboarding_prompt() -> &'static str {
    "You are onboarding an LLM GM to a new TRPG. Do not fully parse the book. Build operational familiarity. Return JSON with keys: game_identity, play_loop, ruleset_kernel, character_sheet_map, book_locator, starter_procedures, lookup_recipes, cold_data_locator. Book locator entries should identify where things are, not extract every item/spell/monster. Cold data categories should default to on_demand or on_first_use. Output valid JSON only."
}

fn module_prep_prompt() -> &'static str {
    "You prepare a TRPG module like a human GM preparing the next session. Do not fully parse later chapters. Return JSON matching ModulePrepPacket fields where possible: module_overview, current_session_packet, required_rule_demands. Include strong start, first/current scenes, NPCs, locations, clues, conflicts, content warnings, and any rules likely needed immediately. GM-only secrets must stay GM-only. Output valid JSON only."
}

fn coerce_gm_onboarding(value: Value, ruleset_id: &str, title: &str, book: &PlainTextBook, book_map: &Value, procedures: &[ProcedureDef]) -> GmOnboardingBundle {
    let obj = value.as_object();
    let source_refs = first_source_ref(book).into_iter().collect::<Vec<_>>();
    let mut book_locator = parse_locator_array(obj.and_then(|o| o.get("book_locator")), ruleset_id, "ruleset", &book.source_id);
    if book_locator.is_empty() {
        book_locator = book_locator_entries_from_book_map(ruleset_id, "ruleset", &book.source_id, book_map, "located");
    }
    let mut cold_data_locator = parse_locator_array(obj.and_then(|o| o.get("cold_data_locator")), ruleset_id, "ruleset", &book.source_id);
    if cold_data_locator.is_empty() {
        cold_data_locator = cold_data_locators_from_book_map(ruleset_id, "ruleset", &book.source_id, book_map);
    }
    let mut lookup_recipes = parse_lookup_recipes(obj.and_then(|o| o.get("lookup_recipes")), Some(ruleset_id.to_string()), None);
    if lookup_recipes.is_empty() {
        lookup_recipes = default_lookup_recipes(ruleset_id, None, &book_locator);
    }
    GmOnboardingBundle {
        schema_version: "chatrpg.gm_onboarding.v1".to_string(),
        onboarding_id: format!("ruleset.{ruleset_id}.gm_onboarding.v1"),
        ruleset_id: ruleset_id.to_string(),
        title: title.to_string(),
        source_refs,
        game_identity: obj.and_then(|o| o.get("game_identity")).cloned().unwrap_or_else(|| json!({"summary": format!("Operational identity for {title}"), "genre": "unknown", "tone": "infer from source"})),
        play_loop: obj.and_then(|o| o.get("play_loop")).cloned().unwrap_or_else(|| json!({"default_loop": ["Describe the situation", "Ask what the players do", "Resolve uncertainty", "Narrate consequences", "Record durable changes"]})),
        ruleset_kernel: obj.and_then(|o| o.get("ruleset_kernel")).cloned().unwrap_or_else(|| json!({"when_to_roll": "roll only when outcome is uncertain and consequences matter", "procedure_policy": "use starter procedures first; look up details on demand"})),
        character_sheet_map: obj.and_then(|o| o.get("character_sheet_map")).cloned().unwrap_or_else(|| json!({"fields": [], "policy": "character sheet map is partial until character creation/on-demand lookup expands it"})),
        book_locator,
        starter_procedures: procedures.to_vec(),
        lookup_recipes,
        cold_data_locator,
        learned_packets_seed: vec![],
        created_at: Utc::now(),
    }
}

fn fallback_gm_onboarding(ruleset_id: &str, title: &str, doc: &SourceDocument, book: &PlainTextBook, book_map: &Value, procedures: &[ProcedureDef]) -> GmOnboardingBundle {
    let book_locator = book_locator_entries_from_book_map(ruleset_id, "ruleset", &doc.source_id, book_map, "located");
    let cold_data_locator = cold_data_locators_from_book_map(ruleset_id, "ruleset", &doc.source_id, book_map);
    GmOnboardingBundle {
        schema_version: "chatrpg.gm_onboarding.v1".to_string(),
        onboarding_id: format!("ruleset.{ruleset_id}.gm_onboarding.v1"),
        ruleset_id: ruleset_id.to_string(),
        title: title.to_string(),
        source_refs: first_source_ref(book).into_iter().collect(),
        game_identity: json!({"summary": format!("{title}: onboarded from introduction, table of contents, and rule locator."), "operational_goal": "be able to run a first scene, create characters, and look up detailed rules on demand"}),
        play_loop: json!({"default_loop": ["Describe scene", "Ask player intent", "Identify whether a rule/procedure is needed", "Resolve with known packet or lookup", "Narrate result", "Record memory/ruling"], "learning_policy": "unknown details are looked up and learned rather than hallucinated"}),
        ruleset_kernel: json!({"starter_procedures": procedures.iter().map(|p| p.procedure_id.clone()).collect::<Vec<_>>(), "ruling_status_required": true}),
        character_sheet_map: json!({"policy": "use CharacterTemplate; expand sheet-field meanings through character creation and lookup events"}),
        lookup_recipes: default_lookup_recipes(ruleset_id, None, &book_locator),
        book_locator,
        starter_procedures: procedures.to_vec(),
        cold_data_locator,
        learned_packets_seed: vec![],
        created_at: Utc::now(),
    }
}

fn coerce_module_prep_packet(value: Value, module_id: &str, ruleset_id: Option<&str>, title: &str, book: &PlainTextBook, book_map: &Value) -> ModulePrepPacket {
    let obj = value.as_object();
    ModulePrepPacket {
        prep_id: format!("module.{module_id}.prep.first_session.v1"),
        module_id: module_id.to_string(),
        ruleset_id: ruleset_id.map(str::to_string),
        mode: "first_session".to_string(),
        title: format!("{title} — First Session Prep"),
        module_overview: obj.and_then(|o| o.get("module_overview")).cloned().unwrap_or_else(|| json!({"summary": format!("Operational overview for {title}"), "book_map": book_map})),
        current_session_packet: obj.and_then(|o| o.get("current_session_packet")).cloned().unwrap_or_else(|| json!({"strong_start": "Use the synopsis and first playable scene from the module.", "current_scenes": [], "current_npcs": [], "current_locations": [], "open_questions": []})),
        required_rule_demands: parse_string_array(obj.and_then(|o| o.get("required_rule_demands")).unwrap_or(&Value::Null)),
        source_refs: first_source_ref(book).into_iter().collect(),
        created_at: Utc::now(),
    }
}

fn fallback_module_prep_packet(module_id: &str, ruleset_id: Option<&str>, title: &str, book: &PlainTextBook, book_map: &Value) -> ModulePrepPacket {
    let sample = module_first_session_pages(book, 12_000);
    ModulePrepPacket {
        prep_id: format!("module.{module_id}.prep.first_session.v1"),
        module_id: module_id.to_string(),
        ruleset_id: ruleset_id.map(str::to_string),
        mode: "first_session".to_string(),
        title: format!("{title} — First Session Prep"),
        module_overview: json!({"summary": "Module overview generated from TOC and synopsis pages.", "book_map": book_map}),
        current_session_packet: json!({"strong_start": "Start from the first actionable scene in the synopsis or first chapter.", "source_excerpt_preview": take_tail_chars(&sample, 4000), "policy": "later chapters remain cold-located until the table approaches them"}),
        required_rule_demands: vec!["core resolution".to_string(), "current scene conflict".to_string()],
        source_refs: first_source_ref(book).into_iter().collect(),
        created_at: Utc::now(),
    }
}

fn onboarding_blocks(bundle: &GmOnboardingBundle) -> Vec<ContextBlock> {
    let mut blocks = Vec::new();
    let compact = json!({
        "game_identity": bundle.game_identity.clone(),
        "play_loop": bundle.play_loop.clone(),
        "ruleset_kernel": bundle.ruleset_kernel.clone(),
        "character_sheet_map": bundle.character_sheet_map.clone(),
        "book_locator_summary": bundle.book_locator.iter().take(20).map(|e| json!({"label": &e.label, "category": &e.category, "pages": [e.page_start, e.page_end], "terms": &e.search_terms})).collect::<Vec<_>>(),
        "lookup_recipes": bundle.lookup_recipes.iter().take(20).collect::<Vec<_>>(),
        "learning_policy": "Do not pretend to know cold data. Locate, look up, rule with status, then learn."
    });
    let mut onboarding = ContextBlock::new(
        format!("ruleset.{}.gm_onboarding", bundle.ruleset_id),
        BlockKind::RulesetOnboarding,
        "GM Onboarding Bundle",
        BlockContent::Json(compact),
        Visibility::GmOnly,
        Stability::RarelyChanged,
        CacheZone::Prefix,
        Scope::ruleset(&bundle.ruleset_id),
        160,
    );
    onboarding.tags = vec!["resident".into(), "onboarding".into(), "operational_familiarity".into()];
    blocks.push(onboarding);

    let mut locator_block = ContextBlock::new(
        format!("ruleset.{}.book_locator.summary", bundle.ruleset_id),
        BlockKind::BookLocator,
        "Book Locator Summary",
        BlockContent::Json(json!({"entries": bundle.book_locator.iter().take(60).collect::<Vec<_>>()})),
        Visibility::GmOnly,
        Stability::RarelyChanged,
        CacheZone::Prefix,
        Scope::ruleset(&bundle.ruleset_id),
        120,
    );
    locator_block.tags = vec!["resident".into(), "locator".into(), "search_router".into()];
    blocks.push(locator_block);
    blocks
}

/// Turn the classified module-reader output into cache-zoned ContextBlocks:
/// - `module_specific_rules` tagged `bp2_custom_rule` -> ModuleSpecificRule /
///   PinnedMiddle (BP2 resident, always in the middle of the prompt).
/// - entities (npc/clue/location/encounter/handout) tagged `bp3_index` are
///   folded into a single ModuleOverview / DynamicTail index block (BP3).
///
/// Scene `deep` content is NOT projected here; that is the Phase 4 runtime
/// projector. Fail-closed: an entity missing `content_class` is treated as
/// `story` and skipped from the index; missing name/body fields read as empty.
fn module_static_blocks(module_id: &str, r: &reader::ModuleReadout) -> Vec<ContextBlock> {
    let mut out = Vec::new();
    let class_of = |v: &Value| {
        v.get("content_class")
            .and_then(Value::as_str)
            .unwrap_or("story")
            .to_string()
    };
    let str_of = |v: &Value, key: &str| {
        v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
    };
    let name_of = |v: &Value| str_of(v, "name");
    let body_of = |v: &Value| {
        v.get("body")
            .or_else(|| v.get("summary"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };

    // BP2: module-specific custom rules -> resident pinned blocks.
    // I1: when a rule lacks `id`, fall back to a stable per-index `idx{i}` so the
    // block_id never collapses (an empty id would yield `module.{m}.rule.` for
    // every id-less rule, silently overwriting peers downstream). Normal ids are
    // kept verbatim to preserve the `module.{id}.rule.{id}` naming intent.
    // Per spec §4 the `module_specific_rules` vec is BY DEFINITION the BP2
    // custom-rule vec — its entries ARE rules regardless of whether the LLM
    // remembered to stamp `content_class=bp2_custom_rule`. Requiring that field
    // here meant an unlabeled rule silently fell through to `story` and was
    // dropped. Treat every entry in this vec as a custom rule; the per-entity
    // `bp3_index` gate below still applies to npc/clue/location/etc.
    for (i, v) in r.module_specific_rules.iter().enumerate() {
        let rid = {
            let s = str_of(v, "id");
            if s.is_empty() { format!("idx{i}") } else { s }
        };
        let mut b = ContextBlock::new(
            format!("module.{module_id}.rule.{rid}"),
            BlockKind::ModuleSpecificRule,
            name_of(v),
            BlockContent::Text(body_of(v)),
            Visibility::GmOnly,
            Stability::RarelyChanged,
            CacheZone::PinnedMiddle,
            Scope::module(module_id),
            50,
        );
        b.tags = vec!["module_custom_rule".into(), "resident".into()];
        out.push(b);
    }

    // BP3: entity index lines -> one dynamic-tail overview block.
    let index_entities = r
        .npcs
        .iter()
        .chain(&r.clues)
        .chain(&r.locations)
        .chain(&r.encounters)
        .chain(&r.handouts);
    let mut idx_lines = Vec::new();
    for v in index_entities {
        if class_of(v) != "bp3_index" {
            continue;
        }
        idx_lines.push(format!("- {} ({}): {}", name_of(v), str_of(v, "id"), body_of(v)));
    }
    if !idx_lines.is_empty() {
        let mut b = ContextBlock::new(
            format!("module.{module_id}.index"),
            BlockKind::ModuleOverview,
            "模组索引",
            BlockContent::Text(idx_lines.join("\n")),
            Visibility::GmOnly,
            Stability::SceneStable,
            CacheZone::DynamicTail,
            Scope::module(module_id),
            30,
        );
        b.tags = vec!["module_index".into()];
        out.push(b);
    }

    out
}

fn module_prep_blocks(packet: &ModulePrepPacket) -> Vec<ContextBlock> {
    let mut blocks = Vec::new();
    let mut overview = ContextBlock::new(
        format!("module.{}.overview", packet.module_id),
        BlockKind::ModuleOverview,
        "Module Overview",
        BlockContent::Json(packet.module_overview.clone()),
        Visibility::GmOnly,
        Stability::RarelyChanged,
        CacheZone::PinnedMiddle,
        Scope::module(&packet.module_id),
        105,
    );
    overview.tags = vec!["module".into(), "overview".into(), "prep".into()];
    blocks.push(overview);
    let mut current = ContextBlock::new(
        format!("module.{}.current_session_packet", packet.module_id),
        BlockKind::CurrentSessionPacket,
        "Current Session Packet",
        BlockContent::Json(packet.current_session_packet.clone()),
        Visibility::GmOnly,
        Stability::SceneStable,
        CacheZone::PinnedMiddle,
        Scope::module(&packet.module_id),
        130,
    );
    current.tags = vec!["module".into(), "first_session".into(), "current_packet".into(), "prep".into()];
    blocks.push(current);
    blocks
}

fn book_locator_entries_from_book_map(owner_id: &str, owner_kind: &str, source_document_id: &str, book_map: &Value, parse_policy: &str) -> Vec<BookLocatorEntry> {
    let mut out = Vec::new();
    if let Some(headings) = book_map.get("headings").and_then(Value::as_array) {
        for h in headings.iter().take(180) {
            let heading = h.get("heading").and_then(Value::as_str).unwrap_or("section").trim();
            let page = h.get("page").and_then(Value::as_u64).map(|v| v as u32);
            let category = locator_category(heading);
            out.push(BookLocatorEntry {
                locator_id: format!("locator.{owner_id}.{}", sanitize_id(&format!("{}_{:?}", heading, page))),
                owner_id: owner_id.to_string(),
                owner_kind: owner_kind.to_string(),
                label: heading.to_string(),
                category: category.clone(),
                source_document_id: source_document_id.to_string(),
                page_start: page,
                page_end: page,
                heading_path: vec![heading.to_string()],
                search_terms: locator_terms(heading, &category),
                summary: format!("Locator for {heading}. Parse policy: {parse_policy}."),
                confidence: 0.72,
                parse_policy: parse_policy.to_string(),
                tags: vec!["locator".into(), category],
                source_refs: vec![SourceRef { source_id: source_document_id.to_string(), page, section_path: vec![heading.to_string()], ..Default::default() }],
            });
        }
    }
    out
}

fn cold_data_locators_from_book_map(owner_id: &str, owner_kind: &str, source_document_id: &str, book_map: &Value) -> Vec<BookLocatorEntry> {
    book_locator_entries_from_book_map(owner_id, owner_kind, source_document_id, book_map, "on_demand")
        .into_iter()
        .filter(|e| matches!(e.category.as_str(), "items" | "spells" | "abilities" | "monsters" | "npcs" | "equipment" | "data" | "vehicles" | "netrunning"))
        .map(|mut e| { e.tags.push("cold_data".into()); e })
        .collect()
}

fn material_from_locator(_bundle_id: &str, entry: &BookLocatorEntry) -> MaterialIndexEntry {
    let load_when = if entry.owner_kind == "module" {
        vec![LoadPredicate::ActiveModule { module_id: entry.owner_id.clone() }]
    } else {
        vec![LoadPredicate::ActiveRuleset { ruleset_id: entry.owner_id.clone() }]
    };
    MaterialIndexEntry {
        material_id: entry.locator_id.clone(),
        material_type: if entry.tags.iter().any(|t| t == "cold_data") { MaterialType::ColdDataLocator } else { MaterialType::BookLocator },
        title: entry.label.clone(),
        summary: entry.summary.clone(),
        default_cache_zone: CacheZone::NeverPrompt,
        visibility: Visibility::GmOnly,
        stability: Stability::RarelyChanged,
        source_refs: entry.source_refs.clone(),
        dependencies: vec![],
        load_when,
        extracted_block_id: None,
        estimated_tokens: None,
        tags: entry.tags.clone(),
    }
}

fn parse_locator_array(value: Option<&Value>, owner_id: &str, owner_kind: &str, source_document_id: &str) -> Vec<BookLocatorEntry> {
    let mut out = Vec::new();
    let Some(arr) = value.and_then(Value::as_array) else { return out; };
    for (idx, item) in arr.iter().enumerate() {
        let obj = item.as_object();
        let label = obj.and_then(|o| o.get("label").or_else(|| o.get("title")).and_then(Value::as_str)).unwrap_or("locator").to_string();
        let category = obj.and_then(|o| o.get("category").and_then(Value::as_str)).map(str::to_string).unwrap_or_else(|| locator_category(&label));
        let page_start = obj.and_then(|o| o.get("page_start").or_else(|| o.get("page")).and_then(Value::as_u64)).map(|v| v as u32);
        let page_end = obj.and_then(|o| o.get("page_end").and_then(Value::as_u64)).map(|v| v as u32).or(page_start);
        let search_terms = obj.and_then(|o| o.get("search_terms")).map(parse_string_array).filter(|v| !v.is_empty()).unwrap_or_else(|| locator_terms(&label, &category));
        out.push(BookLocatorEntry {
            locator_id: obj.and_then(|o| o.get("locator_id").or_else(|| o.get("id")).and_then(Value::as_str)).map(str::to_string).unwrap_or_else(|| format!("locator.{owner_id}.{}.{idx}", sanitize_id(&label))),
            owner_id: owner_id.to_string(),
            owner_kind: owner_kind.to_string(),
            label: label.clone(),
            category: category.clone(),
            source_document_id: source_document_id.to_string(),
            page_start,
            page_end,
            heading_path: obj.and_then(|o| o.get("heading_path")).map(parse_string_array).unwrap_or_else(|| vec![label.clone()]),
            search_terms,
            summary: obj.and_then(|o| o.get("summary").and_then(Value::as_str)).unwrap_or("Locator entry.").to_string(),
            confidence: obj.and_then(|o| o.get("confidence").and_then(Value::as_f64)).unwrap_or(0.7) as f32,
            parse_policy: obj.and_then(|o| o.get("parse_policy").and_then(Value::as_str)).unwrap_or("on_demand").to_string(),
            tags: obj.and_then(|o| o.get("tags")).map(parse_string_array).unwrap_or_else(|| vec!["locator".into(), category.clone()]),
            source_refs: vec![SourceRef { source_id: source_document_id.to_string(), page: page_start, section_path: vec![label], ..Default::default() }],
        });
    }
    out
}

fn parse_lookup_recipes(value: Option<&Value>, ruleset_id: Option<String>, module_id: Option<String>) -> Vec<LookupRecipe> {
    let mut out = Vec::new();
    let Some(arr) = value.and_then(Value::as_array) else { return out; };
    for (idx, item) in arr.iter().enumerate() {
        let obj = item.as_object();
        let demand = obj.and_then(|o| o.get("demand").or_else(|| o.get("query")).and_then(Value::as_str)).unwrap_or("rule lookup").to_string();
        out.push(LookupRecipe {
            recipe_id: obj.and_then(|o| o.get("recipe_id").or_else(|| o.get("id")).and_then(Value::as_str)).map(str::to_string).unwrap_or_else(|| format!("lookup_recipe.{}", idx + 1)),
            ruleset_id: ruleset_id.clone(),
            module_id: module_id.clone(),
            demand: demand.clone(),
            rg_terms: obj.and_then(|o| o.get("rg_terms").or_else(|| o.get("search_terms"))).map(parse_string_array).unwrap_or_else(|| vec![demand.clone()]),
            scope: obj.and_then(|o| o.get("scope")).cloned().unwrap_or_else(|| json!({})),
            expected_sources: obj.and_then(|o| o.get("expected_sources")).map(parse_string_array).unwrap_or_default(),
            confidence: obj.and_then(|o| o.get("confidence").and_then(Value::as_f64)).unwrap_or(0.7) as f32,
        });
    }
    out
}

fn default_lookup_recipes(ruleset_id: &str, module_id: Option<String>, locators: &[BookLocatorEntry]) -> Vec<LookupRecipe> {
    let demands = ["basic check", "combat", "damage", "healing", "character creation", "equipment", "special ability"];
    demands.iter().enumerate().map(|(idx, demand)| {
        let category = locator_category(demand);
        LookupRecipe {
            recipe_id: format!("lookup_recipe.{ruleset_id}.{}", sanitize_id(demand)),
            ruleset_id: Some(ruleset_id.to_string()),
            module_id: module_id.clone(),
            demand: (*demand).to_string(),
            rg_terms: vec![(*demand).to_string()],
            scope: json!({"locator_candidates": locators.iter().filter(|l| l.category.as_str() == category.as_str()).take(5).map(|l| l.locator_id.clone()).collect::<Vec<_>>() }),
            expected_sources: vec![],
            confidence: 0.6 + (idx as f32 * 0.0),
        }
    }).collect()
}

fn locator_category(heading: &str) -> String {
    let h = heading.to_ascii_lowercase();
    if h.contains("combat") || h.contains("firefight") || h.contains("attack") { "combat".into() }
    else if h.contains("skill") || h.contains("check") || h.contains("ability score") || h.contains("resolving") || h.contains("getting it done") { "core_resolution".into() }
    else if h.contains("character") || h.contains("creation") || h.contains("arc") || h.contains("roles") { "character".into() }
    else if h.contains("item") || h.contains("equipment") || h.contains("weapon") || h.contains("armor") || h.contains("gear") { "items".into() }
    else if h.contains("spell") || h.contains("magic") || h.contains("power") { "spells".into() }
    else if h.contains("monster") || h.contains("creature") || h.contains("npc") || h.contains("mook") { "monsters".into() }
    else if h.contains("netrunning") || h.contains("netrun") || h.contains("cyberdeck") { "netrunning".into() }
    else if h.contains("gm") || h.contains("game mastery") || h.contains("running") || h.contains("keeper") { "gm_guidance".into() }
    else if h.contains("data") || h.contains("appendix") || h.contains("index") { "data".into() }
    else { "section".into() }
}

fn locator_terms(heading: &str, category: &str) -> Vec<String> {
    let mut terms = vec![heading.to_string(), category.to_string()];
    match category {
        "combat" => terms.extend(["attack", "damage", "initiative", "action"].into_iter().map(str::to_string)),
        "core_resolution" => terms.extend(["check", "difficulty", "target", "roll"].into_iter().map(str::to_string)),
        "items" => terms.extend(["gear", "weapon", "armor", "price"].into_iter().map(str::to_string)),
        "spells" => terms.extend(["magic", "spell", "power", "ability"].into_iter().map(str::to_string)),
        "netrunning" => terms.extend(["NET", "cyberdeck", "architecture", "control node"].into_iter().map(str::to_string)),
        _ => {}
    }
    terms.sort(); terms.dedup(); terms
}

fn take_tail_chars(input: &str, max_chars: usize) -> String {
    let len = input.chars().count();
    if len <= max_chars {
        input.to_string()
    } else {
        input.chars().skip(len - max_chars).collect()
    }
}

fn parse_string_array(value: &Value) -> Vec<String> {
    if let Some(arr) = value.as_array() {
        arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
    } else if let Some(s) = value.as_str() {
        vec![s.to_string()]
    } else { vec![] }
}

fn onboarding_relevant_pages(book: &PlainTextBook, max_chars: usize) -> String {
    let keywords = ["introduction", "how to play", "how to use", "what is", "game mastery", "gm", "running", "character", "skill", "combat", "check", "contents", "table of contents"];
    gather_pages_by_keywords(book, &keywords, max_chars, 18)
}

fn module_first_session_pages(book: &PlainTextBook, max_chars: usize) -> String {
    let keywords = ["synopsis", "background", "introduction", "read me", "chapter 1", "start", "pre-investigation", "briefing", "first", "strong start", "interests", "contents"];
    gather_pages_by_keywords(book, &keywords, max_chars, 18)
}

fn gather_pages_by_keywords(book: &PlainTextBook, keywords: &[&str], max_chars: usize, max_pages: usize) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    for page in &book.pages {
        let lower = page.text.to_ascii_lowercase();
        if keywords.iter().any(|k| lower.contains(k)) || page.page <= 8 {
            let piece = format!("\n[PAGE {}]\n{}\n", page.page, page.text);
            if out.len() + piece.len() > max_chars || count >= max_pages { break; }
            out.push_str(&piece);
            count += 1;
        }
    }
    if out.is_empty() { sample_pages(book, max_chars) } else { out }
}

fn character_template_prompt() -> &'static str {
    "You extract TRPG character sheet templates. Output one JSON object matching: {template_id,ruleset_id,title,source_refs,sections,fields,derived_values,creation_flow,validation_rules,llm_creation_policy}. Use field IDs in snake_case. Include creation flow steps, required fields, derived formulas when explicit or inferable. Output valid JSON only."
}

fn character_onboarding_pack_prompt() -> &'static str {
    "You are the Character Steward skill inside a Rule Steward Agent. Your job is not to make a pretty character sheet summary; it is to produce a source-backed minimum playable character onboarding pack. Return JSON matching CharacterOnboardingPack: {pack_id,ruleset_id,title,sheet_template,creation_flows,option_catalogs,derived_formula_pack,starter_character_pack,import_mapping_profile,validation_profile,runtime_binding_profile,runtime_bindings,source_refs,validation_report}. Include: (1) sheet fields and sections, (2) legal creation modes such as pregenerated/quick_start/guided/detailed/import/randomized/high_level when present, (3) ordered creation steps, (4) option locators rather than full option databases, (5) derived formulas only if source-backed; leave missing formulas as missing, (6) pregens/sample characters or archetype routes for fast start, (7) runtime bindings from sheet fields to actor resources, equipment, abilities, and parameter facets. Never invent HP, damage, skill totals, or derived formulas. Output valid JSON only."
}

fn character_onboarding_relevant_pages(book: &PlainTextBook, max_chars: usize) -> String {
    let keywords = [
        "character", "character sheet", "creating a character", "create a character", "character creation",
        "step-by-step", "how to make a pc", "investigator creation", "agent", "onboarding questionnaire",
        "easy creation", "detailed creation", "sample character", "pregenerated", "pre-generated",
        "race", "class", "role", "profession", "background", "lifepath", "arc", "competency",
        "ability scores", "characteristics", "statistics", "skills", "derived", "calculation of values",
        "hit points", "sanity", "humanity", "mp", "power points", "equipment", "starting equipment",
        "copy character sheet", "how to read character sheet", "contents", "table of contents"
    ];
    gather_pages_by_keywords(book, &keywords, max_chars, 28)
}

fn rulebook_chunk_prompt() -> &'static str {
    "You receive one source-backed semantic unit from a TRPG rulebook, not an arbitrary page window. Use its [semantic_unit] metadata as routing hints for retrieval/materialization. Return valid JSON only: {context_blocks:[{block_id,kind,title,content_markdown,visibility,cache_zone,priority,tags,summary}], material_index:[{material_id,material_type,title,summary,cache_zone,visibility,tags,block_id}]}. If metadata says noise, return empty arrays. Extract only facts/numbers present in the unit; unknown numeric parameters must stay missing/provisional, never invented. Use kinds such as rule_package, procedure_detail, parameter_family, parameter_entry, state_trait, state_track, ruleset_index, ruleset_character_kernel. Preserve source page refs and exact mechanical values; do not spend effort on cosmetic line-break repair."
}

fn module_system_prompt() -> &'static str {
    "You are a TRPG adventure/module compiler. Extract playable structure, spoiler boundaries, scene graph hints, NPC/location/clue/encounter/material indexes. Output valid JSON only."
}

fn module_chunk_prompt() -> &'static str {
    "You receive one source-backed semantic unit from a TRPG module, not an arbitrary page window. Use its [semantic_unit] metadata as routing hints for retrieval/materialization/redaction. Return valid JSON only: {context_blocks:[{block_id,kind,title,content_markdown,visibility,cache_zone,priority,tags,summary}], material_index:[{material_id,material_type,title,summary,cache_zone,visibility,tags,block_id}]}. If metadata says noise, return empty arrays. Use kinds such as module_spine, mission_static, chapter_static, scene_static, scenario_node, npc_static, location_static, clue, handout, encounter, module_specific_rule, module_style. Honor gm_secret/playwalled markers as gm_only. Player read-aloud snippets may be player_visible only when the text explicitly presents read-aloud material. Extract only facts/numbers present in the unit; unknown NPC HP, DV, damage, SP, AC, SAN, Chaos, or conditions must stay missing/provisional, never invented. Preserve page refs and exact mechanical values; do not optimize cosmetic formatting."
}

fn build_book_map(book: &PlainTextBook) -> Value {
    let mut headings = Vec::new();
    let heading_re = Regex::new(r"(?m)^(#{1,3}\s+)?([A-Z][A-Za-z0-9][A-Za-z0-9 &'’:#,\-]{2,80})$|^(Part \d+|Chapter \d+|CHAPTER [A-Z0-9]+|Appendix|APPENDIX).{0,80}$").unwrap();
    for page in &book.pages {
        for cap in heading_re.captures_iter(&page.text) {
            if let Some(m) = cap.get(0) {
                let h = m.as_str().trim();
                if h.len() > 2 && h.len() < 120 {
                    headings.push(json!({"page": page.page, "heading": h}));
                }
            }
        }
    }
    headings.truncate(400);
    json!({"source_id": &book.source_id, "title": &book.title, "pages": book.pages.len(), "headings": headings})
}

fn sample_pages(book: &PlainTextBook, max_chars: usize) -> String {
    let mut out = String::new();
    for page in book.pages.iter().take(15).chain(book.pages.iter().rev().take(3)) {
        let piece = format!("\n[PAGE {}]\n{}\n", page.page, page.text);
        if out.len() + piece.len() > max_chars { break; }
        out.push_str(&piece);
    }
    out
}

fn character_relevant_pages(book: &PlainTextBook, max_chars: usize) -> String {
    let keywords = ["character", "sheet", "creation", "make a pc", "creating a character", "step", "race", "class", "background", "arc", "competency", "profession", "characteristics"];
    let mut out = String::new();
    for page in &book.pages {
        let lower = page.text.to_ascii_lowercase();
        if keywords.iter().any(|k| lower.contains(k)) {
            let piece = format!("\n[PAGE {}]\n{}\n", page.page, page.text);
            if out.len() + piece.len() > max_chars { break; }
            out.push_str(&piece);
        }
    }
    if out.is_empty() { sample_pages(book, max_chars) } else { out }
}

fn classify_rulebook(title: &str, book: &PlainTextBook) -> DocumentType {
    let hay = format!("{} {}", title, sample_pages(book, 5000)).to_ascii_lowercase();
    if hay.contains("core rulebook") || hay.contains("player's handbook") || hay.contains("keeper rulebook") { DocumentType::CoreRulebook }
    else if hay.contains("orc content") || hay.contains("universal game engine") { DocumentType::RulesetFamily }
    else { DocumentType::Supplement }
}

fn classify_module(title: &str, book: &PlainTextBook) -> DocumentType {
    let hay = format!("{} {}", title, sample_pages(book, 8000)).to_ascii_lowercase();
    if hay.contains("missions for triangle agency") || hay.contains("scenario collection") { DocumentType::ScenarioCollection }
    else if hay.contains("campaign") || hay.contains("masks of nyarlathotep") { DocumentType::Campaign }
    else if hay.contains("chapter 1") && hay.contains("chapter 2") { DocumentType::OneShot }
    else { DocumentType::Unknown }
}

fn infer_ruleset_id(title: &str) -> String {
    let lower = title.to_ascii_lowercase();
    if lower.contains("cyberpunk") { "cyberpunk_red".to_string() }
    else if lower.contains("sword world") { "sword_world_2_5".to_string() }
    else if lower.contains("triangle") { "triangle_agency".to_string() }
    else if lower.contains("cthulhu") { "call_of_cthulhu_7e".to_string() }
    else if lower.contains("basicroleplaying") || lower.contains("basic roleplaying") || lower.contains("brp") { "brp_orc".to_string() }
    else if lower.contains("dnd") || lower.contains("dungeon") || lower.contains("player") { "dnd5e".to_string() }
    else { sanitize_id(title) }
}

fn infer_module_id(title: &str) -> String {
    let lower = title.to_ascii_lowercase();
    if lower.contains("homecoming") { "cyberpunk_red.homecoming".to_string() }
    else if lower.contains("vault") { "triangle_agency.the_vault".to_string() }
    else if lower.contains("masks") { "call_of_cthulhu_7e.masks_of_nyarlathotep".to_string() }
    else { sanitize_id(title) }
}

fn infer_module_ruleset(title: &str, book: &PlainTextBook) -> Option<String> {
    let hay = format!("{} {}", title, sample_pages(book, 4000)).to_ascii_lowercase();
    if hay.contains("cyberpunk red") { Some("cyberpunk_red".to_string()) }
    else if hay.contains("triangle agency") { Some("triangle_agency".to_string()) }
    else if hay.contains("call of cthulhu") || hay.contains("nyarlathotep") { Some("call_of_cthulhu_7e".to_string()) }
    else { None }
}

fn infer_procedures(ruleset_id: &str, title: &str, book: &PlainTextBook) -> Vec<ProcedureDef> {
    let lower = format!("{} {}", title, sample_pages(book, 10_000)).to_ascii_lowercase();
    let mut procedures = Vec::new();
    if ruleset_id.contains("sword_world") || lower.contains("2 dice") || lower.contains("2d6") {
        procedures.push(ProcedureDef {
            procedure_id: format!("{ruleset_id}.skill_check"),
            title: "2d6 Skill Check".to_string(),
            summary: "Roll 2d6 plus relevant standard value against a target number; specific rules may add automatic success/failure handling.".to_string(),
            roll_model: RollModel::Sw2d6,
            inputs: vec!["standard_value".to_string(), "target_number".to_string()],
            outputs: vec!["success".to_string(), "degree".to_string()],
            special: json!({"double_six":"automatic_success_when_applicable", "double_one":"automatic_failure_when_applicable"}),
            source_refs: vec![],
        });
    } else if ruleset_id.contains("cyberpunk") || lower.contains("resolving actions with skills") {
        procedures.push(ProcedureDef {
            procedure_id: format!("{ruleset_id}.skill_check"),
            title: "STAT + Skill + d10 vs DV".to_string(),
            summary: "Roll d10, add relevant STAT and Skill, compare to DV. Natural 10 and 1 may trigger critical behavior per exact rules.".to_string(),
            roll_model: RollModel::D10StatSkill,
            inputs: vec!["stat".to_string(), "skill".to_string(), "dv".to_string()],
            outputs: vec!["success".to_string(), "total".to_string()],
            special: json!({"critical_success":"consult exact rule", "critical_failure":"consult exact rule"}),
            source_refs: vec![],
        });
    } else if ruleset_id.contains("triangle") || lower.contains("six four-sided dice") || lower.contains("chaos") {
        procedures.push(ProcedureDef {
            procedure_id: format!("{ruleset_id}.conflict_resolution"),
            title: "Triangle 6d4 Conflict Resolution".to_string(),
            summary: "Roll six four-sided dice and interpret results according to Triangle Agency's conflict and performance rules.".to_string(),
            roll_model: RollModel::Triangle6d4,
            inputs: vec!["quality".to_string(), "risk".to_string()],
            outputs: vec!["outcome".to_string(), "chaos".to_string()],
            special: json!({"three_is_important": true}),
            source_refs: vec![],
        });
    } else if ruleset_id.contains("cthulhu") || ruleset_id.contains("brp") || lower.contains("percentile") {
        procedures.push(ProcedureDef {
            procedure_id: format!("{ruleset_id}.percentile_check"),
            title: "Percentile Roll-Under".to_string(),
            summary: "Roll d100 and compare against an ability or skill rating; lower is better, with special/critical/fumble thresholds from the exact rules.".to_string(),
            roll_model: RollModel::PercentileRollUnder,
            inputs: vec!["rating".to_string(), "difficulty".to_string()],
            outputs: vec!["success_level".to_string(), "roll".to_string()],
            special: json!({"critical":"consult exact threshold", "fumble":"consult exact threshold"}),
            source_refs: vec![],
        });
    } else {
        procedures.push(ProcedureDef {
            procedure_id: format!("{ruleset_id}.d20_check"),
            title: "d20 Check".to_string(),
            summary: "Roll d20 plus relevant modifiers against a target number.".to_string(),
            roll_model: RollModel::D20 { dc: None, allow_advantage: true },
            inputs: vec!["modifier".to_string(), "dc".to_string()],
            outputs: vec!["success".to_string(), "total".to_string()],
            special: json!({}),
            source_refs: vec![],
        });
    }
    procedures
}

fn fallback_character_template(ruleset_id: &str, title: &str) -> CharacterTemplate {
    CharacterTemplate {
        template_id: format!("{ruleset_id}.character_template.v1"),
        ruleset_id: ruleset_id.to_string(),
        title: format!("{title} Character Template"),
        sections: vec![
            CharacterSection { section_id: "identity".to_string(), title: "Identity".to_string(), field_ids: vec!["character_name".into(), "concept".into(), "background".into()] },
            CharacterSection { section_id: "mechanics".to_string(), title: "Mechanics".to_string(), field_ids: vec!["attributes".into(), "skills".into(), "resources".into(), "equipment".into()] },
        ],
        fields: vec![
            CharacterField { field_id: "character_name".into(), title: "Character Name".into(), field_type: "string".into(), required: true, repeatable: false, choices_material_id: None, default_value: None, visibility: Some(Visibility::Public), notes: None },
            CharacterField { field_id: "concept".into(), title: "Concept".into(), field_type: "text".into(), required: true, repeatable: false, choices_material_id: None, default_value: None, visibility: Some(Visibility::Public), notes: None },
            CharacterField { field_id: "background".into(), title: "Background".into(), field_type: "text".into(), required: false, repeatable: false, choices_material_id: None, default_value: None, visibility: Some(Visibility::Public), notes: None },
            CharacterField { field_id: "attributes".into(), title: "Attributes".into(), field_type: "object".into(), required: true, repeatable: false, choices_material_id: None, default_value: None, visibility: Some(Visibility::Public), notes: Some("Ruleset-specific fields extracted from materials.".into()) },
            CharacterField { field_id: "skills".into(), title: "Skills".into(), field_type: "object".into(), required: false, repeatable: false, choices_material_id: None, default_value: None, visibility: Some(Visibility::Public), notes: None },
            CharacterField { field_id: "resources".into(), title: "Resources / Tracks".into(), field_type: "object".into(), required: false, repeatable: false, choices_material_id: None, default_value: None, visibility: Some(Visibility::Public), notes: None },
            CharacterField { field_id: "equipment".into(), title: "Equipment".into(), field_type: "array".into(), required: false, repeatable: true, choices_material_id: None, default_value: None, visibility: Some(Visibility::Public), notes: None },
        ],
        derived_values: vec![],
        creation_flow: vec![
            CreationStep { step_id: "concept".into(), title: "Define concept and tone".into(), required: true, prompt: None, inputs: vec![], outputs: vec!["concept".into()], source_refs: vec![] },
            CreationStep { step_id: "mechanical_choices".into(), title: "Choose ruleset-specific mechanical options".into(), required: true, prompt: None, inputs: vec!["concept".into()], outputs: vec!["attributes".into(), "skills".into(), "equipment".into()], source_refs: vec![] },
            CreationStep { step_id: "validate".into(), title: "Validate sheet".into(), required: true, prompt: None, inputs: vec!["sheet".into()], outputs: vec!["validation_report".into()], source_refs: vec![] },
        ],
        validation_rules: vec![CharacterValidationRule { rule_id: "required_fields".into(), severity: "error".into(), description: "Required fields must be present.".into(), expression: None }],
        llm_creation_policy: json!({"mode":"interactive_plus_auto", "validator_required": true, "ask_user_before_finalizing": ["concept", "tone", "role_in_party"]}),
        source_refs: vec![],
    }
}

fn blocks_and_materials_from_llm(value: &Value, owner_id: &str, scope: Scope, source_kind: SourceKind, chunk: &BookChunk) -> (Vec<ContextBlock>, Vec<MaterialIndexEntry>) {
    let mut blocks = Vec::new();
    let mut materials = Vec::new();
    if let Some(arr) = value.get("context_blocks").and_then(Value::as_array) {
        for item in arr {
            let block_id = item.get("block_id").and_then(Value::as_str).map(sanitize_id).unwrap_or_else(|| format!("{}.chunk.{}.{}", owner_id, chunk.start_page, blocks.len() + 1));
            let kind = parse_block_kind(item.get("kind").and_then(Value::as_str).unwrap_or("scenario_node"));
            let title = item.get("title").and_then(Value::as_str).unwrap_or("Untitled block");
            let content = item.get("content_markdown").or_else(|| item.get("summary")).and_then(Value::as_str).unwrap_or("");
            let visibility = parse_visibility(item.get("visibility").and_then(Value::as_str).unwrap_or("gm_only"));
            let cache_zone = parse_cache_zone(item.get("cache_zone").and_then(Value::as_str).unwrap_or(match &source_kind { SourceKind::Rulebook => "pinned_middle", SourceKind::Module => "pinned_middle", _ => "dynamic_tail" }));
            let priority = item.get("priority").and_then(Value::as_i64).unwrap_or(50) as i32;
            let tags = item.get("tags").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
            let mut block = ContextBlock::new(block_id, kind, title, BlockContent::Markdown(content.to_string()), visibility, Stability::SceneStable, cache_zone, scope.clone(), priority);
            block.tags = tags;
            block.source_refs = vec![SourceRef { source_id: chunk.source_id.clone(), page: Some(chunk.start_page), anchor_id: Some(format!("{}.p{:04}", chunk.source_id, chunk.start_page)), section_path: vec![], char_start: None, char_end: None, text_hash: None, note: Some(format!("pages {}-{}", chunk.start_page, chunk.end_page)) }];
            blocks.push(block);
        }
    }
    if let Some(arr) = value.get("material_index").and_then(Value::as_array) {
        for item in arr {
            let material_id = item.get("material_id").and_then(Value::as_str).map(sanitize_id).unwrap_or_else(|| format!("{}.material.{}.{}", owner_id, chunk.start_page, materials.len() + 1));
            let material_type = parse_material_type(item.get("material_type").and_then(Value::as_str).unwrap_or("other"));
            let title = item.get("title").and_then(Value::as_str).unwrap_or("Untitled material").to_string();
            let summary = item.get("summary").and_then(Value::as_str).unwrap_or("").to_string();
            let cache_zone = parse_cache_zone(item.get("cache_zone").and_then(Value::as_str).unwrap_or("pinned_middle"));
            let visibility = parse_visibility(item.get("visibility").and_then(Value::as_str).unwrap_or("gm_only"));
            let tags = item.get("tags").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
            let extracted_block_id = item.get("block_id").and_then(Value::as_str).map(sanitize_id);
            materials.push(MaterialIndexEntry {
                material_id,
                material_type,
                title,
                summary,
                default_cache_zone: cache_zone,
                visibility,
                stability: Stability::SceneStable,
                source_refs: vec![SourceRef { source_id: chunk.source_id.clone(), page: Some(chunk.start_page), anchor_id: None, section_path: vec![], char_start: None, char_end: None, text_hash: None, note: Some(format!("pages {}-{}", chunk.start_page, chunk.end_page)) }],
                dependencies: vec![],
                load_when: vec![LoadPredicate::Always],
                extracted_block_id,
                estimated_tokens: None,
                tags,
            });
        }
    }
    (blocks, materials)
}

fn fallback_chunk_block(owner_id: &str, chunk: &BookChunk, source_kind: SourceKind) -> ContextBlock {
    let kind = match &source_kind { SourceKind::Rulebook => BlockKind::RulePackage, SourceKind::Module => BlockKind::ScenarioNode, SourceKind::Unknown => BlockKind::RetrievedMemory };
    let scope = match &source_kind { SourceKind::Rulebook => Scope::ruleset(owner_id), SourceKind::Module => Scope::module(owner_id), SourceKind::Unknown => Scope::global() };
    let mut block = ContextBlock::new(
        format!("{}.fallback.pages_{}_{}", owner_id, chunk.start_page, chunk.end_page),
        kind,
        format!("Fallback pages {}-{}", chunk.start_page, chunk.end_page),
        BlockContent::Markdown(format!("This page range was indexed but not successfully extracted. Use the page-anchored markdown for source pages {}-{}.", chunk.start_page, chunk.end_page)),
        Visibility::GmOnly,
        Stability::SceneStable,
        CacheZone::PinnedMiddle,
        scope,
        10,
    );
    block.tags = vec!["fallback".into(), "needs_repair".into()];
    block.source_refs = vec![SourceRef { source_id: chunk.source_id.clone(), page: Some(chunk.start_page), anchor_id: None, section_path: vec![], char_start: None, char_end: None, text_hash: None, note: Some(format!("pages {}-{}", chunk.start_page, chunk.end_page)) }];
    block
}

fn material_from_block(_bundle_id: &str, block: &ContextBlock) -> MaterialIndexEntry {
    MaterialIndexEntry {
        material_id: block.block_id.clone(),
        material_type: material_type_from_block_kind(&block.kind),
        title: block.title.clone(),
        summary: block.content.render_text().chars().take(300).collect(),
        default_cache_zone: block.cache_zone.clone(),
        visibility: block.visibility.clone(),
        stability: block.stability.clone(),
        source_refs: block.source_refs.clone(),
        dependencies: block.dependencies.clone(),
        load_when: vec![LoadPredicate::Always],
        extracted_block_id: Some(block.block_id.clone()),
        estimated_tokens: block.token_estimate,
        tags: block.tags.clone(),
    }
}

fn tagged_block(
    block_id: String,
    kind: BlockKind,
    title: &str,
    content: &str,
    visibility: Visibility,
    stability: Stability,
    cache_zone: CacheZone,
    scope: Scope,
    priority: i32,
    tags: Vec<&str>,
    source_ref: Option<SourceRef>,
) -> ContextBlock {
    let mut block = ContextBlock::new(block_id, kind, title, BlockContent::Markdown(content.to_string()), visibility, stability, cache_zone, scope, priority);
    block.tags = tags.into_iter().map(str::to_string).collect();
    if let Some(sr) = source_ref { block.source_refs.push(sr); }
    block
}

fn first_source_ref(book: &PlainTextBook) -> Option<SourceRef> {
    book.pages.first().map(|p| SourceRef { source_id: book.source_id.clone(), page: Some(p.page), anchor_id: Some(format!("{}.p{:04}", book.source_id, p.page)), section_path: vec![], char_start: None, char_end: None, text_hash: Some(sha256_hex(&p.text)), note: None })
}

fn material_type_from_block_kind(kind: &BlockKind) -> MaterialType {
    match kind {
        BlockKind::ProcedureDetail | BlockKind::ProcedureVariant => MaterialType::Procedure,
        BlockKind::RulesetCharacterKernel => MaterialType::CharacterTemplate,
        BlockKind::RuleStewardKernel => MaterialType::RuleKernel,
        BlockKind::RuleKernelPatch => MaterialType::RuleKernelPatch,
        BlockKind::RuleEntityLocator => MaterialType::RuleEntityLocator,
        BlockKind::MechanicalSourcePack => MaterialType::MechanicalSourcePack,
        BlockKind::PlayabilityGateReport => MaterialType::PlayabilityGateReport,
        BlockKind::CharacterOnboardingPack => MaterialType::CharacterOnboardingPack,
        BlockKind::CharacterCreationFlow => MaterialType::CharacterCreationFlow,
        BlockKind::CharacterOptionCatalog => MaterialType::CharacterOptionCatalog,
        BlockKind::DerivedFormulaPack => MaterialType::DerivedFormulaPack,
        BlockKind::StarterCharacterPack => MaterialType::StarterCharacterPack,
        BlockKind::RulesetOnboarding | BlockKind::GmOnboarding | BlockKind::GameIdentity | BlockKind::PlayLoop => MaterialType::GmOnboarding,
        BlockKind::BookLocator | BlockKind::BookLocatorSummary => MaterialType::BookLocator,
        BlockKind::ColdDataLocator => MaterialType::ColdDataLocator,
        BlockKind::LookupRecipe => MaterialType::LookupRecipe,
        BlockKind::LearnedPacket => MaterialType::LearnedPacket,
        BlockKind::Ruling | BlockKind::RulingLog | BlockKind::LookupResult => MaterialType::Ruling,
        BlockKind::NpcStatic => MaterialType::Npc,
        BlockKind::LocationStatic => MaterialType::Location,
        BlockKind::SceneStatic | BlockKind::ScenarioNode => MaterialType::Scene,
        BlockKind::MissionStatic => MaterialType::Mission,
        BlockKind::ChapterStatic => MaterialType::Chapter,
        BlockKind::Clue => MaterialType::Clue,
        BlockKind::Handout => MaterialType::Handout,
        BlockKind::ModuleSpecificRule => MaterialType::ModuleSpecificRule,
        BlockKind::CurrentSessionPacket | BlockKind::ModuleOverview => MaterialType::CurrentSessionPacket,
        BlockKind::RulesetDirectorPolicy | BlockKind::ModuleStyle => MaterialType::DirectorPolicy,
        BlockKind::DomainActor => MaterialType::Npc,
        BlockKind::DomainObject => MaterialType::Item,
        BlockKind::DomainAbility => MaterialType::Other,
        _ => MaterialType::Other,
    }
}

pub fn sanitize_id(input: &str) -> String {
    let re = Regex::new(r"[^a-zA-Z0-9]+" ).unwrap();
    let cleaned = re.replace_all(&input.to_ascii_lowercase(), "_").trim_matches('_').to_string();
    if cleaned.is_empty() { "id".into() } else { cleaned }
}

fn parse_cache_zone(s: &str) -> CacheZone {
    match s {
        "prefix" => CacheZone::Prefix,
        "dynamic_tail" => CacheZone::DynamicTail,
        "never_prompt" => CacheZone::NeverPrompt,
        _ => CacheZone::PinnedMiddle,
    }
}

fn parse_visibility(s: &str) -> Visibility {
    match s {
        "public" => Visibility::Public,
        "player_visible" => Visibility::PlayerVisible,
        "npc_private" => Visibility::NpcPrivate,
        "system_only" => Visibility::SystemOnly,
        _ => Visibility::GmOnly,
    }
}

fn parse_block_kind(s: &str) -> BlockKind {
    match s {
        "module_spine" => BlockKind::ModuleSpine,
        "mission_static" => BlockKind::MissionStatic,
        "chapter_static" => BlockKind::ChapterStatic,
        "scene_static" => BlockKind::SceneStatic,
        "scenario_node" => BlockKind::ScenarioNode,
        "npc_static" => BlockKind::NpcStatic,
        "location_static" => BlockKind::LocationStatic,
        "clue" => BlockKind::Clue,
        "handout" => BlockKind::Handout,
        "encounter" => BlockKind::ScenarioNode,
        "module_specific_rule" => BlockKind::ModuleSpecificRule,
        "module_overview" => BlockKind::ModuleOverview,
        "current_session_packet" => BlockKind::CurrentSessionPacket,
        "module_style" => BlockKind::ModuleStyle,
        "rule_package" => BlockKind::RulePackage,
        "procedure_detail" => BlockKind::ProcedureDetail,
        "parameter_family" => BlockKind::ParameterFamily,
        "parameter_entry" => BlockKind::ParameterEntry,
        "state_trait" => BlockKind::StateTrait,
        "state_track" => BlockKind::StateTrack,
        "ruleset_index" => BlockKind::RulesetIndex,
        "ruleset_character_kernel" => BlockKind::RulesetCharacterKernel,
        "ruleset_onboarding" => BlockKind::RulesetOnboarding,
        "book_locator" => BlockKind::BookLocator,
        "gm_onboarding" => BlockKind::GmOnboarding,
        "book_locator_summary" => BlockKind::BookLocatorSummary,
        "cold_data_locator" => BlockKind::ColdDataLocator,
        "lookup_recipe" => BlockKind::LookupRecipe,
        "learned_packet" => BlockKind::LearnedPacket,
        "lookup_result" => BlockKind::LookupResult,
        "ruling" => BlockKind::Ruling,
        "ruling_log" => BlockKind::RulingLog,
        _ => BlockKind::ScenarioNode,
    }
}

fn parse_material_type(s: &str) -> MaterialType {
    match s {
        "rule_package" => MaterialType::RulePackage,
        "procedure" => MaterialType::Procedure,
        "character_template" => MaterialType::CharacterTemplate,
        "gm_onboarding" | "ruleset_onboarding" => MaterialType::GmOnboarding,
        "book_locator" => MaterialType::BookLocator,
        "lookup_recipe" => MaterialType::LookupRecipe,
        "learned_packet" => MaterialType::LearnedPacket,
        "current_session_packet" => MaterialType::CurrentSessionPacket,
        "cold_data" => MaterialType::ColdDataLocator,
        "ruling" => MaterialType::Ruling,
        "spell" => MaterialType::Spell,
        "item" => MaterialType::Item,
        "weapon" => MaterialType::Weapon,
        "armor" => MaterialType::Armor,
        "monster" => MaterialType::Monster,
        "npc" => MaterialType::Npc,
        "location" => MaterialType::Location,
        "scene" => MaterialType::Scene,
        "mission" => MaterialType::Mission,
        "chapter" => MaterialType::Chapter,
        "clue" => MaterialType::Clue,
        "handout" => MaterialType::Handout,
        "encounter" => MaterialType::Encounter,
        "module_specific_rule" => MaterialType::ModuleSpecificRule,
        "director_policy" => MaterialType::DirectorPolicy,
        _ => MaterialType::Other,
    }
}

#[cfg(test)]
mod module_static_block_tests {
    use super::*;
    use trpg_model::CacheZone;

    #[test]
    fn custom_rules_go_pinned_index_goes_dynamic() {
        let readout = reader::ModuleReadout {
            module_specific_rules: vec![json!({"id":"r1","name":"狩猎","content_class":"bp2_custom_rule","body":"..."})],
            npcs: vec![json!({"id":"npc1","name":"拉斯","content_class":"bp3_index","summary":"老板"})],
            ..Default::default()
        };
        let blocks = module_static_blocks("mod1", &readout);
        let rule = blocks.iter().find(|b| b.tags.iter().any(|t| t == "module_custom_rule")).unwrap();
        assert_eq!(rule.cache_zone, CacheZone::PinnedMiddle);
        let idx = blocks.iter().find(|b| b.tags.iter().any(|t| t == "module_index")).unwrap();
        assert_eq!(idx.cache_zone, CacheZone::DynamicTail);
    }

    #[test]
    fn empty_readout_produces_no_blocks() {
        let blocks = module_static_blocks("m", &reader::ModuleReadout::default());
        assert!(blocks.is_empty());
    }

    #[test]
    fn missing_content_class_is_skipped() {
        // No content_class -> fail-closed: treated as `story`, excluded from index.
        let readout = reader::ModuleReadout {
            npcs: vec![json!({"id":"n1","name":"x"})],
            ..Default::default()
        };
        let blocks = module_static_blocks("m", &readout);
        assert!(blocks.iter().all(|b| !b.tags.iter().any(|t| t == "module_index")));
    }

    #[test]
    fn module_specific_rule_without_content_class_still_pinned() {
        // §4: the module_specific_rules vec is the BP2 rule vec by definition; a
        // missing/wrong content_class must NOT cause the rule to be dropped.
        let readout = reader::ModuleReadout {
            module_specific_rules: vec![
                json!({"id":"r1","name":"狩猎规则","body":"掷骰判定"}),               // no content_class
                json!({"id":"r2","name":"潜行","content_class":"story","body":"x"}), // mislabeled
            ],
            ..Default::default()
        };
        let blocks = module_static_blocks("m", &readout);
        let rules: Vec<_> = blocks
            .iter()
            .filter(|b| b.tags.iter().any(|t| t == "module_custom_rule"))
            .collect();
        assert_eq!(rules.len(), 2);
        assert!(rules.iter().all(|b| b.cache_zone == CacheZone::PinnedMiddle));
    }

    #[test]
    fn bp2_rules_missing_id_get_distinct_block_ids() {
        // Two id-less custom rules with distinct content must not collapse onto
        // the same block_id (locks the I1 fix).
        let readout = reader::ModuleReadout {
            module_specific_rules: vec![
                json!({"name":"狩猎","content_class":"bp2_custom_rule","body":"a"}),
                json!({"name":"狩猎","content_class":"bp2_custom_rule","body":"b"}),
            ],
            ..Default::default()
        };
        let blocks = module_static_blocks("m", &readout);
        let rules: Vec<_> = blocks
            .iter()
            .filter(|b| b.tags.iter().any(|t| t == "module_custom_rule"))
            .collect();
        assert_eq!(rules.len(), 2);
        assert_ne!(rules[0].block_id, rules[1].block_id);
    }
}

#[cfg(test)]
mod onboarding_flow_tests {
    use super::*;

    fn step(id: &str, title: &str) -> CreationStep {
        CreationStep { step_id: id.into(), title: title.into(), required: true, prompt: None, inputs: vec![], outputs: vec![], source_refs: vec![] }
    }

    fn tmpl_with_steps(ruleset: &str, ids: &[&str]) -> CharacterTemplate {
        let mut t = CharacterTemplate::default();
        t.ruleset_id = ruleset.into();
        t.creation_flow = ids.iter().map(|id| step(id, id)).collect();
        t
    }

    #[test]
    fn six_steps_become_one_flow_with_six_steps_in_order() {
        let t = tmpl_with_steps("call_of_cthulhu_7e",
            &["pick_occupation", "roll_characteristics", "derive_attributes", "spend_skill_points", "starting_gear", "background_story"]);
        let flows = creation_flows_from_template(&t);
        assert_eq!(flows.len(), 1, "one template flow -> one CharacterCreationFlow");
        let f = &flows[0];
        assert_eq!(f.steps.len(), 6, "all 6 steps preserved");
        assert_eq!(f.ruleset_id, "call_of_cthulhu_7e");
        // Step order + ids preserved verbatim.
        let ids: Vec<&str> = f.steps.iter().map(|s| s.step_id.as_str()).collect();
        assert_eq!(ids, vec!["pick_occupation", "roll_characteristics", "derive_attributes", "spend_skill_points", "starting_gear", "background_story"]);
        // A non-empty flow has steps -> passes the validator's flow check.
        assert!(!f.flow_id.is_empty());
    }

    #[test]
    fn empty_creation_flow_yields_no_flows() {
        let t = tmpl_with_steps("triangle_agency", &[]);
        assert!(creation_flows_from_template(&t).is_empty(),
            "no steps -> no fabricated flow (validator still flags missing flow, the honest state)");
    }

    #[test]
    fn stage1_pack_carries_deterministic_creation_flows() {
        let mut t = CharacterTemplate::default();
        t.ruleset_id = "call_of_cthulhu_7e".into();
        t.fields = vec![CharacterField { field_id: "str".into(), title: "STR".into(), field_type: "stat".into(), ..Default::default() }];
        t.creation_flow = vec![
            CreationStep { step_id: "roll_characteristics".into(), title: "Roll characteristics".into(), required: true, prompt: None, inputs: vec![], outputs: vec![], source_refs: vec![] },
            CreationStep { step_id: "spend_skill_points".into(), title: "Spend skill points".into(), required: true, prompt: None, inputs: vec![], outputs: vec![], source_refs: vec![] },
        ];
        let pack = stage1_onboarding_pack("call_of_cthulhu_7e", "Call of Cthulhu", &t, &json!([]));
        // The thin pack now carries a steps-bearing flow (was Vec::new()).
        assert_eq!(pack.creation_flows.len(), 1, "stage1 pack now has a creation flow");
        assert_eq!(pack.creation_flows[0].steps.len(), 2, "both template steps preserved");
        // The validator no longer reports a missing-flow error for THIS reason.
        assert!(!pack.validation_report.errors.iter().any(|e| e.code == "missing_character_creation_flow"),
            "flow check passes: {:?}", pack.validation_report.errors);
    }

    #[test]
    fn merge_starter_into_pack_lands_starter_and_keeps_flows() {
        // Base pack (stage1) has the deterministic creation_flows; merging a compiled
        // starter pack must (a) land archetypes/shortcuts and (b) preserve the flows.
        let mut t = CharacterTemplate::default();
        t.ruleset_id = "call_of_cthulhu_7e".into();
        t.fields = vec![CharacterField { field_id: "str".into(), title: "STR".into(), field_type: "stat".into(), ..Default::default() }];
        t.creation_flow = vec![CreationStep { step_id: "roll_characteristics".into(), title: "Roll".into(), required: true, prompt: None, inputs: vec![], outputs: vec![], source_refs: vec![] }];
        let base = stage1_onboarding_pack("call_of_cthulhu_7e", "Call of Cthulhu", &t, &json!([]));
        assert_eq!(base.creation_flows.len(), 1, "precondition: base has a flow");

        let starter = json!({
            "archetypes": [{"archetype_id": "investigator", "title": "Investigator", "summary": "follows clues", "required_option_refs": [], "source_refs": []}],
            "creation_shortcuts": [{"shortcut_id": "quick", "title": "Quick start", "mode": "quick_start", "description": "pick occupation then go", "source_refs": []}]
        });
        let pack = merge_starter_into_pack(base, &starter);

        // (a) starter pack landed.
        assert_eq!(pack.starter_character_pack.archetypes.len(), 1, "archetype landed");
        assert_eq!(pack.starter_character_pack.archetypes[0].archetype_id, "investigator");
        assert_eq!(pack.starter_character_pack.creation_shortcuts.len(), 1, "shortcut landed");
        // (b) creation_flows preserved.
        assert_eq!(pack.creation_flows.len(), 1, "flows preserved through merge");
        // (c) the validator now passes BOTH the flow check and the starter-path check.
        let report = validate_character_onboarding_pack(&pack);
        assert!(!report.errors.iter().any(|e| e.code == "missing_character_creation_flow"), "flow ok: {:?}", report.errors);
        assert!(!report.errors.iter().any(|e| e.code == "missing_starter_character_path"), "starter ok: {:?}", report.errors);
    }

    #[test]
    fn merge_lands_starter_with_llm_object_shaped_refs() {
        // REGRESSION: the real compiler emits `required_option_refs` as an OBJECT
        // ({"occupation":["occupation"]}) and `source_pages` as an array — NOT the
        // typed Vec<String>. Before the fix this off-shape field made the all-or-
        // nothing `from_value::<StarterCharacterPack>` fail and the WHOLE starter pack
        // silently emptied (3 compiled -> 0 landed in the live staged parse).
        let mut t = CharacterTemplate::default();
        t.ruleset_id = "call_of_cthulhu_7e".into();
        t.creation_flow = vec![CreationStep { step_id: "roll".into(), title: "Roll".into(), required: true, prompt: None, inputs: vec![], outputs: vec![], source_refs: vec![] }];
        let base = stage1_onboarding_pack("call_of_cthulhu_7e", "Call of Cthulhu", &t, &json!([]));
        let starter = json!({
            "archetypes": [{
                "archetype_id": "private_investigator", "title": "Private Investigator", "summary": "follows clues",
                "fit_tags": ["investigation"],
                "required_option_refs": {"occupation": ["occupation"]},
                "source_pages": ["53", "48"]
            }],
            "creation_shortcuts": [{"shortcut_id": "quick", "title": "Quick", "mode": "quick_start", "description": "pick occupation", "source_pages": "48"}]
        });
        let pack = merge_starter_into_pack(base, &starter);
        assert_eq!(pack.starter_character_pack.archetypes.len(), 1, "object-shaped refs no longer empty the pack");
        assert_eq!(pack.starter_character_pack.archetypes[0].required_option_refs, vec!["occupation".to_string()], "object refs flattened to Vec<String>");
        assert_eq!(pack.starter_character_pack.creation_shortcuts.len(), 1, "shortcut landed");
    }

    #[test]
    fn flatten_str_ids_handles_object_string_array() {
        assert_eq!(flatten_str_ids(&json!({"occupation": ["occupation"]})), vec!["occupation".to_string()]);
        assert_eq!(flatten_str_ids(&json!("firearms")), vec!["firearms".to_string()]);
        assert_eq!(flatten_str_ids(&json!(["a", "b", "a"])), vec!["a".to_string(), "b".to_string()]);
        assert!(flatten_str_ids(&json!(null)).is_empty());
    }

    #[test]
    fn merge_empty_starter_leaves_base_starter_empty() {
        // An empty compiled pack ({}) must NOT fabricate a starter path: the base
        // starter pack stays empty (validator still reports missing_starter, the
        // honest degraded state). The flows still pass.
        let mut t = CharacterTemplate::default();
        t.ruleset_id = "triangle_agency".into();
        t.fields = vec![CharacterField { field_id: "arc".into(), title: "Arc".into(), field_type: "choice".into(), ..Default::default() }];
        t.creation_flow = vec![CreationStep { step_id: "pick_arc".into(), title: "Pick ARC".into(), required: true, prompt: None, inputs: vec![], outputs: vec![], source_refs: vec![] }];
        let base = stage1_onboarding_pack("triangle_agency", "Triangle", &t, &json!([]));
        let pack = merge_starter_into_pack(base, &json!({}));
        assert!(pack.starter_character_pack.archetypes.is_empty(), "empty compiled pack -> no fabricated archetypes");
        assert!(pack.starter_character_pack.creation_shortcuts.is_empty());
    }
}
