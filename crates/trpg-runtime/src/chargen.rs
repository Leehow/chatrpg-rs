//! F9: auto-generate a complete, playable starter character from the parsed
//! `CharacterOnboardingPack`, persist it (`save_character`), and bind it into a
//! session as `runtime_actor_parameters` so the turn/combat engine plays AS it.
//!
//! Before this, `create-character` only wrote a markdown draft (+ a sheet row)
//! and NEVER materialized into the actor-parameter table the turn reads, so a
//! play turn always fell back to a synthetic placeholder `pc.current`. This
//! module closes that gap: generate -> validate -> save_character -> upsert
//! actor params (source_kind = created_character_v1, not a placeholder).

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use trpg_db::Db;
use trpg_llm::{self, LlmClient};
use trpg_model::{
    ActorKind, CharacterOnboardingPack, CharacterSheet, CharacterTemplate, ValidationReport,
    Visibility,
};
use trpg_object::ObjectService;
use trpg_params::{RuntimeActorParameters, RuntimeParameterService};
use uuid::Uuid;

use crate::{validate_character_template_sheet, RuntimeEngine};

/// Outcome of `create_and_bind_character`, surfaced to the CLI for reporting.
pub struct CreatedCharacter {
    pub character_id: String,
    pub name: String,
    pub status: String,
    pub session_id: String,
    pub actor_id: String,
    pub sheet: Value,
    pub validation: ValidationReport,
}

/// Generate ONE complete starter character, grounded in the sheet template
/// (every field, its type + notes), the creation flow, option catalogs, and the
/// derived/resolution formulas. Returns a structured sheet object: one key per
/// `field_id` + a `name`, plus optional `stats`/`skills` integer maps for
/// numeric-attribute rulesets (cyberpunk / CoC) so dice can resolve.
pub async fn generate_starter_character(
    llm: &dyn LlmClient,
    pack: &CharacterOnboardingPack,
    concept: &str,
) -> Result<Value> {
    let t = &pack.sheet_template;
    let fields: Vec<Value> = t
        .fields
        .iter()
        .map(|f| {
            json!({
                "field_id": f.field_id,
                "title": f.title,
                "type": f.field_type,
                "required": f.required,
                "notes": f.notes,
            })
        })
        .collect();
    let steps: Vec<String> = pack
        .creation_flows
        .iter()
        .flat_map(|fl| fl.steps.iter().map(|s| s.title.clone()))
        .collect();
    let catalogs: Vec<String> = pack
        .option_catalogs
        .iter()
        .map(|c| c.title.clone())
        .collect();
    let formulas: Vec<String> = pack
        .derived_formula_pack
        .formulas
        .iter()
        .map(|f| format!("{} = {}", f.field_id, f.formula))
        .collect();

    let schema_hint = json!({
        "name": "<character name>",
        "<each field_id above>": "<a concrete, rules-appropriate value for that field>",
        "stats": "OPTIONAL object {ATTR: int} — include ONLY if this ruleset uses numeric attributes",
        "skills": "OPTIONAL object {Skill: int} — include ONLY if this ruleset uses numeric skills",
    });

    let sys = format!(
        "You are a TRPG character generator for ruleset '{rs}' (title: {title}).\n\
         Create ONE complete, playable starter character by filling EVERY field of the \
         character-sheet template below with a concrete, rules-appropriate value. Do not leave \
         any required field blank or generic — choose specific options (a concrete Anomaly, \
         Competency, Role, archetype, etc.) consistent with this game and internally coherent.\n\n\
         SHEET FIELDS (fill each by its field_id):\n{fields}\n\n\
         CREATION STEPS: {steps}\nOPTION CATALOGS: {catalogs}\n\
         DERIVED / RESOLUTION FORMULAS: {formulas}\n\n\
         If (and only if) this ruleset uses numeric attributes/skills, ALSO include `stats` and \
         `skills` objects mapping this ruleset's canonical attribute and skill names to integers \
         (drawn from the SHEET FIELDS above), so the engine can resolve dice. For purely narrative games, omit them.\n\n\
         Return STRICT JSON only — no markdown, no commentary — in exactly this shape:\n{schema}",
        rs = t.ruleset_id,
        title = t.title,
        fields = serde_json::to_string_pretty(&fields).unwrap_or_default(),
        steps = if steps.is_empty() { "(none parsed)".into() } else { steps.join(" -> ") },
        catalogs = if catalogs.is_empty() { "(none parsed)".into() } else { catalogs.join("; ") },
        formulas = if formulas.is_empty() { "(none parsed)".into() } else { formulas.join("; ") },
        schema = serde_json::to_string_pretty(&schema_hint).unwrap_or_default(),
    );
    let user_msg = if concept.trim().is_empty() {
        "Generate a fresh, interesting starter character. Make all the choices yourself."
            .to_string()
    } else {
        format!("Player concept / constraints to honor: {concept}")
    };

    let messages = vec![trpg_llm::system(sys), trpg_llm::user(user_msg)];
    let mut sheet = llm.complete_json(messages, 0.7).await?;
    if !sheet.is_object() {
        return Err(anyhow!("character generator did not return a JSON object"));
    }
    let needs_name = sheet
        .get("name")
        .and_then(Value::as_str)
        .map(|s| s.trim().is_empty())
        .unwrap_or(true);
    if needs_name {
        if let Some(obj) = sheet.as_object_mut() {
            obj.insert("name".into(), json!(format!("{} Agent", t.title)));
        }
    }
    Ok(sheet)
}

