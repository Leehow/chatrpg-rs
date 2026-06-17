use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::Row;
use trpg_db::Db;
use trpg_llm::{LlmClient, LlmConfig, OpenAiCompatibleClient};
use trpg_model::*;
use trpg_search::SearchService;
use trpg_semantics::{SemanticIntentService, SemanticRuleBindingService};
use uuid::Uuid;

/// On-demand object-schema compilation: fills a `discovered` category stub on
/// first play-time use (Optimization 2 of staged parsing).
mod staged_extract;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub enum MaterializationWritePolicy {
    #[default]
    WriteRuntimeState,
    EvidenceOnly,
}

#[derive(Debug, Clone, Copy)]
pub struct MaterializationTurnInput<'a> {
    pub session_id: &'a str,
    pub turn_id: &'a str,
    pub ruleset_id: &'a str,
    pub module_id: Option<&'a str>,
    pub frame_id: Option<&'a str>,
    pub actor_id: Option<&'a str>,
    pub user_input: &'a str,
    pub world_tick: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MaterializationTurnResult {
    pub handled: bool,
    pub phases: Vec<String>,
    pub semantic: Option<SemanticIntentResult>,
    pub demands: Vec<MaterializationDemand>,
    pub evidence_bundles: Vec<SourceEvidenceBundle>,
    pub extraction_runs: Vec<ExtractionRun>,
    pub verifications: Vec<BindingVerificationResult>,
    pub rule_bindings: Vec<RuleBindingPacket>,
    pub writebacks: Vec<RuntimeBindingWriteback>,
    pub narration_context: Option<String>,
}

impl MaterializationTurnResult {
    pub fn not_handled() -> Self { Self::default() }
}

#[derive(Clone)]
pub struct MaterializationService {
    pub db: Db,
    pub search: Option<SearchService>,
    pub llm: Option<OpenAiCompatibleClient>,
}

impl MaterializationService {
    pub fn from_env(db: Db, search: Option<SearchService>) -> Self {
        let llm = if env_bool("TRPG_REAL_MATERIALIZATION_EXTRACTOR_ENABLE_V110", true) {
            LlmConfig::from_env().ok().and_then(|cfg| OpenAiCompatibleClient::new(cfg).ok())
        } else { None };
        Self { db, search, llm }
    }

    pub async fn materialize_turn(&self, input: MaterializationTurnInput<'_>) -> Result<MaterializationTurnResult> {
        if !env_bool("TRPG_REAL_MATERIALIZATION_ENABLE_V110", true) {
            return Ok(MaterializationTurnResult::not_handled());
        }
        let sem_req = SemanticIntentRequest {
            session_id: input.session_id.to_string(),
            turn_id: input.turn_id.to_string(),
            ruleset_id: input.ruleset_id.to_string(),
            module_id: input.module_id.map(str::to_string),
            active_frame_summary: json!({"frame_id": input.frame_id}),
            active_gate_summary: json!({}),
            player_input: input.user_input.to_string(),
            visible_scene_objects: json!({}),
            actor_parameter_summary: json!({"actor_id": input.actor_id.unwrap_or("pc.current")}),
        };
        let semantic = SemanticIntentService::from_env(self.db.clone()).classify_turn(sem_req).await.ok();
        let mut requests = semantic.as_ref().map(|s| s.materialization_requests.clone()).unwrap_or_default();
        requests.extend(self.demands_from_runtime_context(input, semantic.as_ref()));
        // Deduplicate by target kind/label/target id so the LLM can ask broadly without multiplying DB rows.
        let mut unique = Vec::new();
        for req in requests {
            if req.target_label.trim().is_empty() { continue; }
            let key = (req.target_kind.as_str().to_string(), req.target_id.clone(), normalize_label(&req.target_label));
            if unique.iter().any(|x: &MaterializationRequest| (x.target_kind.as_str().to_string(), x.target_id.clone(), normalize_label(&x.target_label)) == key) { continue; }
            unique.push(req);
        }
        if unique.is_empty() {
            return Ok(MaterializationTurnResult { semantic, ..Default::default() });
        }
        let mut out = MaterializationTurnResult { handled: true, semantic, phases: vec!["materialization_kernel".into()], ..Default::default() };
        for req in unique {
            let demand = self.create_demand(input, &req, out.semantic.as_ref()).await?;
            let bundle = self.collect_evidence(&demand, &req).await?;
            let extraction = self.extract(&demand, &bundle).await?;
            let packet = self.binding_packet(input.session_id, input.turn_id, &demand, &req, &bundle, &extraction).await?;
            let verification = self.verify(&demand, &packet, &extraction).await?;
            let writeback = self.write_back(&demand, &packet, &extraction, &verification).await?;
            self.update_demand_status(&demand.demand_id, status_from_verification(&verification)).await.ok();
            out.demands.push(demand);
            out.evidence_bundles.push(bundle);
            out.extraction_runs.push(extraction);
            out.rule_bindings.push(packet);
            out.verifications.push(verification);
            out.writebacks.extend(writeback);
        }
        out.phases.extend(["materialization_demand_created".into(), "mechanics_search_planned".into(), "source_evidence_collected".into(), "extraction_run_created".into(), "binding_verification_created".into()]);
        if out.writebacks.is_empty() {
            out.phases.push("source_evidence_retained_no_runtime_writeback".into());
        } else {
            out.phases.extend(["runtime_writeback_applied".into(), "parameter_facets_written".into()]);
        }
        out.narration_context = serde_json::to_string_pretty(&json!({
            "materialization_summary": {
                "demands": out.demands.iter().map(|d| json!({"demand_id":d.demand_id,"target_kind":d.target_kind.as_str(),"target_label":d.target_label,"status":d.status.as_str()})).collect::<Vec<_>>(),
                "bindings": out.rule_bindings.iter().map(|b| json!({"binding_id":b.binding_id,"target_kind":b.target_kind.as_str(),"target_id":b.target_id,"verification_status":b.verification_status.as_str()})).collect::<Vec<_>>(),
                "policy": "v1.10: retrieved rules/modules must write back to runtime actor/object/ability/check/effect state; prompt-only mechanical facts are not sufficient"
            }
        })).ok();
        Ok(out)
    }

