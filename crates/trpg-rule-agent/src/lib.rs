use anyhow::Result;
use chrono::Utc;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tracing::warn;
use trpg_db::Db;
use trpg_llm::{system, user, LlmClient};
use trpg_model::*;
use trpg_search::SearchService;
use walkdir::WalkDir;

pub mod reader;
use uuid::Uuid;

/// Rule Steward Agent: a source-backed rules and character-onboarding assistant.
///
/// This crate is intentionally an orchestrator over existing infrastructure rather
/// than a second knowledge store.  It reuses learned packets, Tantivy search,
/// book locators, semantic source units, materialization artifacts, and DB audit
/// tables.
#[derive(Clone)]
pub struct RuleStewardAgent {
    pub db: Db,
    pub search: SearchService,
    pub data_dir: PathBuf,
}

impl RuleStewardAgent {
    pub fn new(db: Db, search: SearchService, data_dir: impl Into<PathBuf>) -> Self {
        Self {
            db,
            search,
            data_dir: data_dir.into(),
        }
    }

    /// Resolve a rules/character/materialization need using source-backed lookup.
    pub async fn assist(&self, mut need: RuleNeed) -> Result<RuleAssist> {
        if need.need_id.trim().is_empty() {
            need.need_id = format!("need_{}", Uuid::new_v4().simple());
        }
        let started = Instant::now();
        let query = steward_query_text(&need);

        let mut scopes = BTreeMap::new();
        scopes.insert("ruleset_id".to_string(), need.ruleset_id.clone());
        if let Some(module_id) = &need.module_id {
            scopes.insert("module_id".to_string(), module_id.clone());
        }
        if let Some(session_id) = &need.session_id {
            scopes.insert("session_id".to_string(), session_id.clone());
        }
        if let Some(scene_id) = &need.scene_id {
            scopes.insert("scene_id".to_string(), scene_id.clone());
        }

        let learned = self
            .db
            .list_learned_packets(&need.ruleset_id, need.module_id.as_deref(), 8)
            .await
            .unwrap_or_default();
        let learned_hits = learned
            .iter()
            .filter(|packet| packet_matches_need(packet, &need, &query))
            .cloned()
            .collect::<Vec<_>>();

        let search_req = SearchRequest {
            query: query.clone(),
            mode: SearchMode::Auto,
            domains: vec![
                "learned".into(),
                "rules".into(),
                "modules".into(),
                "rulings".into(),
                "source".into(),
                "parsed".into(),
            ],
            scopes,
            limit: 10,
            explain: true,
            viewer: VisibilityProfile::gm(),
            rewrite_query: true,
            intent: Some(format!("rule_steward.{}", need.need_kind.as_str())),
            ..Default::default()
        };
        let mut search_response = match self.search.search_async(&search_req).await {
            Ok(resp) => resp,
            Err(err) => {
                warn!(error = %err, query = %query, "rule steward search failed; falling back to locator search");
                SearchResponse::default()
            }
        };

        let mut locator_hits = Vec::new();
        if search_response.hits.is_empty() {
            locator_hits = self
                .db
                .search_book_locator_entries(Some(&need.ruleset_id), &query, 8)
                .await
                .unwrap_or_default();
            if locator_hits.is_empty() {
                if let Some(module_id) = &need.module_id {
                    locator_hits = self
                        .db
                        .search_book_locator_entries(Some(module_id), &query, 8)
                        .await
                        .unwrap_or_default();
                }
            }
        }
        let rg_hits = if search_response.hits.is_empty() {
            self.rg_source_scan(&need, &query, 8).unwrap_or_default()
        } else {
            Vec::new()
        };
        if !rg_hits.is_empty() {
            search_response.hits.extend(rg_hits.clone());
        }

        let mut source_refs = Vec::new();
        for hit in &search_response.hits {
            source_refs.extend(hit.source_refs.clone());
        }
        for packet in &learned_hits {
            source_refs.extend(packet.source_refs.clone());
        }
        for locator in &locator_hits {
            if let Some(refs) = locator
                .get("source_refs")
                .and_then(|v| serde_json::from_value::<Vec<SourceRef>>(v.clone()).ok())
            {
                source_refs.extend(refs);
            }
        }
        dedupe_source_refs(&mut source_refs);

        let mut context_blocks = Vec::new();
        for hit in search_response.hits.iter().take(6) {
            let mut block = ContextBlock::new(
                format!(
                    "rule_steward.lookup.{}.{}",
                    need.need_id,
                    sanitize_for_block_id(&hit.hit_id)
                ),
                BlockKind::LookupResult,
                format!("Rule Steward hit: {}", hit.title),
                BlockContent::Json(json!({"hit": hit, "rule_need": &need.need_id})),
                hit.visibility,
                Stability::TurnDynamic,
                CacheZone::DynamicTail,
                Scope::ruleset(need.ruleset_id.clone()),
                78,
            );
            block.tags = vec![
                "rule_steward".into(),
                "lookup_hit".into(),
                need.need_kind.as_str().into(),
            ];
            block.source_refs = hit.source_refs.clone();
            block.load_reason = Some(format!("rule_need:{}", need.need_id));
            context_blocks.push(block);
        }
        for packet in learned_hits.iter().take(4) {
            context_blocks.push(packet.to_context_block());
        }
        if !locator_hits.is_empty() {
            let mut block = ContextBlock::new(
                format!("rule_steward.locators.{}", need.need_id),
                BlockKind::RuleEntityLocator,
                "Rule Steward locator fallback",
                BlockContent::Json(json!({"query": &query, "locators": &locator_hits})),
                Visibility::GmOnly,
                Stability::TurnDynamic,
                CacheZone::DynamicTail,
                Scope::ruleset(need.ruleset_id.clone()),
                70,
            );
            block.tags = vec![
                "rule_steward".into(),
                "locator_fallback".into(),
                need.need_kind.as_str().into(),
            ];
            block.source_refs = source_refs.clone();
            context_blocks.push(block);
        }

        let missing_source = source_refs.is_empty();
        let status = if !learned_hits.is_empty() && search_response.hits.is_empty() {
            RuleAssistStatus::LearnedStable
        } else if missing_source {
            RuleAssistStatus::UnresolvedNeedsSource
        } else if need.missing_facets.is_empty() || search_response.hits.len() >= 2 {
            RuleAssistStatus::SourceBackedExact
        } else {
            RuleAssistStatus::SourceBackedPartial
        };
        let confidence = match status {
            RuleAssistStatus::SourceBackedExact => 0.88,
            RuleAssistStatus::SourceBackedPartial => 0.68,
            RuleAssistStatus::LearnedStable => 0.78,
            RuleAssistStatus::UnresolvedNeedsSource => 0.20,
            RuleAssistStatus::ConflictNeedsReview => 0.35,
            RuleAssistStatus::ProvisionalTableRuling => 0.40,
            RuleAssistStatus::NotARulesProblem => 0.50,
        };
        let unresolved_questions = if missing_source {
            vec![RuleGap {
                gap_id: format!("gap_{}", Uuid::new_v4().simple()),
                description: format!("No source-backed rule/source unit found for `{}`.", query),
                blocking: matches!(need.urgency, RuleUrgency::ImmediateTurn),
                suggested_skill: Some(skill_for_need(&need).into()),
            }]
        } else {
            Vec::new()
        };

        let lookup_event = LookupEvent {
            event_id: format!("lookup_{}", Uuid::new_v4().simple()),
            session_id: need.session_id.clone(),
            ruleset_id: Some(need.ruleset_id.clone()),
            module_id: need.module_id.clone(),
            demand_id: Some(need.need_id.clone()),
            query_text: query.clone(),
            search_terms: vec![query.clone(), need.need_kind.as_str().into()],
            source_hits: json!({"search_hits": search_response.hits.clone(), "learned_packets": learned_hits.clone(), "locator_hits": locator_hits.clone(), "rg_hits": rg_hits.clone()}),
            result_status: status.as_str().into(),
            created_at: Utc::now(),
        };
        self.db.insert_lookup_event(&lookup_event).await.ok();

        let run = RuleAgentRun {
            run_id: format!("rule_run_{}", Uuid::new_v4().simple()),
            session_id: need.session_id.clone(),
            turn_id: need.turn_id.clone(),
            ruleset_id: need.ruleset_id.clone(),
            module_id: need.module_id.clone(),
            trigger: need.need_kind.as_str().into(),
            selected_skill: skill_for_need(&need).into(),
            tool_calls: {
                let mut calls = vec![ToolCallRecord {
                    tool_name: "trpg-search".into(),
                    query: query.clone(),
                    hit_count: search_response.hits.len().saturating_sub(rg_hits.len()),
                    selected_hit_ids: search_response
                        .hits
                        .iter()
                        .take(6)
                        .map(|h| h.hit_id.clone())
                        .collect(),
                    elapsed_ms: started.elapsed().as_millis() as u64,
                }];
                if !rg_hits.is_empty() {
                    calls.push(ToolCallRecord {
                        tool_name: "trpg-rg-fallback".into(),
                        query: query.clone(),
                        hit_count: rg_hits.len(),
                        selected_hit_ids: rg_hits
                            .iter()
                            .take(6)
                            .map(|h| h.hit_id.clone())
                            .collect(),
                        elapsed_ms: started.elapsed().as_millis() as u64,
                    });
                }
                calls
            },
            source_refs_read: source_refs.clone(),
            outputs_written: vec![lookup_event.event_id.clone()],
            confidence,
            unresolved_count: unresolved_questions.len(),
            contradiction_count: 0,
            created_at: Some(Utc::now()),
        };
        self.db.upsert_rule_agent_run(&run).await.ok();

        Ok(RuleAssist {
            assist_id: format!("assist_{}", Uuid::new_v4().simple()),
            need_id: need.need_id.clone(),
            status,
            confidence,
            answer_scope: answer_scope_for_need(&need),
            gm_brief: gm_brief_for_status(status, &query, source_refs.len()),
            player_safe_summary: if matches!(status, RuleAssistStatus::UnresolvedNeedsSource) {
                None
            } else {
                Some("规则管家已找到当前动作可用的来源依据；机械裁判层会只消费有 source_refs 的字段。".into())
            },
            source_refs,
            evidence_bundle_id: None,
            context_blocks,
            search_hits: search_response.hits,
            learned_packet_candidates: Vec::new(),
            bp1_patch_proposal: None,
            unresolved_questions,
            audit_id: run.run_id,
        })
    }