fn safe_id(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// Re-derive the combat/check-facing VIEW (`stats`/`skills`/`fields`) of a
/// `mechanical_profile` from the authoritative `sheet`, preserving every other key
/// (ruleset, source_quality, any enrichment). `sheet_json` stays the SINGLE source
/// of truth; mechanical_profile is a materialized view — call this after ANY sheet
/// mutation (growth / live recompute) so combat/contest read FRESH derived values
/// (proficiency, level-scaled bonuses, …) instead of the stale create-time snapshot.
pub fn refresh_mechanical_profile(profile: &mut Value, sheet: &Value) {
    let Some(p) = profile.as_object_mut() else {
        return;
    };
    p.insert(
        "stats".into(),
        sheet.get("stats").cloned().unwrap_or_else(|| json!({})),
    );
    p.insert(
        "skills".into(),
        sheet.get("skills").cloned().unwrap_or_else(|| json!({})),
    );
    let mut fields = sheet.clone();
    if let Some(o) = fields.as_object_mut() {
        o.remove("stats");
        o.remove("skills");
    }
    p.insert("fields".into(), fields);
}

/// §10.1 LIVE linkage (single impl): reload the actor's params, re-derive
/// `recompute=live` values from CURRENT base stats, refresh the mechanical-profile
/// view, and persist if anything changed. Idempotent; no-op when there is no stored
/// chargen_spec. Shared by `RuntimeEngine::refresh_actor_live_derived` and the
/// `ParameterNeedResolver` so the two stay byte-equivalent from one source.
pub async fn refresh_actor_live_derived_db(
    db: &Db,
    session_id: &str,
    actor_id: &str,
) -> Result<bool> {
    let service = RuntimeParameterService::new(db.clone());
    if let Some(mut p) = service.load_actor_parameters(session_id, actor_id).await? {
        if recompute_live_derived(&mut p.sheet_json) {
            refresh_mechanical_profile(&mut p.mechanical_profile, &p.sheet_json);
            service.upsert_actor_parameters(&p).await?;
            return Ok(true);
        }
    }
    Ok(false)
}

/// Build runtime actor parameters from a generated sheet so the engine plays AS
/// this character: combat reads `mechanical_profile.stats/skills`, and the GM
/// sees the narrative `fields` (anomaly, qualities, …) via the BP3 actor block.
pub fn materialize_actor_params(
    session_id: &str,
    ruleset_id: &str,
    actor_id: &str,
    template_id: &str,
    display_name: &str,
    sheet: &Value,
    world_tick: i64,
) -> RuntimeActorParameters {
    let mut mechanical_profile = json!({
        "ruleset": ruleset_id,
        "source_quality": "created_character_v1",
    });
    // mechanical_profile is a materialized VIEW of the sheet — derive stats/skills/fields.
    refresh_mechanical_profile(&mut mechanical_profile, sheet);
    let status_json = json!({
        "actor_id": actor_id,
        "conditions": [],
        "source_quality": "created_character_v1",
    });
    RuntimeActorParameters {
        actor_param_id: format!("actor_params.{}.{}", safe_id(session_id), safe_id(actor_id)),
        session_id: session_id.into(),
        actor_id: actor_id.into(),
        actor_kind: ActorKind::PlayerCharacter,
        ruleset_id: ruleset_id.into(),
        source_kind: "created_character_v1".into(),
        template_id: Some(template_id.into()),
        display_name: Some(display_name.into()),
        sheet_json: sheet.clone(),
        mechanical_profile,
        status_json,
        visibility: Visibility::GmOnly,
        created_at_tick: Some(world_tick),
        updated_at_tick: Some(world_tick),
    }
}

/// Deterministically compute the ruleset's source-backed chargen DERIVED/HYBRID
/// values from the player/LLM-filled INPUTs, and write them back into the sheet
/// (OVERWRITING any LLM guesses). Routes each value by `role` to stats/skills/
/// resources and stores `derived_spec` (value+breakdown+status+recompute) so a
/// later runtime can re-evaluate live values. Graceful: an empty/absent spec is
/// a no-op; unresolved values stay provisional, never fabricated.
/// Build the evaluator input context from an actor sheet: stats + skills
/// flattened to top level, plus `player.*` and any top-level numeric fields.
fn build_chargen_inputs(sheet: &Value) -> serde_json::Map<String, Value> {
    let mut inputs = serde_json::Map::new();
    for bucket in ["stats", "skills"] {
        if let Some(m) = sheet.get(bucket).and_then(|v| v.as_object()) {
            for (k, v) in m {
                inputs.insert(k.clone(), v.clone());
            }
        }
    }
    if let Some(p) = sheet.get("player") {
        inputs.insert("player".into(), p.clone());
    }
    // Forward the whole tracks subtree (structured): aggregation fns (sum_tracks/
    // max_tracks) read it by kind, and ctx flattening also exposes {{tracks.<id>.value}}.
    if let Some(t) = sheet.get("tracks") {
        inputs.insert("tracks".into(), t.clone());
    }
    if let Some(o) = sheet.as_object() {
        // Top-level numeric fields are a fallback ONLY. Skip any that case-insensitively
        // duplicate a stats/skills key — otherwise a stale lowercase top-level copy
        // (e.g. `con`) shadows the authoritative `stats.CON` and freezes live derivation.
        let have: std::collections::HashSet<String> =
            inputs.keys().map(|k| k.to_ascii_lowercase()).collect();
        for (k, v) in o {
            // numbers AND categorical strings (e.g. class/race) — the latter let a
            // formula key a table on a player CHOICE (class -> hit die).
            if (v.is_number() || v.is_string()) && !have.contains(&k.to_ascii_lowercase()) {
                inputs.insert(k.clone(), v.clone());
            }
        }
    }
    inputs
}

/// Route evaluated results into the sheet's stats/skills/resources buckets,
/// overwriting any case-insensitive existing key in place (no stale duplicates).
fn route_chargen_results(
    report: &trpg_formula::ChargenReport,
    obj: &mut serde_json::Map<String, Value>,
) {
    for r in &report.values {
        let Some(val) = &r.value else { continue };
        let bucket = match r.role.as_str() {
            "attribute" => "stats",
            "skill" => "skills",
            "resource" | "resource_max" => "resources",
            _ => "derived_values",
        };
        obj.entry(bucket.to_string()).or_insert_with(|| json!({}));
        if let Some(b) = obj.get_mut(bucket).and_then(|v| v.as_object_mut()) {
            let target = b
                .keys()
                .find(|k| k.eq_ignore_ascii_case(&r.id))
                .cloned()
                .unwrap_or_else(|| r.id.clone());
            b.insert(target, val.clone());
        }
    }
}

/// One `derived_spec` display entry (value/breakdown/status) for an EvalResult.
fn derived_spec_entry(r: &trpg_formula::EvalResult) -> Value {
    json!({
        "id": r.id, "role": r.role, "recompute": r.recompute, "result_type": r.result_type,
        "value": r.value, "breakdown": r.breakdown, "status": r.status, "unresolved": r.unresolved, "source_ref": r.source_ref
    })
}

pub fn apply_chargen_formulas(records: &[Value], sheet: &mut Value) -> trpg_formula::ChargenReport {
    let inputs = build_chargen_inputs(sheet);
    let report = trpg_formula::evaluate_chargen(records, &inputs);
    let Some(obj) = sheet.as_object_mut() else {
        return report;
    };
    route_chargen_results(&report, obj);
    obj.insert(
        "derived_spec".into(),
        json!(report
            .values
            .iter()
            .map(derived_spec_entry)
            .collect::<Vec<_>>()),
    );
    // Store the full §4 spec (with expr/recompute/lookup_tables) so the runtime
    // can re-derive LIVE values later without re-loading the template (§10.1).
    obj.insert("chargen_spec".into(), Value::Array(records.to_vec()));
    report
}

/// §10.1 LIVE linkage: re-evaluate the `recompute != "once"` derived values from
/// the actor's CURRENT base stats and write them back into the sheet. Idempotent
/// — call when base parameters may have changed (e.g. at the start of each turn).
/// `recompute=="once"` records keep their chargen snapshot. No-op (false) when no
/// `chargen_spec` is stored. Returns true if any routed value actually changed.
pub fn recompute_live_derived(sheet: &mut Value) -> bool {
    let Some(spec) = sheet
        .get("chargen_spec")
        .and_then(|v| v.as_array())
        .cloned()
    else {
        return false;
    };
    let live: Vec<Value> = spec
        .into_iter()
        .filter(|r| {
            r.get("recompute")
                .and_then(|v| v.as_str())
                .map(|s| s != "once")
                .unwrap_or(true)
        })
        .collect();
    if live.is_empty() {
        return false;
    }
    let inputs = build_chargen_inputs(sheet);
    let report = trpg_formula::evaluate_chargen(&live, &inputs);
    let Some(obj) = sheet.as_object_mut() else {
        return false;
    };
    let snap = |o: &serde_json::Map<String, Value>| json!({"stats": o.get("stats"), "skills": o.get("skills"), "resources": o.get("resources")});
    let before = snap(obj);
    route_chargen_results(&report, obj);
    // Keep the `derived_spec` display VIEW fresh for the live records too (a
    // materialized view of sheet_json, like mechanical_profile); `once` entries
    // keep their create-time snapshot. Update by id; append a newly-resolved one.
    if let Some(arr) = obj.get_mut("derived_spec").and_then(|v| v.as_array_mut()) {
        for r in &report.values {
            let entry = derived_spec_entry(r);
            match arr
                .iter_mut()
                .find(|e| e.get("id").and_then(|x| x.as_str()) == Some(r.id.as_str()))
            {
                Some(slot) => *slot = entry,
                None => arr.push(entry),
            }
        }
    }
    before != snap(obj)
}

/// Generic growth mutate primitive (pure, DB-free → unit-testable). Sets or adds a
/// value to a TRACK (`bucket="tracks"`: a `{value,kind,category}` object, created on
/// first set) or a BASE stat/skill (`bucket="stats"`/`"skills"`: a bare number).
/// `op` = "set" | "add". The engine only MOVES the value — it does NOT enforce any
/// ruleset advancement rule (XP→level, IP cost, prereqs); that is GM/player-driven.
pub fn apply_track_change_to_sheet(
    sheet: &mut Value,
    bucket: &str,
    id: &str,
    op: &str,
    amount: f64,
    text: Option<&str>,
    kind: Option<&str>,
    category: Option<&str>,
) -> Result<(), String> {
    let obj = sheet.as_object_mut().ok_or("sheet is not an object")?;
    // "field": a top-level categorical INPUT (race / ancestry / class choice). String or number.
    if bucket == "field" {
        obj.insert(
            id.to_string(),
            match text {
                Some(t) => json!(t),
                None => json!(amount),
            },
        );
        return Ok(());
    }
    let b = obj.entry(bucket.to_string()).or_insert_with(|| json!({}));
    let bm = b.as_object_mut().ok_or("bucket is not an object")?;
    if bucket == "tracks" {
        let entry = bm
            .entry(id.to_string())
            .or_insert_with(|| json!({"value": 0, "kind": kind.unwrap_or("counter")}));
        let new_val = match text {
            Some(t) => json!(t),
            None => {
                let cur = entry.get("value").and_then(|v| v.as_f64()).unwrap_or(0.0);
                json!(if op == "add" { cur + amount } else { amount })
            }
        };
        if let Some(em) = entry.as_object_mut() {
            em.insert("value".into(), new_val);
            if let Some(k) = kind {
                em.insert("kind".into(), json!(k));
            }
            if let Some(c) = category {
                em.insert("category".into(), json!(c));
            }
        }
    } else {
        let v = match text {
            Some(t) => json!(t),
            None => {
                let cur = bm.get(id).and_then(|v| v.as_f64()).unwrap_or(0.0);
                json!(if op == "add" { cur + amount } else { amount })
            }
        };
        bm.insert(id.to_string(), v);
    }
    Ok(())
}

/// Convert a template's compiled `derived_values` into §4 value records. Only
/// MACHINE-evaluable rows (have `expr` or `attr_derived`) are emitted; prose-only
/// rows are skipped (they would otherwise evaluate to 0). `field_id` -> `id`.
pub fn chargen_spec_from_template(t: &CharacterTemplate) -> Vec<Value> {
    let mut out = Vec::new();
    for d in &t.derived_values {
        let machine = d
            .expr
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
            || d.attr_derived
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
        if !machine {
            continue;
        }
        let mut v = serde_json::to_value(d).unwrap_or(Value::Null);
        if let Some(obj) = v.as_object_mut() {
            if let Some(fid) = obj.remove("field_id") {
                obj.insert("id".into(), fid);
            }
        }
        out.push(v);
    }
    out
}

/// Layer an override list over a base spec: a record whose `id` matches replaces
/// the base record; new ids are appended.
pub fn merge_override(base: Vec<Value>, overrides: Vec<Value>) -> Vec<Value> {
    let idof = |v: &Value| {
        v.get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase()
    };
    let mut out = base;
    for o in overrides {
        let oid = idof(&o);
        if oid.is_empty() {
            continue;
        }
        if let Some(slot) = out.iter_mut().find(|b| idof(b) == oid) {
            *slot = o;
        } else {
            out.push(o);
        }
    }
    out
}

/// §4 chargen spec for a ruleset: compiled template `derived_values` (default
/// source), with an optional hand-authored override layered per-`id`. Override
/// file precedence: `{ruleset}.chargen.override.json`, then legacy
/// `{ruleset}.chargen.json`. Graceful: no override → just the compiled spec.
/// Track-keyed derived overrides go in this same `{ruleset}.chargen.override.json`;
/// pin critical track derived here to defend against compiler re-parse variance
/// (spec decision 4). Auto no-regress-on-reparse is deferred (separate work).
pub fn load_chargen_spec(ruleset_id: &str, template: &CharacterTemplate) -> Vec<Value> {
    let base = chargen_spec_from_template(template);
    let dir = std::env::var("TRPG_DATA_DIR").unwrap_or_else(|_| "data".into());
    let read = |name: String| -> Vec<Value> {
        let p = std::path::Path::new(&dir)
            .join("parsed")
            .join("characters")
            .join(name);
        std::fs::read_to_string(&p)
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<Value>>(&s).ok())
            .unwrap_or_default()
    };
    let mut overrides = read(format!("{}.chargen.override.json", safe_id(ruleset_id)));
    if overrides.is_empty() {
        overrides = read(format!("{}.chargen.json", safe_id(ruleset_id)));
    }
    merge_override(base, overrides)
}

