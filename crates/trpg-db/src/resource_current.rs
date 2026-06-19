//! Single source of truth for actor resource CURRENT values: generic_parameter_states,
//! path `resources.{track_id}.current`, target_kind=actor. One read/write path shared
//! by contest / mechanics / combat. Seed/cap come from the character's derived sheet
//! resources matched to kernel tracks (fail-closed), reusing trpg-model::match_seed.
use anyhow::Result;
use serde_json::{json, Value};
use sqlx::Row;
use std::hash::{Hash, Hasher};
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
        // Read the PRIOR current value before the upsert so the ResourceChanged
        // event_id can key on the actual (prior -> capped) transition (codex#2).
        let prior: Option<i32> = sqlx::query_scalar::<_, Value>(
            "select value_json from generic_parameter_states where session_id=$1 and target_kind='actor' and target_id=$2 and parameter_path=$3",
        )
        .bind(session_id)
        .bind(actor_id)
        .bind(&path)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
        .and_then(|v| jval_to_i32(&v));
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

        // eventlog write-through (additive / fail-soft, mirrors insert_dice_roll): after the
        // successful generic_parameter_states upsert, append a ResourceChanged domain event.
        // The append failing only warns — it MUST NOT change this function's Result (OFF==baseline).
        // The authoritative current value is the generic_parameter_states row written ABOVE; this
        // append is a PARALLEL additive ledger and the state write is NOT rolled back if it fails.
        //
        // # HONEST FRAMING (P6 revision — do NOT overclaim):
        //   ResourceChanged is a *best-effort current-state write-through breadcrumb keyed on the
        //   transition content*. It is NOT faithful append-only event-sourcing and NOT replay-folded
        //   by any projection — nothing reconstructs the current value by folding these events; the
        //   generic_parameter_states row is the source of truth. Because the event_id is a content
        //   hash of (prior, capped, cap, visibility), a repeated-IDENTICAL transition is SWALLOWED
        //   (`on conflict do nothing`), so the ledger is lossy w.r.t. how-many-times a value was
        //   re-asserted — it records distinct transition *shapes*, not an authoritative history of
        //   every write. DefaultHasher is NOT a stable cross-version content hash (its algorithm is
        //   unspecified and may change between std/toolchain versions), so these event_ids are NOT a
        //   durable cross-version identity — they only dedup within a single build's lifetime.
        //
        // # ResourceChanged data schema contract (pin — consumers depend on these keys):
        //   data = { "actor_id": <str>, "track_id": <str>, "value": <i32 capped/stored>,
        //            "cap": <i32|null>, "visibility": <str token> }
        //
        // # event_id uniqueness (codex#2): world_tick is constantly 0 (world_tick_hint()==0),
        // so it CANNOT discriminate. Instead key on a content hash of the actual transition
        // (prior_value, capped, cap, visibility). A genuinely different change (e.g. 65->99 vs
        // 99->65) hashes differently => two rows; a true replay of the SAME transition hashes
        // identically => `on conflict (event_id) do nothing` folds it (idempotent dedup, NOT a
        // replay-fold of state — see HONEST FRAMING above).
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        prior.hash(&mut hasher);
        capped.hash(&mut hasher);
        cap.hash(&mut hasher);
        visibility.as_str().hash(&mut hasher);
        let change_hash = hasher.finish();
        let event_id = format!(
            "de_resource_{}_{}_{}_{:016x}",
            session_id, actor_id, track_id, change_hash
        );
        let ev = trpg_model::DomainEvent::new(
            event_id,
            session_id.to_string(),
            String::new(),
            trpg_model::DomainEventKind::ResourceChanged,
            json!({
                "actor_id": actor_id,
                "track_id": track_id,
                "value": capped,
                "cap": cap,
                "visibility": visibility.as_str(),
            }),
        );
        if let Err(e) = self.append_domain_event(&ev).await {
            tracing::warn!(error=%e, "append ResourceChanged domain event failed (non-fatal)");
        }
        Ok(capped)
    }
}