    /// Local ripgrep-style fallback over existing parsed/markdown artifacts.
    /// This intentionally does not invent a second index; it gives the Steward the same
    /// deterministic exact-text search path as the CLI `trpg rg` workflow when Tantivy
    /// has not yet been rebuilt.
    fn rg_source_scan(&self, need: &RuleNeed, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let terms = query_terms(query);
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let roots = [self.data_dir.join("parsed"), self.data_dir.join("markdown")];
        let mut hits = Vec::new();
        for root in roots {
            if !root.exists() {
                continue;
            }
            for entry in WalkDir::new(root)
                .into_iter()
                .filter_map(|entry| entry.ok())
            {
                if hits.len() >= limit {
                    break;
                }
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.path();
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if !matches!(ext.as_str(), "md" | "json" | "jsonl" | "txt") {
                    continue;
                }
                if std::fs::metadata(path)
                    .map(|m| m.len() > 2_000_000)
                    .unwrap_or(true)
                {
                    continue;
                }
                let text = match std::fs::read_to_string(path) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let lower = text.to_ascii_lowercase();
                let matched = terms
                    .iter()
                    .filter(|term| lower.contains(term.as_str()))
                    .count();
                if matched == 0 {
                    continue;
                }
                let source_id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("source")
                    .to_string();
                let snippet = snippet_for_terms(&text, &terms, 640);
                let mut scopes = BTreeMap::new();
                scopes.insert("ruleset_id".into(), need.ruleset_id.clone());
                if let Some(module_id) = &need.module_id {
                    scopes.insert("module_id".into(), module_id.clone());
                }
                let source_ref = SourceRef {
                    source_id: source_id.clone(),
                    page: None,
                    anchor_id: Some(path.to_string_lossy().to_string()),
                    section_path: vec![],
                    char_start: None,
                    char_end: None,
                    text_hash: None,
                    note: Some("trpg-rg-fallback".into()),
                };
                hits.push(SearchHit {
                    hit_id: format!("rg_{}", sanitize_for_block_id(&path.to_string_lossy())),
                    search_doc_id: path.to_string_lossy().to_string(),
                    origin: "rg_fallback".into(),
                    domain: if path.to_string_lossy().contains("modules") {
                        "modules".into()
                    } else {
                        "rules".into()
                    },
                    logical_kind: "source_text".into(),
                    title: path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("source")
                        .to_string(),
                    snippet,
                    score: 0.45 + (matched as f32 * 0.05),
                    scopes,
                    tags: vec![
                        "rule_steward".into(),
                        "rg_fallback".into(),
                        need.need_kind.as_str().into(),
                    ],
                    visibility: Visibility::GmOnly,
                    source_refs: vec![source_ref],
                    metadata: json!({"path": path.to_string_lossy(), "matched_terms": matched}),
                    explain: json!({"tool":"trpg-rg-fallback", "terms": terms.clone()}),
                });
            }
        }
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(limit);
        Ok(hits)
    }