impl RuntimeEngine {
    /// F9 entry point: generate a complete starter character from the parsed
    /// onboarding pack, persist it, and bind it into a session as the actor the
    /// turn engine plays AS. If `session_id` is None a fresh session is started.
    pub async fn create_and_bind_character(
        &self,
        llm: &dyn LlmClient,
        ruleset_id: &str,
        module_id: Option<&str>,
        session_id: Option<&str>,
        actor_id: &str,
        concept: &str,
    ) -> Result<CreatedCharacter> {
        let pack = self
            .db
            .load_character_onboarding_pack(ruleset_id)
            .await?
            .ok_or_else(|| {
                anyhow!("no character onboarding pack for '{ruleset_id}'; parse the ruleset first")
            })?;
        let template = pack.sheet_template.clone();

        let mut sheet = generate_starter_character(llm, &pack, concept).await?;
        // Deterministic, source-backed derived values OVERWRITE the LLM's guesses
        // (HP/SAN/dodge/DB computed from the input stats per the ruleset's chargen
        // spec — no more LLM-invented parameters). No-op if no spec is present.
        let chargen_spec = load_chargen_spec(ruleset_id, &template);
        let _chargen_report = apply_chargen_formulas(&chargen_spec, &mut sheet);
        let name = sheet
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Unnamed Character")
            .to_string();
        let validation = validate_character_template_sheet(&template, &sheet);
        let status = if validation.status == "ok" {
            "ready"
        } else {
            "draft_needs_rules_source"
        };

        let character = CharacterSheet {
            character_id: format!("character_{}", Uuid::new_v4().simple()),
            ruleset_id: ruleset_id.to_string(),
            template_id: template.template_id.clone(),
            name: name.clone(),
            sheet: sheet.clone(),
            validation_report: validation.clone(),
        };
        self.db.save_character(&character, status).await?;

        let session_id = match session_id {
            Some(s) => {
                self.ensure_session_initialized(s, ruleset_id, module_id)
                    .await?;
                s.to_string()
            }
            None => self.start_session(ruleset_id, module_id).await?,
        };
        let world_tick = self
            .current_world_time(&session_id)
            .await
            .map(|w| w.world_tick)
            .unwrap_or(0);
        let params = materialize_actor_params(
            &session_id,
            ruleset_id,
            actor_id,
            &template.template_id,
            &name,
            &sheet,
            world_tick,
        );
        RuntimeParameterService::new(self.db.clone())
            .upsert_actor_parameters(&params)
            .await?;
        ObjectService::new(self.db.clone())
            .seed_actor_inventory_from_sheet(&session_id, ruleset_id, actor_id, &sheet, world_tick)
            .await?;

        Ok(CreatedCharacter {
            character_id: character.character_id,
            name,
            status: status.to_string(),
            session_id,
            actor_id: actor_id.to_string(),
            sheet,
            validation,
        })
    }
}

