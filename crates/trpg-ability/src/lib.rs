use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::Row;
use trpg_db::Db;
use trpg_model::*;
use trpg_search::SearchService;
use trpg_semantics::{SemanticIntentService, SemanticRuleBindingService};
use uuid::Uuid;

#[derive(Debug, Clone, Copy)]
pub struct AbilityTurnInput<'a> {
    pub session_id: &'a str,
    pub turn_id: &'a str,
    pub ruleset_id: &'a str,
    pub module_id: Option<&'a str>,
    pub actor_id: Option<&'a str>,
    pub frame_id: Option<&'a str>,
    pub user_input: &'a str,
    pub world_tick: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AbilityTurnResult {
    pub handled: bool,
    pub phases: Vec<String>,
    pub semantic: Option<SemanticIntentResult>,
    pub ability_definition: Option<AbilityDefinition>,
    pub ability_instance: Option<AbilityInstance>,
    pub activation: Option<AbilityActivationContract>,
    pub rule_binding: Option<RuleBindingPacket>,
    pub narration_context: Option<String>,
    /// A source-backed check built from the ruleset's CORE mechanic (rule
    /// kernel) so the CLI/API can actually resolve the ability attempt with a
    /// real roll instead of leaving an empty activation contract.
    pub check: Option<CheckContract>,
}
impl AbilityTurnResult { pub fn not_handled() -> Self { Self::default() } pub fn handled() -> Self { Self { handled: true, ..Default::default() } } }

#[derive(Clone)]
pub struct AbilityService { pub db: Db, pub search: Option<SearchService> }
impl AbilityService {
    pub fn new(db: Db, search: Option<SearchService>) -> Self { Self { db, search } }

    pub async fn handle_turn(&self, input: AbilityTurnInput<'_>) -> Result<AbilityTurnResult> {
        if !env_bool("TRPG_ABILITY_KERNEL_ENABLE_V19", true) { return Ok(AbilityTurnResult::not_handled()); }
        let semantic_req = SemanticIntentRequest { session_id: input.session_id.into(), turn_id: input.turn_id.into(), ruleset_id: input.ruleset_id.into(), module_id: input.module_id.map(str::to_string), active_frame_summary: json!({"frame_id": input.frame_id}), active_gate_summary: json!(null), player_input: input.user_input.into(), visible_scene_objects: json!({}), actor_parameter_summary: json!({"actor_id": input.actor_id.unwrap_or("pc.current")}) };
        let semantic = SemanticIntentService::from_env(self.db.clone()).classify_turn(semantic_req).await?;
        let ability_requests: Vec<MaterializationRequest> = semantic.materialization_requests.iter().filter(|r| r.target_kind == RuleBindingTargetKind::AbilityDefinition).cloned().collect();
        if !matches!(semantic.primary_action_kind, SituationActionKind::ActivateAbility | SituationActionKind::TriggerAbility) && ability_requests.is_empty() { return Ok(AbilityTurnResult::not_handled()); }
        let actor_id = input.actor_id.unwrap_or("pc.current");
        let req = ability_requests.first().cloned().unwrap_or_else(|| MaterializationRequest { request_id: format!("matreq_{}", Uuid::new_v4().simple()), target_kind: RuleBindingTargetKind::AbilityDefinition, target_id: None, target_label: infer_ability_label(input.user_input), target_description: input.user_input.into(), evidence_span: input.user_input.into(), requested_fields: vec!["activation".into(),"cost".into(),"target".into(),"effect".into(),"trigger".into()], urgency: RuntimeUrgency::BeforeResolution, visibility: Visibility::GmOnly, metadata: json!({"source":"ability_service_default"}) });
        let ability_def_id = make_ability_def_id(input.ruleset_id, &req.target_label);
        let binding = SemanticRuleBindingService::from_env(self.db.clone(), self.search.clone()).bind_request(input.session_id, Some(input.turn_id), &ability_def_id, input.ruleset_id, &req, Some(input.world_tick)).await.ok();
        let definition = self.ensure_ability_definition(input.ruleset_id, &ability_def_id, &req, binding.as_ref(), input.world_tick).await?;
        let instance = self.ensure_ability_instance(input.session_id, actor_id, &definition, input.world_tick).await?;
        let activation = self.create_activation(input, actor_id, &definition, &instance, binding.as_ref()).await?;
        let check = self.build_core_mechanic_check(input, actor_id, &definition, binding.as_ref()).await?;
        let mut result = AbilityTurnResult::handled();
        result.semantic = Some(semantic);
        result.ability_definition = Some(definition.clone());
        result.ability_instance = Some(instance.clone());
        result.activation = Some(activation.clone());
        result.rule_binding = binding;
        result.phases = vec!["ability_kernel".into(), "semantic_classification".into(), "ability_definition_hydrated".into(), "ability_activation_contract_created".into()];
        if check.is_some() { result.phases.push("ability_core_mechanic_check_created".into()); }
        result.check = check;
        result.narration_context = Some(format!("Ability activation was materialized semantically. ability_def_id={} ability_id={} activation_id={}. Use the activation contract and source-backed rule binding; do not invent unbound ability effects.", definition.ability_def_id, instance.ability_id, activation.activation_id));
        Ok(result)
    }