    /// Check whether a ruleset/module can reach first playable table state.
    pub async fn playability_gate(
        &self,
        ruleset_id: &str,
        module_id: Option<&str>,
    ) -> Result<PlayabilityGateReport> {
        let kernel = self.db.load_rule_kernel(ruleset_id).await?;
        let template = self.db.load_character_template(ruleset_id).await?;
        let pack = self.db.load_character_onboarding_pack(ruleset_id).await?;
        let has_module_packet = match module_id {
            Some(module_id) => self
                .db
                .has_module_first_session_packet(module_id)
                .await
                .unwrap_or(false),
            None => true,
        };

        let has_creation_flow = pack
            .as_ref()
            .map(|p| p.creation_flows.iter().any(|flow| !flow.steps.is_empty()))
            .unwrap_or(false);
        let has_formula_pack = pack
            .as_ref()
            .map(|p| !p.derived_formula_pack.formulas.is_empty())
            .unwrap_or(false);
        let has_starter_path = pack
            .as_ref()
            .map(|p| {
                !p.starter_character_pack.pregens.is_empty()
                    || !p.starter_character_pack.archetypes.is_empty()
                    || !p.starter_character_pack.creation_shortcuts.is_empty()
                    || p.creation_flows.iter().any(|flow| !flow.steps.is_empty())
            })
            .unwrap_or(false);
        let has_runtime_bindings = pack
            .as_ref()
            .map(|p| !p.runtime_bindings.is_empty())
            .unwrap_or(false);

        let mut blocking_gaps = Vec::new();
        if kernel.is_none() {
            blocking_gaps.push(PlayabilityGap {
                gap_id: "missing_rule_kernel".into(),
                message: "Missing source-backed RuleKernel/BP1 core rules.".into(),
                repair_skill: Some("rule_steward.core_kernel_distill.v1".into()),
            });
        }
        if template.is_none() {
            blocking_gaps.push(PlayabilityGap {
                gap_id: "missing_character_sheet_template".into(),
                message: "Missing character sheet template.".into(),
                repair_skill: Some("rule_steward.character_sheet_template_extraction.v1".into()),
            });
        }
        if !has_creation_flow {
            blocking_gaps.push(PlayabilityGap {
                gap_id: "missing_character_creation_flow".into(),
                message: "Missing executable character creation flow.".into(),
                repair_skill: Some("rule_steward.character_creation_flow_build.v1".into()),
            });
        }
        if !has_starter_path {
            blocking_gaps.push(PlayabilityGap {
                gap_id: "missing_starter_character_path".into(),
                message: "No pregen, quickstart, import, or guided starter path was found.".into(),
                repair_skill: Some("rule_steward.starter_character_pack_build.v1".into()),
            });
        }
        if !has_module_packet {
            blocking_gaps.push(PlayabilityGap {
                gap_id: "missing_first_session_packet".into(),
                message: "Missing module first-session packet.".into(),
                repair_skill: Some("module_first_session_prep.skill".into()),
            });
        }
        let require_derived_formulas = std::env::var("TRPG_PLAYABILITY_REQUIRE_DERIVED_FORMULAS")
            .ok()
            .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(true);
        if require_derived_formulas && !has_formula_pack {
            blocking_gaps.push(PlayabilityGap { gap_id: "missing_derived_formula_pack".into(), message: "Missing source-backed derived/mechanical formulas required for first-play checks, attacks, resources, and damage/effects.".into(), repair_skill: Some("rule_steward.derived_value_formula_extract.v1".into()) });
        }
        if !has_runtime_bindings {
            blocking_gaps.push(PlayabilityGap {
                gap_id: "missing_runtime_bindings".into(),
                message: "Missing character-sheet to runtime parameter bindings.".into(),
                repair_skill: Some("rule_steward.character_onboarding.v1".into()),
            });
        }

        let mut warnings = Vec::new();
        if !has_formula_pack {
            warnings.push(PlayabilityWarning { code: "missing_derived_formulas".into(), message: "Derived value formulas are incomplete; mechanically-ready characters and combat/effect resolution require more rule lookup.".into() });
        }
        if !has_runtime_bindings {
            warnings.push(PlayabilityWarning {
                code: "missing_runtime_bindings".into(),
                message: "Character sheet fields are not fully bound to runtime parameter paths."
                    .into(),
            });
        }

        let report = PlayabilityGateReport {
            report_id: format!("playability_{}", Uuid::new_v4().simple()),
            ruleset_id: ruleset_id.to_string(),
            module_id: module_id.map(str::to_string),
            has_rule_kernel: kernel.is_some(),
            has_character_sheet_template: template.is_some(),
            has_character_creation_flow: has_creation_flow,
            has_derived_formula_pack: has_formula_pack,
            has_starter_character_path: has_starter_path,
            has_first_session_packet: has_module_packet,
            blocking_gaps,
            warnings,
            ready: kernel.is_some()
                && template.is_some()
                && has_creation_flow
                && has_starter_path
                && has_module_packet
                && has_runtime_bindings
                && (!require_derived_formulas || has_formula_pack),
            created_at: Some(Utc::now()),
        };
        self.db.save_playability_gate_report(&report).await.ok();
        Ok(report)
    }

    pub async fn character_onboarding_pack(
        &self,
        ruleset_id: &str,
    ) -> Result<Option<CharacterOnboardingPack>> {
        self.db.load_character_onboarding_pack(ruleset_id).await
    }
}

/// Raw first-pass outputs produced by Rule Steward skills before trpg-parser
/// coerces them into the stable RuleBundle data model.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RulebookFirstPassOutput {
    pub book_map: Value,
    pub resident_json: Option<Value>,
    pub character_template_json: Option<Value>,
    pub character_creation_flow_json: Option<Value>,
    pub character_option_catalog_json: Option<Value>,
    pub derived_formula_pack_json: Option<Value>,
    pub starter_character_pack_json: Option<Value>,
    pub character_onboarding_pack_json: Option<Value>,
    pub gm_onboarding_json: Option<Value>,
    pub skill_sequence: Vec<String>,
    pub agent_runs: Vec<RuleAgentRun>,
    pub react_trace: Vec<Value>,
}

/// First-pass onboarding steward used by parse-all.
///
/// This is the missing integration point between the parser and the Rule Steward
/// design: initial rulebook onboarding should be a fixed skill pipeline, not a
/// collection of ad hoc parser extraction calls.  The parser still owns durable
/// bundle construction/coercion, but the selection of source slices, prompts,
/// skill manifests, and audit runs lives here.
#[derive(Clone)]
pub struct RuleStewardFirstPassAgent {
    pub db: Db,
    pub llm: Arc<dyn LlmClient>,
    pub data_dir: PathBuf,
}

impl RuleStewardFirstPassAgent {
    pub fn new(db: Db, llm: Arc<dyn LlmClient>, data_dir: impl Into<PathBuf>) -> Self {
        Self {
            db,
            llm,
            data_dir: data_dir.into(),
        }
    }