#[cfg(test)]
mod chargen_formula_tests {
    use super::*;
    use serde_json::json;

    fn coc_spec() -> Vec<Value> {
        serde_json::from_str(include_str!(
            "../../../data/parsed/characters/call_of_cthulhu_7e.chargen.json"
        ))
        .expect("CoC chargen spec parses")
    }

    #[test]
    fn coc_chargen_computes_derived_overwriting_llm_guesses() {
        // 林景修-like inputs; skills.Dodge is an LLM GUESS (99) that must be replaced.
        let mut sheet = json!({"stats": {"STR":55,"CON":60,"DEX":70,"POW":65,"SIZ":65}, "skills": {"Dodge": 99}});
        let report = apply_chargen_formulas(&coc_spec(), &mut sheet);
        assert_eq!(
            sheet["resources"]["hp_max"],
            json!(12),
            "floor((60+65)/10)=12"
        );
        assert_eq!(sheet["resources"]["mp_max"], json!(13), "floor(65/5)=13");
        assert_eq!(
            sheet["resources"]["sanity"],
            json!(65),
            "starting SAN = POW = 65 (not the kernel's 50)"
        );
        assert_eq!(
            sheet["skills"]["Dodge"],
            json!(35),
            "Dodge=floor(70/2)=35, OVERWRITES the LLM guess 99 in place"
        );
        assert!(
            sheet["skills"].get("dodge").is_none(),
            "no lowercase duplicate key"
        );
        assert_eq!(
            sheet["stats"]["damage_bonus"],
            json!(0),
            "STR55+SIZ65=120 -> DB 0"
        );
        assert_eq!(sheet["stats"]["build"], json!(0));
        assert!(report.gaps.is_empty());
        assert!(
            report.values.iter().all(|r| r.status == "provisional"),
            "pages unverified -> provisional (guardrail 4)"
        );
    }

