//! Single source of truth for actor resource CURRENT values: generic_parameter_states,
//! path `resources.{track_id}.current`, target_kind=actor. One read/write path shared
//! by contest / mechanics / combat. Seed/cap come from the character's derived sheet
//! resources matched to kernel tracks (fail-closed), reusing trpg-model::match_seed.
use anyhow::Result;
use serde_json::{json, Value};
use sqlx::Row;
use trpg_model::*;
use uuid::Uuid;

use crate::Db;

fn jval_to_i32(v: &Value) -> Option<i32> {
    v.as_i64()
        .map(|n| n as i32)
        .or_else(|| v.as_f64().map(|f| f as i32))
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<i32>().ok()))
}

impl Db {
    /// Per-track (current, max) seeds derived from the actor's character sheet.
    /// Prefer `sheet_json.resources`, but also accept top-level scalar fields so
    /// spend pools like Luck can seed from generated sheets that keep the score
    /// at `sheet_json.luck`. Matching stays kernel-track driven.
    pub async fn resource_seeds(
        &self,
        session_id: &str,
        actor_id: &str,
        kernel: &RuleKernel,
    ) -> std::collections::HashMap<String, (Option<i32>, Option<i32>)> {
        let sheet: Value = sqlx::query_scalar::<_, Value>(
            "select coalesce(sheet_json,'{}'::jsonb) from runtime_actor_parameters where session_id=$1 and actor_id=$2 limit 1")
            .bind(session_id).bind(actor_id).fetch_optional(&self.pool).await.ok().flatten().unwrap_or_else(|| json!({}));
        let mut seed_source = serde_json::Map::new();
        if let Some(obj) = sheet.as_object() {
            for (key, value) in obj {
                if key == "resources" {
                    continue;
                }
                if value.is_number() || value.is_string() {
                    seed_source.insert(key.clone(), value.clone());
                }
            }
            if let Some(resources) = obj.get("resources").and_then(|v| v.as_object()) {
                for (key, value) in resources {
                    seed_source.insert(key.clone(), value.clone());
                }
            }
        }
        match_seed(&Value::Object(seed_source), &kernel.resource_tracks)
    }

    /// The live CURRENT value of `track_id` for this actor, from the single source
    /// of truth. No gps row -> character-DERIVED seed current -> kernel `initial`.
    /// Fail-closed None only when all three are absent.
    pub async fn load_resource_current(
        &self,
        session_id: &str,
        actor_id: &str,
        track_id: &str,
        kernel: &RuleKernel,
    ) -> Option<i32> {
        let path = format!("resources.{}.current", track_id);
        let row = sqlx::query("select value_json from generic_parameter_states where session_id=$1 and target_kind='actor' and target_id=$2 and parameter_path=$3")
            .bind(session_id).bind(actor_id).bind(&path)
            .fetch_optional(&self.pool).await.ok().flatten();
        if let Some(r) = row {
            if let Some(n) = jval_to_i32(&r.get::<Value, _>("value_json")) {
                return Some(n);
            }
        }
        let seeds = self.resource_seeds(session_id, actor_id, kernel).await;
        if let Some((Some(c), _)) = seeds.get(track_id) {
            return Some(*c);
        }
        kernel
            .resource_tracks
            .iter()
            .find(|t| {
                t.get("id")
                    .and_then(|x| x.as_str())
                    .map(|s| s.eq_ignore_ascii_case(track_id))
                    .unwrap_or(false)
            })
            .and_then(|t| t.get("initial"))
            .and_then(jval_to_i32)
    }

    /// The actor's cap (max) for `track_id`: char-derived max -> kernel static max.
    pub async fn resource_cap(
        &self,
        session_id: &str,
        actor_id: &str,
        track_id: &str,
        kernel: &RuleKernel,
    ) -> Option<i32> {
        let seeds = self.resource_seeds(session_id, actor_id, kernel).await;
        if let Some((_, Some(m))) = seeds.get(track_id) {
            return Some(*m);
        }
        kernel
            .resource_tracks
            .iter()
            .find(|t| {
                t.get("id")
                    .and_then(|x| x.as_str())
                    .map(|s| s.eq_ignore_ascii_case(track_id))
                    .unwrap_or(false)
            })
            .and_then(|t| t.get("max"))
            .and_then(jval_to_i32)
    }

    /// Write the CURRENT value of `track_id` to the single source of truth, capping
    /// by `cap` first. Returns the capped value actually stored.
    pub async fn write_resource_current(
        &self,
        session_id: &str,
        actor_id: &str,
        track_id: &str,
        value: i32,
        cap: Option<i32>,
        source_refs: &[SourceRef],
        visibility: Visibility,
        world_tick: i64,
    ) -> Result<i32> {
        let capped = match cap {
            Some(m) => value.min(m),
            None => value,
        };
        let path = format!("resources.{}.current", track_id);
        sqlx::query(r#"
            insert into generic_parameter_states
              (id, state_id, session_id, target_kind, target_id, parameter_path, value_json, visibility, source_refs, provisional_reason, world_tick, updated_at)
            values ($1,$2,$3,'actor',$4,$5,$6,$7,$8,null,$9,now())
            on conflict (session_id, target_kind, target_id, parameter_path) do update set
              value_json = excluded.value_json, visibility = excluded.visibility,
              source_refs = excluded.source_refs, world_tick = excluded.world_tick, updated_at = now()
        "#)
        .bind(Uuid::new_v4()).bind(format!("generic_state_{}", Uuid::new_v4().simple())).bind(session_id).bind(actor_id).bind(&path)
        .bind(json!(capped)).bind(visibility.as_str()).bind(serde_json::to_value(source_refs)?).bind(world_tick)
        .execute(&self.pool).await?;
        Ok(capped)
    }
}