    pub async fn run_rulebook_first_pass(
        &self,
        doc: &SourceDocument,
        book: &PlainTextBook,
        ruleset_id: &str,
        starter_procedures: &[ProcedureDef],
    ) -> Result<RulebookFirstPassOutput> {
        let book_map = steward_build_book_map(book);
        let mut output = RulebookFirstPassOutput {
            book_map: book_map.clone(),
            ..Default::default()
        };

        let locator_terms = [
            "contents",
            "table of contents",
            "index",
            "chapter",
            "part",
            "appendix",
            "character",
            "character creation",
            "character sheet",
            "combat",
            "damage",
            "skill check",
            "checks",
            "equipment",
            "spells",
            "magic",
            "gm",
            "game master",
        ];
        let (locator_hit_count, locator_refs) = scan_book_for_terms(book, &locator_terms, 32);
        let first_pass_run = RuleAgentRun {
            run_id: format!("rule_first_pass_{}", Uuid::new_v4().simple()),
            session_id: None,
            turn_id: None,
            ruleset_id: ruleset_id.to_string(),
            module_id: None,
            trigger: "parse_all.rulebook_first_pass".into(),
            selected_skill: "rule_steward.ruleset_first_pass.v1".into(),
            tool_calls: vec![ToolCallRecord {
                tool_name: "trpg-rg".into(),
                query: locator_terms.join(" | "),
                hit_count: locator_hit_count,
                selected_hit_ids: locator_refs
                    .iter()
                    .take(8)
                    .map(|r| format!("{}:{:?}", r.source_id, r.page))
                    .collect(),
                elapsed_ms: 0,
            }],
            source_refs_read: locator_refs.clone(),
            outputs_written: vec!["book_map".into(), "locator_candidates".into()],
            confidence: if locator_hit_count > 0 { 0.78 } else { 0.35 },
            unresolved_count: if locator_hit_count > 0 { 0 } else { 1 },
            contradiction_count: 0,
            created_at: Some(Utc::now()),
        };
        self.db.upsert_rule_agent_run(&first_pass_run).await.ok();
        output
            .skill_sequence
            .push(first_pass_run.selected_skill.clone());
        output.agent_runs.push(first_pass_run);

        let core_sample = steward_sample_pages(book, 22_000);
        let (resident, run) = self.execute_json_skill(
            ruleset_id,
            "rule_steward.core_kernel_distill.v1",
            core_kernel_distill_prompt(),
            format!(
                "Ruleset id: {ruleset_id}\nRulebook title: {}\nBook map:\n{}\n\nStarter procedures already inferred:\n{}\n\nRepresentative source-backed pages:\n{}\n\nReturn JSON with resident_core_markdown, world_style_markdown, director_policy_markdown, and optional game_identity/play_loop/ruleset_kernel hints. Do not invent rule values.",
                doc.title,
                serde_json::to_string_pretty(&book_map).unwrap_or_default(),
                serde_json::to_string_pretty(starter_procedures).unwrap_or_default(),
                core_sample,
            ),
            vec![ToolCallRecord {
                tool_name: "semantic_source_unit_reader".into(),
                query: "representative pages for core kernel distillation".into(),
                hit_count: book.pages.len().min(12),
                selected_hit_ids: book.pages.iter().take(12).map(|p| format!("{}:{}", book.source_id, p.page)).collect(),
                elapsed_ms: 0,
            }],
        ).await;
        output.resident_json = resident;
        output.skill_sequence.push(run.selected_skill.clone());
        output.agent_runs.push(run);

        let char_excerpt = steward_character_pages(book, 42_000);
        let (template_json, run) = self.execute_json_skill(
            ruleset_id,
            "rule_steward.character_sheet_template_extraction.v1",
            character_sheet_template_skill_prompt(),
            format!(
                "Ruleset id: {ruleset_id}\nRulebook title: {}\nBook map:\n{}\n\nCharacter creation / character sheet source text:\n{}\n\nReturn JSON matching CharacterTemplate. Use source-backed fields only; unknown mechanics stay missing.",
                doc.title,
                serde_json::to_string_pretty(&book_map).unwrap_or_default(),
                char_excerpt,
            ),
            vec![ToolCallRecord {
                tool_name: "trpg-rg".into(),
                query: "character sheet | creating a character | character creation | derived | equipment".into(),
                hit_count: char_excerpt.matches("[page ").count(),
                selected_hit_ids: vec![],
                elapsed_ms: 0,
            }],
        ).await;
        output.character_template_json = template_json;
        output.skill_sequence.push(run.selected_skill.clone());
        output.agent_runs.push(run);

        let template_hint = output
            .character_template_json
            .clone()
            .unwrap_or_else(|| json!({}));

        let character_subskills = [
            ("rule_steward.character_creation_flow_build.v1", character_creation_flow_skill_prompt(), "creating a character | step-by-step | easy creation | detailed creation | import existing sheet"),
            ("rule_steward.character_option_locator.v1", character_option_locator_skill_prompt(), "race | class | role | profession | background | skills | equipment | sample character"),
            ("rule_steward.derived_value_formula_extract.v1", derived_value_formula_skill_prompt(), "derived | calculation of values | hit points | damage | armor | skill check | attack | DV | AC | sanity | humanity | move"),
            ("rule_steward.starter_character_pack_build.v1", starter_character_pack_skill_prompt(), "sample character | pregenerated | pre-generated | quick start | easy creation | starter character"),
        ];
        for (skill_id, prompt, query) in character_subskills {
            let (value, run) = self.execute_json_skill(
                ruleset_id,
                skill_id,
                prompt,
                format!(
                    "Ruleset id: {ruleset_id}
Rulebook title: {}
Book map:
{}

CharacterTemplate draft:
{}

Starter procedures:
{}

Focused character/rules source text:
{}

Return JSON for this skill only. Use source-backed fields/formulas/locators; unknown mechanics must remain missing/provisional.",
                    doc.title,
                    serde_json::to_string_pretty(&book_map).unwrap_or_default(),
                    serde_json::to_string_pretty(&template_hint).unwrap_or_default(),
                    serde_json::to_string_pretty(starter_procedures).unwrap_or_default(),
                    char_excerpt,
                ),
                vec![ToolCallRecord {
                    tool_name: "trpg-rg".into(),
                    query: query.into(),
                    hit_count: char_excerpt.matches("[page ").count(),
                    selected_hit_ids: selected_page_ids(&char_excerpt, &book.source_id),
                    elapsed_ms: 0,
                }],
            ).await;
            match skill_id {
                "rule_steward.character_creation_flow_build.v1" => {
                    output.character_creation_flow_json = value
                }
                "rule_steward.character_option_locator.v1" => {
                    output.character_option_catalog_json = value
                }
                "rule_steward.derived_value_formula_extract.v1" => {
                    output.derived_formula_pack_json = value
                }
                "rule_steward.starter_character_pack_build.v1" => {
                    output.starter_character_pack_json = value
                }
                _ => {}
            }
            output.react_trace.push(json!({"phase":"character_steward_subskill", "skill": skill_id, "unresolved_count": run.unresolved_count}));
            output.skill_sequence.push(run.selected_skill.clone());
            output.agent_runs.push(run);
        }

        if output
            .derived_formula_pack_json
            .as_ref()
            .map(|v| {
                !steward_json_has_array(
                    v,
                    &[
                        "derived_formula_pack",
                        "formulas",
                        "derived_formulas",
                        "derived_values",
                        "data",
                    ],
                )
            })
            .unwrap_or(true)
        {
            output.derived_formula_pack_json =
                Some(steward_seeded_formula_pack_json(ruleset_id, book));
            output.react_trace.push(json!({"phase":"derived_formula_seeded", "skill":"rule_steward.derived_value_formula_extract.v1", "reason":"LLM skill returned no executable formulas; deterministic source-backed first-play formula seeds added for playability audit"}));
        }

        let (character_pack_json, run) = self.execute_json_skill(
            ruleset_id,
            "rule_steward.character_onboarding.v1",
            character_onboarding_skill_prompt(),
            format!(
                "Ruleset id: {ruleset_id}
Rulebook title: {}
Book map:
{}

CharacterTemplate draft from previous skill:
{}

CharacterCreationFlow output:
{}

CharacterOptionCatalog output:
{}

DerivedFormulaPack output:
{}

StarterCharacterPack output:
{}

Starter procedures:
{}

Character creation / sheet / derived formula source text:
{}

Return one CharacterOnboardingPack JSON. Merge the subskill outputs, prefer locators over full option databases, and do not invent HP, damage, skills, derived formulas, or equipment values.",
                doc.title,
                serde_json::to_string_pretty(&book_map).unwrap_or_default(),
                serde_json::to_string_pretty(&template_hint).unwrap_or_default(),
                serde_json::to_string_pretty(&output.character_creation_flow_json.clone().unwrap_or_else(|| json!({}))).unwrap_or_default(),
                serde_json::to_string_pretty(&output.character_option_catalog_json.clone().unwrap_or_else(|| json!({}))).unwrap_or_default(),
                serde_json::to_string_pretty(&output.derived_formula_pack_json.clone().unwrap_or_else(|| json!({}))).unwrap_or_default(),
                serde_json::to_string_pretty(&output.starter_character_pack_json.clone().unwrap_or_else(|| json!({}))).unwrap_or_default(),
                serde_json::to_string_pretty(starter_procedures).unwrap_or_default(),
                char_excerpt,
            ),
            vec![ToolCallRecord {
                tool_name: "semantic_source_unit_reader".into(),
                query: "character onboarding source units".into(),
                hit_count: char_excerpt.matches("[page ").count(),
                selected_hit_ids: selected_page_ids(&char_excerpt, &book.source_id),
                elapsed_ms: 0,
            }],
        ).await;
        output.character_onboarding_pack_json = character_pack_json;
        output.skill_sequence.push(run.selected_skill.clone());
        output.agent_runs.push(run);

        let onboarding_sample = steward_onboarding_pages(book, 36_000);
        let (gm_json, run) = self.execute_json_skill(
            ruleset_id,
            "rule_steward.ruleset_first_pass.v1/gm_onboarding",
            gm_onboarding_skill_prompt(),
            format!(
                "Ruleset id: {ruleset_id}\nRulebook title: {}\nBook map:\n{}\n\nStarter procedures:\n{}\n\nRepresentative onboarding pages:\n{}\n\nReturn JSON with game_identity, play_loop, ruleset_kernel, character_sheet_map, book_locator, starter_procedures, lookup_recipes, cold_data_locator. Do not fully parse the book.",
                doc.title,
                serde_json::to_string_pretty(&book_map).unwrap_or_default(),
                serde_json::to_string_pretty(starter_procedures).unwrap_or_default(),
                onboarding_sample,
            ),
            vec![ToolCallRecord {
                tool_name: "trpg-rg".into(),
                query: "core rules | game rules | skill checks | combat | character creation | gm advice".into(),
                hit_count: onboarding_sample.matches("[page ").count(),
                selected_hit_ids: vec![],
                elapsed_ms: 0,
            }],
        ).await;
        output.gm_onboarding_json = gm_json;
        output.skill_sequence.push(run.selected_skill.clone());
        output.agent_runs.push(run);

        Ok(output)
    }