    #[test]
    fn coc_chargen_preserves_valid_player_allocated_dodge_total() {
        // Dodge is hybrid: DEX/2 is the floor, but a legal player-assigned total
        // above that floor must not be erased just because allocation breakdowns
        // were not explicitly provided by the starter-character generator.
        let mut sheet = json!({"stats": {"STR":50,"CON":60,"DEX":65,"POW":60,"SIZ":55}, "skills": {"Dodge": 42}});
        apply_chargen_formulas(&coc_spec(), &mut sheet);
        assert_eq!(
            sheet["skills"]["Dodge"],
            json!(42),
            "valid allocated Dodge total preserved"
        );
        assert!(
            sheet["skills"].get("dodge").is_none(),
            "no lowercase duplicate key"
        );
        let dodge = sheet["derived_spec"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry.get("id").and_then(|v| v.as_str()) == Some("dodge"))
            .expect("dodge derived_spec");
        assert_eq!(dodge["value"], json!(42));
        assert_eq!(dodge["breakdown"]["preserved_player_total"], json!(42));
    }

    #[test]
    fn coc_chargen_missing_input_degrades_not_fabricates() {
        let mut sheet = json!({"stats": {"DEX":70}}); // CON/SIZ/POW/STR missing
        apply_chargen_formulas(&coc_spec(), &mut sheet);
        assert!(
            sheet
                .get("resources")
                .and_then(|r| r.get("hp_max"))
                .is_none(),
            "no fabricated hp_max without CON/SIZ"
        );
        assert_eq!(
            sheet["skills"]["dodge"],
            json!(35),
            "dodge.attr_derived needs only DEX -> 35"
        );
    }

    #[test]
    fn live_recompute_updates_derived_on_base_change() {
        // §10.1: a recompute=live derived value re-derives when its base changes in play.
        let spec = vec![
            json!({"id":"hp_max","role":"resource_max","input_kind":"derived","recompute":"live","result_type":"int","expr":"floor(({{con}}+{{siz}})/10)"}),
        ];
        let mut sheet = json!({"stats":{"CON":60,"SIZ":60}});
        apply_chargen_formulas(&spec, &mut sheet);
        assert_eq!(
            sheet["resources"]["hp_max"],
            json!(12),
            "chargen: floor((60+60)/10)=12"
        );
        assert!(
            sheet.get("chargen_spec").is_some(),
            "full spec stored for live recompute"
        );
        sheet["stats"]["CON"] = json!(40); // CON drained mid-play
        assert!(recompute_live_derived(&mut sheet), "hp_max changed");
        assert_eq!(
            sheet["resources"]["hp_max"],
            json!(10),
            "live: floor((40+60)/10)=10 after drain"
        );
    }

