use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::Row;
use trpg_db::Db;
use trpg_model::*;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuntimeActorParameters {
    pub actor_param_id: String,
    pub session_id: String,
    pub actor_id: String,
    pub actor_kind: ActorKind,
    pub ruleset_id: String,
    pub source_kind: String,
    pub template_id: Option<String>,
    pub display_name: Option<String>,
    pub sheet_json: Value,
    pub mechanical_profile: Value,
    pub status_json: Value,
    pub visibility: Visibility,
    pub created_at_tick: Option<i64>,
    pub updated_at_tick: Option<i64>,
}

#[derive(Clone)]
pub struct RuntimeParameterService {
    pub db: Db,
}

impl RuntimeParameterService {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub async fn ensure_actor_parameters(
        &self,
        session_id: &str,
        ruleset_id: &str,
        actor_id: &str,
        actor_kind: ActorKind,
        world_tick: i64,
    ) -> Result<RuntimeActorParameters> {
        if let Some(existing) = self.load_actor_parameters(session_id, actor_id).await? {
            return Ok(existing);
        }
        let template = self
            .db
            .load_character_template(ruleset_id)
            .await
            .ok()
            .flatten();
        let template_id = template.as_ref().map(|t| t.template_id.clone());
        let kernel = self.db.load_rule_kernel(ruleset_id).await.ok().flatten();
        let mut params = seed_actor_parameters(
            session_id,
            ruleset_id,
            actor_id,
            actor_kind,
            template.as_ref(),
            kernel.as_ref(),
            world_tick,
        );
        params.template_id = template_id;
        self.upsert_actor_parameters(&params).await?;
        let hydration_status = if params.source_kind.contains("unresolved") {
            "unresolved_source_required"
        } else {
            "hydrated"
        };
        self.insert_hydration_event(session_id, None, None, Some(actor_id), None, ruleset_id, "actor_parameters", hydration_status, None, json!({"actor_id": actor_id, "actor_kind": actor_kind, "template_id": params.template_id, "profile": params.mechanical_profile, "source_kind": params.source_kind}), world_tick).await.ok();
        Ok(params)
    }