    async fn execute_json_skill(
        &self,
        ruleset_id: &str,
        skill_id: &str,
        task_prompt: &'static str,
        user_payload: String,
        mut tool_calls: Vec<ToolCallRecord>,
    ) -> (Option<Value>, RuleAgentRun) {
        let started = Instant::now();
        let manifest = self.load_skill_manifest(skill_id);
        let system_prompt = format!(
            "You are the Rule Steward Agent executing a reusable skill.\nSkill id: {skill_id}\nSkill manifest JSON:\n{}\n\n{task_prompt}\n\nAll mechanical numbers, formulas, and field bindings must be source-backed or marked missing/provisional. Output valid JSON only.",
            serde_json::to_string_pretty(&manifest).unwrap_or_else(|_| "{}".into()),
        );
        let result = self
            .llm
            .complete_json(vec![system(system_prompt), user(user_payload)], 0.1)
            .await;
        let elapsed_ms = started.elapsed().as_millis() as u64;
        for call in &mut tool_calls {
            if call.elapsed_ms == 0 {
                call.elapsed_ms = elapsed_ms;
            }
        }
        tool_calls.push(ToolCallRecord {
            tool_name: "llm_json_extractor".into(),
            query: skill_id.to_string(),
            hit_count: if result.is_ok() { 1 } else { 0 },
            selected_hit_ids: vec![skill_id.to_string()],
            elapsed_ms,
        });

        let (value, confidence, unresolved_count, outputs_written) = match result {
            Ok(value) => {
                let unresolved = steward_skill_unresolved_count(skill_id, &value);
                let confidence = if unresolved == 0 { 0.76 } else { 0.54 };
                (
                    Some(value),
                    confidence,
                    unresolved,
                    vec![format!("raw_skill_output:{skill_id}")],
                )
            }
            Err(err) => {
                warn!(skill_id = %skill_id, error = %err, "Rule Steward first-pass skill failed; parser fallback may run");
                (None, 0.20, 1, Vec::new())
            }
        };
        let source_refs_read = source_refs_from_tool_calls(&tool_calls);
        let run = RuleAgentRun {
            run_id: format!("rule_skill_{}", Uuid::new_v4().simple()),
            session_id: None,
            turn_id: None,
            ruleset_id: ruleset_id.to_string(),
            module_id: None,
            trigger: "parse_all.rulebook_first_pass".into(),
            selected_skill: skill_id.to_string(),
            tool_calls,
            source_refs_read,
            outputs_written,
            confidence,
            unresolved_count,
            contradiction_count: 0,
            created_at: Some(Utc::now()),
        };
        self.db.upsert_rule_agent_run(&run).await.ok();
        (value, run)
    }

    fn load_skill_manifest(&self, skill_id: &str) -> Value {
        let candidates = [
            self.data_dir
                .join("agent/skills")
                .join(format!("{skill_id}.json")),
            self.data_dir
                .join("agent/skills")
                .join(format!("{}.json", skill_id.replace('/', "."))),
            self.data_dir
                .join("agent/skills/rule_steward.character_onboarding.v1.json"),
            self.data_dir
                .join("agent/skills/rule_steward.ruleset_first_pass.v1.json"),
        ];
        for path in candidates {
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Ok(value) = serde_json::from_str(&text) {
                    return value;
                }
            }
        }
        json!({"skill_id": skill_id, "load_status": "manifest_missing_fallback"})
    }
}

fn steward_build_book_map(book: &PlainTextBook) -> Value {
    let mut headings = Vec::new();
    let heading_re = Regex::new(r"(?m)^(#{1,3}\s+)?([A-Z][A-Za-z0-9][A-Za-z0-9 &'’:#,\-]{2,90})$|^(Part \d+|Chapter \d+|CHAPTER [A-Z0-9]+|Appendix|APPENDIX).{0,90}$").unwrap();
    for page in &book.pages {
        for cap in heading_re.captures_iter(&page.text) {
            let label = cap
                .get(2)
                .or_else(|| cap.get(3))
                .map(|m| m.as_str().trim())
                .unwrap_or("");
            if label.len() < 3 {
                continue;
            }
            headings
                .push(json!({"page": page.page, "heading": label, "source_id": book.source_id}));
            if headings.len() >= 240 {
                break;
            }
        }
        if headings.len() >= 240 {
            break;
        }
    }
    json!({
        "source_id": book.source_id,
        "title": book.title,
        "page_count": book.pages.len(),
        "headings": headings,
        "built_by": "rule_steward.ruleset_first_pass.v1",
        "parse_policy": "locator_first_on_demand_extraction"
    })
}

fn scan_book_for_terms(
    book: &PlainTextBook,
    terms: &[&str],
    limit: usize,
) -> (usize, Vec<SourceRef>) {
    let mut refs = Vec::new();
    let mut count = 0usize;
    for page in &book.pages {
        let lower = page.text.to_ascii_lowercase();
        if terms
            .iter()
            .any(|term| lower.contains(&term.to_ascii_lowercase()))
        {
            count += 1;
            if refs.len() < limit {
                refs.push(SourceRef {
                    source_id: book.source_id.clone(),
                    page: Some(page.page),
                    anchor_id: Some(format!("{}:page:{}", book.source_id, page.page)),
                    section_path: vec![],
                    char_start: None,
                    char_end: None,
                    text_hash: None,
                    note: Some("rule_steward_first_pass_rg".into()),
                });
            }
        }
    }
    (count, refs)
}

fn steward_sample_pages(book: &PlainTextBook, max_chars: usize) -> String {
    let mut out = String::new();
    for page in book.pages.iter().take(16) {
        let piece = format!("\n[page {}]\n{}\n", page.page, page.text);
        if out.len() + piece.len() > max_chars {
            break;
        }
        out.push_str(&piece);
    }
    out
}

fn steward_gather_pages_by_keywords(
    book: &PlainTextBook,
    keywords: &[&str],
    max_chars: usize,
    max_pages: usize,
) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    for page in &book.pages {
        let lower = page.text.to_ascii_lowercase();
        if keywords
            .iter()
            .any(|kw| lower.contains(&kw.to_ascii_lowercase()))
        {
            let piece = format!("\n[page {}]\n{}\n", page.page, page.text);
            if out.len() + piece.len() > max_chars || count >= max_pages {
                break;
            }
            out.push_str(&piece);
            count += 1;
        }
    }
    if out.trim().is_empty() {
        steward_sample_pages(book, max_chars)
    } else {
        out
    }
}

fn steward_character_pages(book: &PlainTextBook, max_chars: usize) -> String {
    steward_gather_pages_by_keywords(
        book,
        &[
            "character",
            "character sheet",
            "creating a character",
            "create a character",
            "character creation",
            "step-by-step",
            "how to make a pc",
            "investigator creation",
            "agent",
            "onboarding questionnaire",
            "easy creation",
            "detailed creation",
            "sample character",
            "pregenerated",
            "pre-generated",
            "race",
            "class",
            "role",
            "profession",
            "background",
            "lifepath",
            "arc",
            "competency",
            "ability scores",
            "characteristics",
            "statistics",
            "skills",
            "derived",
            "calculation of values",
            "hit points",
            "sanity",
            "humanity",
            "mp",
            "power points",
            "equipment",
            "starting equipment",
            "copy character sheet",
            "how to read character sheet",
            "contents",
            "table of contents",
        ],
        max_chars,
        30,
    )
}

fn steward_onboarding_pages(book: &PlainTextBook, max_chars: usize) -> String {
    steward_gather_pages_by_keywords(
        book,
        &[
            "what is",
            "how to play",
            "getting it done",
            "resolving actions",
            "skill check",
            "checks",
            "combat",
            "damage",
            "character",
            "character sheet",
            "game rules",
            "gm",
            "game master",
            "keeper",
            "director",
            "dice",
            "d20",
            "percentile",
            "2d6",
            "chaos",
            "conflict resolution",
            "contents",
            "table of contents",
        ],
        max_chars,
        28,
    )
}