    #[test]
    fn tracks_drive_derived_and_live_recompute() {
        // D&D multiclass: total level = sum of class-level tracks; proficiency keys it.
        let spec = vec![
            json!({"id":"total_level","role":"attribute","input_kind":"derived","recompute":"live","expr":"sum_tracks(class_level)"}),
            json!({"id":"proficiency","role":"attribute","input_kind":"derived","recompute":"live","result_type":"int",
                   "expr":"lookup(prof_by_level,{{derived.total_level}})","lookup_tables":{"prof_by_level":{"ranges":[
                       {"min":1,"max":4,"value":2},{"min":5,"max":8,"value":3},{"min":9,"max":12,"value":4}]}}}),
        ];
        let mut sheet = json!({"tracks":{"fighter":{"value":4,"kind":"class_level"},"rogue":{"value":1,"kind":"class_level"}}});
        apply_chargen_formulas(&spec, &mut sheet);
        assert_eq!(sheet["stats"]["total_level"], json!(5), "4+1 multiclass");
        assert_eq!(sheet["stats"]["proficiency"], json!(3), "total 5 -> +3");
        // Multiclass advances rogue 1 -> 5 (total 9); live recompute lifts proficiency to +4.
        sheet["tracks"]["rogue"]["value"] = json!(5);
        assert!(
            recompute_live_derived(&mut sheet),
            "track change re-derives"
        );
        assert_eq!(sheet["stats"]["total_level"], json!(9));
        assert_eq!(
            sheet["stats"]["proficiency"],
            json!(4),
            "total 9 -> +4 (live)"
        );
    }

    #[test]
    fn dnd_class_keyed_hp_is_engine_computed_from_player_choice() {
        // The REAL D&D shape: HP max keys a STRING table on the player's `class`
        // CHOICE, plus the CON modifier. The player fills only inputs (class, CON);
        // the engine computes HP deterministically. CON 13 -> mod +1; wizard d6 -> 6;
        // HP = 6 + 1 = 7 — NOT an LLM guess.
        let spec = vec![
            json!({"id":"constitution_modifier","role":"attribute","input_kind":"derived","recompute":"live","result_type":"int",
                   "expr":"floor(({{constitution_score}}-10)/2)"}),
            json!({"id":"hit_points_max","role":"resource_max","input_kind":"hybrid","recompute":"live","result_type":"int",
                   "expr":"lookup(level1_hit_die_max_by_class,{{class}})+{{constitution_modifier}}","min":1,
                   "lookup_tables":{"level1_hit_die_max_by_class":{"ranges":[
                       {"min":"barbarian","max":"barbarian","value":12},
                       {"min":"fighter","max":"fighter","value":10},
                       {"min":"sorcerer","max":"sorcerer","value":6},
                       {"min":"wizard","max":"wizard","value":6}]}}}),
        ];
        // Sheet as generate_starter_character now produces it: `class` is a clean
        // top-level player CHOICE; the engine computes the rest.
        let mut sheet = json!({"class":"Wizard","stats":{"constitution_score":13}});
        let report = apply_chargen_formulas(&spec, &mut sheet);
        assert_eq!(
            sheet["stats"]["constitution_modifier"],
            json!(1),
            "CON 13 -> +1"
        );
        assert_eq!(
            sheet["resources"]["hit_points_max"],
            json!(7),
            "wizard d6(6) + CON mod(1) = 7, engine-computed (case-insensitive)"
        );
        assert!(report.gaps.is_empty());
        // §10.1 live linkage: raising CON to 16 (+3) re-derives HP with NO LLM.
        sheet["stats"]["constitution_score"] = json!(16);
        assert!(
            recompute_live_derived(&mut sheet),
            "HP re-derives on CON change"
        );
        assert_eq!(
            sheet["resources"]["hit_points_max"],
            json!(9),
            "live: wizard d6(6) + CON mod(3) = 9"
        );
        // A different class CHOICE keys a different die: barbarian d12 + 3 = 15.
        sheet["class"] = json!("barbarian");
        assert!(recompute_live_derived(&mut sheet));
        assert_eq!(
            sheet["resources"]["hit_points_max"],
            json!(15),
            "barbarian d12(12) + CON mod(3) = 15"
        );
    }

    #[test]
    fn apply_track_change_sets_adds_and_creates() {
        let mut sheet = json!({"tracks":{"fighter":{"value":4,"kind":"class_level"}}});
        apply_track_change_to_sheet(
            &mut sheet, "tracks", "fighter", "add", 1.0, None, None, None,
        )
        .unwrap();
        assert_eq!(
            sheet["tracks"]["fighter"]["value"],
            json!(5.0),
            "add to existing"
        );
        apply_track_change_to_sheet(
            &mut sheet,
            "tracks",
            "rogue",
            "set",
            1.0,
            None,
            Some("class_level"),
            None,
        )
        .unwrap();
        assert_eq!(
            sheet["tracks"]["rogue"]["value"],
            json!(1.0),
            "created on first set"
        );
        assert_eq!(sheet["tracks"]["rogue"]["kind"], json!("class_level"));
        // base stat (Sword World per-session stat growth) goes through the same primitive.
        let mut s2 = json!({"stats":{"dexterity":12}});
        apply_track_change_to_sheet(&mut s2, "stats", "dexterity", "add", 1.0, None, None, None)
            .unwrap();
        assert_eq!(s2["stats"]["dexterity"], json!(13.0));
    }