    fn demands_from_runtime_context(&self, input: MaterializationTurnInput<'_>, semantic: Option<&SemanticIntentResult>) -> Vec<MaterializationRequest> {
        let mut out = Vec::new();
        if let Some(sem) = semantic {
            // Turn's primary item search key = minimal distinguishing unit (AI-chosen, e.g. ".38").
            // Attached to the generic combat-weapon demand (whose label is "currently used weapon…",
            // useless for retrieval) so the named item's table row is what reaches semantic search.
            let primary_search_key = sem.raw_json.get("object_search_keys").and_then(|v| v.as_array())
                .and_then(|a| a.iter().filter_map(|x| x.as_str()).map(str::trim).find(|s| !s.is_empty()).map(String::from))
                .or_else(|| sem.object_refs.iter().map(|s| s.trim()).find(|s| !s.is_empty()).map(String::from));
            // Name the combat weapon demand by the referenced weapon when there is one, so it and the
            // narrative object_refs demand DEDUP into one (same kind+label) and the object is named
            // correctly (e.g. ".38 revolver") instead of the generic placeholder.
            let weapon_label = sem.object_refs.iter().map(|s| s.trim()).find(|s| !s.is_empty())
                .map(String::from).unwrap_or_else(|| "currently used weapon or attack source".into());
            if matches!(sem.primary_action_kind, SituationActionKind::Attack | SituationActionKind::UnderAttack | SituationActionKind::EnemyInitiatedConflict | SituationActionKind::SceneEntersConflict | SituationActionKind::Counterattack | SituationActionKind::Defend) {
                out.push(MaterializationRequest {
                    request_id: format!("matreq_{}", Uuid::new_v4().simple()),
                    target_kind: RuleBindingTargetKind::ActorParameter,
                    target_id: Some("npc.opposition".into()),
                    target_label: "opposition actor in mechanically relevant scene".into(),
                    target_description: input.user_input.into(),
                    evidence_span: input.user_input.into(),
                    requested_fields: vec!["stats".into(), "skills".into(), "hp".into(), "defense".into(), "loadout".into(), "abilities".into()],
                    urgency: RuntimeUrgency::BeforeResolution,
                    visibility: Visibility::GmOnly,
                    // material_target_kind="npc_stat_block" routes material_kind_from_rule_kind ->
                    // MaterialTargetKind::NpcStatBlock -> SearchSkillKind::NpcStatblock plan, which
                    // PREFERS module NPC cards over the generic ActorProfile rulebook fallback.
                    // HONEST NOTE: the NpcCard/NpcStatBlock extractor schema is IDENTICAL to ActorProfile
                    // (see extractor_for / required_fields_for_target); this only changes the SEARCH query
                    // plan to prefer module NPC cards, NOT the extraction shape.
                    metadata: json!({"source":"runtime_context","v110_urgency":"blocking_mechanical_resolution","material_target_kind":"npc_stat_block"}),
                });
                out.push(MaterializationRequest {
                    request_id: format!("matreq_{}", Uuid::new_v4().simple()),
                    target_kind: RuleBindingTargetKind::ObjectDefinition,
                    target_id: None,
                    target_label: weapon_label.clone(),
                    target_description: input.user_input.into(),
                    evidence_span: input.user_input.into(),
                    requested_fields: vec!["name".into(), "kind".into(), "damage".into(), "range".into(), "attack_skill".into(), "ammo".into(), "special_rules".into()],
                    urgency: RuntimeUrgency::BeforeResolution,
                    visibility: Visibility::GmOnly,
                    metadata: json!({"source":"runtime_context","v110_urgency":"blocking_mechanical_resolution","search_key": primary_search_key.clone()}),
                });
            }
            if matches!(sem.primary_action_kind, SituationActionKind::ActivateAbility | SituationActionKind::TriggerAbility) {
                let label = sem.ability_refs.first().cloned().unwrap_or_else(|| "ability referenced by current input".into());
                out.push(MaterializationRequest {
                    request_id: format!("matreq_{}", Uuid::new_v4().simple()),
                    target_kind: RuleBindingTargetKind::AbilityDefinition,
                    target_id: None,
                    target_label: label,
                    target_description: input.user_input.into(),
                    evidence_span: input.user_input.into(),
                    requested_fields: vec!["activation".into(), "cost".into(), "target".into(), "effect".into(), "duration".into(), "trigger".into()],
                    urgency: RuntimeUrgency::BeforeResolution,
                    visibility: Visibility::GmOnly,
                    metadata: json!({"source":"runtime_context","v110_urgency":"blocking_mechanical_resolution"}),
                });
            }
            // Narrative item -> source-backed params: for each item the player
            // referenced BY NAME (semantic object_refs — meaning, not keywords),
            // materialize its definition keyed BY THAT NAME so the object kernel
            // reads the same source def via find_materialized_object_def. Covers
            // loot/draw/pickup/improvised items; fail-closed when source lacks it.
            // AI-chosen MINIMAL distinguishing search key per object (e.g. ".38" for ".38 revolver",
            // language-neutral / source-language). Display name stays the full object_ref; the key is
            // only the retrieval handle. Falls back to the name if the classifier omitted a key.
            let search_keys: Vec<String> = sem.raw_json.get("object_search_keys").and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.trim().to_string())).collect())
                .unwrap_or_default();
            let mut added = 0;
            for (i, raw_name) in sem.object_refs.iter().enumerate() {
                let name = raw_name.trim();
                if name.is_empty() { continue; }
                if added >= 3 { break; }
                added += 1;
                let search_key = search_keys.get(i).filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| name.to_string());
                out.push(MaterializationRequest {
                    request_id: format!("matreq_{}", Uuid::new_v4().simple()),
                    target_kind: RuleBindingTargetKind::ObjectDefinition,
                    target_id: None,
                    target_label: name.to_string(),
                    target_description: input.user_input.into(),
                    evidence_span: input.user_input.into(),
                    requested_fields: vec!["name".into(), "kind".into(), "damage".into(), "range".into(), "rof".into(), "ammo".into(), "armor".into(), "properties".into(), "special_rules".into()],
                    urgency: RuntimeUrgency::BeforeResolution,
                    visibility: Visibility::GmOnly,
                    metadata: json!({"source":"narrative_item_reference","item_name": name, "search_key": search_key}),
                });
            }
        }
        out
    }

    async fn create_demand(&self, input: MaterializationTurnInput<'_>, req: &MaterializationRequest, semantic: Option<&SemanticIntentResult>) -> Result<MaterializationDemand> {
        let target_kind = material_kind_from_rule_kind(req.target_kind, &req.metadata);
        let urgency = urgency_from_request(req);
        // Resolve the minimal distinguishing search key for EVERY demand path (the request may come
        // from my object loop, the combat-weapon path, OR the classifier's own materialization_requests
        // which carry no key). Prefer an explicit metadata key, else look the demand's label up in the
        // semantic object_refs→object_search_keys mapping. Drives ExactEntity/grep retrieval.
        let search_key = req.metadata.get("search_key").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(String::from)
            .or_else(|| {
                let sem = semantic?;
                let keys = sem.raw_json.get("object_search_keys").and_then(|v| v.as_array())?;
                let idx = sem.object_refs.iter().position(|o| o.trim().eq_ignore_ascii_case(req.target_label.trim()))?;
                keys.get(idx).and_then(|k| k.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(String::from)
            });
        let demand = MaterializationDemand {
            demand_id: format!("matdemand_{}", Uuid::new_v4().simple()),
            session_id: input.session_id.into(),
            turn_id: Some(input.turn_id.into()),
            frame_id: input.frame_id.map(str::to_string),
            target_kind,
            target_id: req.target_id.clone(),
            target_label: req.target_label.clone(),
            target_description: req.target_description.clone(),
            ruleset_id: input.ruleset_id.into(),
            module_id: input.module_id.map(str::to_string),
            requested_fields: if req.requested_fields.is_empty() { required_fields_for_target(target_kind).into_iter().map(str::to_string).collect() } else { req.requested_fields.clone() },
            urgency,
            evidence_json: json!({"semantic_request": req, "input": input.user_input, "search_key": search_key}),
            visibility: req.visibility,
            caused_by_event_id: None,
            world_tick: Some(input.world_tick),
            status: MaterializationDemandStatus::Open,
            created_at: Utc::now(),
        };
        self.insert_demand(&demand).await?;
        Ok(demand)
    }

    async fn collect_evidence(&self, demand: &MaterializationDemand, req: &MaterializationRequest) -> Result<SourceEvidenceBundle> {
        let kernel = self.db.load_rule_kernel(&demand.ruleset_id).await.ok().flatten();
        let module_config = if let Some(mid) = demand.module_id.as_deref() { self.db.load_module_config(mid).await } else { None };
        let plan = self.query_plan(demand, kernel.as_ref(), module_config.as_ref());
        let mut candidates = Vec::new();
        if let Some(search) = &self.search {
            // Semantic retrieval, ITEM-FIRST: run exact-entity (item name) + field queries BEFORE
            // the generic ruleset-prefixed locator queries, so the named item (e.g. ".38 revolver")
            // actually reaches semantic search instead of being truncated by the cap. The extractor
            // LLM then maps the player's phrasing to the matching source row (semantic, not keyword).
            let qp = mechanics_query_plan_for(demand, kernel.as_ref(), module_config.as_ref());
            let mut queries = queries_for_step(&qp, QueryPlanStepKind::ExactEntity);
            queries.extend(queries_for_step(&qp, QueryPlanStepKind::Field));
            queries.extend(queries_for_step(&qp, QueryPlanStepKind::Locator));
            for q in queries.iter().take(10) {
                let resp = search.search_async(&SearchRequest { query: q.clone(), domains: vec![demand.ruleset_id.clone(), demand.module_id.clone().unwrap_or_default(), "rules".into(), "modules".into(), "source".into(), "parsed".into()], limit: 8, intent: Some("real_materialization_candidate_retrieval_v1_15_4_semantic_units".into()), ..Default::default() }).await.unwrap_or_default();
                for h in resp.hits {
                    if hit_is_noise_or_low_signal(&h) { continue; }
                    // Drop cross-ruleset leakage: on a shared search index the generic
                    // "rules"/"source"/"parsed" domains admit other rulesets' docs (e.g. a
                    // Cyberpunk weapon row for a CoC ".38" query). Skip any hit explicitly
                    // scoped to a DIFFERENT ruleset; generic/unscoped hits still pass.
                    if hit_is_foreign_ruleset(&h.scopes, &demand.ruleset_id) { continue; }
                    let evidence_text = evidence_text_from_hit(&h);
                    if evidence_text.trim().is_empty() { continue; }
                    let evidence_hash = sha256_hex(&evidence_text);
                    if candidates.iter().any(|c: &SourceCandidate| c.source_document_id == h.search_doc_id && c.excerpt_hash == evidence_hash) { continue; }
                    let raw_meta = h.metadata.get("raw").cloned().unwrap_or_else(|| json!({}));
                    let semantic_meta = raw_meta.get("metadata").cloned().unwrap_or_else(|| json!({}));
                    candidates.push(SourceCandidate {
                        candidate_id: format!("source_candidate_{}", Uuid::new_v4().simple()),
                        source_document_id: h.search_doc_id.clone(),
                        source_kind: source_kind_from_hit(&h),
                        page_start: h.scopes.get("page_start").or_else(|| h.scopes.get("page")).and_then(|p| p.parse().ok()),
                        page_end: h.scopes.get("page_end").or_else(|| h.scopes.get("page")).and_then(|p| p.parse().ok()),
                        heading_path: h.scopes.get("heading").cloned().or_else(|| semantic_meta.get("heading_path").and_then(Value::as_str).map(str::to_string)),
                        excerpt: trim_excerpt(&evidence_text, 2200),
                        excerpt_hash: evidence_hash,
                        candidate_score: evidence_score_from_hit(&h, h.score),
                        retrieval_reason: q.clone(),
                        source_refs: h.source_refs.clone(),
                        metadata: json!({
                            "title": h.title,
                            "logical_kind": h.logical_kind,
                            "tags": h.tags,
                            "origin": h.origin,
                            "domain": h.domain,
                            "semantic_category": semantic_meta.get("semantic_category").or_else(|| raw_meta.get("semantic_category")).cloned().unwrap_or(Value::Null),
                            "signal_class": semantic_meta.get("signal_class").or_else(|| raw_meta.get("signal_class")).cloned().unwrap_or(Value::Null),
                            "defined_entity": semantic_meta.get("defined_entity").or_else(|| raw_meta.get("defined_entity")).cloned().unwrap_or(Value::Null),
                            "mechanics_tags": semantic_meta.get("mechanics_tags").or_else(|| raw_meta.get("mechanics_tags")).cloned().unwrap_or(Value::Null),
                            "source_unit_id": raw_meta.get("unit_id").or_else(|| raw_meta.get("chunk_id")).or_else(|| semantic_meta.get("source_unit_id")).cloned().unwrap_or(Value::Null),
                            "retrieved_from_semantic_unit": semantic_meta.get("unit_kind").and_then(Value::as_str) == Some("semantic_unit") || raw_meta.get("clean_status").and_then(Value::as_str).unwrap_or("").contains("semantic_unit"),
                        }),
                    });
                }
            }
            // duotext: pull exact ALIGNED table rows (weapon/gear stat rows) from the table pages
            // using the EXACT-ENTITY (item-name) queries — those match table rows. The generic
            // ruleset-prefixed locator queries (and the take(8) Tantivy loop above) never reach the
            // item name, so this grep is what actually surfaces the named item's stat row.
            let name_queries = queries_for_step(&qp, QueryPlanStepKind::ExactEntity);
            for q in name_queries.iter().take(6) {
                for h in search.grep_layout_tables(q, 5) {
                    let evidence_text = if h.context.is_empty() { h.row.clone() } else { format!("{}\n{}", h.context.join("\n"), h.row) };
                    let evidence_hash = sha256_hex(&evidence_text);
                    if candidates.iter().any(|c: &SourceCandidate| c.source_document_id == h.source_id && c.excerpt_hash == evidence_hash) { continue; }
                    candidates.push(SourceCandidate {
                        candidate_id: format!("source_candidate_{}", Uuid::new_v4().simple()),
                        source_document_id: h.source_id.clone(),
                        source_kind: if h.source_kind == "modules" { SourceKind::Module } else { SourceKind::Rulebook },
                        page_start: h.page.map(|p| p as i32),
                        page_end: h.page.map(|p| p as i32),
                        heading_path: h.context.first().cloned(),
                        excerpt: trim_excerpt(&evidence_text, 2200),
                        excerpt_hash: evidence_hash,
                        candidate_score: 0.9 + (h.column_score.min(8) as f32) * 0.01,
                        retrieval_reason: format!("duotext_layout_grep: {q}"),
                        source_refs: vec![],
                        metadata: json!({"source":"duotext_layout_grep","logical_kind":"aligned_table_row","signal_class":"high","column_score": h.column_score}),
                    });
                }
            }
        }
        let bundle = SourceEvidenceBundle { bundle_id: format!("evidence_bundle_{}", Uuid::new_v4().simple()), demand_id: demand.demand_id.clone(), candidates, query_plan_json: plan, source_priority_order: vec![SourceKind::Module, SourceKind::Rulebook, SourceKind::Unknown], created_at_tick: demand.world_tick, created_at: Utc::now() };
        self.insert_evidence_bundle(&bundle).await?;
        Ok(bundle)
    }

    fn query_plan(&self, demand: &MaterializationDemand, kernel: Option<&RuleKernel>, module_config: Option<&ModuleConfig>) -> Value {
        let plan = mechanics_query_plan_for(demand, kernel, module_config);
        let locator_queries = queries_for_step(&plan, QueryPlanStepKind::Locator);
        let exact_name_queries = queries_for_step(&plan, QueryPlanStepKind::ExactEntity);
        let field_queries = queries_for_step(&plan, QueryPlanStepKind::Field);
        let procedure_queries = queries_for_step(&plan, QueryPlanStepKind::Procedure);
        let module_queries = queries_for_step(&plan, QueryPlanStepKind::ModuleCard);
        let broad_queries = queries_for_step(&plan, QueryPlanStepKind::BroadFallback);
        let search_skill = plan.skill_kind.as_str();
        let requested_fields = plan.requested_fields.clone();
        let writeback_targets = plan.writeback_targets.iter().map(|t| t.as_str()).collect::<Vec<_>>();
        let rule_aliases = plan.ruleset_aliases.clone();
        let module_source_preferences = plan.module_source_preferences.clone();
        json!({
            "v": "1.12_mechanics_search_skills",
            "plan": plan,
            "ruleset_id": demand.ruleset_id,
            "module_id": demand.module_id,
            "target_kind": demand.target_kind.as_str(),
            "target_label": demand.target_label,
            "requested_fields": requested_fields,
            "search_skill": search_skill,
            "locator_queries": locator_queries,
            "exact_name_queries": exact_name_queries,
            "field_queries": field_queries,
            "procedure_queries": procedure_queries,
            "module_queries": module_queries,
            "broad_queries": broad_queries,
            "writeback_targets": writeback_targets,
            "rule_aliases": rule_aliases,
            "module_source_preferences": module_source_preferences,
            "policy": "v1.12: GrepSearch/Tantivy only produce candidates. SearchSkill plans multi-step retrieval; semantic extraction writes back facets to actor/object/ability/check/effect parameters."
        })
    }

    async fn extract(&self, demand: &MaterializationDemand, bundle: &SourceEvidenceBundle) -> Result<ExtractionRun> {
        let extractor_kind = extractor_for(demand.target_kind);
        let mut extracted = if let Some(client) = &self.llm {
            self.extract_with_llm(client, demand, bundle, extractor_kind).await.unwrap_or_else(|_| fallback_extraction(demand, bundle))
        } else {
            fallback_extraction(demand, bundle)
        };
        let source_refs = bundle.candidates.iter().flat_map(|c| c.source_refs.clone()).collect::<Vec<_>>();
        // The extractor often omits `source_refs` in its JSON echo even when it read
        // a real candidate row; backfill from the candidates actually used so a
        // complete, source-backed extraction isn't gated to needs_review purely
        // because the model didn't repeat the provenance it was handed. Gate on
        // confidence != "low": a low-confidence/empty extraction (e.g. a retrieval
        // miss) must NOT be lifted to bound_exact just because a candidate existed.
        if !source_refs.is_empty()
            && extracted.get("confidence").and_then(Value::as_str) != Some("low")
            && extracted.get("source_refs").and_then(Value::as_array).map(|a| a.is_empty()).unwrap_or(true)
        {
            if let Some(obj) = extracted.as_object_mut() {
                obj.insert("source_refs".into(), serde_json::to_value(&source_refs).unwrap_or_else(|_| json!([])));
            }
        }
        let missing_preview = missing_required_fields(demand, &extracted);
        let status = if bundle.candidates.is_empty() { BindingStatus::FailedNoSource }
            else if !missing_preview.is_empty() { BindingStatus::NeedsReview }
            else { BindingStatus::BoundExact };
        let confidence = if bundle.candidates.is_empty() || !missing_preview.is_empty() { RulingConfidence::Low } else { RulingConfidence::Medium };
        let run = ExtractionRun { extraction_run_id: format!("extraction_{}", Uuid::new_v4().simple()), demand_id: demand.demand_id.clone(), extractor_kind, input_candidate_ids: bundle.candidates.iter().map(|c| c.candidate_id.clone()).collect(), extracted_json: extracted, source_refs, confidence, status, model_id: self.llm.as_ref().map(|_| "semantic_extractor_v1_15_4".into()), created_at: Utc::now() };
        self.insert_extraction_run(&run).await?;
        Ok(run)
    }

    /// Schema-guided extraction guidance: when the ruleset's kernel carries
    /// parse-time `object_schemas`, surface the matching category SCHEMAS (typed
    /// slots + 2 worked examples) so the extractor fills typed slots against a
    /// known pattern instead of guessing free-form — the structural fix for the
    /// flaky ".38 revolver -> damage null" tail. Returns None (free-form fallback)
    /// when no kernel / no schemas / no category in the demand's family.
    async fn object_schema_guidance(&self, demand: &MaterializationDemand) -> Option<String> {
        // Optimization 2 activation hook: when the lazy path is on, fill any
        // DISCOVERED stub of this demand's family BEFORE reading the kernel, so the
        // schemas picked below carry typed slots. Self-gating + best-effort: a no-op
        // when lazy is off, nothing is a stub (eager already compiled), or the
        // extract fails (then guidance falls back to free-form). Caches into the
        // kernel so a second reference re-extracts nothing. See staged_extract.rs.
        self.ensure_category_compiled(demand).await;
        let kernel = self.db.load_rule_kernel(&demand.ruleset_id).await.ok().flatten()?;
        if kernel.object_schemas.is_empty() { return None; }
        // Family split by target kind: ability demands want spell/psychic-style
        // categories; object demands want everything else. Data-driven over the
        // schema's own `kind` (no per-ruleset category names hardcoded).
        const ABILITY_KINDS: &[&str] = &["spell", "psychic", "ability", "power", "discipline", "maneuver", "talent", "magic"];
        let want_ability = matches!(demand.target_kind, MaterialTargetKind::AbilityDefinition | MaterialTargetKind::AbilityInstance);
        let picked: Vec<Value> = kernel.object_schemas.iter()
            .filter(|s| {
                let kind = s.get("kind").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
                ABILITY_KINDS.iter().any(|k| kind.contains(k)) == want_ability
            })
            .take(4)
            .map(|s| json!({
                "category_id": s.get("category_id").cloned().unwrap_or(Value::Null),
                "kind": s.get("kind").cloned().unwrap_or(Value::Null),
                "schema_slots": s.get("schema_slots").cloned().unwrap_or_else(|| json!([])),
                "examples": s.get("examples").cloned().unwrap_or_else(|| json!([])),
            }))
            .collect();
        if picked.is_empty() { return None; }
        Some(format!(" The ruleset defines these CATEGORY SCHEMAS for this kind of material — FIRST decide which ONE category the requested item belongs to (by meaning: a revolver is a 'weapon', a grimoire is a tome/'gear', a spell is a 'spell'), THEN fill `mechanical_profile` with THAT category's typed slots, using its 2 examples as the EXACT pattern for which slots exist and how each value is formatted (a `dice`-typed slot such as damage/sanity_loss is a dice string like '1D10'; a `range_ref` slot is a literal distance like '15 yards'; a `resource_ref`/`number` slot is its literal value). ALSO copy any category-specific slot not already named in the base schema (e.g. malfunction, cost, mythos_rating) into `mechanical_profile`. Leave a slot null ONLY if no candidate row contains its value. CATEGORY SCHEMAS: {}", serde_json::to_string(&picked).unwrap_or_default()))
    }

    async fn extract_with_llm(&self, client: &OpenAiCompatibleClient, demand: &MaterializationDemand, bundle: &SourceEvidenceBundle, extractor_kind: ExtractorKind) -> Result<Value> {
        // Feed the HIGHEST-scored candidates first: the aligned table rows (duotext_layout_grep,
        // score ~0.9) must beat lower-scored Tantivy noise (prices/appendix mentions of the same
        // token) so they survive the take(8) cutoff and actually reach the extractor.
        let mut ranked: Vec<&SourceCandidate> = bundle.candidates.iter().collect();
        ranked.sort_by(|a, b| b.candidate_score.partial_cmp(&a.candidate_score).unwrap_or(std::cmp::Ordering::Equal));
        let candidates = ranked.iter().take(8).map(|c| json!({"candidate_id": c.candidate_id, "source_kind": c.source_kind.as_str(), "excerpt": c.excerpt, "source_refs": c.source_refs, "metadata": c.metadata})).collect::<Vec<_>>();
        let schema = extractor_schema(extractor_kind);
        let guidance = self.object_schema_guidance(demand).await.unwrap_or_default();
        let messages = vec![
            ChatMessage { role: "system".into(), content: format!("You are a TRPG material extractor. The candidate rows were ALREADY retrieved specifically for the requested item — assume ONE of them IS that item; your job is to READ OFF its values, not to doubt its identity. The source's table/card name and LANGUAGE usually differ from the player's phrasing (e.g. player '.38 revolver' = a '.38 or 9mm' row sitting under a 'Revolver' heading — the caliber/model token '.38' identifies it; translated/transliterated names also count). STEP 1: pick the SINGLE candidate row that best matches by meaning — match on the distinctive token (caliber / model number / proper noun), ignoring generic category words. STEP 2: COPY that row's cell values VERBATIM into the requested fields — the rows are table columns, so a dice value like '1D10' is damage, a distance like '15 yards' is range, a magazine number is ammo, etc. Do NOT paraphrase, summarize, or describe (range must be the literal '15 yards', NOT 'within the base range'); do NOT invent values that are not present in a candidate. When a row clearly matches the item, report medium/high confidence and FILL the fields — do NOT downgrade to low confidence or leave fields null merely because the source's label was not word-for-word identical to the player's phrasing. Mark a field missing ONLY if no candidate row contains a value for it. Worked example: requested '.38 revolver'; candidate '.38 or 9mm  Firearms  1D10  15 yards  1 (3)  6  \\$25/\\$200  100' -> {{\"entry_name\":\".38 revolver\",\"damage\":\"1D10\",\"range\":\"15 yards\",\"ammo\":\"6\",\"confidence\":\"high\"}}. Prefer module NPC/card data over generic rulebook archetypes. Return JSON only. Schema: {schema}{guidance}") },
            ChatMessage { role: "user".into(), content: serde_json::to_string(&json!({"demand": demand, "evidence_bundle": {"query_plan": bundle.query_plan_json, "candidates": candidates}})).unwrap_or_default() },
        ];
        client.complete_json(messages, 0.0).await
    }

    async fn binding_packet(&self, session_id: &str, turn_id: &str, demand: &MaterializationDemand, req: &MaterializationRequest, bundle: &SourceEvidenceBundle, extraction: &ExtractionRun) -> Result<RuleBindingPacket> {
        let target_kind = demand.target_kind.to_rule_binding_target();
        let target_id = demand.target_id.clone().unwrap_or_else(|| make_target_id(demand));
        let source_hits = bundle.candidates.iter().map(candidate_to_search_hit).collect::<Vec<_>>();
        let status = if bundle.candidates.is_empty() { BindingStatus::FailedNoSource } else { extraction.status };
        let packet = RuleBindingPacket { binding_id: format!("rule_binding_{}", Uuid::new_v4().simple()), target_kind, target_id, ruleset_id: demand.ruleset_id.clone(), semantic_request_json: json!({"demand": demand, "request": req, "evidence_bundle_id": bundle.bundle_id, "extraction_run_id": extraction.extraction_run_id}), retrieval_queries: query_strings(&bundle.query_plan_json), source_hits, extracted_json: extraction.extracted_json.clone(), source_refs: extraction.source_refs.clone(), confidence: extraction.confidence, verification_status: status, created_at_tick: demand.world_tick, created_at: Utc::now() };
        SemanticRuleBindingService::from_env(self.db.clone(), self.search.clone()).insert_rule_binding_packet(session_id, Some(turn_id), &packet).await?;
        self.link_rule_binding_to_demand(&packet.binding_id, &demand.demand_id, &extraction.extraction_run_id).await.ok();
        Ok(packet)
    }

    async fn verify(&self, demand: &MaterializationDemand, packet: &RuleBindingPacket, extraction: &ExtractionRun) -> Result<BindingVerificationResult> {
        let missing = missing_required_fields(demand, &extraction.extracted_json);
        let status = if packet.source_refs.is_empty() && packet.source_hits.is_empty() { BindingVerificationStatus::RejectedNoSource }
            else if extraction.extracted_json.get("extraction_status").and_then(Value::as_str) == Some("insufficient_source_for_parameters") { BindingVerificationStatus::ProvisionalNeedsAudit }
            else if !missing.is_empty() { BindingVerificationStatus::ProvisionalNeedsAudit }
            else { BindingVerificationStatus::VerifiedExact };
        let result = BindingVerificationResult { verification_id: format!("verification_{}", Uuid::new_v4().simple()), binding_id: packet.binding_id.clone(), target_kind: packet.target_kind, target_id: packet.target_id.clone(), status, missing_required_fields: missing.clone(), contradictory_sources: vec![], visibility_issues: visibility_issues(demand, packet), confidence_adjustment: if missing.is_empty() { None } else { Some(RulingConfidence::Low) }, repair_suggestions: repair_suggestions_for(demand.target_kind, &missing), verifier_json: json!({"demand_id": demand.demand_id, "extraction_run_id": extraction.extraction_run_id, "required_fields": required_fields_for_target(demand.target_kind), "missing": missing, "writeback_policy":"only verified_exact or explicitly allowed verified_partial may write runtime parameters"}), created_at: Utc::now() };
        self.insert_verification(&result).await?;
        self.link_verification(&packet.binding_id, &result.verification_id).await.ok();
        Ok(result)
    }

    async fn write_back(&self, demand: &MaterializationDemand, packet: &RuleBindingPacket, extraction: &ExtractionRun, verification: &BindingVerificationResult) -> Result<Vec<RuntimeBindingWriteback>> {
        // v1.15.4: parameter writeback is source-backed by default.  A partial
        // extraction may still be useful context, but it must not seed runtime
        // HP/SP/skills/weapon damage with fabricated values.  When strict mode
        // is enabled, only verified_exact bindings write mutable parameter state;
        // every other result is logged as a blocked hydration event for audit and
        // for a narrower follow-up lookup/materialization pass.
        if strict_source_backed_materialization()
            && !matches!(verification.status, BindingVerificationStatus::VerifiedExact)
        {
            self.insert_blocked_materialization_event(demand, packet, extraction, verification).await.ok();
            return Ok(vec![]);
        }

        let mut out = Vec::new();
        match demand.target_kind {
            MaterialTargetKind::ActorProfile | MaterialTargetKind::NpcStatBlock | MaterialTargetKind::VehicleCard | MaterialTargetKind::EncounterCard => out.push(self.writeback_actor(demand, packet, extraction, verification).await?),
            MaterialTargetKind::ObjectDefinition | MaterialTargetKind::ObjectInstance | MaterialTargetKind::DamageProfile | MaterialTargetKind::ArmorProfile => out.push(self.writeback_object(demand, packet, extraction, verification).await?),
            MaterialTargetKind::AbilityDefinition | MaterialTargetKind::AbilityInstance => out.push(self.writeback_ability(demand, packet, extraction, verification).await?),
            MaterialTargetKind::CheckTarget => { if let Some(w) = self.writeback_check(demand, packet).await? { out.push(w); } },
            MaterialTargetKind::EffectProfile => { if let Some(w) = self.writeback_effect(demand, packet).await? { out.push(w); } },
            _ => {}
        }
        for w in &out {
            self.insert_parameter_facet_bindings(demand, packet, extraction, verification, w).await.ok();
        }
        Ok(out)
    }

    async fn insert_blocked_materialization_event(&self, demand: &MaterializationDemand, packet: &RuleBindingPacket, extraction: &ExtractionRun, verification: &BindingVerificationResult) -> Result<()> {
        sqlx::query(r#"
            insert into material_hydration_events
              (id, event_id, session_id, turn_id, frame_id, actor_id, object_id, ruleset_id, material_kind, hydration_status, query_text, result_json, world_tick)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
        "#)
        .bind(Uuid::new_v4())
        .bind(format!("hydration_blocked_{}", Uuid::new_v4().simple()))
        .bind(&demand.session_id)
        .bind(demand.turn_id.as_deref())
        .bind(demand.frame_id.as_deref())
        .bind(if demand.target_kind == MaterialTargetKind::ActorProfile || demand.target_kind == MaterialTargetKind::NpcStatBlock { demand.target_id.as_deref() } else { None })
        .bind(if demand.target_kind == MaterialTargetKind::ObjectDefinition || demand.target_kind == MaterialTargetKind::ObjectInstance { demand.target_id.as_deref() } else { None })
        .bind(&demand.ruleset_id)
        .bind(demand.target_kind.as_str())
        .bind("blocked_missing_source_backed_parameters")
        .bind(&demand.target_description)
        .bind(json!({
            "policy": "strict_source_backed_materialization",
            "binding_id": packet.binding_id,
            "extraction_run_id": extraction.extraction_run_id,
            "verification_status": verification.status.as_str(),
            "missing_required_fields": verification.missing_required_fields.clone(),
            "source_refs": packet.source_refs.clone(),
            "repair_suggestions": verification.repair_suggestions.clone(),
            "note": "No runtime parameter writeback was applied because extracted parameters were incomplete or not source-backed."
        }))
        .bind(demand.world_tick)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    async fn writeback_actor(&self, demand: &MaterializationDemand, packet: &RuleBindingPacket, extraction: &ExtractionRun, verification: &BindingVerificationResult) -> Result<RuntimeBindingWriteback> {
        let actor_id = demand.target_id.clone().unwrap_or_else(|| if demand.target_label.to_lowercase().contains("pc") { "pc.current".into() } else { "npc.opposition".into() });
        let actor_kind = if actor_id.starts_with("pc") { "player_character" } else { "npc" };
        let profile = profile_actor_json(demand, extraction, verification);
        let status = if let Some(hp_max) = profile.pointer("/status/hp_max").and_then(Value::as_i64) {
            json!({"hp_current": hp_max, "hp_max": hp_max, "conditions": [], "actor_id": actor_id, "binding_id": packet.binding_id, "verification_status": verification.status.as_str(), "source_policy": "source_backed"})
        } else {
            json!({"hp_current": null, "hp_max": null, "conditions": [], "actor_id": actor_id, "binding_id": packet.binding_id, "verification_status": verification.status.as_str(), "source_policy": "unresolved_no_synthetic_default"})
        };
        let actor_param_id = format!("actor_params.{}.{}", safe_id(&demand.session_id), safe_id(&actor_id));
        sqlx::query(r#"
            insert into runtime_actor_parameters
              (id, actor_param_id, session_id, actor_id, actor_kind, ruleset_id, source_kind, template_id, display_name,
               sheet_json, mechanical_profile, status_json, visibility, created_at_tick, updated_at_tick)
            values ($1,$2,$3,$4,$5,$6,'real_materialization_extractor_v1_10',null,$7,$8,$9,$10,$11,$12,$13)
            on conflict (session_id, actor_id) do update set
              source_kind='real_materialization_extractor_v1_10',
              display_name=excluded.display_name,
              mechanical_profile=excluded.mechanical_profile,
              status_json=excluded.status_json,
              visibility=excluded.visibility,
              updated_at_tick=excluded.updated_at_tick,
              updated_at=now()
        "#).bind(Uuid::new_v4()).bind(&actor_param_id).bind(&demand.session_id).bind(&actor_id).bind(actor_kind).bind(&demand.ruleset_id).bind(&demand.target_label).bind(json!({"source":"materialization_extractor_v1_10","extracted":extraction.extracted_json,"demand_id":demand.demand_id})).bind(&profile).bind(&status).bind(demand.visibility.as_str()).bind(demand.world_tick).bind(demand.world_tick).execute(&self.db.pool).await?;
        let writeback = RuntimeBindingWriteback { writeback_id: format!("writeback_{}", Uuid::new_v4().simple()), binding_id: packet.binding_id.clone(), target_kind: packet.target_kind, target_id: actor_id.clone(), table_name: "runtime_actor_parameters".into(), writeback_json: profile.clone(), status: verification.status.as_str().into(), created_at_tick: demand.world_tick };
        self.insert_runtime_binding("actor_runtime_bindings", &writeback, &demand.session_id, Some(&actor_id), None, &actor_param_id).await?;
        Ok(writeback)
    }

    async fn writeback_object(&self, demand: &MaterializationDemand, packet: &RuleBindingPacket, extraction: &ExtractionRun, verification: &BindingVerificationResult) -> Result<RuntimeBindingWriteback> {
        let object_def_id = demand.target_id.clone().unwrap_or_else(|| format!("{}.object.{}", safe_id(&demand.ruleset_id), safe_id(&demand.target_label)));
        let kind = infer_object_kind(&demand.target_label, &extraction.extracted_json);
        let profile = profile_object_json(demand, extraction, verification);
        let def = ObjectDefinition { object_def_id: object_def_id.clone(), ruleset_id: demand.ruleset_id.clone(), name: demand.target_label.clone(), object_kind: kind, tags: vec!["materialized_v1_10".into()], mechanical_profile: profile.clone(), rule_bindings: vec![ObjectRuleBinding { binding_id: packet.binding_id.clone(), trigger: ObjectRuleTrigger::OnAttack, rule_packet_id: None, lookup_recipe_id: Some("real_materialization_extractor_v1_10".into()), source_refs: packet.source_refs.clone(), effect_json: extraction.extracted_json.clone(), confidence: packet.confidence }], default_affordances: vec![], equip_slots: vec![], visibility_default: demand.visibility, source_refs: packet.source_refs.clone() };
        sqlx::query(r#"
            insert into object_definitions (id, object_def_id, ruleset_id, name, object_kind, tags, mechanical_profile, rule_bindings, source_refs, hydration_status, source_query)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
            on conflict (object_def_id) do update set mechanical_profile=excluded.mechanical_profile, rule_bindings=excluded.rule_bindings, source_refs=excluded.source_refs, hydration_status=excluded.hydration_status, source_query=excluded.source_query, updated_at=now()
        "#).bind(Uuid::new_v4()).bind(&def.object_def_id).bind(&def.ruleset_id).bind(&def.name).bind(def.object_kind.as_str()).bind(&def.tags).bind(&def.mechanical_profile).bind(serde_json::to_value(&def.rule_bindings)?).bind(serde_json::to_value(&def.source_refs)?).bind(verification.status.as_str()).bind(&demand.target_description).execute(&self.db.pool).await?;
        let writeback = RuntimeBindingWriteback { writeback_id: format!("writeback_{}", Uuid::new_v4().simple()), binding_id: packet.binding_id.clone(), target_kind: packet.target_kind, target_id: object_def_id.clone(), table_name: "object_definitions".into(), writeback_json: profile, status: verification.status.as_str().into(), created_at_tick: demand.world_tick };
        self.insert_runtime_binding("object_runtime_bindings", &writeback, &demand.session_id, None, Some(&object_def_id), &object_def_id).await?;
        Ok(writeback)
    }

    async fn writeback_ability(&self, demand: &MaterializationDemand, packet: &RuleBindingPacket, extraction: &ExtractionRun, verification: &BindingVerificationResult) -> Result<RuntimeBindingWriteback> {
        let ability_def_id = demand.target_id.clone().unwrap_or_else(|| format!("{}.ability.{}", safe_id(&demand.ruleset_id), safe_id(&demand.target_label)));
        let profile = profile_ability_json(demand, extraction, verification);
        let def = AbilityDefinition { ability_def_id: ability_def_id.clone(), ruleset_id: demand.ruleset_id.clone(), name: demand.target_label.clone(), ability_kind: infer_ability_kind(&demand.ruleset_id, &demand.target_label), tags: vec!["materialized_v1_10".into()], source_kind: AbilitySourceKind::RulebookEntry, source_refs: packet.source_refs.clone(), activation: AbilityActivationModel { activation_kind: profile.pointer("/activation/kind").and_then(Value::as_str).unwrap_or("manual").into(), action_cost: profile.pointer("/activation/action_cost").and_then(Value::as_str).map(str::to_string), timing: profile.pointer("/activation/timing").and_then(Value::as_str).map(str::to_string), notes: None }, cost: AbilityCostModel { resource: profile.pointer("/cost/resource").and_then(Value::as_str).map(str::to_string), amount_json: profile.get("cost").cloned().unwrap_or_else(|| json!({})), consumes_use: profile.pointer("/cost/consumes_use").and_then(Value::as_bool).unwrap_or(false), notes: None }, target: AbilityTargetModel { target_kind: profile.pointer("/target/kind").and_then(Value::as_str).unwrap_or("unknown").into(), range: profile.pointer("/target/range").and_then(Value::as_str).map(str::to_string), area: profile.pointer("/target/area").and_then(Value::as_str).map(str::to_string), notes: None }, effect: AbilityEffectModel { effect_kind: profile.pointer("/effect/kind").and_then(Value::as_str).unwrap_or("rules_bound_effect").into(), summary: profile.pointer("/effect/summary").and_then(Value::as_str).unwrap_or("Source-backed ability effect; verify before applying patches.").into(), creates_check: profile.pointer("/effect/creates_check").and_then(Value::as_bool).unwrap_or(false), creates_effect: true, duration: profile.pointer("/effect/duration").and_then(Value::as_str).map(str::to_string), raw_json: profile.clone() }, rule_bindings: vec![AbilityRuleBinding { binding_id: packet.binding_id.clone(), trigger: AbilityTriggerKind::ManualActivation, rule_packet_id: None, lookup_recipe_id: Some("real_materialization_extractor_v1_10".into()), source_refs: packet.source_refs.clone(), effect_json: extraction.extracted_json.clone(), confidence: packet.confidence }], visibility_default: demand.visibility, mechanical_profile: profile.clone(), binding_status: if matches!(verification.status, BindingVerificationStatus::VerifiedExact | BindingVerificationStatus::VerifiedPartial) { BindingStatus::BoundProvisional } else { BindingStatus::NeedsReview } };
        sqlx::query(r#"
            insert into ability_definitions (id, ability_def_id, ruleset_id, name, ability_kind, source_kind, tags, definition_json, binding_status, visibility)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
            on conflict (ability_def_id) do update set definition_json=excluded.definition_json, binding_status=excluded.binding_status, updated_at=now()
        "#).bind(Uuid::new_v4()).bind(&def.ability_def_id).bind(&def.ruleset_id).bind(&def.name).bind(def.ability_kind.as_str()).bind(def.source_kind.as_str()).bind(&def.tags).bind(serde_json::to_value(&def)?).bind(def.binding_status.as_str()).bind(def.visibility_default.as_str()).execute(&self.db.pool).await?;
        let writeback = RuntimeBindingWriteback { writeback_id: format!("writeback_{}", Uuid::new_v4().simple()), binding_id: packet.binding_id.clone(), target_kind: packet.target_kind, target_id: ability_def_id.clone(), table_name: "ability_definitions".into(), writeback_json: profile, status: verification.status.as_str().into(), created_at_tick: demand.world_tick };
        self.insert_runtime_binding("ability_runtime_bindings", &writeback, &demand.session_id, None, Some(&ability_def_id), &ability_def_id).await?;
        Ok(writeback)
    }

    async fn writeback_check(&self, demand: &MaterializationDemand, packet: &RuleBindingPacket) -> Result<Option<RuntimeBindingWriteback>> {
        let Some(check_id) = demand.target_id.as_ref() else { return Ok(None); };
        sqlx::query(r#"update check_contracts set rule_binding_ids = coalesce(rule_binding_ids, '[]'::jsonb) || $2::jsonb, materialization_demand_ids = coalesce(materialization_demand_ids, '[]'::jsonb) || $3::jsonb where check_id=$1"#)
            .bind(check_id).bind(json!([packet.binding_id])).bind(json!([demand.demand_id])).execute(&self.db.pool).await.ok();
        Ok(Some(RuntimeBindingWriteback { writeback_id: format!("writeback_{}", Uuid::new_v4().simple()), binding_id: packet.binding_id.clone(), target_kind: packet.target_kind, target_id: check_id.clone(), table_name: "check_contracts".into(), writeback_json: json!({"rule_binding_id":packet.binding_id,"demand_id":demand.demand_id}), status: "attached".into(), created_at_tick: demand.world_tick }))
    }
    async fn writeback_effect(&self, demand: &MaterializationDemand, packet: &RuleBindingPacket) -> Result<Option<RuntimeBindingWriteback>> {
        let Some(effect_id) = demand.target_id.as_ref() else { return Ok(None); };
        sqlx::query(r#"update effect_contracts set rule_binding_ids = coalesce(rule_binding_ids, '[]'::jsonb) || $2::jsonb, materialization_demand_ids = coalesce(materialization_demand_ids, '[]'::jsonb) || $3::jsonb where effect_id=$1"#)
            .bind(effect_id).bind(json!([packet.binding_id])).bind(json!([demand.demand_id])).execute(&self.db.pool).await.ok();
        Ok(Some(RuntimeBindingWriteback { writeback_id: format!("writeback_{}", Uuid::new_v4().simple()), binding_id: packet.binding_id.clone(), target_kind: packet.target_kind, target_id: effect_id.clone(), table_name: "effect_contracts".into(), writeback_json: json!({"rule_binding_id":packet.binding_id,"demand_id":demand.demand_id}), status: "attached".into(), created_at_tick: demand.world_tick }))
    }

    pub async fn materialization_context_block(&self, session_id: &str, world_tick: i64) -> Result<ContextBlock> {
        let demands = sqlx::query(r#"select demand_id, target_kind, target_label, status, world_tick from materialization_demands where session_id=$1 order by created_at desc limit 20"#).bind(session_id).fetch_all(&self.db.pool).await.unwrap_or_default().into_iter().map(|r| json!({"demand_id":r.get::<String,_>("demand_id"),"target_kind":r.get::<String,_>("target_kind"),"target_label":r.get::<String,_>("target_label"),"status":r.get::<String,_>("status"),"world_tick":r.get::<Option<i64>,_>("world_tick")})).collect::<Vec<_>>();
        let verifications = sqlx::query(r#"select binding_id, status, missing_required_fields from binding_verifications order by created_at desc limit 20"#).fetch_all(&self.db.pool).await.unwrap_or_default().into_iter().map(|r| json!({"binding_id":r.get::<String,_>("binding_id"),"status":r.get::<String,_>("status"),"missing_required_fields":r.get::<Value,_>("missing_required_fields")})).collect::<Vec<_>>();
        let mechanics_query_plans = sqlx::query(r#"select plan_id, demand_id, search_skill, target_kind, target_label, requested_fields, query_plan_json from mechanics_query_plans where session_id=$1 order by created_at desc limit 12"#).bind(session_id).fetch_all(&self.db.pool).await.unwrap_or_default().into_iter().map(|r| json!({"plan_id":r.get::<String,_>("plan_id"),"demand_id":r.get::<String,_>("demand_id"),"search_skill":r.get::<String,_>("search_skill"),"target_kind":r.get::<String,_>("target_kind"),"target_label":r.get::<String,_>("target_label"),"requested_fields":r.get::<Value,_>("requested_fields"),"query_plan":r.get::<Value,_>("query_plan_json")})).collect::<Vec<_>>();
        let parameter_facets = sqlx::query(r#"select target_kind, target_id, facet_kind, binding_id, verification_status, facet_json from parameter_facet_bindings where session_id=$1 order by created_at desc limit 20"#).bind(session_id).fetch_all(&self.db.pool).await.unwrap_or_default().into_iter().map(|r| json!({"target_kind":r.get::<String,_>("target_kind"),"target_id":r.get::<String,_>("target_id"),"facet_kind":r.get::<String,_>("facet_kind"),"binding_id":r.get::<String,_>("binding_id"),"verification_status":r.get::<String,_>("verification_status"),"facet_json":r.get::<Value,_>("facet_json")})).collect::<Vec<_>>();
        let mut block = ContextBlock::new(format!("runtime.materialization.{}", session_id), BlockKind::MaterializationGraph, "Runtime Materialization / Binding Verifier + Mechanics Search Skills", BlockContent::Json(json!({"world_tick":world_tick,"demands":demands,"binding_verifications":verifications,"mechanics_query_plans":mechanics_query_plans,"parameter_facet_bindings":parameter_facets,"cache_policy":"BP3 dynamic: active materialization demands, SearchSkill query plans, extraction/verifier results, and facet writebacks. Stable verified facets may graduate to BP2 definitions."})), Visibility::GmOnly, Stability::TurnDynamic, CacheZone::DynamicTail, Scope { scope_type: ScopeType::Session, scope_id: session_id.into() }, 201);
        block.tags = vec!["materialization".into(), "rule_binding".into(), "bp3".into()];
        Ok(block)
    }

    // -- DB helpers --------------------------------------------------------------------------
    async fn insert_demand(&self, demand: &MaterializationDemand) -> Result<()> {
        sqlx::query(r#"
            insert into materialization_demands
              (id, demand_id, session_id, turn_id, frame_id, target_kind, target_id, target_label, target_description, ruleset_id, module_id, requested_fields, urgency, evidence_json, visibility, status, world_tick)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)
            on conflict (demand_id) do nothing
        "#).bind(Uuid::new_v4()).bind(&demand.demand_id).bind(&demand.session_id).bind(&demand.turn_id).bind(&demand.frame_id).bind(demand.target_kind.as_str()).bind(&demand.target_id).bind(&demand.target_label).bind(&demand.target_description).bind(&demand.ruleset_id).bind(&demand.module_id).bind(json!(demand.requested_fields)).bind(demand.urgency.as_str()).bind(&demand.evidence_json).bind(demand.visibility.as_str()).bind(demand.status.as_str()).bind(demand.world_tick).execute(&self.db.pool).await?;
        Ok(())
    }
    async fn insert_evidence_bundle(&self, bundle: &SourceEvidenceBundle) -> Result<()> {
        sqlx::query(r#"insert into source_evidence_bundles (id, bundle_id, demand_id, query_plan_json, source_priority_order, bundle_json, world_tick) values ($1,$2,$3,$4,$5,$6,$7) on conflict (bundle_id) do nothing"#)
            .bind(Uuid::new_v4()).bind(&bundle.bundle_id).bind(&bundle.demand_id).bind(&bundle.query_plan_json).bind(serde_json::to_value(&bundle.source_priority_order)?).bind(serde_json::to_value(bundle)?).bind(bundle.created_at_tick).execute(&self.db.pool).await?;
        self.insert_mechanics_query_plan(bundle).await.ok();
        for c in &bundle.candidates {
            sqlx::query(r#"insert into source_candidates (id, candidate_id, bundle_id, demand_id, source_document_id, source_kind, page_start, page_end, heading_path, excerpt, excerpt_hash, candidate_score, retrieval_reason, source_refs, metadata) values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15) on conflict (candidate_id) do nothing"#)
                .bind(Uuid::new_v4()).bind(&c.candidate_id).bind(&bundle.bundle_id).bind(&bundle.demand_id).bind(&c.source_document_id).bind(c.source_kind.as_str()).bind(c.page_start).bind(c.page_end).bind(&c.heading_path).bind(&c.excerpt).bind(&c.excerpt_hash).bind(c.candidate_score).bind(&c.retrieval_reason).bind(serde_json::to_value(&c.source_refs)?).bind(&c.metadata).execute(&self.db.pool).await?;
        }
        Ok(())
    }
    async fn insert_mechanics_query_plan(&self, bundle: &SourceEvidenceBundle) -> Result<()> {
        let Some(plan) = bundle.query_plan_json.get("plan") else { return Ok(()); };
        let demand_id = plan.get("demand_id").and_then(Value::as_str).unwrap_or(&bundle.demand_id);
        let session_id: String = sqlx::query_scalar("select session_id from materialization_demands where demand_id=$1")
            .bind(demand_id)
            .fetch_optional(&self.db.pool)
            .await?
            .unwrap_or_default();
        if session_id.is_empty() { return Ok(()); }
        let plan_id = plan.get("plan_id").and_then(Value::as_str).unwrap_or("mechanics_query_plan_unknown");
        let search_skill = bundle.query_plan_json.get("search_skill").and_then(Value::as_str).or_else(|| plan.get("skill_kind").and_then(Value::as_str)).unwrap_or("generic_mechanical_search");
        let target_kind = bundle.query_plan_json.get("target_kind").and_then(Value::as_str).unwrap_or("unknown");
        let target_label = bundle.query_plan_json.get("target_label").and_then(Value::as_str).unwrap_or_default();
        let requested_fields = bundle.query_plan_json.get("requested_fields").cloned().unwrap_or_else(|| json!([]));
        sqlx::query(r#"
            insert into mechanics_query_plans
              (id, plan_id, demand_id, session_id, search_skill, target_kind, target_label, requested_fields, query_plan_json, world_tick)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
            on conflict (plan_id) do nothing
        "#)
        .bind(Uuid::new_v4())
        .bind(plan_id)
        .bind(demand_id)
        .bind(&session_id)
        .bind(search_skill)
        .bind(target_kind)
        .bind(target_label)
        .bind(requested_fields)
        .bind(&bundle.query_plan_json)
        .bind(bundle.created_at_tick)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    async fn insert_parameter_facet_bindings(&self, demand: &MaterializationDemand, packet: &RuleBindingPacket, extraction: &ExtractionRun, verification: &BindingVerificationResult, writeback: &RuntimeBindingWriteback) -> Result<()> {
        let search_skill = search_skill_for_demand(demand).as_str().to_string();
        for facet in facet_kinds_for_demand(demand) {
            let facet_binding_id = format!("facet_{}", Uuid::new_v4().simple());
            let facet_json = json!({
                "v": "1.12_parameter_facet_binding",
                "search_skill": search_skill,
                "demand_id": demand.demand_id,
                "target_label": demand.target_label,
                "requested_fields": demand.requested_fields,
                "extracted": extraction.extracted_json,
                "writeback_table": writeback.table_name,
                "runtime_target_id": writeback.target_id,
                "policy": "facet bindings extend existing actor/object/ability/check/effect parameter systems; they do not create a parallel procedure engine"
            });
            sqlx::query(r#"
                insert into parameter_facet_bindings
                  (id, facet_binding_id, session_id, target_kind, target_id, facet_kind, binding_id, demand_id, facet_json, source_refs, confidence, verification_status, world_tick)
                values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
                on conflict (facet_binding_id) do nothing
            "#)
            .bind(Uuid::new_v4())
            .bind(&facet_binding_id)
            .bind(&demand.session_id)
            .bind(packet.target_kind.as_str())
            .bind(&writeback.target_id)
            .bind(facet.as_str())
            .bind(&packet.binding_id)
            .bind(&demand.demand_id)
            .bind(facet_json)
            .bind(serde_json::to_value(&packet.source_refs)?)
            .bind(packet.confidence.as_str())
            .bind(verification.status.as_str())
            .bind(demand.world_tick)
            .execute(&self.db.pool)
            .await?;
        }
        Ok(())
    }

    async fn insert_extraction_run(&self, run: &ExtractionRun) -> Result<()> {
        sqlx::query(r#"insert into extraction_runs (id, extraction_run_id, demand_id, extractor_kind, input_candidates, extracted_json, source_refs, confidence, status, model_id) values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) on conflict (extraction_run_id) do nothing"#)
            .bind(Uuid::new_v4()).bind(&run.extraction_run_id).bind(&run.demand_id).bind(run.extractor_kind.as_str()).bind(json!(run.input_candidate_ids)).bind(&run.extracted_json).bind(serde_json::to_value(&run.source_refs)?).bind(run.confidence.as_str()).bind(run.status.as_str()).bind(&run.model_id).execute(&self.db.pool).await?;
        Ok(())
    }
    async fn insert_verification(&self, v: &BindingVerificationResult) -> Result<()> {
        sqlx::query(r#"insert into binding_verifications (id, verification_id, binding_id, target_kind, target_id, status, missing_required_fields, contradictory_sources, visibility_issues, verifier_json) values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) on conflict (verification_id) do nothing"#)
            .bind(Uuid::new_v4()).bind(&v.verification_id).bind(&v.binding_id).bind(v.target_kind.as_str()).bind(&v.target_id).bind(v.status.as_str()).bind(json!(v.missing_required_fields)).bind(json!(v.contradictory_sources)).bind(json!(v.visibility_issues)).bind(&v.verifier_json).execute(&self.db.pool).await?;
        Ok(())
    }
    async fn update_demand_status(&self, demand_id: &str, status: MaterializationDemandStatus) -> Result<()> {
        sqlx::query("update materialization_demands set status=$2, updated_at=now() where demand_id=$1").bind(demand_id).bind(status.as_str()).execute(&self.db.pool).await?; Ok(())
    }
    async fn link_rule_binding_to_demand(&self, binding_id: &str, demand_id: &str, extraction_id: &str) -> Result<()> {
        sqlx::query("update rule_binding_packets set demand_id=$2, extraction_run_id=$3 where binding_id=$1").bind(binding_id).bind(demand_id).bind(extraction_id).execute(&self.db.pool).await?; Ok(())
    }
    async fn link_verification(&self, binding_id: &str, verification_id: &str) -> Result<()> {
        sqlx::query("update rule_binding_packets set verification_id=$2 where binding_id=$1").bind(binding_id).bind(verification_id).execute(&self.db.pool).await?; Ok(())
    }
    async fn insert_runtime_binding(&self, table: &str, writeback: &RuntimeBindingWriteback, session_id: &str, actor_id: Option<&str>, object_or_ability_id: Option<&str>, runtime_target_id: &str) -> Result<()> {
        let table = match table { "actor_runtime_bindings" => "actor_runtime_bindings", "object_runtime_bindings" => "object_runtime_bindings", "ability_runtime_bindings" => "ability_runtime_bindings", _ => return Ok(()) };
        if table == "actor_runtime_bindings" {
            sqlx::query(r#"insert into actor_runtime_bindings (id, runtime_binding_id, binding_id, session_id, actor_id, actor_param_id, profile_json, verification_status, world_tick) values ($1,$2,$3,$4,$5,$6,$7,$8,$9) on conflict (runtime_binding_id) do nothing"#)
                .bind(Uuid::new_v4()).bind(&writeback.writeback_id).bind(&writeback.binding_id).bind(session_id).bind(actor_id.unwrap_or(runtime_target_id)).bind(runtime_target_id).bind(&writeback.writeback_json).bind(&writeback.status).bind(writeback.created_at_tick).execute(&self.db.pool).await?;
        } else if table == "object_runtime_bindings" {
            sqlx::query(r#"insert into object_runtime_bindings (id, runtime_binding_id, binding_id, session_id, object_id, object_def_id, profile_json, verification_status, world_tick) values ($1,$2,$3,$4,$5,$6,$7,$8,$9) on conflict (runtime_binding_id) do nothing"#)
                .bind(Uuid::new_v4()).bind(&writeback.writeback_id).bind(&writeback.binding_id).bind(session_id).bind(object_or_ability_id).bind(runtime_target_id).bind(&writeback.writeback_json).bind(&writeback.status).bind(writeback.created_at_tick).execute(&self.db.pool).await?;
        } else {
            sqlx::query(r#"insert into ability_runtime_bindings (id, runtime_binding_id, binding_id, session_id, ability_id, ability_def_id, profile_json, verification_status, world_tick) values ($1,$2,$3,$4,$5,$6,$7,$8,$9) on conflict (runtime_binding_id) do nothing"#)
                .bind(Uuid::new_v4()).bind(&writeback.writeback_id).bind(&writeback.binding_id).bind(session_id).bind(object_or_ability_id).bind(runtime_target_id).bind(&writeback.writeback_json).bind(&writeback.status).bind(writeback.created_at_tick).execute(&self.db.pool).await?;
        }
        Ok(())
    }
}

fn search_skill_for_demand(demand: &MaterializationDemand) -> SearchSkillKind {
    let fields = demand.requested_fields.iter().map(|s| s.to_lowercase()).collect::<Vec<_>>().join(" ");
    let label = format!("{} {} {}", demand.target_label, demand.target_description, fields).to_lowercase();
    match demand.target_kind {
        MaterialTargetKind::ActorProfile | MaterialTargetKind::NpcStatBlock | MaterialTargetKind::VehicleCard | MaterialTargetKind::EncounterCard => SearchSkillKind::NpcStatblock,
        MaterialTargetKind::ObjectDefinition | MaterialTargetKind::ObjectInstance => {
            if label.contains("armor") || label.contains("shield") || label.contains("sp") || label.contains("ac") || label.contains("护甲") { SearchSkillKind::ArmorDefense }
            else if label.contains("weapon") || label.contains("damage") || label.contains("gun") || label.contains("pistol") || label.contains("sword") || label.contains("枪") || label.contains("刀") || label.contains("武器") { SearchSkillKind::WeaponParameter }
            else { SearchSkillKind::SceneObject }
        }
        MaterialTargetKind::DamageProfile => SearchSkillKind::WeaponParameter,
        MaterialTargetKind::ArmorProfile => SearchSkillKind::ArmorDefense,
        MaterialTargetKind::AbilityDefinition | MaterialTargetKind::AbilityInstance => SearchSkillKind::AbilityActivation,
        MaterialTargetKind::CheckTarget => SearchSkillKind::CombatResolution,
        MaterialTargetKind::EffectProfile => {
            if label.contains("san") || label.contains("sanity") || label.contains("chaos") || label.contains("harm") || label.contains("condition") || label.contains("状态") || label.contains("理智") || label.contains("混沌") { SearchSkillKind::ConditionResource } else { SearchSkillKind::CombatResolution }
        }
        MaterialTargetKind::ConditionDefinition => SearchSkillKind::ConditionResource,
        MaterialTargetKind::Unknown => SearchSkillKind::GenericMechanical,
    }
}

fn mechanics_query_plan_for(demand: &MaterializationDemand, kernel: Option<&RuleKernel>, module_config: Option<&ModuleConfig>) -> MechanicsQueryPlan {
    let skill = search_skill_for_demand(demand);
    let fields = if demand.requested_fields.is_empty() { required_fields_for_target(demand.target_kind).into_iter().map(str::to_string).collect::<Vec<_>>() } else { demand.requested_fields.clone() };
    let aliases = ruleset_aliases_for(kernel, skill);
    let module_prefs = module_preferences_for(module_config, skill);
    let mut steps = Vec::new();
    let label = demand.target_label.clone();
    // AI-chosen minimal distinguishing search key (e.g. ".38"); the ExactEntity step queries it FIRST
    // so the named item's table row is what reaches semantic search + grep. Falls back to the label.
    let search_key = demand.evidence_json.get("search_key").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(String::from).unwrap_or_else(|| label.clone());
    let desc = demand.target_description.clone();
    let ruleset = demand.ruleset_id.clone();
    let module = demand.module_id.clone().unwrap_or_default();
    let preferred = aliases.get("preferred_sections").and_then(Value::as_array).cloned().unwrap_or_default().into_iter().filter_map(|v| v.as_str().map(str::to_string)).collect::<Vec<_>>();
    let field_aliases = aliases.get("field_aliases").cloned().unwrap_or_else(|| json!({}));
    let locator_queries = if preferred.is_empty() { vec![format!("{} {} rules locator", ruleset, skill.as_str())] } else { preferred.iter().map(|s| format!("{} {}", ruleset, s)).collect() };
    steps.push(QueryPlanStep { step_kind: QueryPlanStepKind::Locator, queries: locator_queries, purpose: "Locate the rulebook/module regions that usually contain this mechanical family before searching exact values.".into(), stop_when: Some("At least one high-confidence section or locator hit is found.".into()), metadata: json!({"preferred_sections": preferred}) });
    steps.push(QueryPlanStep { step_kind: QueryPlanStepKind::ExactEntity, queries: vec![search_key.clone(), label.clone(), format!("{} {}", ruleset, search_key)], purpose: "Find the exact entity/card/table row if a weapon, armor, ability, NPC, vehicle, scene object, or condition is named.".into(), stop_when: Some("Exact entry/card found with requested fields or source refs.".into()), metadata: json!({}) });
    steps.push(QueryPlanStep { step_kind: QueryPlanStepKind::Field, queries: fields.iter().flat_map(|f| vec![format!("{} {} {}", ruleset, label, f), format!("{} {} {}", ruleset, skill.as_str(), f)]).collect(), purpose: "Retrieve requested parameter fields without assuming a system-specific table name.".into(), stop_when: Some("Required fields are present or verified absent for this ruleset.".into()), metadata: json!({"field_aliases": field_aliases}) });
    steps.push(QueryPlanStep { step_kind: QueryPlanStepKind::Procedure, queries: procedure_queries_for(skill, &ruleset, &label, &desc), purpose: "If values imply a procedure, retrieve the resolution/damage/resource/visibility process and not just the numeric field.".into(), stop_when: Some("Resolution model and downstream state patch requirements are known.".into()), metadata: json!({"skill": skill.as_str()}) });
    if !module.is_empty() {
        steps.push(QueryPlanStep { step_kind: QueryPlanStepKind::ModuleCard, queries: module_prefs.iter().map(|p| format!("{} {} {}", module, p, label)).chain(vec![format!("{} {} npc card object card vehicle card scene", module, label)]).collect(), purpose: "Prefer module-local NPC cards, object cards, vehicle cards, encounter cards, and scene affordances over generic rulebook fallback.".into(), stop_when: Some("Module-local source supplies enough mechanical fields or explicitly defers to core rules.".into()), metadata: json!({"module_preferences": module_prefs}) });
    }
    steps.push(QueryPlanStep { step_kind: QueryPlanStepKind::BroadFallback, queries: vec![format!("{} {} {}", ruleset, label, desc), format!("{} {}", ruleset, skill.as_str()), format!("{} {}", label, fields.join(" "))], purpose: "Broad OR/should recall when exact or field queries are too narrow; semantic reranker/extractor decides relevance.".into(), stop_when: Some("Candidate evidence is sufficient for semantic extraction or provisional fallback is justified.".into()), metadata: json!({"avoid":"do not use keyword match as final business decision"}) });
    steps.push(QueryPlanStep { step_kind: QueryPlanStepKind::SemanticRerank, queries: vec![], purpose: "LLM/semantic reranker reads candidates and selects evidence; grep/Tantivy candidates are never final truth.".into(), stop_when: None, metadata: json!({}) });
    steps.push(QueryPlanStep { step_kind: QueryPlanStepKind::RuntimeWriteback, queries: vec![], purpose: "Write extracted facets back to actor/object/ability/check/effect parameters instead of leaving rules prompt-only.".into(), stop_when: None, metadata: json!({"writeback_targets": writeback_targets_for_skill(skill).iter().map(|t| t.as_str()).collect::<Vec<_>>()}) });
    MechanicsQueryPlan { plan_id: format!("mechplan_{}", Uuid::new_v4().simple()), demand_id: Some(demand.demand_id.clone()), ruleset_id: ruleset, module_id: demand.module_id.clone(), skill_kind: skill, target_kind: demand.target_kind, target_label: demand.target_label.clone(), requested_fields: fields, steps, ruleset_aliases: aliases, module_source_preferences: module_prefs, writeback_targets: writeback_targets_for_skill(skill), created_at_tick: demand.world_tick }
}

fn queries_for_step(plan: &MechanicsQueryPlan, step_kind: QueryPlanStepKind) -> Vec<String> {
    plan.steps.iter().filter(|s| s.step_kind == step_kind).flat_map(|s| s.queries.clone()).filter(|q| !q.trim().is_empty()).collect()
}

fn procedure_queries_for(skill: SearchSkillKind, ruleset: &str, label: &str, desc: &str) -> Vec<String> {
    match skill {
        SearchSkillKind::CombatResolution => vec![format!("{} attack combat resolution opposed check defense target number", ruleset), format!("{} {} attack hit miss defense range cover", ruleset, label), format!("{} {}", ruleset, desc)],
        SearchSkillKind::WeaponParameter => vec![format!("{} weapon damage range ammo attack skill", ruleset), format!("{} {} damage weapon table", ruleset, label), format!("{} damage armor hit weapon", ruleset)],
        SearchSkillKind::ArmorDefense => vec![format!("{} armor defense protection damage reduction", ruleset), format!("{} {} armor shield defense", ruleset, label)],
        SearchSkillKind::AbilityActivation => vec![format!("{} ability spell activation cost target effect trigger", ruleset), format!("{} {} ability spell effect", ruleset, label)],
        SearchSkillKind::ConditionResource => vec![format!("{} condition resource harm sanity chaos wound recovery", ruleset), format!("{} {} resource condition track", ruleset, label)],
        SearchSkillKind::NpcStatblock => vec![format!("{} npc stat block monster card enemy stats hp defense attacks", ruleset), format!("{} {} npc card stat block", ruleset, label)],
        SearchSkillKind::ModuleCard => vec![format!("{} module npc card object card vehicle card encounter", ruleset), format!("{} {} module card", ruleset, label)],
        SearchSkillKind::SceneObject => vec![format!("{} scene object interaction hazard device lock cable affordance", ruleset), format!("{} {} scene object", ruleset, label)],
        SearchSkillKind::GenericMechanical => vec![format!("{} {} rules procedure", ruleset, label)],
    }
}

fn ruleset_aliases_for(kernel: Option<&RuleKernel>, skill: SearchSkillKind) -> Value {
    let mut base = match skill {
        SearchSkillKind::CombatResolution => json!({"preferred_sections":["combat","attack","defense","opposed check","resolving actions"],"field_aliases":{"target_number":["DC","DV","AC","defense","evasion","resistance"],"opposition":["opposed","contest","save","resist"]}}),
        SearchSkillKind::WeaponParameter => json!({"preferred_sections":["weapons","equipment","item data","weapon table"],"field_aliases":{"damage":["damage","DMG","power","damage dice"],"range":["range","base range","distance"],"ammo":["ammo","magazine","uses"]}}),
        SearchSkillKind::ArmorDefense => json!({"preferred_sections":["armor","defense","protection","damage reduction"],"field_aliases":{"armor":["AC","SP","armor","defense","protection","soak","ablation"]}}),
        SearchSkillKind::AbilityActivation => json!({"preferred_sections":["spellcasting","spells","abilities","feats","techniques","programs"],"field_aliases":{"activation":["action","reaction","trigger","timing"],"cost":["slot","resource","cost"],"effect":["effect","damage","condition"]}}),
        SearchSkillKind::ConditionResource => json!({"preferred_sections":["conditions","damage","healing","sanity","harm","chaos","resources"],"field_aliases":{"resource":["HP","SAN","harm","chaos","wound","condition"]}}),
        SearchSkillKind::NpcStatblock => json!({"preferred_sections":["NPC","monster","creature statistics","stat block","cards"],"field_aliases":{"hp":["HP","hit points","health"],"defense":["AC","DV","defense","armor"],"attack":["attack","weapon","action"]}}),
        _ => json!({"preferred_sections":["rules","data","GM toolkit"],"field_aliases":{}}),
    };
    // Apply overrides from kernel.search_profile (data-driven; no ruleset name
    // branches). A `"*"` wildcard key applies to ALL skills — this reproduces
    // legacy ruleset_aliases_for, which OVERWROTE preferred_sections and SET
    // field_aliases for every skill. Per-skill keys win over the wildcard.
    if let Some(sp) = kernel.and_then(|k| k.search_profile.as_ref()) {
        let key = skill.skill_key();
        // sections: per-skill wins, else wildcard, else base.
        if let Some(sections) = sp.preferred_sections_by_skill.get(key)
            .or_else(|| sp.preferred_sections_by_skill.get("*"))
        {
            base["preferred_sections"] = serde_json::to_value(sections).unwrap_or(base["preferred_sections"].clone());
        }
        // field_aliases: merge wildcard FIRST, then per-skill on top (per-skill
        // overrides wildcard), both merged onto base.
        if let Some(base_obj) = base["field_aliases"].as_object_mut() {
            for src in [sp.field_aliases_by_skill.get("*"), sp.field_aliases_by_skill.get(key)] {
                if let Some(over_obj) = src.and_then(|v| v.as_object()) {
                    for (k, v) in over_obj { base_obj.insert(k.clone(), v.clone()); }
                }
            }
        }
    }
    base
}

fn module_preferences_for(module_config: Option<&ModuleConfig>, skill: SearchSkillKind) -> Vec<String> {
    let mut prefs = match skill {
        SearchSkillKind::NpcStatblock => vec!["npc card", "stat block", "enemy", "creature", "monster"],
        SearchSkillKind::ModuleCard => vec!["npc card", "vehicle card", "object card", "encounter", "resources"],
        SearchSkillKind::SceneObject => vec!["scene object", "map", "location", "tech", "hazard", "device"],
        SearchSkillKind::CombatResolution => vec!["combat note", "encounter", "enemy behavior", "tactics"],
        _ => vec!["resources", "appendix", "data", "card"],
    }.into_iter().map(str::to_string).collect::<Vec<_>>();
    // Apply extra sections from module_config.module_search_profile (data-driven;
    // no module name branches). Legacy module_preferences_for APPENDED the
    // module's section list to ALL skills — reproduce via a `"*"` wildcard key
    // appended in addition to any per-skill key.
    if let Some(cfg) = module_config {
        if let Some(sp) = cfg.module_search_profile.as_ref() {
            if let Some(extra) = sp.preferred_sections_by_skill.get(skill.skill_key()) {
                prefs.extend(extra.iter().cloned());
            }
            if let Some(extra) = sp.preferred_sections_by_skill.get("*") {
                prefs.extend(extra.iter().cloned());
            }
        }
    }
    prefs.sort(); prefs.dedup(); prefs
}

fn writeback_targets_for_skill(skill: SearchSkillKind) -> Vec<RuleBindingTargetKind> {
    match skill {
        SearchSkillKind::CombatResolution => vec![RuleBindingTargetKind::CheckContract, RuleBindingTargetKind::ContestProfile, RuleBindingTargetKind::EffectContract],
        SearchSkillKind::WeaponParameter | SearchSkillKind::ArmorDefense | SearchSkillKind::SceneObject => vec![RuleBindingTargetKind::ObjectDefinition, RuleBindingTargetKind::EffectContract],
        SearchSkillKind::AbilityActivation => vec![RuleBindingTargetKind::AbilityDefinition, RuleBindingTargetKind::EffectContract, RuleBindingTargetKind::CheckContract],
        SearchSkillKind::ConditionResource => vec![RuleBindingTargetKind::ConditionDefinition, RuleBindingTargetKind::EffectContract, RuleBindingTargetKind::ActorParameter],
        SearchSkillKind::NpcStatblock | SearchSkillKind::ModuleCard => vec![RuleBindingTargetKind::ActorParameter, RuleBindingTargetKind::ObjectDefinition, RuleBindingTargetKind::AbilityDefinition],
        SearchSkillKind::GenericMechanical => vec![RuleBindingTargetKind::CheckContract],
    }
}

fn facet_kinds_for_demand(demand: &MaterializationDemand) -> Vec<ParameterFacetKind> {
    match demand.target_kind {
        MaterialTargetKind::ActorProfile | MaterialTargetKind::NpcStatBlock | MaterialTargetKind::VehicleCard | MaterialTargetKind::EncounterCard => vec![ParameterFacetKind::ActorCheckFacet, ParameterFacetKind::ActorDefenseFacet, ParameterFacetKind::ActorResourceTrack],
        MaterialTargetKind::ObjectDefinition | MaterialTargetKind::ObjectInstance => vec![ParameterFacetKind::ObjectAttackFacet, ParameterFacetKind::ObjectDamageFacet, ParameterFacetKind::ObjectArmorFacet, ParameterFacetKind::ObjectDurabilityFacet],
        MaterialTargetKind::DamageProfile => vec![ParameterFacetKind::ObjectDamageFacet, ParameterFacetKind::EffectDamageBinding],
        MaterialTargetKind::ArmorProfile => vec![ParameterFacetKind::ObjectArmorFacet, ParameterFacetKind::ActorDefenseFacet],
        MaterialTargetKind::AbilityDefinition | MaterialTargetKind::AbilityInstance => vec![ParameterFacetKind::AbilityActivationFacet, ParameterFacetKind::AbilityTriggerFacet, ParameterFacetKind::AbilityCostFacet, ParameterFacetKind::AbilityEffectFacet],
        MaterialTargetKind::CheckTarget => vec![ParameterFacetKind::CheckResolutionBinding, ParameterFacetKind::ContestResolutionBinding],
        MaterialTargetKind::EffectProfile => vec![ParameterFacetKind::EffectDamageBinding, ParameterFacetKind::EffectResourceBinding, ParameterFacetKind::EffectConditionBinding],
        MaterialTargetKind::ConditionDefinition => vec![ParameterFacetKind::ActorConditionTrack, ParameterFacetKind::EffectConditionBinding, ParameterFacetKind::EffectResourceBinding],
        MaterialTargetKind::Unknown => vec![ParameterFacetKind::Unknown],
    }
}

fn material_kind_from_rule_kind(kind: RuleBindingTargetKind, meta: &Value) -> MaterialTargetKind {
    if let Some(s) = meta.get("material_target_kind").and_then(Value::as_str) { return parse_material_target_kind(s); }
    match kind {
        RuleBindingTargetKind::ActorParameter => MaterialTargetKind::ActorProfile,
        RuleBindingTargetKind::ObjectDefinition => MaterialTargetKind::ObjectDefinition,
        RuleBindingTargetKind::AbilityDefinition => MaterialTargetKind::AbilityDefinition,
        RuleBindingTargetKind::CheckContract => MaterialTargetKind::CheckTarget,
        RuleBindingTargetKind::EffectContract => MaterialTargetKind::EffectProfile,
        RuleBindingTargetKind::ConditionDefinition => MaterialTargetKind::ConditionDefinition,
        _ => MaterialTargetKind::Unknown,
    }
}
fn parse_material_target_kind(s: &str) -> MaterialTargetKind { match s { "actor_profile" => MaterialTargetKind::ActorProfile, "object_definition" => MaterialTargetKind::ObjectDefinition, "object_instance" => MaterialTargetKind::ObjectInstance, "ability_definition" => MaterialTargetKind::AbilityDefinition, "ability_instance" => MaterialTargetKind::AbilityInstance, "npc_stat_block" => MaterialTargetKind::NpcStatBlock, "vehicle_card" => MaterialTargetKind::VehicleCard, "encounter_card" => MaterialTargetKind::EncounterCard, "check_target" => MaterialTargetKind::CheckTarget, "effect_profile" => MaterialTargetKind::EffectProfile, "damage_profile" => MaterialTargetKind::DamageProfile, "armor_profile" => MaterialTargetKind::ArmorProfile, "condition_definition" => MaterialTargetKind::ConditionDefinition, _ => MaterialTargetKind::Unknown } }
fn urgency_from_request(req: &MaterializationRequest) -> MaterializationUrgency { match req.urgency { RuntimeUrgency::Immediate | RuntimeUrgency::BeforeResolution => MaterializationUrgency::BlockingMechanicalResolution, RuntimeUrgency::Background => MaterializationUrgency::BackgroundPrewarm, RuntimeUrgency::Soon => MaterializationUrgency::BeforeNarration } }
fn query_strings(plan: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for key in ["locator_queries", "exact_name_queries", "field_queries", "procedure_queries", "module_queries", "broad_queries"] { if let Some(arr)=plan.get(key).and_then(Value::as_array) { for v in arr { if let Some(s)=v.as_str() { if !s.trim().is_empty() && !out.contains(&s.to_string()) { out.push(s.to_string()); } } } } }
    out
}
fn source_kind_from_hit(hit: &SearchHit) -> SourceKind {
    let raw = hit.metadata.get("raw").unwrap_or(&Value::Null);
    let meta = raw.get("metadata").unwrap_or(&Value::Null);
    match meta.get("source_kind").and_then(Value::as_str).or_else(|| raw.get("source_kind").and_then(Value::as_str)).unwrap_or("") {
        "module" => return SourceKind::Module,
        "rulebook" => return SourceKind::Rulebook,
        _ => {}
    }
    if hit.domain.contains("module") || hit.origin.contains("module") { SourceKind::Module }
    else if hit.domain.contains("rule") || hit.origin.contains("rule") { SourceKind::Rulebook }
    else { SourceKind::Unknown }
}
/// True when a search hit is EXPLICITLY scoped to a ruleset other than the
/// demand's. Conservative cross-ruleset guard for shared search indexes: only
/// hits carrying a concrete foreign ruleset scope are rejected, so generic or
/// unscoped content (legitimately shared) is never dropped.
fn hit_is_foreign_ruleset(scopes: &std::collections::BTreeMap<String, String>, ruleset_id: &str) -> bool {
    if ruleset_id.trim().is_empty() { return false; }
    let want = ruleset_id.trim().to_ascii_lowercase();
    if scopes.get("scope_type").map(String::as_str) == Some("ruleset") {
        if let Some(sid) = scopes.get("scope_id") {
            return sid.trim().to_ascii_lowercase() != want;
        }
    }
    if let Some(rid) = scopes.get("ruleset_id") {
        return rid.trim().to_ascii_lowercase() != want;
    }
    false
}

fn hit_is_noise_or_low_signal(hit: &SearchHit) -> bool {
    let tags = hit.tags.iter().map(|t| t.to_ascii_lowercase()).collect::<Vec<_>>();
    if tags.iter().any(|t| matches!(t.as_str(), "noise" | "low_signal" | "toc" | "index" | "copyright" | "credits" | "front_matter" | "page_header_footer")) { return true; }
    let raw = hit.metadata.get("raw").unwrap_or(&Value::Null);
    let meta = raw.get("metadata").unwrap_or(&Value::Null);
    let signal = meta.get("signal_class").or_else(|| raw.get("signal_class")).and_then(Value::as_str).unwrap_or("");
    if signal == "noise" { return true; }
    let category = meta.get("semantic_category").or_else(|| raw.get("semantic_category")).and_then(Value::as_str).unwrap_or("");
    matches!(category, "toc" | "index" | "copyright" | "credits" | "front_matter" | "page_header_footer")
}

fn evidence_text_from_hit(hit: &SearchHit) -> String {
    let raw = hit.metadata.get("raw").unwrap_or(&Value::Null);
    for key in ["full_text", "text", "content_text", "body", "summary"] {
        if let Some(s) = raw.get(key).and_then(Value::as_str) {
            if !s.trim().is_empty() { return s.to_string(); }
        }
    }
    if !hit.snippet.trim().is_empty() { return hit.snippet.clone(); }
    raw.to_string()
}

fn evidence_score_from_hit(hit: &SearchHit, base_score: f32) -> f32 {
    let raw = hit.metadata.get("raw").unwrap_or(&Value::Null);
    let meta = raw.get("metadata").unwrap_or(&Value::Null);
    let mut score = base_score;
    if meta.get("unit_kind").and_then(Value::as_str) == Some("semantic_unit") { score += 0.25; }
    if meta.get("signal_class").and_then(Value::as_str) == Some("signal") { score += 0.15; }
    match meta.get("semantic_category").and_then(Value::as_str).unwrap_or("") {
        "stat_block" | "npc" | "object" | "ability" | "mechanic_rule" | "table" | "scene" | "clue" => score += 0.2,
        _ => {}
    }
    score
}
fn candidate_to_search_hit(c: &SourceCandidate) -> SearchHit { SearchHit { hit_id: c.candidate_id.clone(), search_doc_id: c.source_document_id.clone(), origin: c.source_kind.as_str().into(), domain: c.source_kind.as_str().into(), logical_kind: "materialization_candidate".into(), title: c.heading_path.clone().unwrap_or_else(|| c.source_document_id.clone()), snippet: c.excerpt.clone(), score: c.candidate_score, scopes: Default::default(), tags: vec!["materialization".into()], visibility: Visibility::GmOnly, source_refs: c.source_refs.clone(), metadata: c.metadata.clone(), explain: json!({"retrieval_reason": c.retrieval_reason}) } }
fn extractor_for(kind: MaterialTargetKind) -> ExtractorKind { match kind { MaterialTargetKind::ActorProfile => ExtractorKind::ActorProfile, MaterialTargetKind::NpcStatBlock => ExtractorKind::NpcCard, MaterialTargetKind::VehicleCard => ExtractorKind::VehicleCard, MaterialTargetKind::ObjectDefinition | MaterialTargetKind::ObjectInstance => ExtractorKind::ObjectEntry, MaterialTargetKind::ArmorProfile => ExtractorKind::ArmorProfile, MaterialTargetKind::DamageProfile => ExtractorKind::DamageProfile, MaterialTargetKind::AbilityDefinition | MaterialTargetKind::AbilityInstance => ExtractorKind::AbilityEntry, MaterialTargetKind::CheckTarget => ExtractorKind::CheckProcedure, MaterialTargetKind::ConditionDefinition => ExtractorKind::Condition, _ => ExtractorKind::Generic } }
fn extractor_schema(kind: ExtractorKind) -> &'static str { match kind { ExtractorKind::ActorProfile | ExtractorKind::NpcCard => r#"{entry_name, actor_kind, archetype, threat_tier, stats, skills, hp, defense, equipment_refs, ability_refs, source_refs, missing_fields, confidence}"#, ExtractorKind::ObjectEntry | ExtractorKind::DamageProfile | ExtractorKind::ArmorProfile => r#"{entry_name, object_kind, aliases, mechanical_profile:{damage,range,rof,ammo,armor,properties,special_rules}, rule_bindings, source_refs, missing_fields, confidence}"#, ExtractorKind::AbilityEntry => r#"{entry_name, ability_kind, activation, cost, target, effect, trigger_bindings, source_refs, missing_fields, confidence}"#, _ => r#"{entry_name, fields, source_refs, missing_fields, confidence}"# } }
fn fallback_extraction(demand: &MaterializationDemand, bundle: &SourceEvidenceBundle) -> Value {
    let required = required_fields_for_target(demand.target_kind);
    let has_sources = !bundle.candidates.is_empty();
    json!({
        "entry_name": demand.target_label,
        "target_kind": demand.target_kind.as_str(),
        "fields": {},
        "mechanical_profile": {},
        "source_refs": bundle.candidates.iter().flat_map(|c| c.source_refs.clone()).collect::<Vec<_>>(),
        "missing_fields": required,
        "confidence": "low",
        "extraction_status": "insufficient_source_for_parameters",
        "candidate_evidence_present": has_sources,
        "provisional_reason": if has_sources {"candidate evidence exists but no complete source-backed parameter schema was extracted; do not write runtime values"} else {"no source candidate found; do not treat as exact"}
    })
}
fn required_fields_for_target(kind: MaterialTargetKind) -> Vec<&'static str> { match kind { MaterialTargetKind::ActorProfile | MaterialTargetKind::NpcStatBlock => vec!["actor_kind","hp","defense","skills","equipment_refs"], MaterialTargetKind::ObjectDefinition | MaterialTargetKind::ObjectInstance => vec!["entry_name","object_kind","mechanical_profile","source_refs"], MaterialTargetKind::DamageProfile => vec!["source_actor","target_actor","source_object_or_ability","damage_expression_or_effect"], MaterialTargetKind::ArmorProfile => vec!["armor_type","defense_or_sp","coverage_or_slot"], MaterialTargetKind::AbilityDefinition | MaterialTargetKind::AbilityInstance => vec!["activation","cost","target","effect","source_refs"], MaterialTargetKind::CheckTarget => vec!["check_label","target","dice_or_resolution_model"], MaterialTargetKind::EffectProfile => vec!["source","target","effect_model"], MaterialTargetKind::ConditionDefinition => vec!["condition_name","mechanical_effect"], _ => vec!["entry_name","fields"] } }
fn missing_required_fields(demand: &MaterializationDemand, extracted: &Value) -> Vec<String> {
    required_fields_for_target(demand.target_kind).into_iter().filter(|f| !json_has_field(extracted, f)).map(str::to_string).collect()
}
fn json_has_field(v: &Value, field: &str) -> bool {
    // Recursive: an object/array is "meaningful" only if it has at least one
    // meaningful leaf — so a `mechanical_profile` of all-null values does NOT count
    // as present, keeping an empty extraction (e.g. a retrieval miss) out of
    // bound_exact instead of passing the shallow key-presence check.
    fn meaningful(x: &Value) -> bool {
        match x {
            Value::Null => false,
            Value::String(s) => !s.trim().is_empty(),
            Value::Array(a) => a.iter().any(meaningful),
            Value::Object(o) => o.values().any(meaningful),
            _ => true,
        }
    }
    if v.get(field).is_some_and(meaningful) { return true; }
    if v.get("fields").and_then(|x| x.get(field)).is_some_and(meaningful) { return true; }
    if v.get("mechanical_profile").and_then(|x| x.get(field)).is_some_and(meaningful) { return true; }
    false
}
fn status_from_verification(v: &BindingVerificationResult) -> MaterializationDemandStatus { match v.status { BindingVerificationStatus::VerifiedExact => MaterializationDemandStatus::Verified, BindingVerificationStatus::VerifiedPartial => MaterializationDemandStatus::Bound, BindingVerificationStatus::ProvisionalNeedsAudit => MaterializationDemandStatus::Provisional, _ => MaterializationDemandStatus::Failed } }
fn visibility_issues(demand: &MaterializationDemand, packet: &RuleBindingPacket) -> Vec<String> { if demand.visibility == Visibility::PlayerVisible && packet.extracted_json.get("playwalled").and_then(Value::as_bool).unwrap_or(false) { vec!["playwalled material cannot be player-visible".into()] } else { vec![] } }
fn repair_suggestions_for(kind: MaterialTargetKind, missing: &[String]) -> Vec<String> { if missing.is_empty() { vec![] } else { vec![format!("Run a narrower {} extractor for missing fields: {}", kind.as_str(), missing.join(", "))] } }
fn profile_actor_json(demand: &MaterializationDemand, extraction: &ExtractionRun, verification: &BindingVerificationResult) -> Value {
    let hp = extracted_i64(&extraction.extracted_json, &["/hp", "/hp_max", "/status/hp_max", "/fields/hp", "/fields/hp_max", "/mechanical_profile/hp", "/mechanical_profile/hp_max"]);
    let defense = extraction.extracted_json.get("defense").or_else(|| extraction.extracted_json.pointer("/fields/defense")).cloned();
    let skills = extraction.extracted_json.get("skills").or_else(|| extraction.extracted_json.pointer("/fields/skills")).cloned();
    let equipment_refs = extraction.extracted_json.get("equipment_refs").or_else(|| extraction.extracted_json.pointer("/fields/equipment_refs")).cloned();
    let mut missing = Vec::new();
    if hp.is_none() { missing.push("hp"); }
    if defense.is_none() { missing.push("defense"); }
    if skills.is_none() { missing.push("skills"); }
    if equipment_refs.is_none() { missing.push("equipment_refs"); }
    json!({
        "ruleset": demand.ruleset_id,
        "label": demand.target_label,
        "extracted": extraction.extracted_json,
        "status": match hp { Some(v) => json!({"hp_max": v, "hp_current": v}), None => json!({"hp_max": null, "hp_current": null}) },
        "defense": defense,
        "skills": skills,
        "equipment_refs": equipment_refs,
        "missing_source_backed_fields": missing,
        "binding_verification": verification.status.as_str(),
        "source_policy": "source_backed_only_no_synthetic_defaults",
        "source": "real_materialization_extractor_v1_10"
    })
}
fn profile_object_json(demand: &MaterializationDemand, extraction: &ExtractionRun, verification: &BindingVerificationResult) -> Value { json!({"ruleset":demand.ruleset_id,"label":demand.target_label,"extracted":extraction.extracted_json,"binding_verification":verification.status.as_str(),"source_policy":"source_backed_only_no_synthetic_defaults","source":"real_materialization_extractor_v1_10"}) }
fn profile_ability_json(demand: &MaterializationDemand, extraction: &ExtractionRun, verification: &BindingVerificationResult) -> Value {
    json!({
        "ruleset": demand.ruleset_id,
        "label": demand.target_label,
        "activation": extraction.extracted_json.get("activation").cloned().unwrap_or_else(|| json!({"missing": true})),
        "cost": extraction.extracted_json.get("cost").cloned().unwrap_or_else(|| json!({"missing": true})),
        "target": extraction.extracted_json.get("target").cloned().unwrap_or_else(|| json!({"missing": true})),
        "effect": extraction.extracted_json.get("effect").cloned().unwrap_or_else(|| json!({"missing": true})),
        "extracted": extraction.extracted_json,
        "binding_verification": verification.status.as_str(),
        "source_policy": "source_backed_only_no_synthetic_defaults",
        "source": "real_materialization_extractor_v1_10"
    })
}
fn extracted_i64(v: &Value, paths: &[&str]) -> Option<i64> {
    for path in paths {
        if let Some(value) = v.pointer(path) {
            if let Some(n) = value.as_i64() { return Some(n); }
            if let Some(s) = value.as_str() { if let Ok(n) = s.trim().parse::<i64>() { return Some(n); } }
        }
    }
    None
}
fn strict_source_backed_materialization() -> bool { env_bool("TRPG_STRICT_SOURCE_BACKED_MATERIALIZATION", true) }
fn infer_object_kind(label: &str, extracted: &Value) -> ObjectKind {
    // Honor the extractor's OWN classification first — schema-guided extraction
    // sets `object_kind` reliably (e.g. "weapon" for a revolver). The label/keyword
    // heuristic below is only a fallback for un-typed extractions.
    if let Some(k) = extracted.get("object_kind").and_then(Value::as_str) {
        match k.to_ascii_lowercase().as_str() {
            "weapon" | "firearm" | "gun" => return ObjectKind::Weapon,
            "armor" | "armour" => return ObjectKind::Armor,
            "shield" => return ObjectKind::Shield,
            "vehicle" => return ObjectKind::Vehicle,
            "device" | "gear" | "tool" | "equipment" => return ObjectKind::Device,
            _ => {}
        }
    }
    let l = label.to_lowercase();
    if l.contains("armor") || l.contains("护甲") { ObjectKind::Armor }
    else if l.contains("shield") { ObjectKind::Shield }
    else if l.contains("gun") || l.contains("pistol") || l.contains("shotgun") || l.contains("revolver") || l.contains("rifle") || l.contains("knife") || l.contains("枪") || l.contains("刀") || extracted.pointer("/mechanical_profile/damage").is_some() || extracted.get("damage").is_some() { ObjectKind::Weapon }
    else if l.contains("vehicle") || l.contains("车") { ObjectKind::Vehicle }
    else if l.contains("cable") || l.contains("线缆") { ObjectKind::Device }
    else { ObjectKind::Unknown }
}
// Classify by LABEL keywords only (no per-ruleset branching); metadata, not resolution.
fn infer_ability_kind(_ruleset_id: &str, label: &str) -> AbilityKind {
    let l = label.to_lowercase();
    if l.contains("spell") || l.contains("shield") || l.contains("fireball") || label.contains("法术") || label.contains("魔法") { AbilityKind::Spell }
    else if l.contains("program") || l.contains("netrun") || l.contains("daemon") { AbilityKind::Program }
    else if l.contains("requisition") || label.contains("申请") || label.contains("异常") { AbilityKind::Requisition }
    else if l.contains("technique") || l.contains("stunt") { AbilityKind::Technique }
    else if l.contains("skill") { AbilityKind::SkillUse }
    else { AbilityKind::Unknown }
}
fn trim_excerpt(s: &str, max_chars: usize) -> String { s.chars().take(max_chars).collect() }
fn make_target_id(demand: &MaterializationDemand) -> String { format!("{}.{}.{}", safe_id(&demand.ruleset_id), demand.target_kind.as_str(), safe_id(&demand.target_label)) }
fn safe_id(s: &str) -> String { s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect::<String>().trim_matches('_').to_string() }
fn normalize_label(s: &str) -> String { safe_id(s) }
fn env_bool(key: &str, default: bool) -> bool { std::env::var(key).ok().map(|v| matches!(v.to_ascii_lowercase().as_str(), "1"|"true"|"yes"|"on")).unwrap_or(default) }

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn scopes(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn foreign_ruleset_hit_is_dropped() {
        // A hit scoped to another ruleset is foreign (the .38-saga contamination:
        // a Cyberpunk row leaking into a CoC query on a shared index).
        let cyber = scopes(&[("scope_type", "ruleset"), ("scope_id", "cyberpunk_red")]);
        assert!(hit_is_foreign_ruleset(&cyber, "call_of_cthulhu_7e"));
        // Same ruleset (case-insensitive) is kept.
        let coc = scopes(&[("scope_type", "ruleset"), ("scope_id", "Call_Of_Cthulhu_7e")]);
        assert!(!hit_is_foreign_ruleset(&coc, "call_of_cthulhu_7e"));
        // Generic / unscoped content is NEVER dropped (no false negatives).
        assert!(!hit_is_foreign_ruleset(&scopes(&[("scope_type", "source")]), "call_of_cthulhu_7e"));
        assert!(!hit_is_foreign_ruleset(&scopes(&[]), "call_of_cthulhu_7e"));
        // A direct ruleset_id scope is honored too.
        assert!(hit_is_foreign_ruleset(&scopes(&[("ruleset_id", "dnd5e")]), "call_of_cthulhu_7e"));
    }

    #[test]
    fn all_null_mechanical_profile_is_not_present() {
        // An all-null mechanical_profile must read as MISSING (retrieval miss),
        // not pass the shallow key-presence check into bound_exact.
        let empty = json!({"mechanical_profile": {"damage": null, "range": null, "ammo": null}});
        assert!(!json_has_field(&empty, "mechanical_profile"));
        // A populated profile is present.
        let filled = json!({"mechanical_profile": {"damage": "1D10", "range": null}});
        assert!(json_has_field(&filled, "mechanical_profile"));
        // Nested lookup: a required field living inside mechanical_profile is found.
        assert!(json_has_field(&filled, "damage"));
        // Numbers (incl. 0) count as meaningful.
        assert!(json_has_field(&json!({"mechanical_profile": {"ammo": 0}}), "mechanical_profile"));
    }

    // -------------------------------------------------------------------------
    // P0-2 T5: equivalence tests — data-driven search_profile / module_search_profile
    // -------------------------------------------------------------------------

    fn kernel_with_cyberpunk_search_profile() -> RuleKernel {
        let mut k: RuleKernel = serde_json::from_str(
            r#"{"kernel_id":"t","ruleset_id":"cyberpunk_red","version":"1"}"#
        ).unwrap();
        let mut by_skill = std::collections::HashMap::new();
        by_skill.insert("combat_resolution".to_string(), vec!["Friday Night Firefight".to_string(), "Getting it Done".to_string()]);
        let mut fa = std::collections::HashMap::new();
        fa.insert("combat_resolution".to_string(), json!({"target_number": ["DV","range DV"]}));
        k.search_profile = Some(RuleKernelSearchProfile {
            preferred_sections_by_skill: by_skill,
            field_aliases_by_skill: fa,
        });
        k
    }

    #[test]
    fn profile_sections_override_generic_base() {
        let k = kernel_with_cyberpunk_search_profile();
        let aliases = ruleset_aliases_for(Some(&k), SearchSkillKind::CombatResolution);
        let sections = aliases["preferred_sections"].as_array().unwrap();
        assert!(sections.iter().any(|v| v.as_str() == Some("Friday Night Firefight")),
            "cyberpunk search_profile must supply FNF section; got {:?}", sections);
    }

    #[test]
    fn profile_field_aliases_merged_onto_base() {
        let k = kernel_with_cyberpunk_search_profile();
        let aliases = ruleset_aliases_for(Some(&k), SearchSkillKind::CombatResolution);
        let tn = aliases["field_aliases"]["target_number"].as_array().unwrap();
        assert!(tn.iter().any(|v| v.as_str() == Some("DV")),
            "field_aliases must include DV from search_profile override; got {:?}", tn);
    }

    #[test]
    fn no_profile_falls_back_to_generic_base() {
        let k: RuleKernel = serde_json::from_str(
            r#"{"kernel_id":"t","ruleset_id":"generic_ruleset","version":"1"}"#
        ).unwrap();
        let aliases = ruleset_aliases_for(Some(&k), SearchSkillKind::CombatResolution);
        let sections = aliases["preferred_sections"].as_array().unwrap();
        let has_fnf = sections.iter().any(|v| v.as_str() == Some("Friday Night Firefight"));
        assert!(!has_fnf, "generic kernel must NOT have cyberpunk-specific FNF section");
        assert!(!sections.is_empty(), "generic base must still provide sections");
    }

    #[test]
    fn none_kernel_uses_generic_base() {
        let aliases = ruleset_aliases_for(None, SearchSkillKind::WeaponParameter);
        assert!(aliases.get("preferred_sections").and_then(|v| v.as_array()).is_some(),
            "none kernel must still return preferred_sections from generic base");
    }

    /// FIX 2: a kernel mirroring the live cyberpunk_red override — flat list +
    /// field_aliases under the `"*"` wildcard (legacy applied them to ALL skills).
    fn kernel_with_cyberpunk_wildcard_override() -> RuleKernel {
        let mut k: RuleKernel = serde_json::from_str(
            r#"{"kernel_id":"t","ruleset_id":"cyberpunk_red","version":"1"}"#
        ).unwrap();
        let mut by_skill = std::collections::HashMap::new();
        by_skill.insert("*".to_string(), vec![
            "Getting it Done".to_string(),
            "Resolving Actions with Skills".to_string(),
            "Weapons and Armor".to_string(),
            "Friday Night Firefight".to_string(),
            "Ranged Combat".to_string(),
            "Melee Combat".to_string(),
            "Before You Take Damage".to_string(),
            "When Armor Doesn't Cut It".to_string(),
            "Role Abilities".to_string(),
        ]);
        let mut fa = std::collections::HashMap::new();
        fa.insert("*".to_string(), json!({
            "target_number": ["DV","range DV","difficulty value"],
            "armor": ["SP","armor","ablation"],
        }));
        k.search_profile = Some(RuleKernelSearchProfile {
            preferred_sections_by_skill: by_skill,
            field_aliases_by_skill: fa,
        });
        k
    }

    #[test]
    fn wildcard_override_applies_to_uncovered_skills() {
        // Legacy ruleset_aliases_for applied the flat list to ALL 9 skills. The
        // old per-skill map never enumerated AbilityActivation / GenericMechanical
        // for cyberpunk, so those were dropped. The "*" wildcard restores them.
        let k = kernel_with_cyberpunk_wildcard_override();
        let expected = json!([
            "Getting it Done","Resolving Actions with Skills","Weapons and Armor",
            "Friday Night Firefight","Ranged Combat","Melee Combat",
            "Before You Take Damage","When Armor Doesn't Cut It","Role Abilities"
        ]);
        for skill in [SearchSkillKind::AbilityActivation, SearchSkillKind::GenericMechanical, SearchSkillKind::ConditionResource] {
            let aliases = ruleset_aliases_for(Some(&k), skill);
            assert_eq!(aliases["preferred_sections"], expected,
                "uncovered skill {:?} must get the cyberpunk flat list via wildcard", skill);
            // field_aliases set on ALL skills (legacy SET behavior).
            assert_eq!(aliases["field_aliases"]["target_number"], json!(["DV","range DV","difficulty value"]),
                "wildcard target_number must apply to {:?}", skill);
            assert_eq!(aliases["field_aliases"]["armor"], json!(["SP","armor","ablation"]),
                "wildcard armor must apply to {:?}", skill);
        }
        // Per-skill key still wins over wildcard when both present.
        let mut k2 = kernel_with_cyberpunk_wildcard_override();
        if let Some(sp) = k2.search_profile.as_mut() {
            sp.preferred_sections_by_skill.insert("weapon_parameter".to_string(), vec!["Weapons and Armor".to_string()]);
        }
        let wp = ruleset_aliases_for(Some(&k2), SearchSkillKind::WeaponParameter);
        assert_eq!(wp["preferred_sections"], json!(["Weapons and Armor"]),
            "per-skill key must override the wildcard");
    }

    fn module_config_with_homecoming_prefs() -> ModuleConfig {
        let mut cfg = ModuleConfig::default();
        let mut by_skill = std::collections::HashMap::new();
        by_skill.insert("npc_stat_block".to_string(), vec!["Redesigned NPC cards".to_string(), "vehicle cards".to_string()]);
        cfg.module_search_profile = Some(SearchProfile { preferred_sections_by_skill: by_skill });
        cfg
    }

    #[test]
    fn module_config_prefs_extend_base_for_skill() {
        let cfg = module_config_with_homecoming_prefs();
        let prefs = module_preferences_for(Some(&cfg), SearchSkillKind::NpcStatblock);
        assert!(prefs.contains(&"npc card".to_string()), "base 'npc card' must remain");
        assert!(prefs.contains(&"Redesigned NPC cards".to_string()),
            "homecoming-specific section must be present; got {:?}", prefs);
    }

    #[test]
    fn no_module_config_gives_generic_base_only() {
        let prefs = module_preferences_for(None, SearchSkillKind::NpcStatblock);
        assert!(prefs.contains(&"npc card".to_string()), "generic base must include 'npc card'");
        assert!(!prefs.iter().any(|s| s.contains("Redesigned NPC")),
            "generic base must NOT include homecoming-specific sections; got {:?}", prefs);
    }

    #[test]
    fn module_config_wrong_skill_key_gives_base_only() {
        // Homecoming has npc_stat_block prefs but NOT combat_resolution prefs
        let cfg = module_config_with_homecoming_prefs();
        let prefs = module_preferences_for(Some(&cfg), SearchSkillKind::CombatResolution);
        assert!(!prefs.iter().any(|s| s.contains("Redesigned NPC")),
            "combat_resolution skill must not get npc sections from homecoming config; got {:?}", prefs);
    }
}