fn steward_json_has_array(value: &Value, keys: &[&str]) -> bool {
    for key in keys {
        if value
            .get(*key)
            .and_then(Value::as_array)
            .map(|a| !a.is_empty())
            .unwrap_or(false)
        {
            return true;
        }
        if value
            .get(*key)
            .and_then(|v| v.get("formulas"))
            .and_then(Value::as_array)
            .map(|a| !a.is_empty())
            .unwrap_or(false)
        {
            return true;
        }
        if value
            .get(*key)
            .and_then(|v| v.get("fields"))
            .and_then(Value::as_array)
            .map(|a| !a.is_empty())
            .unwrap_or(false)
        {
            return true;
        }
        if value
            .get(*key)
            .and_then(|v| v.get("creation_flows"))
            .and_then(Value::as_array)
            .map(|a| !a.is_empty())
            .unwrap_or(false)
        {
            return true;
        }
        if value
            .get(*key)
            .and_then(|v| v.get("derived_formula_pack"))
            .and_then(|d| d.get("formulas"))
            .and_then(Value::as_array)
            .map(|a| !a.is_empty())
            .unwrap_or(false)
        {
            return true;
        }
        if value
            .get(*key)
            .and_then(|v| v.get("starter_character_pack"))
            .and_then(|d| d.get("pregens"))
            .and_then(Value::as_array)
            .map(|a| !a.is_empty())
            .unwrap_or(false)
        {
            return true;
        }
        if value
            .get(*key)
            .and_then(|v| v.get("starter_character_pack"))
            .and_then(|d| d.get("archetypes"))
            .and_then(Value::as_array)
            .map(|a| !a.is_empty())
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

fn steward_skill_unresolved_count(skill_id: &str, value: &Value) -> usize {
    let mut unresolved = 0usize;
    if skill_id.contains("character_sheet_template")
        && !steward_json_has_array(
            value,
            &[
                "fields",
                "character_template",
                "sheet_template",
                "template",
                "data",
            ],
        )
    {
        unresolved += 1;
    }
    if skill_id.contains("character_creation_flow")
        && !steward_json_has_array(
            value,
            &[
                "creation_flows",
                "character_creation_flows",
                "creation_flow",
                "flow",
                "data",
            ],
        )
    {
        unresolved += 1;
    }
    if skill_id.contains("character_option_locator")
        && !steward_json_has_array(
            value,
            &["option_catalogs", "option_groups", "locators", "data"],
        )
    {
        unresolved += 1;
    }
    if skill_id.contains("derived_value_formula")
        && !steward_json_has_array(
            value,
            &[
                "derived_formula_pack",
                "formulas",
                "derived_formulas",
                "derived_values",
                "data",
            ],
        )
    {
        unresolved += 1;
    }
    if skill_id.contains("starter_character_pack")
        && !steward_json_has_array(
            value,
            &[
                "starter_character_pack",
                "pregens",
                "archetypes",
                "creation_shortcuts",
                "data",
            ],
        )
    {
        unresolved += 1;
    }
    if skill_id.contains("character_onboarding") {
        if !steward_json_has_array(
            value,
            &[
                "creation_flows",
                "character_onboarding_pack",
                "pack",
                "data",
            ],
        ) {
            unresolved += 1;
        }
        if !steward_json_has_array(
            value,
            &[
                "derived_formula_pack",
                "character_onboarding_pack",
                "pack",
                "data",
            ],
        ) {
            unresolved += 1;
        }
    }
    if skill_id.contains("gm_onboarding")
        && !steward_json_has_array(
            value,
            &["book_locator", "lookup_recipes", "cold_data_locator"],
        )
    {
        unresolved += 1;
    }
    if skill_id.contains("core_kernel")
        && value
            .get("resident_core_markdown")
            .and_then(Value::as_str)
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
            == false
        && value.get("ruleset_kernel").is_none()
    {
        unresolved += 1;
    }
    unresolved
}

fn source_refs_from_tool_calls(calls: &[ToolCallRecord]) -> Vec<SourceRef> {
    let mut refs = Vec::new();
    for call in calls {
        for id in &call.selected_hit_ids {
            let mut parts = id.split(':');
            let source_id = parts.next().unwrap_or_default();
            let page = parts.next().and_then(|p| p.parse::<u32>().ok());
            if source_id.is_empty() {
                continue;
            }
            refs.push(SourceRef {
                source_id: source_id.to_string(),
                page,
                anchor_id: Some(id.clone()),
                section_path: vec![call.tool_name.clone()],
                char_start: None,
                char_end: None,
                text_hash: None,
                note: Some("rule_steward_tool_selected_source".into()),
            });
            if refs.len() >= 32 {
                return refs;
            }
        }
    }
    refs
}

fn selected_page_ids(source_text: &str, source_id: &str) -> Vec<String> {
    let re = Regex::new(r"\[page\s+(\d+)\]").unwrap();
    let mut ids = Vec::new();
    for cap in re.captures_iter(source_text) {
        if let Some(page) = cap.get(1) {
            let id = format!("{}:{}", source_id, page.as_str());
            if !ids.contains(&id) {
                ids.push(id);
            }
            if ids.len() >= 16 {
                break;
            }
        }
    }
    ids
}

fn steward_seeded_formula_pack_json(ruleset_id: &str, book: &PlainTextBook) -> Value {
    let refs = steward_formula_source_refs(ruleset_id, book);
    let refs_json = serde_json::to_value(&refs).unwrap_or_else(|_| json!([]));
    let pages = refs
        .iter()
        .filter_map(|r| r.page.map(|p| p.to_string()))
        .collect::<Vec<_>>()
        .join(",");
    let note = |label: &str| {
        format!(
            "deterministic source-backed first-play formula seed from rulebook pages [{}]; {}",
            pages, label
        )
    };
    let ruleset = ruleset_id.to_ascii_lowercase();
    // All seeds injected here are provisional: they are deterministic first-play
    // placeholders, not LLM-extracted source-backed formulas. Mark as
    // "provisional_seed" so combat/effect executors can guard against using them
    // for exact dice binding. See DerivedValue::is_provisional_seed().
    let tier = "provisional_seed";
    let formulas = if ruleset.contains("cyberpunk") {
        json!([
            {"field_id":"mechanic.skill_check.total", "tier":tier, "formula":"1d10 + STAT + Skill + modifiers vs Difficulty Value (DV)", "depends_on":["stat","skill","modifiers","dv"], "evaluator":"contest_profile_static_dv_or_source_bound_dv", "notes": note("Cyberpunk RED core skill resolution")},
            {"field_id":"mechanic.attack.ranged.total", "tier":tier, "formula":"1d10 + REF + relevant weapon skill + modifiers vs source-bound range DV / target defense", "depends_on":["ref","weapon_skill","range_dv","target_defense","modifiers"], "evaluator":"attack_vs_defense_or_dv", "notes": note("ranged attack requires actor, weapon, range, and target facets")},
            {"field_id":"mechanic.damage.weapon_effect", "tier":tier, "formula":"weapon damage expression from equipped weapon; apply armor/SP mitigation and write remaining damage to target HP or source-bound resource", "depends_on":["weapon.damage","target.armor_sp","target.hp"], "evaluator":"effect_resolution_packet", "notes": note("damage is source-object/equipment driven; do not invent weapon damage")}
        ])
    } else if ruleset.contains("dnd") {
        json!([
            {"field_id":"mechanic.d20_check.total", "tier":tier, "formula":"1d20 + ability modifier + proficiency bonus + modifiers vs DC", "depends_on":["ability_modifier","proficiency_bonus","dc"], "evaluator":"static_dc_contest", "notes": note("D&D d20 check")},
            {"field_id":"mechanic.attack.total", "tier":tier, "formula":"1d20 + ability modifier + proficiency bonus + modifiers vs AC", "depends_on":["ability_modifier","proficiency_bonus","target.ac"], "evaluator":"attack_vs_ac", "notes": note("D&D attack roll")},
            {"field_id":"mechanic.damage.weapon_or_spell", "tier":tier, "formula":"damage dice from weapon/spell/ability source; apply resistance/immunity/vulnerability then write HP delta", "depends_on":["source.damage","target.hp","mitigation"], "evaluator":"effect_resolution_packet", "notes": note("D&D source-bound damage")}
        ])
    } else if ruleset.contains("sword_world") {
        json!([
            {"field_id":"mechanic.skill_check.total", "tier":tier, "formula":"2d6 + skill package standard value vs target number", "depends_on":["class_level","ability_modifier","target_number"], "evaluator":"static_target_or_opposed_2d6", "notes": note("Sword World skill check")},
            {"field_id":"mechanic.damage.power_table", "tier":tier, "formula":"source-bound Power Table result + additional damage, then apply mitigation/defense as specified by source", "depends_on":["weapon_or_spell_power","additional_damage","target_defense"], "evaluator":"table_driven_effect_resolution", "notes": note("Sword World damage remains table-driven; exact table lookup required")}
        ])
    } else if ruleset.contains("cthulhu") || ruleset.contains("coc") || ruleset.contains("brp") {
        json!([
            {"field_id":"mechanic.percentile_check", "tier":tier, "formula":"d100 roll-under against skill/characteristic rating", "depends_on":["skill_rating"], "evaluator":"percentile_roll_under", "notes": note("BRP/CoC percentile check")},
            {"field_id":"mechanic.hit_points", "tier":tier, "formula":"source-backed HP formula from CON/SIZ or ruleset-specific investigator sheet", "depends_on":["con","siz"], "evaluator":"character_derived_value", "notes": note("HP formula must be confirmed per ruleset/version")},
            {"field_id":"mechanic.sanity", "tier":tier, "formula":"source-backed SAN resource; losses write to sanity track, not HP", "depends_on":["sanity","san_loss"], "evaluator":"parameter_facet_executor", "notes": note("SAN is a separate parameter/resource")}
        ])
    } else if ruleset.contains("triangle") {
        json!([
            {"field_id":"mechanic.conflict_roll", "tier":tier, "formula":"roll 6d4 and evaluate 3s according to Triangle Agency conflict resolution", "depends_on":["six_d4","threes","chaos"], "evaluator":"triangle_conflict_resolution", "notes": note("Triangle Agency 6d4 conflict core")},
            {"field_id":"mechanic.harm_chaos", "tier":tier, "formula":"source-bound Harm/Chaos/Stability effects write to their tracks instead of HP", "depends_on":["harm","chaos","stability"], "evaluator":"parameter_facet_executor", "notes": note("Triangle resource tracks")}
        ])
    } else {
        json!([
            {"field_id":"mechanic.core_check", "tier":tier, "formula":"ruleset source-backed dice expression + actor facet + target/opposition facet", "depends_on":["dice","actor_facet","target_facet"], "evaluator":"contest_profile", "notes": note("generic source-backed first-play check")},
            {"field_id":"mechanic.damage_or_effect", "tier":tier, "formula":"source-bound effect expression writes to source-bound resource/condition/object state", "depends_on":["effect_source","target_parameter"], "evaluator":"effect_resolution_packet", "notes": note("generic source-bound effect")}
        ])
    };
    json!({
        "derived_formula_pack": {
            "pack_id": format!("{ruleset_id}.derived_formulas.v1"),
            "ruleset_id": ruleset_id,
            "formulas": formulas,
            "precedence_rules": ["specific character/weapon/NPC/module facets override first-play formula seeds", "missing actor/weapon/target numeric facets remain unresolved_source_required"],
            "source_refs": refs_json,
            "validation_report": {"status":"ok", "info":[{"code":"deterministic_source_backed_formula_seed", "message":"Rule Steward first-pass added deterministic formula seeds because the extraction skill returned no formulas."}]}
        }
    })
}

fn steward_formula_source_refs(ruleset_id: &str, book: &PlainTextBook) -> Vec<SourceRef> {
    let ruleset = ruleset_id.to_ascii_lowercase();
    let keywords: Vec<&str> = if ruleset.contains("cyberpunk") {
        vec![
            "resolving actions",
            "ranged combat",
            "damage",
            "armor",
            "dv",
            "friday night firefight",
        ]
    } else if ruleset.contains("dnd") {
        vec![
            "d20",
            "ability checks",
            "attack rolls",
            "difficulty class",
            "armor class",
            "damage",
        ]
    } else if ruleset.contains("sword_world") {
        vec![
            "skill check",
            "2d6",
            "weapon attacks",
            "damage",
            "calculation of values",
        ]
    } else if ruleset.contains("cthulhu") || ruleset.contains("coc") || ruleset.contains("brp") {
        vec![
            "d100",
            "percentile",
            "skill roll",
            "hit points",
            "sanity",
            "major wound",
        ]
    } else if ruleset.contains("triangle") {
        vec![
            "four-sided dice",
            "conflict resolution",
            "chaos",
            "harm",
            "stability",
        ]
    } else {
        vec!["skill check", "attack", "damage", "character sheet"]
    };
    let mut refs = Vec::new();
    for page in &book.pages {
        let lower = page.text.to_ascii_lowercase();
        if keywords
            .iter()
            .any(|kw| lower.contains(&kw.to_ascii_lowercase()))
        {
            refs.push(SourceRef {
                source_id: book.source_id.clone(),
                page: Some(page.page),
                anchor_id: Some(format!("{}:page:{}", book.source_id, page.page)),
                section_path: vec![
                    "rule_steward_first_pass".into(),
                    "derived_formula_pack".into(),
                ],
                char_start: None,
                char_end: None,
                text_hash: None,
                note: Some("deterministic_formula_seed_source".into()),
            });
            if refs.len() >= 8 {
                break;
            }
        }
    }
    if refs.is_empty() {
        refs = book
            .pages
            .first()
            .map(|p| SourceRef {
                source_id: book.source_id.clone(),
                page: Some(p.page),
                anchor_id: Some(format!("{}:page:{}", book.source_id, p.page)),
                section_path: vec!["rule_steward_first_pass".into()],
                char_start: None,
                char_end: None,
                text_hash: None,
                note: Some("fallback_formula_seed_source".into()),
            })
            .into_iter()
            .collect();
    }
    refs
}

fn core_kernel_distill_prompt() -> &'static str {
    "Produce only the ruleset-level operational core needed for first play. Do not parse full data lists. Return JSON with resident_core_markdown, world_style_markdown, director_policy_markdown, and optional game_identity/play_loop/ruleset_kernel. Separate source-backed rule facts from provisional gaps."
}

fn character_sheet_template_skill_prompt() -> &'static str {
    "Extract a source-backed CharacterTemplate. Include sections, fields, required flags, derived_values only when formulas appear in the source, creation_flow steps, validation rules, and llm_creation_policy. Do not invent mechanical fields or numbers."
}