    #[test]
    fn accumulator_base_value_not_clobbered_by_live_recompute() {
        // Spec §6.7: path-dependent HP is a BASE accumulator (GM adds on level-up),
        // NEVER a level-keyed live derived. The live pass must leave it untouched.
        let spec = vec![
            json!({"id":"total_level","role":"attribute","input_kind":"derived","recompute":"live","expr":"sum_tracks(class_level)"}),
        ];
        let mut sheet = json!({"tracks":{"fighter":{"value":2,"kind":"class_level"}}, "stats":{"hit_points":17}});
        apply_chargen_formulas(&spec, &mut sheet);
        apply_track_change_to_sheet(
            &mut sheet, "tracks", "fighter", "add", 1.0, None, None, None,
        )
        .unwrap();
        apply_track_change_to_sheet(
            &mut sheet,
            "stats",
            "hit_points",
            "add",
            7.0,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(recompute_live_derived(&mut sheet));
        assert_eq!(
            sheet["stats"]["total_level"],
            json!(3),
            "derived re-derives"
        );
        assert_eq!(
            sheet["stats"]["hit_points"],
            json!(24.0),
            "accumulated HP NOT recomputed/clobbered"
        );
    }

    #[test]
    fn live_recompute_refreshes_derived_spec_view() {
        // derived_spec is a display VIEW; live recompute must refresh it (not leave
        // the create-time snapshot), consistent with the mechanical_profile fix.
        let spec = vec![
            json!({"id":"hp_max","role":"resource_max","input_kind":"derived","recompute":"live","expr":"floor(({{con}}+{{siz}})/10)"}),
        ];
        let mut sheet = json!({"stats":{"CON":60,"SIZ":60}});
        apply_chargen_formulas(&spec, &mut sheet);
        let val = |s: &Value| {
            s["derived_spec"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["id"] == "hp_max")
                .unwrap()["value"]
                .clone()
        };
        assert_eq!(val(&sheet), json!(12), "create: floor((60+60)/10)=12");
        sheet["stats"]["CON"] = json!(40);
        recompute_live_derived(&mut sheet);
        assert_eq!(
            val(&sheet),
            json!(10),
            "derived_spec view refreshed live (not stale 12)"
        );
    }

    #[test]
    fn refresh_mechanical_profile_reprojects_from_sheet_preserving_metadata() {
        // mechanical_profile is a materialized VIEW of sheet_json: after growth the
        // re-projection must surface the new derived (combat reads mech.stats/skills),
        // while preserving non-projection keys (ruleset, source_quality).
        let mut profile = json!({"ruleset":"dnd5e","source_quality":"created_character_v1",
            "stats":{"STR":10},"skills":{}});
        let sheet = json!({"stats":{"STR":10,"proficiency_bonus":4,"total_level":9},
            "skills":{"Stealth":5}, "tracks":{"fighter":{"value":9,"kind":"class_level"}}});
        refresh_mechanical_profile(&mut profile, &sheet);
        assert_eq!(
            profile["stats"]["proficiency_bonus"],
            json!(4),
            "grown derived now visible to combat"
        );
        assert_eq!(profile["stats"]["total_level"], json!(9));
        assert_eq!(profile["skills"]["Stealth"], json!(5));
        assert_eq!(
            profile["fields"]["tracks"]["fighter"]["value"],
            json!(9),
            "tracks re-projected into fields"
        );
        assert_eq!(profile["ruleset"], json!("dnd5e"), "metadata preserved");
        assert_eq!(
            profile["source_quality"],
            json!("created_character_v1"),
            "metadata preserved"
        );
        assert!(
            profile["fields"].get("stats").is_none(),
            "fields excludes stats (no duplication)"
        );
    }

    #[test]
    fn live_recompute_skips_once_snapshots() {
        // recompute=once keeps its chargen snapshot even if the base changes.
        let spec = vec![
            json!({"id":"sanity","role":"resource","input_kind":"derived","recompute":"once","result_type":"int","expr":"{{pow}}"}),
        ];
        let mut sheet = json!({"stats":{"POW":50}});
        apply_chargen_formulas(&spec, &mut sheet);
        assert_eq!(sheet["resources"]["sanity"], json!(50));
        sheet["stats"]["POW"] = json!(70);
        assert!(
            !recompute_live_derived(&mut sheet),
            "once record not recomputed"
        );
        assert_eq!(
            sheet["resources"]["sanity"],
            json!(50),
            "once snapshot unchanged despite POW change"
        );
    }

    #[test]
    fn cyberpunk_role_rank_track_feeds_derived() {
        // Initiative bonus reads a rank track; raising the rank re-derives live.
        let spec = vec![
            json!({"id":"initiative","role":"attribute","input_kind":"derived","recompute":"live","result_type":"int",
                               "expr":"{{reflexes}}+{{tracks.solo.value}}"}),
        ];
        let mut sheet = json!({"stats":{"reflexes":8},"tracks":{"solo":{"value":2,"kind":"rank"}}});
        apply_chargen_formulas(&spec, &mut sheet);
        assert_eq!(sheet["stats"]["initiative"], json!(10), "8 + rank 2");
        apply_track_change_to_sheet(&mut sheet, "tracks", "solo", "add", 1.0, None, None, None)
            .unwrap();
        assert!(recompute_live_derived(&mut sheet));
        assert_eq!(
            sheet["stats"]["initiative"],
            json!(11),
            "rank 3 -> 11 (live)"
        );
    }

    #[test]
    fn categorical_field_set_drives_race_keyed_derived() {
        // A play-time categorical CHOICE (draconic ancestry) keys a derived value;
        // setting it via the "field" bucket + a string value makes it recompute.
        let spec = vec![
            json!({"id":"breath_damage_type","role":"attribute","input_kind":"derived","recompute":"live","result_type":"dice_or_int",
            "expr":"lookup(ancestry_dmg,{{draconic_ancestry}})","lookup_tables":{"ancestry_dmg":{"ranges":[
                {"key":"red","value":"fire"},{"key":"blue","value":"lightning"}]}}}),
        ];
        let mut sheet = json!({});
        apply_track_change_to_sheet(
            &mut sheet,
            "field",
            "draconic_ancestry",
            "set",
            0.0,
            Some("red"),
            None,
            None,
        )
        .unwrap();
        apply_chargen_formulas(&spec, &mut sheet);
        assert_eq!(
            sheet["stats"]["breath_damage_type"],
            json!("fire"),
            "red ancestry -> fire"
        );
        // switch ancestry to blue -> live re-derive to lightning
        apply_track_change_to_sheet(
            &mut sheet,
            "field",
            "draconic_ancestry",
            "set",
            0.0,
            Some("blue"),
            None,
            None,
        )
        .unwrap();
        assert!(
            recompute_live_derived(&mut sheet),
            "categorical change re-derives"
        );
        assert_eq!(
            sheet["stats"]["breath_damage_type"],
            json!("lightning"),
            "blue ancestry -> lightning (live)"
        );
    }

    #[test]
    fn fate_servant_ordinal_grade_resolves_via_categorical_lookup() {
        // Fate letter-grade attribute "A" -> number via a per-ruleset rank table
        // (reuses categorical lookup); the number then feeds a derived. Class-keyed
        // coefficient is the same categorical pattern as D&D class HP.
        let spec = vec![
            json!({"id":"strength","role":"attribute","input_kind":"derived","result_type":"int",
                   "expr":"lookup(rank_value,{{strength_rank}})","lookup_tables":{"rank_value":{"ranges":[
                       {"min":"a","max":"a","value":14},{"min":"ex","max":"ex","value":18}]}}}),
            json!({"id":"hp_max","role":"resource_max","input_kind":"derived","result_type":"int",
                   "expr":"{{derived.strength}}*lookup(class_coef,{{class}})","lookup_tables":{"class_coef":{"ranges":[
                       {"min":"saber","max":"saber","value":10},{"min":"caster","max":"caster","value":7}]}}}),
        ];
        let mut sheet = json!({"class":"Saber","strength_rank":"A"});
        apply_chargen_formulas(&spec, &mut sheet);
        assert_eq!(
            sheet["stats"]["strength"],
            json!(14),
            "rank A -> 14 (case-insensitive)"
        );
        assert_eq!(
            sheet["resources"]["hp_max"],
            json!(140),
            "STR 14 * Saber coef 10"
        );
    }
}

#[cfg(test)]
mod compiler_runtime_tests {
    use super::*;
    use serde_json::json;
    use trpg_model::{CharacterTemplate, DerivedValue};

