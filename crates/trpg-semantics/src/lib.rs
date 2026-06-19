use anyhow::Result;
use chrono::Utc;
use serde_json::{json, Value};
use sqlx::Row;
use trpg_db::Db;
use trpg_llm::{LlmClient, LlmConfig, OpenAiCompatibleClient};
use trpg_model::*;
use trpg_search::SearchService;
use uuid::Uuid;

mod need;
pub use need::{NeedSignal, SemanticNeedClassifier};

#[derive(Clone)]
pub struct SemanticIntentService {
    pub db: Db,
    pub llm: Option<OpenAiCompatibleClient>,
    pub lexical_fallback: bool,
}

impl SemanticIntentService {
    pub fn from_env(db: Db) -> Self {
        let enabled = env_bool("TRPG_SEMANTIC_CLASSIFIER_ENABLE_V19", true)
            && env_bool("TRPG_SEMANTIC_PRIMARY", true);
        let llm = if enabled {
            LlmConfig::from_env()
                .ok()
                .and_then(|cfg| OpenAiCompatibleClient::new(cfg).ok())
        } else {
            None
        };
        let lexical_fallback = env_bool("TRPG_LEXICAL_FALLBACK_ENABLE", false);
        Self {
            db,
            llm,
            lexical_fallback,
        }
    }

    pub async fn classify_turn(&self, req: SemanticIntentRequest) -> Result<SemanticIntentResult> {
        if let Some(client) = &self.llm {
            match self.classify_turn_llm(client, &req).await {
                Ok(result) => {
                    self.insert_semantic_event(&result).await.ok();
                    return Ok(result);
                }
                Err(err) => {
                    tracing::warn!(error=%err, "semantic classifier failed; using audited fallback")
                }
            }
        }
        let result = if self.lexical_fallback {
            classify_turn_fallback(req, "fallback_lexical")
        } else {
            semantic_result_from_json(
                &req,
                json!({"primary_action_kind":"unknown", "gate_relation":"ambiguous", "frame_relation":"outside_frame_action", "target_refs":[], "object_refs":[], "ability_refs":[], "materialization_requests":[], "secrecy_policy":"gm_only", "confidence":"low", "rationale_brief":"semantic classifier unavailable and lexical fallback disabled"}),
                "semantic_unavailable_no_route",
            )
        };
        self.insert_semantic_event(&result).await.ok();
        Ok(result)
    }

    async fn classify_turn_llm(
        &self,
        client: &OpenAiCompatibleClient,
        req: &SemanticIntentRequest,
    ) -> Result<SemanticIntentResult> {
        let schema = r#"Return ONLY JSON with fields:
{
  "primary_action_kind": "attack|under_attack|enemy_initiated_conflict|scene_enters_conflict|defend|dodge|counterattack|take_cover|move|flee|hide|negotiate|surrender|hack|disable_device|rescue|activate_ability|trigger_ability|use_item|ask_question|unknown",
  "gate_relation": "no_active_gate|roll_reply|choice_reply|superseding_intent|unrelated_new_action|ambiguous",
  "frame_relation": "no_active_frame|inside_frame_action|required_gate_response|exit_attempt|deescalation_attempt|surrender|flee|hide_to_disengage|pause_and_observe|outside_frame_action|clarification|invalid_or_ambiguous",
  "target_refs": [],
  "object_refs": [],
  "object_search_keys": [],
  "object_interaction_kind": "none|disarm|grab_held_object|pick_up|drop|equip|reload|cut_connection|trace_connection|inspect|unlock|hack|break|steal|loot|search",
  "ability_refs": [],
  "materialization_requests": [
    {"target_kind":"actor_parameter|object_definition|ability_definition|check_contract|effect_contract|condition_definition", "target_label":"", "target_description":"", "evidence_span":"", "requested_fields":[], "urgency":"immediate|before_resolution|soon|background", "visibility":"gm_only|player_visible|public"}
  ],
  "route_cues": {
    "combat_action": false,
    "sustained_combat_action": false,
    "incoming_harm": false,
    "terminal_or_exit": false,
    "roll_or_effect_reply": false,
    "source_object_use": false,
    "object_interaction": false,
    "assessment_check": false
  },
  "secrecy_policy": "player_visible|gm_only|secret_roll|playwalled",
  "confidence": "high|medium|low",
  "rationale_brief": "short debug-only rationale"
}"#;
        let messages = vec![
            ChatMessage { role: "system".into(), content: format!("You are a bounded semantic classifier for a Rust TRPG GM agent. Do not route by keywords. Infer player intent, targets, materialization needs, and secrecy from meaning across languages and paraphrases. Treat phrases like firing a weapon, swinging a blade, using a spell offensively, or continuing pressure as combat_action even if no canonical keyword appears. A named weapon in an attack is usually a source_object_use, not an object_interaction. Compound actions may have multiple labels: disarming an enemy is both a combat maneuver and an object interaction; report object_interaction_kind and object materialization requests instead of collapsing it to generic attack. route_cues are evidence for the deterministic Rust reducer, not final authority. For EACH object_refs item, emit object_search_keys[i] (same order and length) = the MINIMAL distinguishing token to find that item in the rulebook: a caliber/model/proper-noun like '.38' (for '.38 revolver'), 'Glock 17', or 'Desert Eagle'; DROP generic category words (revolver/pistol/gun/handgun/sword/blade). Prefer language-neutral tokens (numbers/model codes that read the same across languages); only if none exists, give the term in the RULEBOOK's language (translate the player's word — do NOT invent synonyms). {schema}") },
            ChatMessage { role: "user".into(), content: serde_json::to_string(&req).unwrap_or_default() },
        ];
        let value = client.complete_json(messages, 0.0).await?;
        Ok(semantic_result_from_json(
            req,
            value,
            "llm_semantic_classifier_v1_9",
        ))
    }