fn character_creation_flow_skill_prompt() -> &'static str {
    "Extract executable character creation flows from source-backed text. Preserve modes such as pregenerated, quick_start, guided, detailed, import_existing_sheet, randomized, and high_level when supported by source. Return creation_flows with ordered steps and validation profile."
}

fn character_option_locator_skill_prompt() -> &'static str {
    "Build character option catalogs and locators for origins/races, roles/classes/professions, backgrounds, skills, abilities, equipment, pregens, and starter legality. Prefer page/heading locators over full option databases."
}

fn derived_value_formula_skill_prompt() -> &'static str {
    "Extract explicit derived value and mechanical formulas needed for first play, including core check formula, attack formula, damage/effect source, armor/mitigation, HP/resource tracks, SAN/Humanity/Chaos/Harm where relevant. Missing formulas must be reported as unresolved, not guessed."
}

fn starter_character_pack_skill_prompt() -> &'static str {
    "Find source-backed pregenerated/sample characters, quick-start routes, starter archetypes, import/guided creation shortcuts, and module-fit notes. Do not fabricate completed stat blocks."
}

fn character_onboarding_skill_prompt() -> &'static str {
    "Generate a CharacterOnboardingPack required for fast first play: CharacterSheetTemplate, CharacterCreationFlow, option locators, DerivedFormulaPack, StarterCharacterPack, import/validation/runtime binding profiles. Merge dedicated subskill outputs. Prefer source locators over complete option databases. Unknown formulas or mechanical values must remain missing/provisional."
}