    fn dv(field_id: &str, expr: &str) -> DerivedValue {
        DerivedValue {
            field_id: field_id.into(),
            formula: expr.into(),
            expr: Some(expr.into()),
            role: Some("resource_max".into()),
            input_kind: Some("derived".into()),
            ..Default::default()
        }
    }

    #[test]
    fn spec_from_template_maps_field_id_to_id_and_skips_prose_only() {
        let mut t = CharacterTemplate::default();
        t.derived_values = vec![
            dv("hp_max", "floor(({{con}}+{{siz}})/10)"),
            DerivedValue {
                field_id: "luck".into(),
                formula: "3D6 x 5".into(),
                ..Default::default()
            },
        ];
        let spec = chargen_spec_from_template(&t);
        assert_eq!(spec.len(), 1, "prose-only luck skipped");
        assert_eq!(spec[0]["id"], json!("hp_max"));
        assert_eq!(spec[0]["expr"], json!("floor(({{con}}+{{siz}})/10)"));
    }

    #[test]
    fn override_replaces_by_id_keeps_others() {
        let base = vec![
            json!({"id":"hp_max","expr":"floor(({{con}}+{{siz}})/10)"}),
            json!({"id":"sanity","expr":"{{pow}}"}),
        ];
        let ovr = vec![json!({"id":"sanity","expr":"{{pow}}","max":99,"status":"source_backed"})];
        let merged = merge_override(base, ovr);
        assert_eq!(merged.len(), 2);
        let san = merged.iter().find(|r| r["id"] == "sanity").unwrap();
        assert_eq!(san["max"], json!(99), "override won");
        assert!(
            merged.iter().any(|r| r["id"] == "hp_max"),
            "non-overridden kept"
        );
    }

    #[test]
    fn authored_track_derived_override_wins_over_compiled() {
        // Decision 4: a hand-authored track-keyed derived (override) beats the
        // flaky-compiler one by id. Defends against re-parse extraction variance.
        let compiled =
            vec![json!({"id":"total_level","expr":"{{level_guess}}","status":"provisional"})];
        let authored = vec![
            json!({"id":"total_level","expr":"sum_tracks(class_level)","status":"source_backed"}),
        ];
        let merged = merge_override(compiled, authored);
        let r = merged.iter().find(|r| r["id"] == "total_level").unwrap();
        assert_eq!(
            r["expr"],
            json!("sum_tracks(class_level)"),
            "authored override replaced compiled"
        );
        assert_eq!(r["status"], json!("source_backed"));
    }
}