    pub async fn ensure_ability_definition(&self, ruleset_id: &str, ability_def_id: &str, req: &MaterializationRequest, binding: Option<&RuleBindingPacket>, _world_tick: i64) -> Result<AbilityDefinition> {
        if let Some(existing) = self.load_ability_definition(ability_def_id).await? { return Ok(existing); }
        let extracted = binding.map(|b| b.extracted_json.clone()).unwrap_or_else(|| json!({"extraction_status":"not_bound_yet"}));
        let kind = infer_ability_kind(ruleset_id, &req.target_label, &extracted);
        let binding_status = binding.map(|b| b.verification_status).unwrap_or(BindingStatus::HydrationRequested);
        let def = AbilityDefinition { ability_def_id: ability_def_id.into(), ruleset_id: ruleset_id.into(), name: req.target_label.clone(), ability_kind: kind, tags: vec!["runtime_hydrated".into(), kind.as_str().into()], source_kind: AbilitySourceKind::RulebookEntry, source_refs: binding.map(|b| b.source_refs.clone()).unwrap_or_default(), activation: AbilityActivationModel { activation_kind: if matches!(kind, AbilityKind::Reaction) { "reaction".into() } else { "manual".into() }, action_cost: extracted.get("cost").and_then(Value::as_str).map(str::to_string), timing: extracted.get("timing").and_then(Value::as_str).map(str::to_string), notes: extracted.get("activation").and_then(Value::as_str).map(str::to_string) }, cost: AbilityCostModel { resource: extracted.get("resource").and_then(Value::as_str).map(str::to_string), amount_json: extracted.get("cost_amount").cloned().unwrap_or_else(|| json!(null)), consumes_use: extracted.get("consumes_use").and_then(Value::as_bool).unwrap_or(false), notes: extracted.get("cost").and_then(Value::as_str).map(str::to_string) }, target: AbilityTargetModel { target_kind: extracted.get("target_kind").and_then(Value::as_str).unwrap_or("contextual").into(), range: extracted.get("range").and_then(Value::as_str).map(str::to_string), area: extracted.get("area").and_then(Value::as_str).map(str::to_string), notes: extracted.get("target").and_then(Value::as_str).map(str::to_string) }, effect: AbilityEffectModel { effect_kind: extracted.get("effect_kind").and_then(Value::as_str).unwrap_or("rules_bound_effect").into(), summary: extracted.get("summary").or_else(|| extracted.get("effect")).and_then(Value::as_str).unwrap_or("Ability effect requires source-backed resolution before applying patches.").into(), creates_check: extracted.get("creates_check").and_then(Value::as_bool).unwrap_or(false), creates_effect: true, duration: extracted.get("duration").and_then(Value::as_str).map(str::to_string), raw_json: extracted.clone() }, rule_bindings: binding.map(|b| vec![AbilityRuleBinding { binding_id: b.binding_id.clone(), trigger: AbilityTriggerKind::ManualActivation, rule_packet_id: None, lookup_recipe_id: Some("semantic_rule_binding_v1_9".into()), source_refs: b.source_refs.clone(), effect_json: b.extracted_json.clone(), confidence: b.confidence }]).unwrap_or_default(), visibility_default: req.visibility, mechanical_profile: json!({"semantic_request":req,"binding":binding,"extracted":extracted}), binding_status };
        self.upsert_ability_definition(&def).await?;
        Ok(def)
    }