    pub async fn insert_semantic_event(&self, result: &SemanticIntentResult) -> Result<()> {
        sqlx::query(r#"
            insert into semantic_classification_events
              (id, semantic_id, session_id, turn_id, ruleset_id, classifier, confidence, result_json)
            values ($1,$2,$3,$4,$5,$6,$7,$8)
            on conflict (semantic_id) do nothing
        "#)
        .bind(Uuid::new_v4())
        .bind(&result.semantic_id)
        .bind(&result.session_id)
        .bind(&result.turn_id)
        .bind(&result.ruleset_id)
        .bind(&result.classifier)
        .bind(result.confidence.as_str())
        .bind(serde_json::to_value(result)?)
        .execute(&self.db.pool).await?;
        Ok(())
    }
}

pub struct SemanticRuleBindingService {
    pub db: Db,
    pub search: Option<SearchService>,
    pub llm: Option<OpenAiCompatibleClient>,
}
impl SemanticRuleBindingService {
    pub fn from_env(db: Db, search: Option<SearchService>) -> Self {
        let llm = if env_bool("TRPG_SEMANTIC_EXTRACTOR_ENABLE_V19", true) {
            LlmConfig::from_env()
                .ok()
                .and_then(|cfg| OpenAiCompatibleClient::new(cfg).ok())
        } else {
            None
        };
        Self { db, search, llm }
    }

    pub fn plan_queries(
        &self,
        request: &MaterializationRequest,
        ruleset_id: &str,
    ) -> SemanticQueryPlan {
        let label = request.target_label.trim();
        let fields = if request.requested_fields.is_empty() {
            default_fields_for(request.target_kind)
        } else {
            request.requested_fields.clone()
        };
        let mut exact = vec![label.to_string()];
        if !request.evidence_span.is_empty() && request.evidence_span != label {
            exact.push(request.evidence_span.clone());
        }
        let field_queries = fields
            .iter()
            .map(|f| format!("{} {} {}", ruleset_id, label, f))
            .collect::<Vec<_>>();
        let broad = vec![
            format!("{} {} rules entry", ruleset_id, label),
            format!(
                "{} {} source rule ability object actor",
                ruleset_id, request.target_description
            ),
        ];
        SemanticQueryPlan {
            plan_id: format!("query_plan_{}", Uuid::new_v4().simple()),
            target_kind: request.target_kind,
            target_label: label.to_string(),
            ruleset_id: ruleset_id.to_string(),
            exact_name_queries: exact,
            alias_queries: vec![format!("{} alias", label)],
            field_queries,
            broad_queries: broad,
            requested_fields: fields,
            query_json: serde_json::to_value(request).unwrap_or_else(|_| json!({})),
        }
    }