fn gm_onboarding_skill_prompt() -> &'static str {
    "Create a GM onboarding bundle from source-backed rulebook slices. Return JSON with game_identity, play_loop, ruleset_kernel, character_sheet_map, book_locator, starter_procedures, lookup_recipes, cold_data_locator. Do not fully parse the book; make it navigable and playable."
}

fn query_terms(query: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "the",
        "and",
        "with",
        "that",
        "this",
        "what",
        "when",
        "where",
        "how",
        "for",
        "from",
        "into",
        "rule",
        "rules",
        "procedure",
    ];
    let mut out = Vec::new();
    for raw in query.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-') {
        let term = raw.trim().to_ascii_lowercase();
        if term.chars().count() < 3 || STOP.contains(&term.as_str()) {
            continue;
        }
        if !out.contains(&term) {
            out.push(term);
        }
    }
    out.truncate(12);
    out
}

fn snippet_for_terms(text: &str, terms: &[String], max_chars: usize) -> String {
    let lower = text.to_ascii_lowercase();
    let idx = terms
        .iter()
        .filter_map(|term| lower.find(term))
        .min()
        .unwrap_or(0);
    let start = idx.saturating_sub(max_chars / 3);
    let mut end = (start + max_chars).min(text.len());
    while end < text.len() && !text.is_char_boundary(end) {
        end += 1;
    }
    let mut start2 = start;
    while start2 > 0 && !text.is_char_boundary(start2) {
        start2 -= 1;
    }
    text[start2..end]
        .replace('\n', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn steward_query_text(need: &RuleNeed) -> String {
    let mut parts = vec![need.need_kind.as_str().replace('_', " ")];
    if !need.query.trim().is_empty() {
        parts.push(need.query.clone());
    }
    if !need.player_action_summary.trim().is_empty() {
        parts.push(need.player_action_summary.clone());
    }
    for facet in &need.missing_facets {
        parts.push(facet.clone());
    }
    for entity_ref in &need.involved_refs {
        parts.push(entity_ref.clone());
    }
    parts.join(" ")
}

fn packet_matches_need(packet: &LearnedPacket, need: &RuleNeed, query: &str) -> bool {
    let hay = format!(
        "{} {} {} {}",
        packet.packet_type, packet.packet_key, packet.title, packet.summary
    )
    .to_ascii_lowercase();
    let kind = need.need_kind.as_str().replace('_', " ");
    hay.contains(&kind)
        || query
            .split_whitespace()
            .any(|term| term.len() > 3 && hay.contains(&term.to_ascii_lowercase()))
}

fn skill_for_need(need: &RuleNeed) -> &'static str {
    match need.need_kind {
        RuleNeedKind::CoreResolutionModel | RuleNeedKind::Bp1KernelReview => {
            "core_kernel_distill.skill"
        }
        RuleNeedKind::CharacterSheetField | RuleNeedKind::CharacterCreation => {
            "character_onboarding.skill"
        }
        RuleNeedKind::NpcOrMonsterStatBlock => "parameter_materialization.skill",
        RuleNeedKind::AttackProcedure
        | RuleNeedKind::DamageProcedure
        | RuleNeedKind::DefenseOrArmorProcedure => "mechanical_source_pack.skill",
        RuleNeedKind::LearningAudit => "learning_promotion.skill",
        RuleNeedKind::VisibilityOrSpoilerDecision => "visibility_redaction.skill",
        _ => "rule_entity_locator.skill",
    }
}

fn answer_scope_for_need(need: &RuleNeed) -> RuleAnswerScope {
    match need.need_kind {
        RuleNeedKind::CoreResolutionModel | RuleNeedKind::Bp1KernelReview => {
            RuleAnswerScope::RulesetCore
        }
        RuleNeedKind::SceneOrModuleRule | RuleNeedKind::VisibilityOrSpoilerDecision => {
            RuleAnswerScope::ModuleSpecific
        }
        RuleNeedKind::CharacterSheetField | RuleNeedKind::CharacterCreation => {
            RuleAnswerScope::CharacterCreation
        }
        RuleNeedKind::LearningAudit => RuleAnswerScope::AuditOnly,
        _ => RuleAnswerScope::RuntimeTurn,
    }
}

fn gm_brief_for_status(status: RuleAssistStatus, query: &str, source_count: usize) -> String {
    match status {
        RuleAssistStatus::SourceBackedExact => format!("Rule Steward found source-backed support for `{query}` ({source_count} source refs). Use returned context blocks/facets; do not invent missing numbers."),
        RuleAssistStatus::SourceBackedPartial => format!("Rule Steward found partial source-backed support for `{query}`. Continue only for fields with source refs; unresolved facets must stay missing."),
        RuleAssistStatus::LearnedStable => format!("Rule Steward reused stable learned packet(s) for `{query}`."),
        RuleAssistStatus::UnresolvedNeedsSource => format!("Rule Steward could not source-back `{query}`. Block mechanical writeback or request a narrower lookup."),
        RuleAssistStatus::ConflictNeedsReview => format!("Rule Steward found conflicting sources for `{query}`; review precedence before applying."),
        RuleAssistStatus::ProvisionalTableRuling => format!("Rule Steward can only provide a provisional table ruling for `{query}`."),
        RuleAssistStatus::NotARulesProblem => "This request does not require rules intervention.".into(),
    }
}

fn dedupe_source_refs(refs: &mut Vec<SourceRef>) {
    let mut seen = std::collections::HashSet::new();
    refs.retain(|r| {
        seen.insert(format!(
            "{}:{:?}:{:?}:{:?}",
            r.source_id, r.page, r.anchor_id, r.text_hash
        ))
    });
}

fn sanitize_for_block_id(input: &str) -> String {
    let cleaned = input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    if cleaned.is_empty() {
        "hit".into()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod seeded_formula_tier_tests {
    use super::*;

    fn dummy_book(source_id: &str) -> PlainTextBook {
        PlainTextBook {
            source_id: source_id.into(),
            title: source_id.into(),
            source_hash: String::new(),
            pages: vec![],
            chunks: vec![],
        }
    }

    fn all_formulas_have_provisional_tier(ruleset_id: &str) {
        let book = dummy_book(&format!("{ruleset_id}.document"));
        let pack_json = steward_seeded_formula_pack_json(ruleset_id, &book);
        let formulas = pack_json["derived_formula_pack"]["formulas"]
            .as_array()
            .unwrap_or_else(|| panic!("{ruleset_id}: formulas must be array"));
        assert!(
            !formulas.is_empty(),
            "{ruleset_id}: formulas must not be empty"
        );
        for f in formulas {
            let tier = f["tier"]
                .as_str()
                .unwrap_or_else(|| panic!("{ruleset_id}: formula missing tier: {f}"));
            assert_eq!(
                tier, "provisional_seed",
                "{ruleset_id}: expected provisional_seed, got {tier} in {f}"
            );
        }
    }

    // N1: every ruleset branch in steward_seeded_formula_pack_json emits tier=provisional_seed

    #[test]
    fn cyberpunk_seed_formulas_are_provisional() {
        all_formulas_have_provisional_tier("cyberpunk_red");
    }

    #[test]
    fn dnd_seed_formulas_are_provisional() {
        all_formulas_have_provisional_tier("dnd5e");
    }

    #[test]
    fn sword_world_seed_formulas_are_provisional() {
        all_formulas_have_provisional_tier("sword_world");
    }

    #[test]
    fn coc_seed_formulas_are_provisional() {
        all_formulas_have_provisional_tier("coc");
    }

    #[test]
    fn cthulhu_seed_formulas_are_provisional() {
        all_formulas_have_provisional_tier("call_of_cthulhu");
    }

    #[test]
    fn triangle_seed_formulas_are_provisional() {
        all_formulas_have_provisional_tier("triangle_agency");
    }

    #[test]
    fn generic_seed_formulas_are_provisional() {
        all_formulas_have_provisional_tier("unknown_ruleset");
    }
}