    pub async fn load_actor_parameters(
        &self,
        session_id: &str,
        actor_id: &str,
    ) -> Result<Option<RuntimeActorParameters>> {
        let row = sqlx::query(r#"
            select actor_param_id, session_id, actor_id, actor_kind, ruleset_id, source_kind, template_id, display_name,
                   sheet_json, mechanical_profile, status_json, visibility, created_at_tick, updated_at_tick
            from runtime_actor_parameters where session_id=$1 and actor_id=$2 limit 1
        "#).bind(session_id).bind(actor_id).fetch_optional(&self.db.pool).await?;
        Ok(row.and_then(row_to_params))
    }

    /// All player-character actor rows for a session, the de-facto roster source
    /// (there is no session→PC table; this per-session actor surface IS the
    /// roster). Used by the director's spotlight tracker. Deterministic order.
    pub async fn list_player_actor_parameters(
        &self,
        session_id: &str,
    ) -> Result<Vec<RuntimeActorParameters>> {
        let rows = sqlx::query(r#"
            select actor_param_id, session_id, actor_id, actor_kind, ruleset_id, source_kind, template_id, display_name,
                   sheet_json, mechanical_profile, status_json, visibility, created_at_tick, updated_at_tick
            from runtime_actor_parameters
            where session_id=$1 and actor_kind='player_character'
            order by created_at asc, actor_id asc
        "#).bind(session_id).fetch_all(&self.db.pool).await?;
        Ok(rows.into_iter().filter_map(row_to_params).collect())
    }

    pub async fn upsert_actor_parameters(&self, p: &RuntimeActorParameters) -> Result<()> {
        sqlx::query(r#"
            insert into runtime_actor_parameters
              (id, actor_param_id, session_id, actor_id, actor_kind, ruleset_id, source_kind, template_id, display_name,
               sheet_json, mechanical_profile, status_json, visibility, created_at_tick, updated_at_tick)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)
            on conflict (session_id, actor_id) do update set
              actor_param_id=excluded.actor_param_id,
              actor_kind=excluded.actor_kind,
              ruleset_id=excluded.ruleset_id,
              source_kind=excluded.source_kind,
              template_id=excluded.template_id,
              display_name=excluded.display_name,
              sheet_json=excluded.sheet_json,
              mechanical_profile=excluded.mechanical_profile,
              status_json=excluded.status_json,
              visibility=excluded.visibility,
              updated_at_tick=excluded.updated_at_tick,
              updated_at=now()
        "#)
        .bind(Uuid::new_v4())
        .bind(&p.actor_param_id)
        .bind(&p.session_id)
        .bind(&p.actor_id)
        .bind(p.actor_kind.as_str())
        .bind(&p.ruleset_id)
        .bind(&p.source_kind)
        .bind(&p.template_id)
        .bind(&p.display_name)
        .bind(&p.sheet_json)
        .bind(&p.mechanical_profile)
        .bind(&p.status_json)
        .bind(p.visibility.as_str())
        .bind(p.created_at_tick)
        .bind(p.updated_at_tick)
        .execute(&self.db.pool).await?;
        Ok(())
    }

    pub async fn actor_parameters_context_block(
        &self,
        session_id: &str,
        world_tick: i64,
    ) -> Result<ContextBlock> {
        let rows = sqlx::query(r#"
            select actor_param_id, session_id, actor_id, actor_kind, ruleset_id, source_kind, template_id, display_name,
                   sheet_json, mechanical_profile, status_json, visibility, created_at_tick, updated_at_tick
            from runtime_actor_parameters where session_id=$1 order by updated_at desc limit 20
        "#).bind(session_id).fetch_all(&self.db.pool).await?;
        let params: Vec<RuntimeActorParameters> =
            rows.into_iter().filter_map(row_to_params).collect();
        let mut block = ContextBlock::new(
            format!("runtime.actor_parameters.{}", session_id),
            BlockKind::NpcStatic,
            "Runtime Actor / Character Parameters",
            BlockContent::Json(json!({
                "world_tick": world_tick,
                "parameters": params,
                "cache_policy": "BP3 dynamic: actor HP/status/skills/loadouts may change during play; stable character template remains BP1/BP2 material"
            })),
            Visibility::GmOnly,
            Stability::TurnDynamic,
            CacheZone::DynamicTail,
            Scope {
                scope_type: ScopeType::Session,
                scope_id: session_id.to_string(),
            },
            147,
        );
        block.tags = vec![
            "actor_parameters".into(),
            "bp3".into(),
            "runtime_hydration".into(),
        ];
        Ok(block)
    }

    pub async fn insert_hydration_event(
        &self,
        session_id: &str,
        turn_id: Option<&str>,
        frame_id: Option<&str>,
        actor_id: Option<&str>,
        object_id: Option<&str>,
        ruleset_id: &str,
        material_kind: &str,
        status: &str,
        query_text: Option<&str>,
        result_json: Value,
        world_tick: i64,
    ) -> Result<()> {
        sqlx::query(r#"
            insert into material_hydration_events
              (id, event_id, session_id, turn_id, frame_id, actor_id, object_id, ruleset_id, material_kind, hydration_status, query_text, result_json, world_tick)
            values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
        "#)
        .bind(Uuid::new_v4())
        .bind(format!("hydration_event_{}", Uuid::new_v4().simple()))
        .bind(session_id)
        .bind(turn_id)
        .bind(frame_id)
        .bind(actor_id)
        .bind(object_id)
        .bind(ruleset_id)
        .bind(material_kind)
        .bind(status)
        .bind(query_text)
        .bind(result_json)
        .bind(world_tick)
        .execute(&self.db.pool).await?;
        Ok(())
    }
}

fn row_to_params(row: sqlx::postgres::PgRow) -> Option<RuntimeActorParameters> {
    Some(RuntimeActorParameters {
        actor_param_id: row.get("actor_param_id"),
        session_id: row.get("session_id"),
        actor_id: row.get("actor_id"),
        actor_kind: actor_kind_from_str(&row.get::<String, _>("actor_kind")),
        ruleset_id: row.get("ruleset_id"),
        source_kind: row.get("source_kind"),
        template_id: row.get("template_id"),
        display_name: row.get("display_name"),
        sheet_json: row.get("sheet_json"),
        mechanical_profile: row.get("mechanical_profile"),
        status_json: row.get("status_json"),
        visibility: visibility_from_str(&row.get::<String, _>("visibility")),
        created_at_tick: row.get("created_at_tick"),
        updated_at_tick: row.get("updated_at_tick"),
    })
}

fn seed_actor_parameters(
    session_id: &str,
    ruleset_id: &str,
    actor_id: &str,
    actor_kind: ActorKind,
    template: Option<&CharacterTemplate>,
    kernel: Option<&RuleKernel>,
    world_tick: i64,
) -> RuntimeActorParameters {
    let is_pc = matches!(actor_kind, ActorKind::PlayerCharacter);
    let synthetic_allowed = if is_pc {
        allow_synthetic_actor_seeds()
    } else {
        allow_synthetic_actor_seeds() || allow_synthetic_npc_seeds()
    };
    if !synthetic_allowed {
        return unresolved_actor_parameters(
            session_id, ruleset_id, actor_id, actor_kind, template, world_tick,
        );
    }
    // De-hardcoded: ONE generic synthetic seed (debug scaffolding only). Resource
    // tracks come from the parsed kernel; no per-ruleset stat blocks. Real
    // mechanical values come from create-character, not this placeholder path.
    let (sheet_json, mut mechanical_profile, mut status_json) =
        synthetic_seed(actor_id, is_pc, template, ruleset_id, kernel);
    if let Some(obj) = mechanical_profile.as_object_mut() {
        obj.insert(
            "source_quality".into(),
            json!("placeholder_not_source_backed"),
        );
        obj.insert("requires_source_backed_materialization".into(), json!(true));
        obj.insert("notes".into(), json!("Seeded placeholder values exist only to keep lifecycle/state tables shaped; mechanical resolution must not consume them unless explicitly enabled."));
    }
    if let Some(obj) = status_json.as_object_mut() {
        obj.insert(
            "source_quality".into(),
            json!("placeholder_not_source_backed"),
        );
        obj.insert("requires_source_backed_materialization".into(), json!(true));
    }
    RuntimeActorParameters {
        actor_param_id: format!("actor_params.{}.{}", safe_id(session_id), safe_id(actor_id)),
        session_id: session_id.into(),
        actor_id: actor_id.into(),
        actor_kind,
        ruleset_id: ruleset_id.into(),
        source_kind: "runtime_placeholder_v1_15_4".into(),
        template_id: template.map(|t| t.template_id.clone()),
        display_name: Some(actor_id.replace('.', " ")),
        sheet_json,
        mechanical_profile,
        status_json,
        visibility: Visibility::GmOnly,
        created_at_tick: Some(world_tick),
        updated_at_tick: Some(world_tick),
    }
}

fn unresolved_actor_parameters(
    session_id: &str,
    ruleset_id: &str,
    actor_id: &str,
    actor_kind: ActorKind,
    template: Option<&CharacterTemplate>,
    world_tick: i64,
) -> RuntimeActorParameters {
    RuntimeActorParameters {
        actor_param_id: format!("actor_params.{}.{}", safe_id(session_id), safe_id(actor_id)),
        session_id: session_id.into(),
        actor_id: actor_id.into(),
        actor_kind,
        ruleset_id: ruleset_id.into(),
        source_kind: "unresolved_source_backed_required_v1_15_4".into(),
        template_id: template.map(|t| t.template_id.clone()),
        display_name: Some(actor_id.replace('.', " ")),
        sheet_json: json!({
            "template_id": template.map(|t| t.template_id.clone()),
            "source_policy": "no_synthetic_defaults",
            "note": "Actor parameters must be filled by source-backed character sheet/statblock materialization before mechanical resolution."
        }),
        mechanical_profile: json!({
            "hydration": "unresolved_source_required",
            "requires_materialization": true,
            "synthetic_values_allowed": false,
            "missing_source_backed_fields": ["stats", "skills", "defense", "hp", "equipment_refs"],
            "source_policy": "source_backed_only_no_synthetic_defaults"
        }),
        status_json: json!({
            "actor_id": actor_id,
            "hp_current": null,
            "hp_max": null,
            "conditions": [],
            "not_mechanically_resolvable": true,
            "unknown_parameters": ["hp_max", "defense", "skills", "equipment_refs"],
            "source_policy": "unresolved_no_synthetic_default"
        }),
        visibility: Visibility::GmOnly,
        created_at_tick: Some(world_tick),
        updated_at_tick: Some(world_tick),
    }
}

// Legacy synthetic debug seeds. They are unreachable on the default product path
// because TRPG_ALLOW_SYNTHETIC_ACTOR_SEEDS=false. Keep them only for explicit
// offline/debug harnesses that knowingly trade source fidelity for continuity.
/// ONE generic synthetic seed (debug scaffolding). No per-ruleset stat blocks:
/// resource tracks (incl. their initial values) come from the parsed kernel;
/// HP/conditions get neutral placeholders just to keep the state tables shaped.
/// Real mechanical values arrive via create-character, not this path.
fn synthetic_seed(
    actor_id: &str,
    is_pc: bool,
    template: Option<&CharacterTemplate>,
    ruleset_id: &str,
    kernel: Option<&RuleKernel>,
) -> (Value, Value, Value) {
    let hp = if is_pc { 12 } else { 10 };
    let mut status = serde_json::Map::new();
    status.insert("hp_current".into(), json!(hp));
    status.insert("hp_max".into(), json!(hp));
    status.insert("wound_state".into(), json!("healthy"));
    status.insert("conditions".into(), json!([]));
    status.insert("actor_id".into(), json!(actor_id));
    status.insert("is_pc".into(), json!(is_pc));
    // Seed each kernel resource track at its declared initial value (data-driven).
    if let Some(k) = kernel {
        for t in &k.resource_tracks {
            if let Some(id) = t
                .get("id")
                .and_then(|v| v.as_str())
                .or_else(|| t.get("name").and_then(|v| v.as_str()))
            {
                if id.trim().is_empty() {
                    continue;
                }
                let init = t.get("initial").and_then(|v| v.as_i64()).unwrap_or(0);
                status.insert(id.trim().to_string(), json!(init));
            }
        }
    }
    let sheet = json!({"template_id": template.map(|t| t.template_id.clone()), "fields": {}});
    let mech = json!({"ruleset": ruleset_id, "stats": {}, "skills": {}, "hp_max": hp, "hydration": "seeded_generic_from_kernel"});
    (sheet, mech, Value::Object(status))
}

fn actor_kind_from_str(s: &str) -> ActorKind {
    match s {
        "player_character" => ActorKind::PlayerCharacter,
        "npc" => ActorKind::Npc,
        "environment" => ActorKind::Environment,
        "hazard" => ActorKind::Hazard,
        _ => ActorKind::System,
    }
}
fn visibility_from_str(s: &str) -> Visibility {
    match s {
        "public" => Visibility::Public,
        "player_visible" => Visibility::PlayerVisible,
        "npc_private" => Visibility::NpcPrivate,
        "system_only" => Visibility::SystemOnly,
        _ => Visibility::GmOnly,
    }
}
fn safe_id(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}
fn allow_synthetic_actor_seeds() -> bool {
    env_bool("TRPG_ALLOW_SYNTHETIC_ACTOR_SEEDS", false)
}
fn allow_synthetic_npc_seeds() -> bool {
    env_bool("TRPG_RUNTIME_PARAM_ALLOW_SYNTHETIC_NPC_SEEDS", false)
}
fn env_bool(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(default)
}