    pub async fn bind_request(
        &self,
        session_id: &str,
        turn_id: Option<&str>,
        target_id: &str,
        ruleset_id: &str,
        request: &MaterializationRequest,
        world_tick: Option<i64>,
    ) -> Result<RuleBindingPacket> {
        let plan = self.plan_queries(request, ruleset_id);
        let mut hits = Vec::new();
        if let Some(search) = &self.search {
            for q in plan
                .exact_name_queries
                .iter()
                .chain(plan.field_queries.iter())
                .chain(plan.broad_queries.iter())
                .take(6)
            {
                let resp = search
                    .search_async(&SearchRequest {
                        query: q.clone(),
                        domains: vec![ruleset_id.to_string()],
                        limit: 4,
                        intent: Some("rule_binding_candidate_retrieval".into()),
                        ..Default::default()
                    })
                    .await
                    .unwrap_or_default();
                for h in resp.hits {
                    if !hits
                        .iter()
                        .any(|x: &SearchHit| x.search_doc_id == h.search_doc_id)
                    {
                        hits.push(h);
                    }
                }
            }
        }
        let extracted_json = if let Some(client) = &self.llm {
            self.extract_with_llm(client, request, &plan, &hits)
                .await
                .unwrap_or_else(|_| fallback_extracted_json(request, &plan, &hits))
        } else {
            fallback_extracted_json(request, &plan, &hits)
        };
        let source_refs = hits
            .iter()
            .flat_map(|h| h.source_refs.clone())
            .collect::<Vec<_>>();
        let status = if hits.is_empty() {
            BindingStatus::NeedsReview
        } else {
            BindingStatus::BoundProvisional
        };
        let packet = RuleBindingPacket {
            binding_id: format!("rule_binding_{}", Uuid::new_v4().simple()),
            target_kind: request.target_kind,
            target_id: target_id.into(),
            ruleset_id: ruleset_id.into(),
            semantic_request_json: serde_json::to_value(request).unwrap_or_else(|_| json!({})),
            retrieval_queries: plan
                .exact_name_queries
                .iter()
                .chain(plan.field_queries.iter())
                .chain(plan.broad_queries.iter())
                .cloned()
                .collect(),
            source_hits: hits,
            extracted_json,
            source_refs,
            confidence: if matches!(status, BindingStatus::BoundProvisional) {
                RulingConfidence::Medium
            } else {
                RulingConfidence::Low
            },
            verification_status: status,
            created_at_tick: world_tick,
            created_at: Utc::now(),
        };
        self.insert_rule_binding_packet(session_id, turn_id, &packet)
            .await?;
        Ok(packet)
    }

    async fn extract_with_llm(
        &self,
        client: &OpenAiCompatibleClient,
        request: &MaterializationRequest,
        plan: &SemanticQueryPlan,
        hits: &[SearchHit],
    ) -> Result<Value> {
        let compact_hits = hits.iter().take(6).map(|h| json!({"title":h.title,"snippet":h.snippet,"kind":h.logical_kind,"source_refs":h.source_refs})).collect::<Vec<_>>();
        let messages = vec![
            ChatMessage { role: "system".into(), content: "You extract concise structured TRPG rule fields from retrieved candidates. Do not invent fields. Return JSON only with extracted fields, missing_fields, confidence, and source_refs.".into() },
            ChatMessage { role: "user".into(), content: serde_json::to_string(&json!({"request":request,"query_plan":plan,"hits":compact_hits})).unwrap_or_default() },
        ];
        client.complete_json(messages, 0.0).await
    }