    pub async fn load_ability_definition(&self, ability_def_id: &str) -> Result<Option<AbilityDefinition>> {
        let row = sqlx::query("select definition_json from ability_definitions where ability_def_id=$1 limit 1").bind(ability_def_id).fetch_optional(&self.db.pool).await?;
        Ok(row.and_then(|r| serde_json::from_value::<AbilityDefinition>(r.get("definition_json")).ok()))
    }
    pub async fn upsert_ability_definition(&self, def: &AbilityDefinition) -> Result<()> {
        sqlx::query(r#"
            insert into ability_definitions (id, ability_def_id, ruleset_id, name, ability_kind, source_kind, tags, definition_json, binding_status, visibility)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
            on conflict (ability_def_id) do update set definition_json=excluded.definition_json, binding_status=excluded.binding_status, updated_at=now()
        "#).bind(Uuid::new_v4()).bind(&def.ability_def_id).bind(&def.ruleset_id).bind(&def.name).bind(def.ability_kind.as_str()).bind(def.source_kind.as_str()).bind(&def.tags).bind(serde_json::to_value(def)?).bind(def.binding_status.as_str()).bind(def.visibility_default.as_str()).execute(&self.db.pool).await?;
        for rb in &def.rule_bindings {
            let trig = AbilityTriggerBinding { trigger_id: format!("trigger_{}", Uuid::new_v4().simple()), ability_def_id: def.ability_def_id.clone(), trigger_kind: rb.trigger, condition_json: json!({"source":"ability_rule_binding","binding_id":rb.binding_id}), required: false, player_choice_required: matches!(rb.trigger, AbilityTriggerKind::OnDefense | AbilityTriggerKind::OnTakeDamage), gm_secret_allowed: true, creates_gate: matches!(rb.trigger, AbilityTriggerKind::OnDefense | AbilityTriggerKind::OnTakeDamage), creates_check: false, creates_effect: true, source_refs: rb.source_refs.clone() };
            self.upsert_trigger_binding(&trig).await.ok();
        }
        Ok(())
    }
    pub async fn upsert_trigger_binding(&self, trig: &AbilityTriggerBinding) -> Result<()> {
        sqlx::query(r#"insert into ability_trigger_bindings (id, trigger_id, ability_def_id, trigger_kind, binding_json) values ($1,$2,$3,$4,$5) on conflict (trigger_id) do nothing"#)
            .bind(Uuid::new_v4()).bind(&trig.trigger_id).bind(&trig.ability_def_id).bind(trig.trigger_kind.as_str()).bind(serde_json::to_value(trig)?).execute(&self.db.pool).await?;
        Ok(())
    }

    pub async fn ensure_ability_instance(&self, session_id: &str, actor_id: &str, def: &AbilityDefinition, world_tick: i64) -> Result<AbilityInstance> {
        let ability_id = format!("ability.{}.{}.{}", safe_id(session_id), safe_id(actor_id), safe_id(&def.name));
        let instance = AbilityInstance { ability_id, ability_def_id: def.ability_def_id.clone(), session_id: session_id.into(), owner_actor_id: Some(actor_id.into()), granted_by_object_id: None, granted_by_status_id: None, known_state: AbilityKnownState::KnownByActor, prepared_state: AbilityPreparedState::AlwaysAvailable, uses_state: AbilityUsesState::default(), cooldown_state: AbilityCooldownState::default(), visibility_state: AbilityVisibilityState { visibility: Visibility::GmOnly, known_state: AbilityKnownState::KnownByActor, reveal_conditions: vec![] }, runtime_state: json!({"hydrated_from":"semantic_ability_kernel_v1_9"}), created_at_tick: Some(world_tick), updated_at_tick: Some(world_tick) };
        sqlx::query(r#"
            insert into ability_instances (id, ability_id, ability_def_id, session_id, owner_actor_id, granted_by_object_id, granted_by_status_id, known_state, prepared_state, instance_json, visibility, created_at_tick, updated_at_tick)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
            on conflict (session_id, ability_def_id, owner_actor_id) do update set instance_json=excluded.instance_json, updated_at_tick=excluded.updated_at_tick, updated_at=now()
        "#).bind(Uuid::new_v4()).bind(&instance.ability_id).bind(&instance.ability_def_id).bind(&instance.session_id).bind(&instance.owner_actor_id).bind(&instance.granted_by_object_id).bind(&instance.granted_by_status_id).bind(instance.known_state.as_str()).bind(instance.prepared_state.as_str()).bind(serde_json::to_value(&instance)?).bind(instance.visibility_state.visibility.as_str()).bind(instance.created_at_tick).bind(instance.updated_at_tick).execute(&self.db.pool).await?;
        Ok(instance)
    }

    pub async fn create_activation(&self, input: AbilityTurnInput<'_>, actor_id: &str, def: &AbilityDefinition, instance: &AbilityInstance, binding: Option<&RuleBindingPacket>) -> Result<AbilityActivationContract> {
        let activation = AbilityActivationContract { activation_id: format!("ability_activation_{}", Uuid::new_v4().simple()), session_id: input.session_id.into(), turn_id: input.turn_id.into(), frame_id: input.frame_id.map(str::to_string), actor_id: actor_id.into(), ability_id: instance.ability_id.clone(), ability_def_id: def.ability_def_id.clone(), target_actor_ids: vec![], target_object_ids: vec![], target_zone_ids: vec![], activation_kind: if def.ability_kind == AbilityKind::Reaction { AbilityActivationKind::Reaction } else { AbilityActivationKind::Manual }, check_contracts: vec![], effect_contracts: vec![], object_patches: vec![], actor_patches: vec![], cost_patches: vec![], source_refs: def.source_refs.clone(), rule_binding_ids: binding.map(|b| vec![b.binding_id.clone()]).unwrap_or_default(), status: AbilityActivationStatus::Created, created_at_tick: Some(input.world_tick) };
        sqlx::query(r#"
            insert into ability_activation_contracts (id, activation_id, session_id, turn_id, frame_id, actor_id, ability_id, ability_def_id, activation_kind, status, contract_json, world_tick)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
            on conflict (activation_id) do nothing
        "#).bind(Uuid::new_v4()).bind(&activation.activation_id).bind(&activation.session_id).bind(&activation.turn_id).bind(&activation.frame_id).bind(&activation.actor_id).bind(&activation.ability_id).bind(&activation.ability_def_id).bind(activation.activation_kind.as_str()).bind(activation.status.as_str()).bind(serde_json::to_value(&activation)?).bind(activation.created_at_tick).execute(&self.db.pool).await?;
        Ok(activation)
    }

    /// Build a source-backed check from the ruleset's CORE mechanic (rule
    /// kernel dice + success rule). Returns None when there is no kernel, no
    /// dice, or no citable source — so the turn safely stays narration-only
    /// (no regression). The kernel's source_refs are copied onto the check so
    /// it passes the source-backed materialization gate honestly.
    pub async fn build_core_mechanic_check(&self, input: AbilityTurnInput<'_>, actor_id: &str, def: &AbilityDefinition, binding: Option<&RuleBindingPacket>) -> Result<Option<CheckContract>> {
        let kernel = match self.db.load_rule_kernel(input.ruleset_id).await? { Some(k) => k, None => return Ok(None) };
        let dice = kernel.dice_core.get("dice").and_then(Value::as_str)
            .or_else(|| kernel.check_model.get("dice").and_then(Value::as_str))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let dice = match dice { Some(d) => d, None => return Ok(None) };
        let mut source_refs = kernel.source_refs.clone();
        if let Some(b) = binding { merge_source_refs(&mut source_refs, &b.source_refs); }
        merge_source_refs(&mut source_refs, &def.source_refs);
        if source_refs.is_empty() { return Ok(None); }
        let target = core_mechanic_target_model(&kernel);
        Ok(Some(CheckContract {
            check_id: format!("check_{}", Uuid::new_v4().simple()),
            session_id: input.session_id.into(),
            turn_id: input.turn_id.into(),
            ruleset_id: input.ruleset_id.into(),
            module_id: input.module_id.map(str::to_string),
            initiator: ActorRef { actor_id: actor_id.into(), actor_kind: ActorKind::PlayerCharacter, display_name: Some("current actor".into()) },
            target_actor: None,
            opposition: OppositionModel::NoMechanicalOpposition,
            action_summary: input.user_input.chars().take(500).collect(),
            intent_kind: format!("ability:{}", def.ability_kind.as_str()),
            check_label: format!("{} (core mechanic)", def.name),
            dice_expression: dice,
            modifiers: vec![],
            target,
            // The ability/skill being used IS the tested parameter. The resolver
            // canonicalizes this against the kernel's resource_tracks + the
            // actor's stat/skill keys to read the real value (no global default).
            tested_parameter: Some(TestedParameter { domain: None, key: def.name.clone(), label: def.name.clone() }),
            opponent_tested_parameter: None,
            actor_snapshot_ids: input.frame_id.map(|id| vec![id.to_string()]).unwrap_or_default(),
            source_refs,
            learned_packet_ids: binding.map(|b| vec![b.binding_id.clone()]).unwrap_or_default(),
            roll_visibility: RollVisibility::PublicGmRoll,
            roll_authority: RollAuthority::System,
            disclosure: RollDisclosurePolicy::for_visibility(RollVisibility::PublicGmRoll),
            stakes: CheckStakes {
                before_roll_public: format!("You attempt to use {}. The core mechanic resolves the attempt.", def.name),
                success_public: "The attempt succeeds.".into(),
                failure_public: "The attempt fails or backfires.".into(),
                critical_public: None,
                fumble_public: None,
                success_patches_allowed: vec![],
                failure_patches_allowed: vec![],
                irreversible: false,
            },
            confidence: RulingConfidence::Medium,
            ruling_status: RulingStatus::SourceBacked,
            advice_refs: vec!["ability.core_mechanic.kernel".into()],
            expires_at_turn: Some(input.turn_id.into()),
        }))
    }

    pub async fn ability_context_block(&self, session_id: &str, world_tick: i64) -> Result<ContextBlock> {
        let rows = sqlx::query(r#"select instance_json from ability_instances where session_id=$1 order by updated_at desc limit 20"#).bind(session_id).fetch_all(&self.db.pool).await?;
        let abilities = rows.into_iter().filter_map(|r| serde_json::from_value::<AbilityInstance>(r.get("instance_json")).ok()).collect::<Vec<_>>();
        let mut block = ContextBlock::new(format!("runtime.abilities.{}", session_id), BlockKind::AbilityGraph, "Runtime Ability / Trigger Graph", BlockContent::Json(json!({"world_tick":world_tick,"abilities":abilities,"cache_policy":"BP3 dynamic ability instances, uses, cooldowns, and triggers; stable names/locators remain BP2"})), Visibility::GmOnly, Stability::TurnDynamic, CacheZone::DynamicTail, Scope { scope_type: ScopeType::Session, scope_id: session_id.into() }, 190);
        block.tags = vec!["ability".into(),"rule_binding".into(),"bp3".into()];
        Ok(block)
    }
}

fn infer_ability_label(input: &str) -> String { let t = input.trim(); let lower = t.to_lowercase(); if lower.contains("shield") { "Shield".into() } else if lower.contains("fireball") { "Fireball".into() } else if t.contains("护盾") { "护盾".into() } else if t.contains("火球") { "火球".into() } else if t.contains("战斗特技") { "战斗特技".into() } else if lower.contains("role ability") { "Role Ability".into() } else if lower.contains("program") { "Netrunning Program".into() } else { t.chars().take(48).collect() } }
fn make_ability_def_id(ruleset_id: &str, label: &str) -> String { format!("{}.ability.{}", safe_id(ruleset_id), safe_id(label)) }

/// Classify an ability by its LABEL (content keywords), not by ruleset — so no
/// per-game branching. Resolution doesn't depend on this; it's metadata only.
pub fn ability_kind_from_label(label: &str) -> AbilityKind {
    let l = label.to_lowercase();
    if l.contains("spell") || l.contains("fireball") || l.contains("shield") || label.contains("法术") || label.contains("魔法") { AbilityKind::Spell }
    else if l.contains("program") || l.contains("netrun") || l.contains("daemon") { AbilityKind::Program }
    else if l.contains("requisition") || label.contains("申请") || label.contains("异常") { AbilityKind::Requisition }
    else if l.contains("technique") || l.contains("stunt") { AbilityKind::Technique }
    else if l.contains("skill") { AbilityKind::SkillUse }
    else { AbilityKind::Unknown }
}
fn infer_ability_kind(_ruleset_id: &str, label: &str, _extracted: &Value) -> AbilityKind { ability_kind_from_label(label) }
fn safe_id(s: &str) -> String { s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect::<String>().trim_matches('_').to_string() }
fn env_bool(key: &str, default: bool) -> bool { std::env::var(key).ok().map(|v| matches!(v.to_ascii_lowercase().as_str(), "1"|"true"|"yes"|"on")).unwrap_or(default) }

/// Append source refs from `src` into `dst`, deduped by (source_id, anchor_id).
fn merge_source_refs(dst: &mut Vec<SourceRef>, src: &[SourceRef]) {
    for r in src {
        if !dst.iter().any(|e| e.source_id == r.source_id && e.anchor_id == r.anchor_id) {
            dst.push(r.clone());
        }
    }
}

/// Map a rule kernel's core dice mechanic into a CheckTargetModel. Pool-count
/// games (e.g. Triangle Agency 6d4 count 3s) resolve by counting target faces;
/// everything else falls through to the contest kernel's normal inference.
fn core_mechanic_target_model(kernel: &RuleKernel) -> CheckTargetModel {
    // Shared, data-driven mapping (no per-game defaults). If the parse didn't
    // supply enough, stay UnknownUntilLookup so the contest kernel resolves it.
    target_model_from_dice_core(&kernel.dice_core).unwrap_or(CheckTargetModel::UnknownUntilLookup)
}
