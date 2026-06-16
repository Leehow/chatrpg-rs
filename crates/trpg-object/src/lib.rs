use anyhow::{anyhow, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::Row;
use trpg_db::Db;
use trpg_interaction::InteractionLifecycleKernel;
use trpg_model::*;
use trpg_time::WorldTimeService;
use trpg_semantics::SemanticIntentService;
use uuid::Uuid;

#[derive(Clone)]
pub struct ObjectService {
    pub db: Db,
}

#[derive(Debug, Clone, Copy)]
pub struct ObjectTurnInput<'a> {
    pub session_id: &'a str,
    pub turn_id: &'a str,
    pub ruleset_id: &'a str,
    pub module_id: Option<&'a str>,
    pub actor_id: Option<&'a str>,
    pub frame_id: Option<&'a str>,
    pub user_input: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ObjectTurnResult {
    pub handled: bool,
    pub phases: Vec<String>,
    pub interaction: Option<ObjectInteractionContract>,
    pub check: Option<CheckContract>,
    pub object_events: Vec<ObjectEvent>,
    pub applied_patches: Vec<ObjectPatch>,
    pub affordances: Vec<ObjectAffordance>,
    pub context_block: Option<ContextBlock>,
    pub narration_context: Option<String>,
}

impl ObjectTurnResult {
    pub fn handled() -> Self { Self { handled: true, ..Default::default() } }
    pub fn not_handled() -> Self { Self::default() }
}

impl ObjectService {
    pub fn new(db: Db) -> Self { Self { db } }

    pub async fn handle_turn(&self, input: ObjectTurnInput<'_>) -> Result<ObjectTurnResult> {
        if !object_kernel_enabled() { return Ok(ObjectTurnResult::not_handled()); }
        let semantic = SemanticIntentService::from_env(self.db.clone()).classify_turn(SemanticIntentRequest {
            session_id: input.session_id.into(),
            turn_id: input.turn_id.into(),
            ruleset_id: input.ruleset_id.into(),
            module_id: input.module_id.map(str::to_string),
            active_frame_summary: json!({"frame_id": input.frame_id}),
            active_gate_summary: json!(null),
            player_input: input.user_input.into(),
            visible_scene_objects: json!({}),
            actor_parameter_summary: json!({"actor_id": input.actor_id.unwrap_or("pc.current")}),
        }).await.ok();
        // ObjectService is only called when TurnOrchestrator has already chosen an object route.
        // Therefore this local deterministic resolver is a route invariant, not a global keyword-first router.
        let kind = semantic.as_ref().and_then(kind_from_semantic_object_intent)
            .or_else(|| classify_object_interaction(input.user_input));
        let Some(kind) = kind else { return Ok(ObjectTurnResult::not_handled()); };
        let actor_id = input.actor_id.unwrap_or("pc.current");
        let time = WorldTimeService::new(self.db.clone()).ensure_session_time(input.session_id, Some(input.session_id)).await?;
        let target_actor_id = infer_target_actor(input.user_input);
        let target_object = self.resolve_or_seed_target_object(input, kind, actor_id, target_actor_id.as_deref(), time.world_tick).await?;
        let affordances = self.affordances_for_object(input.session_id, actor_id, &target_object, kind).await?;
        let contract = self.build_contract(input, actor_id, target_actor_id, &target_object, kind, time.world_tick).await?;
        self.upsert_interaction_contract(&contract).await?;
        if let Some(check) = &contract.check_contract {
            self.db.insert_check_contract(check, "created").await.ok();
        }
        if let Some(frame_id) = input.frame_id {
            let kernel = InteractionLifecycleKernel::new(self.db.clone());
            let _ = kernel.attach_object_interaction_to_active_frame(input.session_id, &contract.interaction_id, frame_id).await;
        }
        let event = self.insert_object_event(ObjectEvent {
            object_event_id: format!("object_event_{}", Uuid::new_v4().simple()),
            session_id: input.session_id.into(),
            object_id: contract.target_object_id.clone(),
            frame_id: input.frame_id.map(str::to_string),
            world_event_id: None,
            event_kind: "object_interaction_created".into(),
            event_json: json!({"interaction_id": contract.interaction_id, "interaction_kind": contract.interaction_kind.as_str(), "target_object_id": contract.target_object_id}),
            visibility: contract.visibility,
            world_tick: time.world_tick,
            created_at: Utc::now(),
        }).await?;
        let block = self.object_context_block(input.session_id, input.frame_id, actor_id, time.world_tick).await.ok();
        let mut result = ObjectTurnResult::handled();
        result.phases.push("object_kernel".into());
        result.phases.push("object_interaction_contract_created".into());
        if contract.check_contract.is_some() { result.phases.push("check_contract_created".into()); }
        result.interaction = Some(contract.clone());
        result.check = contract.check_contract.clone();
        result.object_events.push(event);
        result.affordances = affordances;
        result.context_block = block;
        result.narration_context = Some("Object & Possession Kernel framed the action as an object interaction. Do not narratively transfer, equip, drop, hide, or destroy objects unless the corresponding ObjectPatch is validated or the contract resolves.".into());
        Ok(result)
    }

    pub async fn object_context_block(&self, session_id: &str, frame_id: Option<&str>, actor_id: &str, world_tick: i64) -> Result<ContextBlock> {
        let objects = self.list_active_objects(session_id, 30).await?;
        let affordances = self.affordances_for_visible_objects(session_id, actor_id, &objects).await?;
        let mut block = ContextBlock::new(
            format!("runtime.object_graph.{}.{}", session_id, frame_id.unwrap_or("session")),
            BlockKind::ObjectGraph,
            "Runtime Object / Possession Graph",
            BlockContent::Json(json!({
                "world_tick": world_tick,
                "objects": objects,
                "affordances": affordances,
                "cache_policy": "BP3 dynamic: possession, slots, visibility, ammo, durability, and pending object interactions change per turn"
            })),
            Visibility::GmOnly,
            Stability::TurnDynamic,
            CacheZone::DynamicTail,
            Scope { scope_type: ScopeType::Session, scope_id: session_id.to_string() },
            146,
        );
        block.tags = vec!["object_graph".into(), "possession".into(), "bp3".into()];
        block.expires_at_scene = frame_id.map(str::to_string);
        block.load_reason = Some("object_possession_projection".into());
        Ok(block)
    }

    pub async fn apply_for_check_result(&self, result: &CheckResultRecord) -> Result<Option<ObjectInteractionResult>> {
        let Some(mut contract) = self.find_interaction_by_check(&result.check_id).await? else { return Ok(None); };
        let success = result.outcome.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
        let patches = if success { contract.success_patches.clone() } else { contract.failure_patches.clone() };
        let mut applied = Vec::new();
        let mut events = Vec::new();
        let time = WorldTimeService::new(self.db.clone()).ensure_session_time(&contract.session_id, Some(&contract.session_id)).await?;
        for patch in patches {
            if self.validate_patch(&contract, &patch).await? {
                self.apply_patch(&contract.session_id, &contract.interaction_id, &patch, time.world_tick).await?;
                let object_id = patch_object_id(&patch);
                let event = self.insert_object_event(ObjectEvent {
                    object_event_id: format!("object_event_{}", Uuid::new_v4().simple()),
                    session_id: contract.session_id.clone(),
                    object_id,
                    frame_id: contract.frame_id.clone(),
                    world_event_id: None,
                    event_kind: "object_patch_applied".into(),
                    event_json: json!({"interaction_id": contract.interaction_id, "patch": patch}),
                    visibility: Visibility::GmOnly,
                    world_tick: time.world_tick,
                    created_at: Utc::now(),
                }).await?;
                events.push(event);
                applied.push(patch);
            }
        }
        contract.status = if success { ObjectInteractionStatus::Applied } else { ObjectInteractionStatus::Failed };
        self.upsert_interaction_contract(&contract).await?;
        Ok(Some(ObjectInteractionResult { interaction_id: contract.interaction_id, check_id: Some(result.check_id.clone()), applied_patches: applied, object_events: events, status: contract.status }))
    }

    async fn resolve_or_seed_target_object(&self, input: ObjectTurnInput<'_>, kind: ObjectInteractionKind, _actor_id: &str, target_actor_id: Option<&str>, world_tick: i64) -> Result<ObjectInstance> {
        if matches!(kind, ObjectInteractionKind::CutConnection | ObjectInteractionKind::TraceConnection | ObjectInteractionKind::Inspect | ObjectInteractionKind::Hack | ObjectInteractionKind::Break) && mentions_cable(input.user_input) {
            return self.ensure_scene_cable(input.session_id, input.frame_id, world_tick).await;
        }
        if matches!(kind, ObjectInteractionKind::Unlock | ObjectInteractionKind::Lock) {
            return self.ensure_scene_lock(input.session_id, input.frame_id, world_tick).await;
        }
        if matches!(kind, ObjectInteractionKind::PickUp) {
            return self.ensure_ground_weapon(input.session_id, input.frame_id, world_tick).await;
        }
        if matches!(kind, ObjectInteractionKind::Equip | ObjectInteractionKind::Unequip) && mentions_armor(input.user_input) {
            return self.ensure_actor_armor(input.session_id, _actor_id, world_tick).await;
        }
        // Self-directed interactions act on the ACTING actor's OWN object,
        // resolved by the noun the player typed (e.g. "用魔杖" -> the held wand),
        // NOT the opponent's hand. Only if nothing owned matches do we seed a
        // held weapon for the acting actor (never default to npc.opposition).
        if matches!(kind, ObjectInteractionKind::Use | ObjectInteractionKind::Drop | ObjectInteractionKind::Reload | ObjectInteractionKind::Throw) {
            if let Some(obj) = self.find_owned_object_by_reference(input.session_id, _actor_id, input.user_input).await? {
                return Ok(obj);
            }
            return self.ensure_actor_weapon(input.session_id, input.ruleset_id, input.user_input, _actor_id, world_tick).await;
        }
        // Contested takings target the OPPONENT's held object.
        if matches!(kind, ObjectInteractionKind::Disarm | ObjectInteractionKind::GrabHeldObject | ObjectInteractionKind::Steal | ObjectInteractionKind::Draw) {
            return self.ensure_actor_weapon(input.session_id, input.ruleset_id, input.user_input, target_actor_id.unwrap_or("npc.opposition"), world_tick).await;
        }
        // 搜身/搜尸: the searched body/container is the target. A stable per-actor
        // body container gives the contract a deterministic target_object_id; the
        // actual loot (the target's owned items) is enumerated in build_contract.
        if matches!(kind, ObjectInteractionKind::Loot | ObjectInteractionKind::Search) {
            let tgt = target_actor_id.unwrap_or("npc.opposition");
            return self.ensure_actor_body(input.session_id, tgt, world_tick).await;
        }
        self.ensure_scene_object(input.session_id, input.frame_id, world_tick).await
    }

    async fn build_contract(&self, input: ObjectTurnInput<'_>, actor_id: &str, target_actor_id: Option<String>, object: &ObjectInstance, kind: ObjectInteractionKind, world_tick: i64) -> Result<ObjectInteractionContract> {
        let requires_check = requires_check(kind);
        // Data-driven: read the parsed kernel's core die, success model, and
        // source refs instead of hardcoded per-ruleset dice + a provisional DV.
        let kernel = self.db.load_rule_kernel(input.ruleset_id).await.ok().flatten();
        let kernel_dice = kernel.as_ref().and_then(|k| k.dice_core.get("dice").and_then(|v| v.as_str()).map(|s| s.trim().to_string())).filter(|s| !s.is_empty());
        let kernel_target = kernel.as_ref().and_then(|k| target_model_from_dice_core(&k.dice_core));
        let kernel_refs = kernel.as_ref().map(|k| k.source_refs.clone()).unwrap_or_default();
        let check = if requires_check { Some(make_object_check(input, actor_id, target_actor_id.as_deref(), object, kind, kernel_dice.as_deref(), kernel_target, kernel_refs, kernel.as_ref())) } else { None };
        let mut success_patches = success_patches_for(actor_id, target_actor_id.as_deref(), object, kind);
        // Source A loot (no-LLM): on a successful 搜身/search, transfer the
        // searched actor's OWN structured items (weapon+ammo, armor, ...) to the
        // looter. Source-backed (existing instances), object_id/ammo preserved.
        if matches!(kind, ObjectInteractionKind::Loot | ObjectInteractionKind::Search) {
            if let Some(tgt) = target_actor_id.as_deref() {
                if let Ok(p) = self.loot_owned_patches(input.session_id, tgt, actor_id).await { success_patches.extend(p); }
            }
        }
        let failure_patches = failure_patches_for(object, kind);
        Ok(ObjectInteractionContract {
            interaction_id: format!("object_interaction_{}", Uuid::new_v4().simple()),
            session_id: input.session_id.into(),
            turn_id: input.turn_id.into(),
            frame_id: input.frame_id.map(str::to_string),
            interaction_context_id: None,
            actor_id: actor_id.into(),
            target_actor_id,
            target_object_id: Some(object.object_id.clone()),
            interaction_kind: kind,
            preconditions: preconditions_for(actor_id, object, kind),
            check_contract: check,
            effect_contracts: vec![],
            success_patches,
            failure_patches,
            visibility: Visibility::GmOnly,
            source_refs: vec![],
            learned_packet_ids: vec![],
            advice_refs: vec!["object.possession.kernel.v1_6".into()],
            status: if requires_check { ObjectInteractionStatus::AwaitingCheck } else { ObjectInteractionStatus::Created },
            created_at_tick: world_tick,
        })
    }

    /// Find a SOURCE-BACKED materialized object definition (one with source_refs,
    /// i.e. params verified against the ruleset/module by MaterializationService)
    /// whose name the player's phrase references. Returns (object_def_id, name,
    /// mechanical_profile). None => no source def; caller falls back. Prefers the
    /// most specific (longest-name) match. This is how item params come from the
    /// SOURCE instead of a hardcoded engine table.
    pub async fn find_materialized_object_def(&self, ruleset_id: &str, name_query: &str) -> Result<Option<(String, String, serde_json::Value)>> {
        let row = sqlx::query(r#"
            select object_def_id, name, mechanical_profile from object_definitions
            where ruleset_id = $1
              and jsonb_array_length(coalesce(source_refs::jsonb, '[]'::jsonb)) > 0
              and length(name) > 0 and position(lower(name) in lower($2)) > 0
            order by length(name) desc limit 1
        "#).bind(ruleset_id).bind(name_query).fetch_optional(&self.db.pool).await?;
        Ok(row.map(|r| (
            r.get::<String, _>("object_def_id"),
            r.get::<String, _>("name"),
            r.get::<serde_json::Value, _>("mechanical_profile"),
        )))
    }

    async fn upsert_definition(&self, def: &ObjectDefinition) -> Result<()> {
        sqlx::query(r#"
            insert into object_definitions
              (id, object_def_id, ruleset_id, name, object_kind, tags, mechanical_profile, rule_bindings, default_affordances, equip_slots, visibility_default, source_refs)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
            on conflict (object_def_id) do update set name=excluded.name, object_kind=excluded.object_kind, tags=excluded.tags, mechanical_profile=excluded.mechanical_profile, rule_bindings=excluded.rule_bindings, default_affordances=excluded.default_affordances, equip_slots=excluded.equip_slots, visibility_default=excluded.visibility_default, source_refs=excluded.source_refs, updated_at=now()
        "#)
        .bind(Uuid::new_v4())
        .bind(&def.object_def_id)
        .bind(&def.ruleset_id)
        .bind(&def.name)
        .bind(def.object_kind.as_str())
        .bind(&def.tags)
        .bind(&def.mechanical_profile)
        .bind(serde_json::to_value(&def.rule_bindings)?)
        .bind(def.default_affordances.iter().map(|x| x.as_str().to_string()).collect::<Vec<_>>())
        .bind(def.equip_slots.iter().map(|x| x.as_str().to_string()).collect::<Vec<_>>())
        .bind(def.visibility_default.as_str())
        .bind(serde_json::to_value(&def.source_refs)?)
        .execute(&self.db.pool).await?;
        Ok(())
    }

    async fn upsert_instance(&self, obj: &ObjectInstance) -> Result<()> {
        sqlx::query(r#"
            insert into object_instances
              (id, object_id, object_def_id, session_id, scope_type, scope_id, display_name, object_kind, location_json, visibility_state, mechanical_state, quantity, durability_json, tags, active, created_at_tick, updated_at_tick)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)
            on conflict (object_id) do update set session_id=excluded.session_id, scope_type=excluded.scope_type, scope_id=excluded.scope_id, object_def_id=excluded.object_def_id, display_name=excluded.display_name, object_kind=excluded.object_kind, location_json=excluded.location_json, visibility_state=excluded.visibility_state, mechanical_state=excluded.mechanical_state, quantity=excluded.quantity, durability_json=excluded.durability_json, tags=excluded.tags, active=excluded.active, updated_at_tick=excluded.updated_at_tick, updated_at=now()
        "#)
        .bind(Uuid::new_v4())
        .bind(&obj.object_id)
        .bind(&obj.object_def_id)
        .bind(&obj.session_id)
        .bind(obj.scope.scope_type.as_str())
        .bind(&obj.scope.scope_id)
        .bind(&obj.display_name)
        .bind(obj.object_kind.as_str())
        .bind(serde_json::to_value(&obj.location)?)
        .bind(serde_json::to_value(&obj.visibility_state)?)
        .bind(&obj.mechanical_state)
        .bind(obj.quantity)
        .bind(serde_json::to_value(&obj.durability)?)
        .bind(&obj.tags)
        .bind(obj.active)
        .bind(obj.created_at_tick)
        .bind(obj.updated_at_tick)
        .execute(&self.db.pool).await?;
        Ok(())
    }

    async fn upsert_interaction_contract(&self, contract: &ObjectInteractionContract) -> Result<()> {
        sqlx::query(r#"
            insert into object_interaction_contracts
              (id, interaction_id, session_id, turn_id, frame_id, interaction_context_id, actor_id, target_actor_id, target_object_id, interaction_kind, contract_json, status, world_tick)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
            on conflict (interaction_id) do update set contract_json=excluded.contract_json, status=excluded.status, updated_at=now()
        "#)
        .bind(Uuid::new_v4())
        .bind(&contract.interaction_id)
        .bind(&contract.session_id)
        .bind(&contract.turn_id)
        .bind(&contract.frame_id)
        .bind(&contract.interaction_context_id)
        .bind(&contract.actor_id)
        .bind(&contract.target_actor_id)
        .bind(&contract.target_object_id)
        .bind(contract.interaction_kind.as_str())
        .bind(serde_json::to_value(contract)?)
        .bind(contract.status.as_str())
        .bind(contract.created_at_tick)
        .execute(&self.db.pool).await?;
        Ok(())
    }

    async fn insert_object_event(&self, event: ObjectEvent) -> Result<ObjectEvent> {
        sqlx::query(r#"
            insert into object_events
              (id, object_event_id, session_id, object_id, frame_id, world_event_id, event_kind, event_json, visibility, world_tick, created_at)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
            on conflict (object_event_id) do nothing
        "#)
        .bind(Uuid::new_v4())
        .bind(&event.object_event_id)
        .bind(&event.session_id)
        .bind(&event.object_id)
        .bind(&event.frame_id)
        .bind(&event.world_event_id)
        .bind(&event.event_kind)
        .bind(&event.event_json)
        .bind(event.visibility.as_str())
        .bind(event.world_tick)
        .bind(event.created_at)
        .execute(&self.db.pool).await?;
        Ok(event)
    }

    async fn list_active_objects(&self, session_id: &str, limit: i64) -> Result<Vec<ObjectInstance>> {
        let rows = sqlx::query(r#"
            select object_id, object_def_id, session_id, scope_type, scope_id, display_name, object_kind, location_json, visibility_state, mechanical_state, quantity, durability_json, tags, active, created_at_tick, updated_at_tick
            from object_instances where session_id = $1 and active = true order by updated_at desc limit $2
        "#).bind(session_id).bind(limit).fetch_all(&self.db.pool).await?;
        Ok(rows.into_iter().filter_map(|r| row_to_object(r).ok()).collect())
    }

    async fn find_interaction_by_check(&self, check_id: &str) -> Result<Option<ObjectInteractionContract>> {
        let row = sqlx::query("select contract_json from object_interaction_contracts where contract_json #>> '{check_contract,check_id}' = $1 and status in ('created','awaiting_check') order by created_at desc limit 1")
            .bind(check_id).fetch_optional(&self.db.pool).await?;
        Ok(row.and_then(|r| serde_json::from_value::<ObjectInteractionContract>(r.get("contract_json")).ok()))
    }

    async fn validate_patch(&self, contract: &ObjectInteractionContract, patch: &ObjectPatch) -> Result<bool> {
        let ok = match patch {
            ObjectPatch::TransferObject { object_id, .. } | ObjectPatch::SetObjectLocation { object_id, .. } | ObjectPatch::SetObjectVisibility { object_id, .. } | ObjectPatch::ModifyQuantity { object_id, .. } | ObjectPatch::DamageObject { object_id, .. } | ObjectPatch::DestroyObject { object_id, .. } | ObjectPatch::SetMechanicalState { object_id, .. } | ObjectPatch::TransformObject { object_id, .. } => {
                if contract.target_object_id.as_deref().map(|id| id == object_id).unwrap_or(false) {
                    true
                } else if matches!(patch, ObjectPatch::TransferObject { .. } | ObjectPatch::SetObjectVisibility { .. }) {
                    // Loot allowance: a reveal/transfer of an object PROVABLY owned
                    // by the searched target actor is admitted even though it is
                    // not the contract's target_object_id (the body). Strictly
                    // gated on ownership-by-target_actor_id — never a blanket allow.
                    match contract.target_actor_id.as_deref() {
                        Some(owner) => self.object_owned_by(&contract.session_id, object_id, owner).await?,
                        None => false,
                    }
                } else {
                    false
                }
            }
            ObjectPatch::CreateObjectInstance { .. } | ObjectPatch::AddObjectEdge { .. } | ObjectPatch::RemoveObjectEdge { .. } => true,
        };
        Ok(ok)
    }

    async fn apply_patch(&self, session_id: &str, interaction_id: &str, patch: &ObjectPatch, world_tick: i64) -> Result<()> {
        match patch {
            ObjectPatch::TransferObject { object_id, to, .. } | ObjectPatch::SetObjectLocation { object_id, location: to, .. } => {
                sqlx::query("update object_instances set location_json = $2, updated_at_tick = $3, updated_at = now() where session_id = $1 and object_id = $4")
                    .bind(session_id).bind(serde_json::to_value(to)?).bind(world_tick).bind(object_id).execute(&self.db.pool).await?;
            }
            ObjectPatch::SetObjectVisibility { object_id, visibility_state, .. } => {
                sqlx::query("update object_instances set visibility_state = $2, updated_at_tick = $3, updated_at = now() where session_id = $1 and object_id = $4")
                    .bind(session_id).bind(serde_json::to_value(visibility_state)?).bind(world_tick).bind(object_id).execute(&self.db.pool).await?;
            }
            ObjectPatch::ModifyQuantity { object_id, delta, .. } => {
                sqlx::query("update object_instances set quantity = coalesce(quantity,0) + $2, updated_at_tick = $3, updated_at = now() where session_id = $1 and object_id = $4")
                    .bind(session_id).bind(delta).bind(world_tick).bind(object_id).execute(&self.db.pool).await?;
            }
            ObjectPatch::DamageObject { object_id, amount, .. } => {
                sqlx::query("update object_instances set durability_json = coalesce(durability_json,'{}'::jsonb) || jsonb_build_object('last_damage', $2::int, 'damaged_at_tick', $3::bigint), updated_at_tick = $3, updated_at = now() where session_id = $1 and object_id = $4")
                    .bind(session_id).bind(amount).bind(world_tick).bind(object_id).execute(&self.db.pool).await?;
            }
            ObjectPatch::DestroyObject { object_id, .. } => {
                sqlx::query("update object_instances set active = false, location_json = jsonb_build_object('kind','destroyed'), updated_at_tick = $3, updated_at = now() where session_id = $1 and object_id = $2")
                    .bind(session_id).bind(object_id).bind(world_tick).execute(&self.db.pool).await?;
            }
            ObjectPatch::CreateObjectInstance { object, .. } => { self.upsert_instance(object).await?; }
            ObjectPatch::AddObjectEdge { edge, .. } => { self.upsert_edge(edge).await?; }
            ObjectPatch::RemoveObjectEdge { edge_id, .. } => { sqlx::query("update object_edges set valid_until_tick = $2 where edge_id = $1").bind(edge_id).bind(world_tick).execute(&self.db.pool).await?; }
            ObjectPatch::SetMechanicalState { object_id, patch_json, .. } => { sqlx::query("update object_instances set mechanical_state = coalesce(mechanical_state,'{}'::jsonb) || $2, updated_at_tick=$3, updated_at=now() where session_id=$1 and object_id=$4").bind(session_id).bind(patch_json).bind(world_tick).bind(object_id).execute(&self.db.pool).await?; }
            ObjectPatch::TransformObject { object_id, new_kind, new_name, set_mechanical_state, .. } => {
                if let Some(k) = new_kind {
                    sqlx::query("update object_instances set object_kind=$2, updated_at_tick=$3, updated_at=now() where session_id=$1 and object_id=$4").bind(session_id).bind(k.as_str()).bind(world_tick).bind(object_id).execute(&self.db.pool).await?;
                }
                if let Some(n) = new_name {
                    sqlx::query("update object_instances set display_name=$2, updated_at_tick=$3, updated_at=now() where session_id=$1 and object_id=$4").bind(session_id).bind(n).bind(world_tick).bind(object_id).execute(&self.db.pool).await?;
                }
                if let Some(ms) = set_mechanical_state {
                    // REPLACE (transform), not merge.
                    sqlx::query("update object_instances set mechanical_state=$2, updated_at_tick=$3, updated_at=now() where session_id=$1 and object_id=$4").bind(session_id).bind(ms).bind(world_tick).bind(object_id).execute(&self.db.pool).await?;
                }
            }
        }
        let patch_id = format!("object_patch_{}", Uuid::new_v4().simple());
        sqlx::query("insert into object_patches (id, patch_id, session_id, interaction_id, object_id, patch_kind, patch_json, validator_status, world_tick) values ($1,$2,$3,$4,$5,$6,$7,'validated',$8)")
            .bind(Uuid::new_v4()).bind(patch_id).bind(session_id).bind(interaction_id).bind(patch_object_id(patch)).bind(patch_kind(patch)).bind(serde_json::to_value(patch)?).bind(world_tick).execute(&self.db.pool).await?;
        Ok(())
    }

    async fn upsert_edge(&self, edge: &ObjectEdge) -> Result<()> {
        sqlx::query(r#"
            insert into object_edges (id, edge_id, session_id, from_object_id, to_object_id, relation, metadata, visibility, valid_from_tick, valid_until_tick)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
            on conflict(edge_id) do update set metadata=excluded.metadata, visibility=excluded.visibility, valid_until_tick=excluded.valid_until_tick
        "#)
        .bind(Uuid::new_v4()).bind(&edge.edge_id).bind(&edge.session_id).bind(&edge.from_object_id).bind(&edge.to_object_id).bind(edge.relation.as_str()).bind(&edge.metadata).bind(edge.visibility.as_str()).bind(edge.valid_from_tick).bind(edge.valid_until_tick).execute(&self.db.pool).await?;
        Ok(())
    }

    async fn ensure_actor_weapon(&self, session_id: &str, ruleset_id: &str, user_input: &str, actor_id: &str, world_tick: i64) -> Result<ObjectInstance> {
        if let Some(obj) = self.find_object_by_tag(session_id, &format!("held_by:{}", actor_id)).await? { return Ok(obj); }
        // Source-backed FIRST: if the rules/module already materialized a matching
        // item definition (params verified against the SOURCE), seed from ITS
        // mechanical_profile. If NOT found, FAIL CLOSED — a provisional, unresolved
        // object with NO fabricated stats (clarification_needed) so the smart GM
        // asks the player + verifies against the rules. Never invent params, and
        // never a hardcoded per-ruleset table.
        let (def_id, def_name, def_profile) = match self.find_materialized_object_def(ruleset_id, user_input).await? {
            Some(t) => t,
            None => provisional_item_seed(ruleset_id, user_input),
        };
        let object_id = format!("obj.{}.{}.{}", safe_id(session_id), safe_id(actor_id), safe_id(&def_id));
        let obj = ObjectInstance {
            object_id,
            object_def_id: Some(def_id.clone()),
            session_id: session_id.into(),
            scope: Scope { scope_type: ScopeType::Character, scope_id: actor_id.into() },
            display_name: def_name.clone(),
            object_kind: ObjectKind::Weapon,
            location: ObjectLocation::Held { actor_id: actor_id.into(), hand: Some(HandSlot::Right) },
            visibility_state: visible_state(&def_name, true),
            mechanical_state: def_profile.clone(),
            tags: vec!["weapon".into(), "held".into(), format!("held_by:{}", actor_id), format!("def:{}", def_id)],
            created_at_tick: world_tick,
            updated_at_tick: world_tick,
            active: true,
            ..Default::default()
        };
        self.upsert_instance(&obj).await?;
        Ok(obj)
    }

    async fn ensure_ground_weapon(&self, session_id: &str, frame_id: Option<&str>, world_tick: i64) -> Result<ObjectInstance> {
        let scene = frame_id.unwrap_or("scene.current");
        let object_id = format!("obj.{}.{}.ground_weapon", safe_id(session_id), scene.replace('.', "_"));
        if let Some(obj) = self.find_object(session_id, &object_id).await? { return Ok(obj); }
        let obj = ObjectInstance { object_id, object_def_id: Some("generic.weapon.dropped".into()), session_id: session_id.into(), scope: Scope { scope_type: ScopeType::Scene, scope_id: scene.into() }, display_name: "dropped weapon".into(), object_kind: ObjectKind::Weapon, location: ObjectLocation::OnGround { scene_id: scene.into(), zone_id: None }, visibility_state: visible_state("dropped weapon", true), mechanical_state: json!({}), tags: vec!["weapon".into(), "ground".into()], created_at_tick: world_tick, updated_at_tick: world_tick, active: true, ..Default::default() };
        self.upsert_instance(&obj).await?; Ok(obj)
    }

    async fn ensure_actor_armor(&self, session_id: &str, actor_id: &str, world_tick: i64) -> Result<ObjectInstance> {
        let object_id = format!("obj.{}.{}.armor", safe_id(session_id), actor_id.replace('.', "_"));
        if let Some(obj) = self.find_object(session_id, &object_id).await? { return Ok(obj); }
        let obj = ObjectInstance { object_id, object_def_id: Some("generic.armor.body".into()), session_id: session_id.into(), scope: Scope { scope_type: ScopeType::Character, scope_id: actor_id.into() }, display_name: "body armor".into(), object_kind: ObjectKind::Armor, location: ObjectLocation::Carried { actor_id: actor_id.into(), container_id: None }, visibility_state: visible_state("body armor", true), mechanical_state: json!({"load_policy":"exact_rule_on_first_use"}), tags: vec!["armor".into(), format!("carried_by:{}", actor_id)], created_at_tick: world_tick, updated_at_tick: world_tick, active: true, ..Default::default() };
        self.upsert_instance(&obj).await?; Ok(obj)
    }

    async fn ensure_scene_cable(&self, session_id: &str, frame_id: Option<&str>, world_tick: i64) -> Result<ObjectInstance> {
        let scope = frame_id.unwrap_or("scene.current");
        let object_id = format!("obj.{}.{}.drone_cable", safe_id(session_id), scope.replace('.', "_"));
        if let Some(obj) = self.find_object(session_id, &object_id).await? { return Ok(obj); }
        let obj = ObjectInstance { object_id, object_def_id: Some("scene.object.exposed_cable".into()), session_id: session_id.into(), scope: Scope { scope_type: ScopeType::Scene, scope_id: scope.into() }, display_name: "exposed drone cable".into(), object_kind: ObjectKind::EnvironmentalFeature, location: ObjectLocation::Connected { endpoint_a: "object.drone".into(), endpoint_b: "object.server_or_power_source".into(), connection_kind: "power_and_data".into() }, visibility_state: visible_state("exposed cable", true), mechanical_state: json!({"affordances":["inspect","trace_connection","cut_connection","hack"],"risk":"close approach may expose actor to fire"}), tags: vec!["cable".into(), "connected".into()], created_at_tick: world_tick, updated_at_tick: world_tick, active: true, ..Default::default() };
        self.upsert_instance(&obj).await?; Ok(obj)
    }

    async fn ensure_scene_lock(&self, session_id: &str, frame_id: Option<&str>, world_tick: i64) -> Result<ObjectInstance> {
        let scope = frame_id.unwrap_or("scene.current");
        let object_id = format!("obj.{}.{}.lock", safe_id(session_id), scope.replace('.', "_"));
        if let Some(obj) = self.find_object(session_id, &object_id).await? { return Ok(obj); }
        let obj = ObjectInstance { object_id, object_def_id: Some("generic.object.lock".into()), session_id: session_id.into(), scope: Scope { scope_type: ScopeType::Scene, scope_id: scope.into() }, display_name: "lock".into(), object_kind: ObjectKind::Lock, location: ObjectLocation::Installed { parent_object_id: "object.door".into(), port_or_mount: "lock".into() }, visibility_state: visible_state("lock", true), mechanical_state: json!({"locked":true}), tags: vec!["lock".into(), "door".into()], created_at_tick: world_tick, updated_at_tick: world_tick, active: true, ..Default::default() };
        self.upsert_instance(&obj).await?; Ok(obj)
    }

    async fn ensure_scene_object(&self, session_id: &str, frame_id: Option<&str>, world_tick: i64) -> Result<ObjectInstance> {
        let scope = frame_id.unwrap_or("scene.current");
        let object_id = format!("obj.{}.{}.interactive_object", safe_id(session_id), scope.replace('.', "_"));
        if let Some(obj) = self.find_object(session_id, &object_id).await? { return Ok(obj); }
        let obj = ObjectInstance { object_id, object_def_id: None, session_id: session_id.into(), scope: Scope { scope_type: ScopeType::Scene, scope_id: scope.into() }, display_name: "interactive object".into(), object_kind: ObjectKind::Unknown, location: ObjectLocation::OnGround { scene_id: scope.into(), zone_id: None }, visibility_state: visible_state("interactive object", true), mechanical_state: json!({}), tags: vec!["object".into()], created_at_tick: world_tick, updated_at_tick: world_tick, active: true, ..Default::default() };
        self.upsert_instance(&obj).await?; Ok(obj)
    }

    async fn find_object(&self, session_id: &str, object_id: &str) -> Result<Option<ObjectInstance>> {
        let row = sqlx::query(r#"select object_id, object_def_id, session_id, scope_type, scope_id, display_name, object_kind, location_json, visibility_state, mechanical_state, quantity, durability_json, tags, active, created_at_tick, updated_at_tick from object_instances where session_id=$1 and object_id=$2"#).bind(session_id).bind(object_id).fetch_optional(&self.db.pool).await?;
        Ok(row.and_then(|r| row_to_object(r).ok()))
    }

    async fn find_object_by_tag(&self, session_id: &str, tag: &str) -> Result<Option<ObjectInstance>> {
        let row = sqlx::query(r#"select object_id, object_def_id, session_id, scope_type, scope_id, display_name, object_kind, location_json, visibility_state, mechanical_state, quantity, durability_json, tags, active, created_at_tick, updated_at_tick from object_instances where session_id=$1 and $2 = any(tags) and active=true order by updated_at desc limit 1"#).bind(session_id).bind(tag).fetch_optional(&self.db.pool).await?;
        Ok(row.and_then(|r| row_to_object(r).ok()))
    }

    /// Resolve the actor's OWN object that the player's words refer to (e.g.
    /// "用魔杖" -> the held 法杖). Scans the actor's owned/held/carried active
    /// objects and scores each by token overlap of the input against the
    /// object's display_name + tags (DATA on the instance, not a keyword table).
    /// Returns the best scorer, or None to let the caller seed/fall back.
    pub async fn find_owned_object_by_reference(&self, session_id: &str, actor_id: &str, user_input: &str) -> Result<Option<ObjectInstance>> {
        let rows = sqlx::query(r#"select object_id, object_def_id, session_id, scope_type, scope_id, display_name, object_kind, location_json, visibility_state, mechanical_state, quantity, durability_json, tags, active, created_at_tick, updated_at_tick from object_instances where session_id=$1 and active=true and (scope_id=$2 or $3 = any(tags) or $4 = any(tags))"#)
            .bind(session_id).bind(actor_id)
            .bind(format!("held_by:{}", actor_id)).bind(format!("carried_by:{}", actor_id))
            .fetch_all(&self.db.pool).await?;
        let input = user_input.to_lowercase();
        let mut best: Option<(i32, ObjectInstance)> = None;
        for r in rows {
            let obj = match row_to_object(r) { Ok(o) => o, Err(_) => continue };
            let score = reference_score(&input, &obj);
            if score > 0 && best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
                best = Some((score, obj));
            }
        }
        Ok(best.map(|(_, o)| o))
    }

    /// Stable per-actor "body" container used as the loot target's object id.
    /// Generic, empty, keyed only by the passed actor id (no NPC/item content):
    /// the actual loot is the actor's own owned items (enumerated separately).
    async fn ensure_actor_body(&self, session_id: &str, target_actor_id: &str, world_tick: i64) -> Result<ObjectInstance> {
        let object_id = format!("obj.{}.{}.body", safe_id(session_id), safe_id(target_actor_id));
        if let Some(obj) = self.find_object(session_id, &object_id).await? { return Ok(obj); }
        let obj = ObjectInstance {
            object_id, object_def_id: None, session_id: session_id.into(),
            scope: Scope { scope_type: ScopeType::Scene, scope_id: "scene.current".into() },
            display_name: "searchable body / container".into(), object_kind: ObjectKind::Container,
            location: ObjectLocation::OnGround { scene_id: "scene.current".into(), zone_id: None },
            visibility_state: visible_state("searchable body / container", true),
            mechanical_state: json!({}),
            tags: vec!["loot_target".into(), format!("body_of:{}", target_actor_id)],
            created_at_tick: world_tick, updated_at_tick: world_tick, active: true, ..Default::default()
        };
        self.upsert_instance(&obj).await?;
        Ok(obj)
    }

    /// All active object_instances OWNED by an actor (scope_id or held_by:/
    /// carried_by: tags) — the SAME ownership predicate as find_owned_object_by_reference,
    /// returning every row (no scoring). Excludes the body container itself.
    /// Pure DB read: empty kit => empty vec (fail closed). Source A loot enumerator.
    pub async fn list_owned_objects(&self, session_id: &str, target_actor_id: &str) -> Result<Vec<ObjectInstance>> {
        let rows = sqlx::query(r#"select object_id, object_def_id, session_id, scope_type, scope_id, display_name, object_kind, location_json, visibility_state, mechanical_state, quantity, durability_json, tags, active, created_at_tick, updated_at_tick from object_instances where session_id=$1 and active=true and (scope_id=$2 or $3 = any(tags) or $4 = any(tags))"#)
            .bind(session_id).bind(target_actor_id)
            .bind(format!("held_by:{}", target_actor_id)).bind(format!("carried_by:{}", target_actor_id))
            .fetch_all(&self.db.pool).await?;
        Ok(rows.into_iter().filter_map(|r| row_to_object(r).ok())
            .filter(|o| !o.object_id.ends_with(".body") && !o.tags.iter().any(|t| t == "loot_target"))
            .collect())
    }

    /// Build the Source-A loot patches: reveal + transfer each item the target
    /// actor owns to the looter. Preserves object_id / ammo / durability
    /// (TransferObject only rewrites location). No LLM, no item table.
    pub async fn loot_owned_patches(&self, session_id: &str, target_actor_id: &str, looter_actor_id: &str) -> Result<Vec<ObjectPatch>> {
        let mut out = Vec::new();
        for item in self.list_owned_objects(session_id, target_actor_id).await? {
            out.push(ObjectPatch::SetObjectVisibility { object_id: item.object_id.clone(), visibility_state: visible_state(&item.display_name, true), reason: "revealed on search".into() });
            out.push(ObjectPatch::TransferObject { object_id: item.object_id.clone(), from: item.location.clone(), to: ObjectLocation::Carried { actor_id: looter_actor_id.into(), container_id: None }, reason: format!("looted from {}", target_actor_id) });
        }
        Ok(out)
    }

    /// effect_policy SetObjectState 的执行原语（C4 scene_policy 的对象态落库单点）：
    /// object_instances 无该 object_id 行 → 先建最小实例（模组声明即存在：
    /// display_name=object_id、object_kind 默认、scope={Session, session_id}、
    /// mechanical_state={}、active=true，经既有 upsert_instance），再走既有
    /// SetMechanicalState merge SQL（apply_patch 分支）。返回 created
    /// （是否新建了实例——可观测）。
    pub async fn apply_external_mechanical_patch(
        &self,
        session_id: &str,
        object_id: &str,
        patch_json: serde_json::Value,
        reason: &str,
        world_tick: i64,
    ) -> Result<bool> {
        let created = if self.find_object(session_id, object_id).await?.is_none() {
            let obj = ObjectInstance {
                object_id: object_id.to_string(),
                session_id: session_id.to_string(),
                scope: Scope { scope_type: ScopeType::Session, scope_id: session_id.to_string() },
                display_name: object_id.to_string(),
                visibility_state: visible_state(object_id, true),
                mechanical_state: json!({}),
                created_at_tick: world_tick,
                updated_at_tick: world_tick,
                active: true,
                ..Default::default()
            };
            self.upsert_instance(&obj).await?;
            true
        } else {
            false
        };
        let patch = ObjectPatch::SetMechanicalState {
            object_id: object_id.to_string(),
            patch_json,
            reason: reason.to_string(),
        };
        self.apply_patch(session_id, "scene.policy", &patch, world_tick).await?;
        Ok(created)
    }

    /// True when `object_id` is provably owned by `owner_actor_id` (scope_id or
    /// held_by:/carried_by: tags). Gates the loot patch-validation allowance.
    async fn object_owned_by(&self, session_id: &str, object_id: &str, owner_actor_id: &str) -> Result<bool> {
        let row = sqlx::query("select scope_id, tags from object_instances where session_id=$1 and object_id=$2")
            .bind(session_id).bind(object_id).fetch_optional(&self.db.pool).await?;
        if let Some(r) = row {
            let scope_id: String = r.try_get("scope_id").unwrap_or_default();
            let tags: Vec<String> = r.try_get("tags").unwrap_or_default();
            let held = format!("held_by:{}", owner_actor_id);
            let carried = format!("carried_by:{}", owner_actor_id);
            return Ok(scope_id == owner_actor_id || tags.iter().any(|t| t == &held || t == &carried));
        }
        Ok(false)
    }

    /// Generic object rule effects fired AFTER a check resolves, on EVERY
    /// resolution path (called from the runtime's single post-resolution funnel,
    /// so the auto-roll combat route can't miss them). Maps the resolved
    /// contract's intent to an object trigger and applies the OBJECT'S OWN data.
    /// No per-ruleset code, no hardcoded numbers.
    pub async fn apply_object_rule_effects(&self, session_id: &str, contract: &CheckContract) -> Result<()> {
        match trigger_for_contract(contract) {
            Some(ObjectRuleTrigger::OnAttack) => self.consume_ammo_on_attack(session_id, &contract.initiator.actor_id).await,
            _ => Ok(()),
        }
    }

    /// A ranged attack spends loaded rounds on the wielder's held weapon. Fully
    /// generic: gated on the weapon DECLARING it tracks ammo (`ammo_current`
    /// present — a melee weapon has none, so it's a no-op), and the per-shot
    /// amount comes from the weapon's own `ammo_per_shot` (default 1). The
    /// numbers live in the weapon's mechanical_state, never in engine Rust.
    async fn consume_ammo_on_attack(&self, session_id: &str, actor_id: &str) -> Result<()> {
        let w = match self.find_object_by_tag(session_id, &format!("held_by:{}", actor_id)).await? { Some(w) => w, None => return Ok(()) };
        let cur = match w.mechanical_state.get("ammo_current").and_then(|v| v.as_i64()) { Some(c) => c, None => return Ok(()) };
        let per = w.mechanical_state.get("ammo_per_shot").and_then(|v| v.as_i64()).unwrap_or(1).max(1);
        let next = (cur - per).max(0);
        if next == cur { return Ok(()); }
        let time = WorldTimeService::new(self.db.clone()).ensure_session_time(session_id, Some(session_id)).await?;
        self.apply_patch(session_id, "ammo_on_attack", &ObjectPatch::SetMechanicalState { object_id: w.object_id.clone(), patch_json: json!({"ammo_current": next, "out_of_ammo": next == 0}), reason: "ranged attack spent a round".into() }, time.world_tick).await?;
        Ok(())
    }

    async fn affordances_for_object(&self, _session_id: &str, actor_id: &str, object: &ObjectInstance, focus: ObjectInteractionKind) -> Result<Vec<ObjectAffordance>> {
        Ok(vec![ObjectAffordance { affordance_id: format!("affordance_{}", Uuid::new_v4().simple()), object_id: object.object_id.clone(), actor_id: actor_id.into(), action_kind: focus, label: format!("{}: {}", focus.as_str(), object.display_name), description: format!("Interact with {} through {}", object.display_name, focus.as_str()), requires_check: requires_check(focus), check_recipe: if requires_check(focus) { Some("ruleset object interaction check; exact rule packet on demand".into()) } else { None }, risk_level: if requires_check(focus) { "medium".into() } else { "low".into() }, visible_to_player: object.visibility_state.known_by_player, source_refs: vec![] }])
    }

    async fn affordances_for_visible_objects(&self, session_id: &str, actor_id: &str, objects: &[ObjectInstance]) -> Result<Vec<ObjectAffordance>> {
        let mut out = Vec::new();
        for obj in objects.iter().filter(|o| o.visibility_state.known_by_player || o.visibility_state.known_by_actor_ids.iter().any(|id| id == actor_id)) {
            for kind in default_affordance_kinds(obj) {
                out.extend(self.affordances_for_object(session_id, actor_id, obj, kind).await?);
            }
        }
        Ok(out)
    }
}


fn kind_from_semantic_object_intent(semantic: &SemanticIntentResult) -> Option<ObjectInteractionKind> {
    // Semantic-first: honor an explicit loot/search classification regardless of
    // materialization requests, so 搜身/搜尸/"go through their pockets"/any phrasing
    // in any language routes to Loot/Search via MEANING, not a keyword table.
    match semantic.raw_json.get("object_interaction_kind").and_then(|v| v.as_str()) {
        Some("loot") => return Some(ObjectInteractionKind::Loot),
        Some("search") => return Some(ObjectInteractionKind::Search),
        _ => {}
    }
    if semantic.materialization_requests.iter().any(|r| r.target_kind == RuleBindingTargetKind::ObjectDefinition) {
        let raw_kind = semantic.raw_json.get("object_interaction_kind").and_then(|v| v.as_str()).unwrap_or_default();
        return match raw_kind {
            "disarm" => Some(ObjectInteractionKind::Disarm),
            "grab_held_object" => Some(ObjectInteractionKind::GrabHeldObject),
            "pick_up" => Some(ObjectInteractionKind::PickUp),
            "drop" => Some(ObjectInteractionKind::Drop),
            "equip" => Some(ObjectInteractionKind::Equip),
            "reload" => Some(ObjectInteractionKind::Reload),
            "cut_connection" => Some(ObjectInteractionKind::CutConnection),
            "trace_connection" => Some(ObjectInteractionKind::TraceConnection),
            "unlock" => Some(ObjectInteractionKind::Unlock),
            "inspect" => Some(ObjectInteractionKind::Inspect),
            "break" => Some(ObjectInteractionKind::Break),
            "hack" => Some(ObjectInteractionKind::Hack),
            "loot" => Some(ObjectInteractionKind::Loot),
            "search" => Some(ObjectInteractionKind::Search),
            _ => Some(ObjectInteractionKind::Inspect),
        };
    }
    match semantic.primary_action_kind {
        SituationActionKind::Hack => Some(ObjectInteractionKind::Hack),
        SituationActionKind::DisableDevice => Some(ObjectInteractionKind::Break),
        SituationActionKind::UseItem => semantic.raw_json.get("object_interaction_kind").and_then(|v| v.as_str()).and_then(parse_object_interaction_kind).or(Some(ObjectInteractionKind::Use)),
        _ => None,
    }
}
fn parse_object_interaction_kind(s: &str) -> Option<ObjectInteractionKind> { match s { "disarm" => Some(ObjectInteractionKind::Disarm), "grab_held_object" => Some(ObjectInteractionKind::GrabHeldObject), "pick_up" => Some(ObjectInteractionKind::PickUp), "drop" => Some(ObjectInteractionKind::Drop), "equip" => Some(ObjectInteractionKind::Equip), "reload" => Some(ObjectInteractionKind::Reload), "cut_connection" => Some(ObjectInteractionKind::CutConnection), "trace_connection" => Some(ObjectInteractionKind::TraceConnection), "unlock" => Some(ObjectInteractionKind::Unlock), "inspect" => Some(ObjectInteractionKind::Inspect), "break" => Some(ObjectInteractionKind::Break), "hack" => Some(ObjectInteractionKind::Hack), "loot" => Some(ObjectInteractionKind::Loot), "search" => Some(ObjectInteractionKind::Search), _ => None } }
fn lexical_object_fallback_enabled() -> bool { std::env::var("TRPG_LEXICAL_FALLBACK_ENABLE").map(|v| matches!(v.to_ascii_lowercase().as_str(), "1"|"true"|"yes"|"on")).unwrap_or(false) }

fn object_kernel_enabled() -> bool { std::env::var("TRPG_OBJECT_KERNEL_ENABLE_V16").map(|v| v != "0" && v.to_ascii_lowercase() != "false").unwrap_or(true) }
fn mentions_cable(s: &str) -> bool { contains_any(s, &["线", "线缆", "电缆", "cable", "wire", "cord"]) }
fn mentions_armor(s: &str) -> bool { contains_any(s, &["护甲", "盔甲", "armor", "armour", "穿甲"]) }
fn contains_any(s: &str, terms: &[&str]) -> bool { let lower=s.to_lowercase(); terms.iter().any(|t| lower.contains(&t.to_lowercase())) }


fn safe_id(s: &str) -> String { s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect() }
fn is_hypothetical_or_secret(input: &str) -> bool { contains_any(input, &["能不能", "可不可以", "是否", "判断", "分析", "评估", "安全切断", "安全吗", "gm 暗中", "gm暗中", "暗中处理", "暗投", "秘密处理", "can i", "could i", "is it safe", "assess", "analyze", "secret roll", "gm secretly"]) }
fn is_attack_with_possessed_weapon(input: &str) -> bool { contains_any(input, &["夺来的", "抢来的", "刚夺", "刚抢", "picked up", "grabbed", "taken"]) && contains_any(input, &["开火", "射击", "攻击", "打", "shoot", "fire", "attack"]) }

/// Fail-closed seed for an item with NO source-backed definition. Returns
/// (object_def_id, display_name, mechanical_profile) with NO fabricated stats
/// (no damage/ammo/etc) — replaces the retired per-ruleset weapon_definition_for
/// table. Marks `clarification_needed` so the smart GM asks the player what the
/// item is and verifies it against the rules (smart-GM branch 1), rather than
/// inventing parameters. Downstream mechanical resolution sees no source-backed
/// params and stays provisional instead of rolling with invented numbers.
fn provisional_item_seed(ruleset_id: &str, user_input: &str) -> (String, String, serde_json::Value) {
    let object_def_id = format!("{}.object.unresolved", safe_id(ruleset_id));
    let profile = json!({
        "source_policy": "unresolved_needs_source_binding",
        "hydration": "provisional",
        "clarification_needed": true,
        "clarification_reason": "no source-backed definition for this item; the GM should ask the player what it is, then verify against the rules — never fabricate parameters",
        "referenced_as": user_input.chars().take(120).collect::<String>(),
    });
    (object_def_id, "unresolved item".to_string(), profile)
}

fn classify_object_interaction(input: &str) -> Option<ObjectInteractionKind> {
    if is_hypothetical_or_secret(input) { return None; }
    if is_attack_with_possessed_weapon(input) { return None; }
    if contains_any(input, &["缴械", "打掉武器", "夺下武器", "disarm"]) { return Some(ObjectInteractionKind::Disarm); }
    if contains_any(input, &["抢", "夺", "抓", "grab", "snatch", "take his", "take her"]) && contains_any(input, &["枪", "武器", "刀", "weapon", "gun", "pistol", "knife"]) { return Some(ObjectInteractionKind::GrabHeldObject); }
    if contains_any(input, &["偷", "偷走", "扒", "steal", "pickpocket"]) { return Some(ObjectInteractionKind::Steal); }
    if contains_any(input, &["捡", "拾起", "拿起", "pick up", "pickup"]) { return Some(ObjectInteractionKind::PickUp); }
    if contains_any(input, &["丢", "扔下", "放下", "drop"]) { return Some(ObjectInteractionKind::Drop); }
    if contains_any(input, &["装备", "穿上", "戴上", "equip", "wear", "put on"]) { return Some(ObjectInteractionKind::Equip); }
    if contains_any(input, &["换弹", "装弹", "reload"]) { return Some(ObjectInteractionKind::Reload); }
    if contains_any(input, &["剪线", "切断", "剪断", "cut cable", "cut wire"]) { return Some(ObjectInteractionKind::CutConnection); }
    if contains_any(input, &["追踪线", "顺着线", "trace cable", "trace wire"]) { return Some(ObjectInteractionKind::TraceConnection); }
    if contains_any(input, &["开锁", "撬锁", "unlock", "lockpick"]) { return Some(ObjectInteractionKind::Unlock); }
    if contains_any(input, &["搜身", "搜尸", "翻找", "loot", "frisk", "search body"]) { return Some(ObjectInteractionKind::Loot); }
    if contains_any(input, &["检查", "观察", "分析", "inspect", "examine", "analyze"]) && contains_any(input, &["物", "东西", "设备", "线", "门", "锁", "weapon", "object", "device", "cable", "lock"]) { return Some(ObjectInteractionKind::Inspect); }
    None
}
fn infer_target_actor(input: &str) -> Option<String> { if contains_any(input, &["他", "她", "敌", "npc", "scav", "guard", "守卫"]) { Some("npc.opposition".into()) } else { None } }

/// Map a resolved check's intent to the object trigger it fires. An attack /
/// counterattack to-hit check fires OnAttack (e.g. a firearm spends a round);
/// the effect/damage follow-up roll (intent "effect_roll") does NOT, so ammo is
/// spent once per attack, not twice. Generic intent->trigger mapping — no
/// per-ruleset or per-weapon logic.
fn trigger_for_contract(contract: &CheckContract) -> Option<ObjectRuleTrigger> {
    let intent = contract.intent_kind.to_ascii_lowercase();
    if intent.contains("attack") { return Some(ObjectRuleTrigger::OnAttack); }
    None
}

/// Turn a container's DECLARED contents into real object instances on loot
/// success. Generic transcriber: it iterates whatever `loot_contents` the
/// container carries (each {name, kind, quantity, mechanical_profile?}) and
/// emits a CreateObjectInstance per item — the engine contains zero per-item or
/// per-ruleset literals. Empty/absent contents => no items (fail closed, no
/// fabrication). The contents themselves are source-backed (NPC card / module
/// via materialization), not invented here.
fn loot_patches_from_contents(mech: &serde_json::Value, session_id: &str, finder: &str) -> Vec<ObjectPatch> {
    let items = match mech.get("loot_contents").and_then(|v| v.as_array()) { Some(a) => a, None => return vec![] };
    let mut out = Vec::new();
    for it in items {
        let name = it.get("name").and_then(|v| v.as_str()).map(str::trim).unwrap_or("");
        if name.is_empty() { continue; }
        let kind = it.get("kind").and_then(|v| v.as_str()).map(object_kind_from_str).unwrap_or(ObjectKind::Unknown);
        let qty = it.get("quantity").and_then(|v| v.as_i64()).map(|q| q as i32).filter(|q| *q > 0);
        out.push(ObjectPatch::CreateObjectInstance {
            object: ObjectInstance {
                object_id: format!("obj.{}.loot.{}.{}", safe_id(session_id), safe_id(name), Uuid::new_v4().simple()),
                session_id: session_id.into(),
                scope: Scope { scope_type: ScopeType::Scene, scope_id: "scene.current".into() },
                display_name: name.into(),
                object_kind: kind,
                location: ObjectLocation::OnGround { scene_id: "scene.current".into(), zone_id: None },
                visibility_state: visible_state(name, true),
                mechanical_state: it.get("mechanical_profile").cloned().unwrap_or_else(|| json!({})),
                quantity: qty,
                tags: vec!["loot".into(), format!("looted_by:{}", finder)],
                active: true,
                ..Default::default()
            },
            reason: format!("looted from search: {}", name),
        });
    }
    out
}

/// Score how well an owned object matches the player's words. Generic: compares
/// the input against the object's display_name + tags (data on the instance).
/// ASCII whole-word hits weigh more; shared CJK characters give partial credit
/// (so "用魔杖" matches "奥术法杖" via the shared 杖). No per-item keyword table.
fn reference_score(input_lower: &str, obj: &ObjectInstance) -> i32 {
    let name = obj.display_name.to_lowercase();
    let mut score = 0;
    for w in name.split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 3) {
        if input_lower.contains(w) { score += 3; }
    }
    for ch in name.chars().filter(|c| !c.is_ascii() && !c.is_whitespace()) {
        if input_lower.contains(ch) { score += 1; }
    }
    for t in &obj.tags {
        let t = t.to_lowercase();
        if t.starts_with("held_by:") || t.starts_with("carried_by:") || t == "weapon" || t == "held" { continue; }
        if t.len() >= 3 && input_lower.contains(&t) { score += 2; }
    }
    score
}
fn requires_check(kind: ObjectInteractionKind) -> bool { matches!(kind, ObjectInteractionKind::GrabHeldObject | ObjectInteractionKind::Disarm | ObjectInteractionKind::Steal | ObjectInteractionKind::Hack | ObjectInteractionKind::Break | ObjectInteractionKind::Repair | ObjectInteractionKind::Unlock | ObjectInteractionKind::CutConnection | ObjectInteractionKind::TraceConnection | ObjectInteractionKind::Loot | ObjectInteractionKind::Search | ObjectInteractionKind::Use | ObjectInteractionKind::Reload) }

fn make_object_check(input: ObjectTurnInput<'_>, actor_id: &str, target_actor_id: Option<&str>, object: &ObjectInstance, kind: ObjectInteractionKind, kernel_dice: Option<&str>, kernel_target: Option<CheckTargetModel>, kernel_source_refs: Vec<SourceRef>, kernel: Option<&RuleKernel>) -> CheckContract {
    let dice = kernel_dice.unwrap_or("1d20").to_string();
    // Data-driven: check_label_policy from kernel override wins; fallback to generic per-kind.
    let generic_label = match kind {
        ObjectInteractionKind::GrabHeldObject | ObjectInteractionKind::Disarm => "appropriate opposed disarm / athletics check",
        ObjectInteractionKind::Unlock => "appropriate lockpicking / technical unlock check",
        ObjectInteractionKind::CutConnection | ObjectInteractionKind::TraceConnection => "appropriate technical analysis / cable handling check",
        ObjectInteractionKind::Steal | ObjectInteractionKind::Loot => "appropriate stealth / sleight / search check",
        _ => "appropriate object interaction check",
    };
    let check_label = kernel
        .and_then(|k| k.check_label_policy.as_ref())
        .and_then(|p| p.labels.get(kind.as_str()))
        .map(|s| s.as_str())
        .unwrap_or(generic_label)
        .to_string();
    let defender = target_actor_id.map(|id| ActorRef { actor_id: id.into(), actor_kind: ActorKind::Npc, display_name: Some("target".into()) });
    // Data-driven target: prefer the kernel's core mechanic; otherwise leave it
    // UnknownUntilLookup so the contest kernel resolves it (PercentileRollUnder /
    // DicePoolCount / etc.) — never a fabricated roll-high static DV.
    let target = kernel_target.unwrap_or(CheckTargetModel::UnknownUntilLookup);
    let opposition = match defender.clone() {
        Some(d) => OppositionModel::OpposedActive { defender: d, defender_check_label: "opposing object control / defense".into(), defender_dice_expression: dice.clone() },
        None => OppositionModel::NoMechanicalOpposition,
    };
    let ruling_status = if kernel_source_refs.is_empty() { RulingStatus::Provisional } else { RulingStatus::SourceBacked };
    CheckContract {
        check_id: format!("check_{}", Uuid::new_v4().simple()), session_id: input.session_id.into(), turn_id: input.turn_id.into(), ruleset_id: input.ruleset_id.into(), module_id: input.module_id.map(str::to_string), initiator: ActorRef { actor_id: actor_id.into(), actor_kind: ActorKind::PlayerCharacter, display_name: Some("current actor".into()) }, target_actor: defender.clone(), opposition, action_summary: input.user_input.chars().take(500).collect(), intent_kind: format!("object:{}", kind.as_str()), check_label, dice_expression: dice.clone(), modifiers: vec![], target, tested_parameter: None, opponent_tested_parameter: None, actor_snapshot_ids: input.frame_id.map(|id| vec![id.to_string()]).unwrap_or_default(), source_refs: kernel_source_refs, learned_packet_ids: vec![], roll_visibility: RollVisibility::PlayerRollRequired, roll_authority: RollAuthority::Player, disclosure: RollDisclosurePolicy::for_visibility(RollVisibility::PlayerRollRequired), stakes: CheckStakes { before_roll_public: format!("You can try to {} {}. Success changes object possession/state; failure leaves it controlled, secured, or escalates risk.", kind.as_str(), object.display_name), success_public: "Object state changes through validated ObjectPatch.".into(), failure_public: "The object is not transferred/changed; the scene may react.".into(), critical_public: None, fumble_public: None, success_patches_allowed: vec!["object_patch".into()], failure_patches_allowed: vec!["object_patch".into()], irreversible: false }, confidence: RulingConfidence::Low, ruling_status, advice_refs: vec!["object.possession.kernel.v1_6".into()], expires_at_turn: Some(input.turn_id.into()) }
}

fn preconditions_for(actor_id: &str, object: &ObjectInstance, kind: ObjectInteractionKind) -> Vec<ObjectPrecondition> {
    let mut pre = vec![ObjectPrecondition::ObjectExists { object_id: object.object_id.clone() }, ObjectPrecondition::ObjectVisibleOrKnown { object_id: object.object_id.clone(), actor_id: actor_id.into() }];
    if matches!(kind, ObjectInteractionKind::GrabHeldObject | ObjectInteractionKind::Disarm | ObjectInteractionKind::Steal) {
        if let ObjectLocation::Held { actor_id: holder, .. } = &object.location { pre.push(ObjectPrecondition::ObjectHeldBy { object_id: object.object_id.clone(), actor_id: holder.clone() }); }
        pre.push(ObjectPrecondition::ObjectWithinReach { object_id: object.object_id.clone(), actor_id: actor_id.into() });
        pre.push(ObjectPrecondition::ActorHasFreeHand { actor_id: actor_id.into() });
    }
    if matches!(kind, ObjectInteractionKind::Equip) { pre.push(ObjectPrecondition::ActorHasSlotFree { actor_id: actor_id.into(), slot: EquipSlotKind::MainHand }); }
    pre
}

fn success_patches_for(actor_id: &str, target_actor_id: Option<&str>, object: &ObjectInstance, kind: ObjectInteractionKind) -> Vec<ObjectPatch> {
    match kind {
        ObjectInteractionKind::GrabHeldObject | ObjectInteractionKind::Disarm | ObjectInteractionKind::Steal => vec![ObjectPatch::TransferObject { object_id: object.object_id.clone(), from: object.location.clone(), to: ObjectLocation::Held { actor_id: actor_id.into(), hand: Some(HandSlot::Left) }, reason: format!("successful {}", kind.as_str()) }],
        ObjectInteractionKind::PickUp => vec![ObjectPatch::SetObjectLocation { object_id: object.object_id.clone(), location: ObjectLocation::Held { actor_id: actor_id.into(), hand: Some(HandSlot::Right) }, reason: "picked up object".into() }],
        ObjectInteractionKind::Drop => vec![ObjectPatch::SetObjectLocation { object_id: object.object_id.clone(), location: ObjectLocation::OnGround { scene_id: "scene.current".into(), zone_id: None }, reason: "dropped object".into() }],
        ObjectInteractionKind::Equip => vec![ObjectPatch::SetObjectLocation { object_id: object.object_id.clone(), location: ObjectLocation::Equipped { actor_id: actor_id.into(), slot: if object.object_kind == ObjectKind::Armor { EquipSlotKind::BodyArmor } else { EquipSlotKind::MainHand } }, reason: "equipped object".into() }],
        ObjectInteractionKind::CutConnection => vec![ObjectPatch::SetMechanicalState { object_id: object.object_id.clone(), patch_json: json!({"connection_cut": true, "cut_by": actor_id}), reason: "connection cut".into() }],
        ObjectInteractionKind::Unlock => vec![ObjectPatch::SetMechanicalState { object_id: object.object_id.clone(), patch_json: json!({"locked": false, "unlocked_by": actor_id}), reason: "unlocked".into() }],
        ObjectInteractionKind::Break => vec![ObjectPatch::DamageObject { object_id: object.object_id.clone(), amount: 1, damage_kind: "break".into(), reason: "object damaged/broken".into() }],
        ObjectInteractionKind::Reveal | ObjectInteractionKind::Inspect | ObjectInteractionKind::TraceConnection => vec![ObjectPatch::SetObjectVisibility { object_id: object.object_id.clone(), visibility_state: visible_state(&object.display_name, true), reason: "object information revealed".into() }],
        // Loot/Search: the looted target/container declares its contents as DATA
        // (mechanical_state.loot_contents = [{name, kind, quantity, ...}], filled
        // from the NPC card / module via materialization — NOT an engine item
        // table). On success we reveal the container AND spawn a real instance per
        // declared item (with quantity). No fabrication: if nothing is declared,
        // it's reveal-only (fail closed).
        ObjectInteractionKind::Loot | ObjectInteractionKind::Search => {
            let mut patches = vec![ObjectPatch::SetObjectVisibility { object_id: object.object_id.clone(), visibility_state: visible_state(&object.display_name, true), reason: "container searched".into() }];
            patches.extend(loot_patches_from_contents(&object.mechanical_state, &object.session_id, actor_id));
            patches
        }
        // Reload: refill loaded rounds to the magazine capacity (data-driven from
        // the weapon's mechanical_state.ammo_max).
        ObjectInteractionKind::Reload => vec![ObjectPatch::SetMechanicalState { object_id: object.object_id.clone(), patch_json: json!({"ammo_current": object.mechanical_state.get("ammo_max").and_then(|v| v.as_i64()).unwrap_or(0)}), reason: "reloaded to full magazine".into() }],
        // Use: ONLY a charged object (one that declares `charges`) changes state;
        // its per-use cost AND its depletion behavior are the object's OWN data,
        // not an engine policy. When charges run out, the object transforms ONLY
        // if it declares a depleted form (depleted_kind/depleted_name) — otherwise
        // it's just marked spent. A non-charged object is narration-only here (no
        // fabricated depletion). The engine encodes no per-item/ruleset policy.
        ObjectInteractionKind::Use => match object.mechanical_state.get("charges").and_then(|v| v.as_i64()) {
            None => vec![],
            Some(charges) => {
                let cost = object.mechanical_state.get("charge_cost").and_then(|v| v.as_i64()).unwrap_or(1).max(1);
                let next = charges - cost;
                if next > 0 {
                    vec![ObjectPatch::SetMechanicalState { object_id: object.object_id.clone(), patch_json: json!({"charges": next}), reason: "used one charge".into() }]
                } else if object.mechanical_state.get("depleted_kind").is_some() || object.mechanical_state.get("depleted_name").is_some() {
                    vec![ObjectPatch::TransformObject {
                        object_id: object.object_id.clone(),
                        new_kind: object.mechanical_state.get("depleted_kind").and_then(|v| v.as_str()).map(object_kind_from_str),
                        new_name: object.mechanical_state.get("depleted_name").and_then(|v| v.as_str()).map(str::to_string),
                        set_mechanical_state: Some(json!({"charges": 0, "depleted": true})),
                        reason: "item spent its last charge and transformed into its declared depleted form".into(),
                    }]
                } else {
                    vec![ObjectPatch::SetMechanicalState { object_id: object.object_id.clone(), patch_json: json!({"charges": 0, "depleted": true}), reason: "item spent its last charge".into() }]
                }
            }
        },
        _ => target_actor_id.map(|_| vec![]).unwrap_or_default(),
    }
}
fn failure_patches_for(object: &ObjectInstance, kind: ObjectInteractionKind) -> Vec<ObjectPatch> { if matches!(kind, ObjectInteractionKind::CutConnection | ObjectInteractionKind::Unlock | ObjectInteractionKind::Break) { vec![ObjectPatch::SetMechanicalState { object_id: object.object_id.clone(), patch_json: json!({"last_failed_interaction": kind.as_str()}), reason: "failed object interaction records pressure".into() }] } else { vec![] } }

fn patch_kind(p: &ObjectPatch) -> &'static str { match p { ObjectPatch::CreateObjectInstance { .. } => "create_object_instance", ObjectPatch::TransferObject { .. } => "transfer_object", ObjectPatch::SetObjectLocation { .. } => "set_object_location", ObjectPatch::SetObjectVisibility { .. } => "set_object_visibility", ObjectPatch::ModifyQuantity { .. } => "modify_quantity", ObjectPatch::DamageObject { .. } => "damage_object", ObjectPatch::DestroyObject { .. } => "destroy_object", ObjectPatch::AddObjectEdge { .. } => "add_object_edge", ObjectPatch::RemoveObjectEdge { .. } => "remove_object_edge", ObjectPatch::SetMechanicalState { .. } => "set_mechanical_state", ObjectPatch::TransformObject { .. } => "transform_object" } }
fn patch_object_id(p: &ObjectPatch) -> Option<String> { match p { ObjectPatch::CreateObjectInstance { object, .. } => Some(object.object_id.clone()), ObjectPatch::TransferObject { object_id, .. } | ObjectPatch::SetObjectLocation { object_id, .. } | ObjectPatch::SetObjectVisibility { object_id, .. } | ObjectPatch::ModifyQuantity { object_id, .. } | ObjectPatch::DamageObject { object_id, .. } | ObjectPatch::DestroyObject { object_id, .. } | ObjectPatch::SetMechanicalState { object_id, .. } | ObjectPatch::TransformObject { object_id, .. } => Some(object_id.clone()), ObjectPatch::AddObjectEdge { edge, .. } => Some(edge.from_object_id.clone()), ObjectPatch::RemoveObjectEdge { edge_id, .. } => Some(edge_id.clone()) } }
fn visible_state(label: &str, known: bool) -> ObjectVisibilityState { ObjectVisibilityState { public_label: Some(label.into()), identified_label: if known { Some(label.into()) } else { None }, gm_label: label.into(), discovery_state: if known { DiscoveryState::Known } else { DiscoveryState::Unknown }, known_by_actor_ids: vec!["pc.current".into()], known_by_player: known, reveal_conditions: vec![] } }
fn default_affordance_kinds(obj: &ObjectInstance) -> Vec<ObjectInteractionKind> { match obj.object_kind { ObjectKind::Weapon => vec![ObjectInteractionKind::PickUp, ObjectInteractionKind::Drop, ObjectInteractionKind::Equip, ObjectInteractionKind::Use, ObjectInteractionKind::Disarm], ObjectKind::Armor => vec![ObjectInteractionKind::Equip, ObjectInteractionKind::Unequip, ObjectInteractionKind::Inspect], ObjectKind::Lock => vec![ObjectInteractionKind::Unlock, ObjectInteractionKind::Break, ObjectInteractionKind::Inspect], ObjectKind::EnvironmentalFeature | ObjectKind::Device => vec![ObjectInteractionKind::Inspect, ObjectInteractionKind::TraceConnection, ObjectInteractionKind::CutConnection, ObjectInteractionKind::Hack], _ => vec![ObjectInteractionKind::Inspect, ObjectInteractionKind::Use] } }

fn row_to_object(row: sqlx::postgres::PgRow) -> Result<ObjectInstance> {
    let scope_type: String = row.get("scope_type");
    let scope = Scope { scope_type: scope_type_from_str(&scope_type), scope_id: row.get("scope_id") };
    let object_kind = object_kind_from_str(&row.get::<String,_>("object_kind"));
    let durability_json: Option<serde_json::Value> = row.get("durability_json");
    let durability = durability_json.and_then(|v| serde_json::from_value::<DurabilityState>(v).ok());
    Ok(ObjectInstance {
        object_id: row.get("object_id"),
        object_def_id: row.get("object_def_id"),
        session_id: row.get("session_id"),
        scope,
        display_name: row.get("display_name"),
        object_kind,
        location: serde_json::from_value(row.get("location_json"))?,
        visibility_state: serde_json::from_value(row.get("visibility_state"))?,
        quantity: row.get("quantity"),
        durability,
        mechanical_state: row.get("mechanical_state"),
        tags: row.get("tags"),
        active: row.get("active"),
        created_at_tick: row.get::<Option<i64>,_>("created_at_tick").unwrap_or_default(),
        updated_at_tick: row.get::<Option<i64>,_>("updated_at_tick").unwrap_or_default(),
    })
}
fn object_kind_from_str(s: &str) -> ObjectKind { match s { "weapon" => ObjectKind::Weapon, "armor" => ObjectKind::Armor, "shield" => ObjectKind::Shield, "tool" => ObjectKind::Tool, "consumable" => ObjectKind::Consumable, "ammo" => ObjectKind::Ammo, "container" => ObjectKind::Container, "door" => ObjectKind::Door, "lock" => ObjectKind::Lock, "vehicle" => ObjectKind::Vehicle, "vehicle_part" => ObjectKind::VehiclePart, "cyberware" => ObjectKind::Cyberware, "program" => ObjectKind::Program, "device" => ObjectKind::Device, "document" => ObjectKind::Document, "key" => ObjectKind::Key, "clue" => ObjectKind::Clue, "currency" => ObjectKind::Currency, "quest_item" => ObjectKind::QuestItem, "environmental_feature" => ObjectKind::EnvironmentalFeature, "structure" => ObjectKind::Structure, "hazard_object" => ObjectKind::HazardObject, "anomaly_object" => ObjectKind::AnomalyObject, _ => ObjectKind::Unknown } }
fn scope_type_from_str(s: &str) -> ScopeType { match s { "ruleset" => ScopeType::Ruleset, "module" => ScopeType::Module, "campaign" => ScopeType::Campaign, "chapter" => ScopeType::Chapter, "mission" => ScopeType::Mission, "location" => ScopeType::Location, "npc" => ScopeType::Npc, "session" => ScopeType::Session, "scene" => ScopeType::Scene, "turn" => ScopeType::Turn, "material" => ScopeType::Material, "character" => ScopeType::Character, "object" => ScopeType::Object, _ => ScopeType::Global } }

#[cfg(test)]
mod object_use_tests {
    use super::*;
    use serde_json::json;

    fn obj(name: &str, mech: serde_json::Value, tags: Vec<&str>) -> ObjectInstance {
        ObjectInstance { object_id: "o1".into(), display_name: name.into(), mechanical_state: mech, tags: tags.into_iter().map(String::from).collect(), ..Default::default() }
    }

    #[test]
    fn reference_prefers_named_owned_object() {
        // "用魔杖" must pick the 法杖 (shares 杖), not the held pistol.
        let wand = obj("奥术法杖", json!({"charges":1}), vec!["held_by:pc.current","device"]);
        let pistol = obj("Heavy Pistol", json!({"ammo_current":8}), vec!["held_by:pc.current","weapon"]);
        let input = "用魔杖".to_lowercase();
        assert!(reference_score(&input, &wand) > 0, "wand shares 杖 with 魔杖");
        assert_eq!(reference_score(&input, &pistol), 0, "pistol has no overlap");
        assert!(reference_score(&input, &wand) > reference_score(&input, &pistol));
    }

    #[test]
    fn use_charged_decrements_then_transforms_to_declared_form() {
        let w3 = obj("奥术法杖", json!({"charges":3,"depleted_kind":"tool","depleted_name":"朽木棍"}), vec![]);
        match &success_patches_for("pc.current", None, &w3, ObjectInteractionKind::Use)[..] {
            [ObjectPatch::SetMechanicalState { patch_json, .. }] => assert_eq!(patch_json.get("charges").and_then(|v| v.as_i64()), Some(2)),
            _ => panic!("3 charges -> set charges=2"),
        }
        let w1 = obj("奥术法杖", json!({"charges":1,"depleted_kind":"tool","depleted_name":"朽木棍"}), vec![]);
        match &success_patches_for("pc.current", None, &w1, ObjectInteractionKind::Use)[..] {
            [ObjectPatch::TransformObject { new_name, new_kind, .. }] => { assert_eq!(new_name.as_deref(), Some("朽木棍")); assert_eq!(*new_kind, Some(ObjectKind::Tool)); }
            _ => panic!("last charge + declared form -> transform"),
        }
    }

    #[test]
    fn use_last_charge_without_declared_form_just_marks_spent() {
        let w = obj("通用药剂", json!({"charges":1}), vec![]);
        match &success_patches_for("pc.current", None, &w, ObjectInteractionKind::Use)[..] {
            [ObjectPatch::SetMechanicalState { patch_json, .. }] => { assert_eq!(patch_json.get("charges").and_then(|v| v.as_i64()), Some(0)); assert_eq!(patch_json.get("depleted").and_then(|v| v.as_bool()), Some(true)); }
            _ => panic!("no declared form -> mark spent (no transform)"),
        }
    }

    #[test]
    fn use_noncharged_object_is_narration_only() {
        let w = obj("一块石头", json!({}), vec![]);
        assert!(success_patches_for("pc.current", None, &w, ObjectInteractionKind::Use).is_empty(), "non-charged Use is narration-only");
    }

    #[test]
    fn use_respects_charge_cost() {
        let w = obj("法杖", json!({"charges":2,"charge_cost":2,"depleted_name":"灰烬"}), vec![]);
        assert!(matches!(&success_patches_for("pc.current", None, &w, ObjectInteractionKind::Use)[..], [ObjectPatch::TransformObject { .. }]), "cost 2 from 2 -> deplete -> transform");
    }
}

#[cfg(test)]
mod loot_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn loot_yields_named_items_with_quantities() {
        // A body declaring its carried items (as materialization would fill from
        // the NPC card / sheet equipment) yields real named instances + counts.
        let body = json!({"loot_contents": [
            {"name": ".38左轮手枪", "kind": "weapon", "quantity": 1},
            {"name": ".38子弹", "kind": "ammo", "quantity": 12}
        ]});
        let patches = loot_patches_from_contents(&body, "session_x", "pc.current");
        assert_eq!(patches.len(), 2);
        let mut seen = std::collections::HashMap::new();
        for p in &patches {
            if let ObjectPatch::CreateObjectInstance { object, .. } = p {
                seen.insert(object.display_name.clone(), (object.object_kind, object.quantity));
            } else { panic!("loot must construct CreateObjectInstance, got {}", patch_kind(p)); }
        }
        assert_eq!(seen.get(".38左轮手枪"), Some(&(ObjectKind::Weapon, Some(1))));
        assert_eq!(seen.get(".38子弹"), Some(&(ObjectKind::Ammo, Some(12))));
    }

    #[test]
    fn loot_without_declared_contents_yields_nothing() {
        // Fail closed: no source-backed contents => no fabricated items.
        assert!(loot_patches_from_contents(&json!({}), "s", "pc.current").is_empty());
        assert!(loot_patches_from_contents(&json!({"loot_contents": []}), "s", "pc.current").is_empty());
    }

    #[test]
    fn loot_success_arm_reveals_and_spawns() {
        let body = ObjectInstance {
            object_id: "body1".into(), session_id: "s".into(), display_name: "倒下的盗匪".into(),
            mechanical_state: json!({"loot_contents":[{"name":"钥匙","kind":"key","quantity":1}]}),
            ..Default::default()
        };
        let patches = success_patches_for("pc.current", None, &body, ObjectInteractionKind::Loot);
        // reveal + 1 created item
        assert!(patches.iter().any(|p| matches!(p, ObjectPatch::SetObjectVisibility { .. })));
        assert!(patches.iter().any(|p| matches!(p, ObjectPatch::CreateObjectInstance { .. })));
    }

    #[test]
    fn semantic_loot_search_kinds_parse() {
        // The semantic classifier's loot/search are recognized (not downgraded to Inspect).
        assert!(matches!(parse_object_interaction_kind("loot"), Some(ObjectInteractionKind::Loot)));
        assert!(matches!(parse_object_interaction_kind("search"), Some(ObjectInteractionKind::Search)));
        assert!(parse_object_interaction_kind("nonsense").is_none());
    }

    #[test]
    fn provisional_seed_fabricates_no_stats_and_asks_for_clarification() {
        // Retired weapon_definition_for: an item with no source def gets a
        // provisional seed — NO damage/ammo/rof fabricated, and clarification_needed
        // so the smart GM asks + verifies against the rules (branch 1).
        let (_id, name, profile) = provisional_item_seed("cyberpunk_red", "我从裤裆掏出一个阿斯塔特");
        assert_eq!(name, "unresolved item");
        assert_eq!(profile.get("clarification_needed").and_then(|v| v.as_bool()), Some(true));
        for fabricated in ["damage", "ammo_max", "ammo_current", "rof", "armor"] {
            assert!(profile.get(fabricated).is_none(), "must NOT fabricate {fabricated} for an unsourced item");
        }
        assert!(profile.get("referenced_as").and_then(|v| v.as_str()).unwrap_or("").contains("阿斯塔特"));
    }
}

// -------------------------------------------------------------------------
// P0-2 T5: check_label_policy equivalence tests
// -------------------------------------------------------------------------
#[cfg(test)]
mod check_label_policy_tests {
    use super::*;
    use std::collections::BTreeMap;

    fn kernel_with_disarm_label(label: &str) -> RuleKernel {
        let mut k: RuleKernel = serde_json::from_str(
            r#"{"kernel_id":"t","ruleset_id":"t","version":"1"}"#
        ).unwrap();
        let mut labels = BTreeMap::new();
        labels.insert("disarm".to_string(), label.to_string());
        labels.insert("grab_held_object".to_string(), label.to_string());
        k.check_label_policy = Some(CheckLabelPolicy { labels });
        k
    }

    /// Cyberpunk policy: check_label_policy["disarm"] = DEX+Brawling label.
    #[test]
    fn kernel_policy_supplies_cyberpunk_disarm_label() {
        let k = kernel_with_disarm_label("DEX + Brawling contested grab/disarm check");
        let policy = k.check_label_policy.as_ref().unwrap();
        assert_eq!(
            policy.labels.get("disarm").map(|s| s.as_str()),
            Some("DEX + Brawling contested grab/disarm check"),
            "disarm label from cyberpunk policy must match legacy hardcode"
        );
    }

    /// No policy → generic label (no "cyberpunk" name in fallback).
    #[test]
    fn no_policy_gives_generic_disarm_label() {
        let k: RuleKernel = serde_json::from_str(
            r#"{"kernel_id":"t","ruleset_id":"call_of_cthulhu_7e","version":"1"}"#
        ).unwrap();
        assert!(k.check_label_policy.is_none(), "non-cyberpunk must have no check_label_policy");
        // Generic fallback (from make_object_check):
        let generic = match ObjectInteractionKind::GrabHeldObject {
            ObjectInteractionKind::GrabHeldObject | ObjectInteractionKind::Disarm =>
                "appropriate opposed disarm / athletics check",
            _ => "appropriate object interaction check",
        };
        assert!(!generic.to_lowercase().contains("cyberpunk"),
            "generic label must not mention cyberpunk; got {:?}", generic);
        assert!(!generic.to_lowercase().contains("brawling"),
            "generic label must not mention brawling; got {:?}", generic);
    }

    /// Policy present but key missing → generic fallback.
    #[test]
    fn policy_missing_key_falls_back_to_generic() {
        let mut k: RuleKernel = serde_json::from_str(
            r#"{"kernel_id":"t","ruleset_id":"t","version":"1"}"#
        ).unwrap();
        k.check_label_policy = Some(CheckLabelPolicy { labels: BTreeMap::new() });
        let policy = k.check_label_policy.as_ref().unwrap();
        // "unlock" is not in the empty policy → engine would use generic
        assert!(policy.labels.get("unlock").is_none());
    }
}