    pub async fn insert_rule_binding_packet(
        &self,
        session_id: &str,
        turn_id: Option<&str>,
        packet: &RuleBindingPacket,
    ) -> Result<()> {
        sqlx::query(r#"
            insert into rule_binding_packets
              (id, binding_id, session_id, turn_id, target_kind, target_id, ruleset_id, semantic_request_json,
               retrieval_queries, source_hits, extracted_json, source_refs, confidence, verification_status, world_tick)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)
            on conflict (binding_id) do nothing
        "#)
        .bind(Uuid::new_v4())
        .bind(&packet.binding_id)
        .bind(session_id)
        .bind(turn_id)
        .bind(packet.target_kind.as_str())
        .bind(&packet.target_id)
        .bind(&packet.ruleset_id)
        .bind(&packet.semantic_request_json)
        .bind(serde_json::to_value(&packet.retrieval_queries)?)
        .bind(serde_json::to_value(&packet.source_hits)?)
        .bind(&packet.extracted_json)
        .bind(serde_json::to_value(&packet.source_refs)?)
        .bind(packet.confidence.as_str())
        .bind(packet.verification_status.as_str())
        .bind(packet.created_at_tick)
        .execute(&self.db.pool).await?;
        Ok(())
    }

    pub async fn rule_binding_context_block(
        &self,
        session_id: &str,
        world_tick: i64,
    ) -> Result<ContextBlock> {
        let rows = sqlx::query(r#"
            select binding_id, target_kind, target_id, ruleset_id, extracted_json, confidence, verification_status, world_tick, created_at
            from rule_binding_packets where session_id=$1 order by created_at desc limit 12
        "#).bind(session_id).fetch_all(&self.db.pool).await?;
        let packets = rows.into_iter().map(|r| json!({"binding_id":r.get::<String,_>("binding_id"),"target_kind":r.get::<String,_>("target_kind"),"target_id":r.get::<String,_>("target_id"),"ruleset_id":r.get::<String,_>("ruleset_id"),"extracted_json":r.get::<Value,_>("extracted_json"),"confidence":r.get::<String,_>("confidence"),"verification_status":r.get::<String,_>("verification_status"),"world_tick":r.get::<Option<i64>,_>("world_tick")})).collect::<Vec<_>>();
        let mut block = ContextBlock::new(
            format!("runtime.rule_bindings.{}", session_id),
            BlockKind::RuleBindingPacket,
            "Runtime Rule Binding Packets",
            BlockContent::Json(
                json!({"world_tick":world_tick,"packets":packets,"cache_policy":"BP3 dynamic; exact rules hydrate actor/object/ability/check/effect parameters and must not remain prompt-only"}),
            ),
            Visibility::GmOnly,
            Stability::TurnDynamic,
            CacheZone::DynamicTail,
            Scope {
                scope_type: ScopeType::Session,
                scope_id: session_id.into(),
            },
            169,
        );
        block.tags = vec![
            "rule_binding".into(),
            "semantic_hydration".into(),
            "bp3".into(),
        ];
        Ok(block)
    }
}

fn semantic_result_from_json(
    req: &SemanticIntentRequest,
    value: Value,
    classifier: &str,
) -> SemanticIntentResult {
    let primary = parse_action(value.get("primary_action_kind").and_then(Value::as_str));
    let frame_relation = parse_frame_relation(value.get("frame_relation").and_then(Value::as_str));
    let confidence = parse_confidence(value.get("confidence").and_then(Value::as_str));
    let target_refs = string_vec(value.get("target_refs"));
    let object_refs = string_vec(value.get("object_refs"));
    let ability_refs = string_vec(value.get("ability_refs"));
    let materialization_requests = value
        .get("materialization_requests")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().map(materialization_request_from_json).collect())
        .unwrap_or_default();
    SemanticIntentResult {
        semantic_id: format!("semantic_{}", Uuid::new_v4().simple()),
        session_id: req.session_id.clone(),
        turn_id: req.turn_id.clone(),
        ruleset_id: req.ruleset_id.clone(),
        primary_action_kind: primary,
        gate_relation: value
            .get("gate_relation")
            .and_then(Value::as_str)
            .unwrap_or("ambiguous")
            .into(),
        frame_relation,
        target_refs,
        object_refs,
        ability_refs,
        materialization_requests,
        secrecy_policy: value
            .get("secrecy_policy")
            .and_then(Value::as_str)
            .unwrap_or("gm_only")
            .into(),
        confidence,
        classifier: classifier.into(),
        rationale_brief: value
            .get("rationale_brief")
            .and_then(Value::as_str)
            .map(str::to_string),
        raw_json: value,
        created_at: Utc::now(),
    }
}

fn classify_turn_fallback(req: SemanticIntentRequest, classifier: &str) -> SemanticIntentResult {
    // This fallback is intentionally audited. It should not be the normal business route.
    let lower = req.player_input.to_lowercase();
    let mut requests = Vec::new();
    let mut action = SituationActionKind::Unknown;
    if lower.contains("shield")
        || lower.contains("fireball")
        || lower.contains("法术")
        || lower.contains("施放")
        || lower.contains("咒")
        || lower.contains("能力")
        || lower.contains("role ability")
        || lower.contains("program")
        || lower.contains("战斗特技")
    {
        action = SituationActionKind::ActivateAbility;
        requests.push(MaterializationRequest {
            request_id: format!("matreq_{}", Uuid::new_v4().simple()),
            target_kind: RuleBindingTargetKind::AbilityDefinition,
            target_id: None,
            target_label: infer_ability_label(&req.player_input),
            target_description: req.player_input.clone(),
            evidence_span: req.player_input.clone(),
            requested_fields: vec![
                "activation".into(),
                "cost".into(),
                "target".into(),
                "effect".into(),
                "trigger".into(),
            ],
            urgency: RuntimeUrgency::BeforeResolution,
            visibility: Visibility::GmOnly,
            metadata: json!({"fallback":"lexical_audit_only"}),
        });
    }
    semantic_result_from_json(
        &req,
        json!({"primary_action_kind": action.as_str(), "gate_relation":"ambiguous", "frame_relation":"outside_frame_action", "target_refs":[], "object_refs":[], "ability_refs": requests.iter().map(|r| r.target_label.clone()).collect::<Vec<_>>(), "materialization_requests": requests, "secrecy_policy":"gm_only", "confidence":"low", "rationale_brief":"fallback lexical classifier used; configure semantic LLM for production"}),
        classifier,
    )
}

fn materialization_request_from_json(v: &Value) -> MaterializationRequest {
    MaterializationRequest {
        request_id: format!("matreq_{}", Uuid::new_v4().simple()),
        target_kind: parse_target_kind(v.get("target_kind").and_then(Value::as_str)),
        target_id: v
            .get("target_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        target_label: v
            .get("target_label")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        target_description: v
            .get("target_description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        evidence_span: v
            .get("evidence_span")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        requested_fields: string_vec(v.get("requested_fields")),
        urgency: parse_urgency(v.get("urgency").and_then(Value::as_str)),
        visibility: parse_visibility(v.get("visibility").and_then(Value::as_str)),
        metadata: v.clone(),
    }
}
fn parse_target_kind(s: Option<&str>) -> RuleBindingTargetKind {
    match s.unwrap_or_default() {
        "actor_parameter" => RuleBindingTargetKind::ActorParameter,
        "object_definition" => RuleBindingTargetKind::ObjectDefinition,
        "ability_definition" => RuleBindingTargetKind::AbilityDefinition,
        "check_contract" => RuleBindingTargetKind::CheckContract,
        "effect_contract" => RuleBindingTargetKind::EffectContract,
        "contest_profile" => RuleBindingTargetKind::ContestProfile,
        "condition_definition" => RuleBindingTargetKind::ConditionDefinition,
        _ => RuleBindingTargetKind::Unknown,
    }
}
fn parse_urgency(s: Option<&str>) -> RuntimeUrgency {
    match s.unwrap_or_default() {
        "immediate" => RuntimeUrgency::Immediate,
        "before_resolution" => RuntimeUrgency::BeforeResolution,
        "background" => RuntimeUrgency::Background,
        _ => RuntimeUrgency::Soon,
    }
}
fn parse_visibility(s: Option<&str>) -> Visibility {
    match s.unwrap_or_default() {
        "public" => Visibility::Public,
        "player_visible" => Visibility::PlayerVisible,
        "npc_private" => Visibility::NpcPrivate,
        "system_only" => Visibility::SystemOnly,
        _ => Visibility::GmOnly,
    }
}
fn parse_confidence(s: Option<&str>) -> RulingConfidence {
    match s.unwrap_or_default() {
        "high" => RulingConfidence::High,
        "medium" => RulingConfidence::Medium,
        _ => RulingConfidence::Low,
    }
}
fn parse_frame_relation(s: Option<&str>) -> FrameRelation {
    match s.unwrap_or_default() {
        "no_active_frame" => FrameRelation::OutsideFrameAction,
        "inside_frame_action" => FrameRelation::InsideFrameAction,
        "required_gate_response" | "gate_response" => FrameRelation::GateResponse,
        "exit_attempt" => FrameRelation::ExitAttempt,
        "deescalation_attempt" => FrameRelation::DeescalationAttempt,
        "surrender" => FrameRelation::Surrender,
        "flee" => FrameRelation::Flee,
        "hide_to_disengage" => FrameRelation::HideToDisengage,
        "pause_and_observe" => FrameRelation::PauseAndObserve,
        "clarification" => FrameRelation::Clarification,
        "invalid_or_ambiguous" => FrameRelation::InvalidOrAmbiguous,
        _ => FrameRelation::OutsideFrameAction,
    }
}
fn parse_action(s: Option<&str>) -> SituationActionKind {
    match s.unwrap_or_default() {
        "attack" => SituationActionKind::Attack,
        "under_attack" => SituationActionKind::UnderAttack,
        "enemy_initiated_conflict" => SituationActionKind::EnemyInitiatedConflict,
        "scene_enters_conflict" => SituationActionKind::SceneEntersConflict,
        "defend" => SituationActionKind::Defend,
        "dodge" => SituationActionKind::Dodge,
        "counterattack" => SituationActionKind::Counterattack,
        "take_cover" => SituationActionKind::TakeCover,
        "move" => SituationActionKind::Move,
        "flee" => SituationActionKind::Flee,
        "hide" => SituationActionKind::Hide,
        "negotiate" => SituationActionKind::Negotiate,
        "surrender" => SituationActionKind::Surrender,
        "hack" => SituationActionKind::Hack,
        "disable_device" => SituationActionKind::DisableDevice,
        "rescue" => SituationActionKind::Rescue,
        "activate_ability" => SituationActionKind::ActivateAbility,
        "trigger_ability" => SituationActionKind::TriggerAbility,
        "use_item" => SituationActionKind::UseItem,
        "ask_question" => SituationActionKind::AskQuestion,
        _ => SituationActionKind::Unknown,
    }
}
fn string_vec(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}
fn default_fields_for(kind: RuleBindingTargetKind) -> Vec<String> {
    match kind {
        RuleBindingTargetKind::AbilityDefinition => vec![
            "activation".into(),
            "cost".into(),
            "target".into(),
            "effect".into(),
            "duration".into(),
            "trigger".into(),
        ],
        RuleBindingTargetKind::ObjectDefinition => vec![
            "damage".into(),
            "range".into(),
            "ammo".into(),
            "armor".into(),
            "properties".into(),
        ],
        RuleBindingTargetKind::ActorParameter => vec![
            "stats".into(),
            "skills".into(),
            "hp".into(),
            "defense".into(),
            "abilities".into(),
        ],
        _ => vec!["rule".into(), "effect".into()],
    }
}
fn fallback_extracted_json(
    request: &MaterializationRequest,
    plan: &SemanticQueryPlan,
    hits: &[SearchHit],
) -> Value {
    json!({"entry_name": request.target_label, "requested_fields": plan.requested_fields, "extraction_status": if hits.is_empty(){"needs_review_no_hits"}else{"candidate_hits_available"}, "fields": {}, "missing_fields": plan.requested_fields, "confidence": if hits.is_empty(){"low"}else{"medium"}, "source_hit_count": hits.len()})
}
fn infer_ability_label(input: &str) -> String {
    let t = input.trim();
    if t.to_lowercase().contains("shield") {
        "Shield".into()
    } else if t.to_lowercase().contains("fireball") {
        "Fireball".into()
    } else if t.contains("战斗特技") {
        "战斗特技".into()
    } else if t.contains("role ability") {
        "Role Ability".into()
    } else {
        t.chars().take(48).collect()
    }
}
fn env_bool(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(default)
}
