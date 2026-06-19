//! Mechanics-catalog compiler — the rule agent's focused THIRD pass: one
//! single-purpose LLM loop whose only output is the kernel's
//! `mechanics_catalog` array (open enumeration over the whole rulebook).
//! Fail-closed = "never invent": no submit / LLM error / nothing coercible
//! -> the kernel is left byte-for-byte untouched and a gap note is returned.
//!
//! Design: docs/superpowers/specs/2026-06-10-rule-aware-gm-design.md (§4).

use super::chargen_compile;
use super::tools;
use super::units::Unit;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use trpg_llm::LlmClient;
use trpg_model::RuleKernel;

/// Compile context — mirrors `chargen_compile::CompileCtx`.
pub struct MechCompileCtx<'a> {
    pub units: &'a [Unit],
    /// Merged rulebook `.md` full text (caller reads
    /// `markdown/rulebooks/{source_id}.md` directly — same sidecar fix as
    /// chargen; never read the `layout_sidecar_path` metadata).
    pub sidecar_text: Option<String>,
    /// Hint pages (may be empty — mechanics have no prior locator pages; the
    /// model surveys the whole book via get_toc/search).
    pub located_pages: String,
    /// `crate::skill_ids` output: option-catalog skills that may not appear in
    /// the sheet-schema fields; legal `tested_parameter` supplement.
    pub skill_names: Vec<String>,
}

const MECH_SYS: &str = r#"You COMPILE a tabletop RPG rulebook's PLAY-TIME mechanics into a machine catalog for a rules-aware GM agent. Survey the book with the tools, then produce ONE array `mechanics_catalog` — nothing else — via submit_mechanics.

WHAT QUALIFIES: every mechanic that is TRIGGERED while the game is being played and needs the GM to cooperate in the narration — checks and when to call for them, subsystem procedures (pursuit, breakdown/madness, repair, downtime, travel...), reactions, resource spends, timed or recurring upkeep, conditional unlocks, standing passive fields the table must keep visible. Social/relationship standing mechanics (reputation, standing/credit ratings, contacts/bonds the player can invoke, background entries the rules read or rewrite), information-economy rules (GM-side hidden rolls, knowledge grants) and player-facing meta-currencies are play-time mechanics too — include them even when they are pure prose. ALSO include per-skill trigger guidance: when the skills chapter defines WHEN a specific skill or characteristic is called for a concrete feat, the distinctive ones get entries with that trigger semantics in `when_to_use` and the skill key as `tested_parameter` (null when the key is not in the LEGAL KEYS list). Prioritise the feats adventure fiction constantly demands: physical traversal (leaping a gap, climbing a wall, swimming), stealth and pursuit, perception/search, social pressure.

OPEN ENUMERATION (MUST): call get_toc first, then sweep the book chapter by chapter. Do NOT collect from a preset category checklist. A mechanic that fits none of skill_check / subsystem_procedure / reaction / spend still goes in, with a free-form snake_case `kind` string of your choosing — `kind` classifies the SHAPE of an entry; it is never a discovery filter.

EACH ENTRY: {id, name, kind, description, when_to_use, tested_parameter, procedure, hooks, passive_projection, followup_links, locked_until, source_refs}.
- `id`: namespaced snake_case, unique in this submission (e.g. "<game>.<mechanic>").
- `description`: what the mechanic IS (semantic knowledge for the GM agent).
- `when_to_use`: TRIGGER SEMANTICS — which player actions / fiction situations call for it. Write meaning; keyword lists are FORBIDDEN.
- `tested_parameter`: ONLY a key from the LEGAL KEYS list in the user message, or null when no character parameter is tested.

EXPRESSIVENESS TIERS — express each entry as richly as the text supports, no richer:
1. `procedure` steps when the text gives an executable sequence. Step shapes: {"step":"roll","dice":"1d100","vs":"<target>","note":"..."} | {"step":"apply","track":"<track id>","op":"add|subtract|set","amount":"1d6","note":"..."} | {"step":"table_roll","table_ref":"<table>","note":"..."} | {"step":"gate","condition":"...","note":"..."}.
2. `hooks` when the book ties the mechanic to an engine event. EXACT vocabulary: {"event":"scene_enter"} | {"event":"turn_start"} | {"event":"time_advance"} | {"event":"rest"} | {"event":"combat_start"} | {"event":"combat_end"} | {"event":"calendar","granularity":{"unit":"day|week|segment|...","seconds_per_unit":N,"segments_per_day":N,"label":"..."}} | {"event":"session_end"} | {"event":"development_phase"}. For calendar, data-fy the book's REAL period (e.g. a day split into N watches -> unit "segment" + segments_per_day=N).
3. `passive_projection` for an always-on standing field: a display template such as "<field name> {value}: {band}".
4. An entry you cannot structure AT ALL is still submitted with empty procedure/hooks — the semantic tier is legal. NEVER omit a real mechanic because it resists structuring, and NEVER pad a prose-only mechanic with invented steps: when the book gives judgement guidance rather than an executable sequence, submit `procedure: []` and let `description`/`when_to_use` carry the meaning. A healthy catalog of a full rulebook normally contains entries of ALL FOUR tiers.

FOLLOWUPS: `followup_links` = [{condition, procedure_id}] with condition {"kind":"threshold","track_id":"...","threshold_ref":"..."} | {"kind":"outcome_band","band_id":"..."} | {"kind":"outcome","value":"success|failure"}. `procedure_id` MUST be the id of another entry in THIS submission. `locked_until` is prose unlock semantics for mechanics the book gates behind a condition.

GROUNDING (MUST): `source_refs` = [{"source_id":"<book id>","page":N,"section_path":[]}] citing pages you actually READ this session. A mechanic you cannot ground in a read page must be LEFT OUT — never invent.

EXCLUDE: character-creation derived math (a separate chargen pass compiles it) and a single check's immediate dice math (check_model/dice_core already hold it). This catalog captures WHEN something triggers + the PROCEDURE to run + the FOLLOW-UP links — do not duplicate those layers.

COMPANION KERNEL UPGRADES — the user message lists the kernel's EXISTING resource-track thresholds, success-band ids and sheet fields; alongside `mechanics_catalog`, submit these OPTIONAL arrays (additive merges: an existing kernel value is NEVER changed, only missing keys are filled):
- `thresholds_upgrades`: when a listed threshold has a follow-up procedure in the book (e.g. losing N+ points at once triggers a breakdown), submit {"track_id":"...", "at":N or "loss_in_one_go":N, "direction":"...", "followup_procedure_id":"<id of an entry in THIS submission>"}. Echo the threshold's EXISTING shape exactly so it can be matched; never invent new thresholds.
- `success_bands_upgrades`: for EVERY existing band id submit {"id":"...","semantics":"what this outcome degree means in fiction and mechanics"}; then sweep the book's FULL ladder of outcome degrees — from the best possible result (lowest-roll/highest-roll special cases included) down to the PLAIN FAILURE degree — and for every degree the kernel list LACKS you MUST submit the COMPLETE band object (id/label/semantics + the same boundary fields the existing bands carry). Mirror the existing bands' machine-readable predicate shape EXACTLY — if they carry a `test` object (e.g. {"test":{"kind":"exact","value":N}} / {"kind":"in_range","min":..,"max":..} / {"kind":"otherwise"} for the plain-failure fallback), every band you add MUST carry one too, with a `rank` placing it in the ladder; a band without that predicate cannot be evaluated by the engine. A kernel that only lists success-side bands is INCOMPLETE: the failure degree and any special best/worst degrees are bands too, INCLUDING degrees keyed to an EXACT die result — a best-possible-roll auto-success degree that outranks every fraction band, or a worst-possible-roll mishap — when the book defines them.
- `field_notes`: for sheet fields lacking usage notes (including derived values like sanity/luck/mp), submit {"field_id":"...","notes":"what it is / how it is used in play"}.
All upgrade content MUST be grounded in pages you read this session — never invent.

TOOLS: get_toc, search(keywords), read(pages) for prose; read_layout(pages) for the aligned table view. Then call submit_mechanics with {mechanics_catalog:[...]} plus any companion upgrade arrays."#;

/// Third compile pass entry: upgrades `kernel.mechanics_catalog` IN PLACE and
/// returns gap notes for the caller's tracing. The LLM client is injected by
/// the caller (this file reads no env). No submit / LLM Err / nothing
/// coercible -> kernel untouched + gap note (fail-closed, never invent).
pub async fn compile_mechanics_catalog(
    client: &dyn LlmClient,
    kernel: &mut RuleKernel,
    ctx: MechCompileCtx<'_>,
    budget: usize,
) -> Vec<String> {
    let mut gaps: Vec<String> = Vec::new();
    let base_seed = build_mech_seed(kernel, &ctx);
    let mut tool_schemas = tools::nav_tools();
    tool_schemas.push(json!({"type":"function","function":{"name":"read_layout","description":"(optional) aligned-table view of page(s) like \"33\". Tables are usually already inline in read(); use this only if a table looks misaligned.","parameters":{"type":"object","properties":{"pages":{"type":"string"}},"required":["pages"]}}}));
    tool_schemas.push(tools::submit_tool(
        "submit_mechanics",
        "Submit the compiled mechanics catalog (plus optional companion kernel upgrades) for this ruleset.",
        json!({
            "mechanics_catalog": {"type": "array", "items": {"type": "object"}},
            "thresholds_upgrades": {"type": "array", "items": {"type": "object"}, "description": "OPTIONAL: link an EXISTING resource-track threshold to a catalog procedure: {track_id, at?|loss_in_one_go?, direction?, followup_procedure_id}. Echo the threshold's existing shape exactly; never invent thresholds."},
            "success_bands_upgrades": {"type": "array", "items": {"type": "object"}, "description": "OPTIONAL: per success band {id, semantics, ...}; an outcome degree the kernel lacks is submitted as a COMPLETE band object."},
            "field_notes": {"type": "array", "items": {"type": "object"}, "description": "OPTIONAL: {field_id, notes} usage notes for sheet fields that lack them."}
        }),
        &["mechanics_catalog"],
    ));

    // Merge raw records across rounds by id (a later round's record wins);
    // companion upgrade arrays concatenate across rounds (exact dups skipped —
    // the apply_* merges are additive so replays would only add noise warnings).
    let mut merged: BTreeMap<String, Value> = BTreeMap::new();
    let mut ups = MechUpgrades::default();
    for round in 0..2usize {
        let seed = if round == 0 {
            format!("{base_seed}\n\nSurvey the book (get_toc first), read the mechanic chapters, then submit_mechanics with the full catalog.")
        } else {
            // Round 2 runs UNCONDITIONALLY: mechanics have no prior required
            // list, so the coverage sweep against the TOC is the main guard.
            let have: Vec<String> = merged
                .values()
                .map(|r| {
                    let id = r.get("id").and_then(Value::as_str).unwrap_or("?");
                    let name = r.get("name").and_then(Value::as_str).unwrap_or("?");
                    let kind = r.get("kind").and_then(Value::as_str).unwrap_or("?");
                    format!("{id} | {name} | {kind}")
                })
                .collect();
            format!(
                "{base_seed}\n\nROUND 2 — COVERAGE SWEEP. Collected so far (id | name | kind):\n{}\n\nRe-check the TOC chapter by chapter against this list and submit_mechanics with EVERY play-time mechanic still missing. You may resubmit a corrected version of a collected id (same id overwrites).",
                have.join("\n")
            )
        };
        let round_budget = if round == 0 { budget } else { budget.min(8) };
        if let Some(sub) = run_mech_loop(client, &seed, &tool_schemas, &ctx, round_budget).await {
            for r in sub.catalog {
                match r
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    Some(id) => {
                        merged.insert(id.to_ascii_lowercase(), r);
                    }
                    None => gaps.push(format!(
                        "submitted mechanic without usable id dropped: {}",
                        snippet(&r)
                    )),
                }
            }
            extend_dedup(&mut ups.thresholds, sub.upgrades.thresholds);
            extend_dedup(&mut ups.bands, sub.upgrades.bands);
            extend_dedup(&mut ups.field_notes, sub.upgrades.field_notes);
        }
    }

    // Deterministic guardrail write-back (mechanics_finalize): validation
    // messages land in kernel.validation_report.warnings AND are echoed as
    // gap notes for the caller's tracing. No submit -> catalog untouched.
    // Pre-pass snapshot: the round-trip guard below vetoes ALL of this pass's
    // write-backs at once (never half-written).
    let backup = kernel.clone();
    // The on_outcome `=field` reference guard runs UNCONDITIONALLY: it audits
    // the READER pass's resource_tracks (already on the kernel), not this
    // pass's submissions — a dead =ref must surface even when no mechanic lands.
    let mut messages = super::mechanics_finalize::apply_on_outcome_ref_guard(kernel);
    if merged.is_empty() && ups.is_empty() {
        gaps.push("mechanics compiler produced nothing; kernel catalog left untouched".into());
    } else {
        let raw: Vec<Value> = merged.into_values().collect();
        let (entries, more) =
            super::mechanics_finalize::finalize_catalog(raw, kernel, &ctx.skill_names);
        messages.extend(more);
        if entries.is_empty() {
            gaps.push(
                "mechanics compiler produced nothing usable; kernel catalog left untouched".into(),
            );
        } else {
            kernel.mechanics_catalog = entries;
        }
        // A4 companion upgrades — applied AFTER the catalog is final (threshold
        // followup refs validate against it); all merges additive, never overwrite.
        let catalog = kernel.mechanics_catalog.clone();
        messages.extend(super::mechanics_finalize::apply_thresholds_upgrades(
            kernel,
            &ups.thresholds,
            &catalog,
        ));
        messages.extend(super::mechanics_finalize::apply_success_bands_upgrades(
            kernel, &ups.bands,
        ));
        messages.extend(super::mechanics_finalize::apply_field_notes(
            kernel,
            &ups.field_notes,
        ));
    }
    for m in &messages {
        gaps.push(format!(
            "{} [{}]: {}",
            m.code,
            m.target.as_deref().unwrap_or("-"),
            m.message
        ));
    }
    kernel.validation_report.warnings.extend(messages);
    if let Err(e) = super::mechanics_finalize::kernel_round_trip_guard(kernel) {
        *kernel = backup;
        gaps.push(format!(
            "kernel round-trip guard failed; mechanics pass fully reverted: {e}"
        ));
    }
    gaps
}

/// Companion upgrade arrays carried alongside the catalog (A4).
#[derive(Default)]
struct MechUpgrades {
    thresholds: Vec<Value>,
    bands: Vec<Value>,
    field_notes: Vec<Value>,
}

impl MechUpgrades {
    fn is_empty(&self) -> bool {
        self.thresholds.is_empty() && self.bands.is_empty() && self.field_notes.is_empty()
    }
}

/// One round's `submit_mechanics` payload: the required catalog plus the
/// OPTIONAL companion upgrades (absent / non-array keys coerce to empty).
struct MechSubmission {
    catalog: Vec<Value>,
    upgrades: MechUpgrades,
}

fn extend_dedup(acc: &mut Vec<Value>, more: Vec<Value>) {
    for v in more {
        if !acc.contains(&v) {
            acc.push(v);
        }
    }
}

/// Seed user prompt — every datum comes from the kernel at hand (zero
/// hardcoding): ruleset id/title, hint pages, legal tested_parameter keys
/// (sheet fields + derived + option-catalog skills + track ids), existing
/// resource-track thresholds, and existing success-band ids.
fn build_mech_seed(kernel: &RuleKernel, ctx: &MechCompileCtx<'_>) -> String {
    let schema = &kernel.character_sheet_schema;
    let mut keys: Vec<String> = Vec::new();
    keys.extend(field_ids(schema.get("fields")));
    keys.extend(field_ids(schema.get("derived_values")));
    keys.extend(
        ctx.skill_names
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
    );
    let track_ids: Vec<String> = kernel
        .resource_tracks
        .iter()
        .filter_map(|t| t.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    keys.extend(track_ids);
    keys.sort();
    keys.dedup();

    let tracks: Vec<String> = kernel
        .resource_tracks
        .iter()
        .map(|t| {
            let id = t.get("id").and_then(Value::as_str).unwrap_or("?");
            let ths = t.get("thresholds").cloned().unwrap_or_else(|| json!([]));
            format!(
                "- {id} thresholds={}",
                serde_json::to_string(&ths).unwrap_or_default()
            )
        })
        .collect();

    let bands: Vec<String> = kernel
        .dice_core
        .get("success_bands")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|b| b.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    // A4 — sheet fields whose usage notes are missing/empty: the prompt's
    // `field_notes` candidates (the apply merge only ever fills these anyway).
    let noteless: Vec<String> = kernel
        .character_sheet_schema
        .get("fields")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|f| match f.get("notes") {
                    None => true,
                    Some(v) => {
                        v.is_null() || v.as_str().map(|s| s.trim().is_empty()).unwrap_or(false)
                    }
                })
                .filter_map(|f| f.get("field_id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let title = kernel
        .game_identity
        .as_str()
        .or_else(|| kernel.game_identity.get("name").and_then(Value::as_str))
        .or_else(|| kernel.game_identity.get("title").and_then(Value::as_str))
        .unwrap_or("");

    format!(
        "Compile the play-time mechanics catalog for ruleset `{}` {}.\n\
         Hint pages (may be empty — survey the WHOLE book via get_toc yourself): {}\n\
         LEGAL tested_parameter KEYS (the character sheet's REAL keys — use EXACTLY these, or null): {:?}\n\
         EXISTING resource tracks (aim threshold followup_links AND thresholds_upgrades at these REAL thresholds):\n{}\n\
         EXISTING success-band ids (for outcome_band conditions; give EACH a semantics via success_bands_upgrades, and submit COMPLETE band objects for outcome degrees the book has but this list lacks): {:?}\n\
         Sheet fields MISSING usage notes (field_notes candidates): {:?}",
        kernel.ruleset_id,
        title,
        ctx.located_pages,
        keys,
        tracks.join("\n"),
        bands,
        noteless
    )
}

/// `[*].field_id` strings of a JSON array, trimmed and non-empty.
fn field_ids(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.get("field_id").and_then(Value::as_str))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn snippet(v: &Value) -> String {
    v.to_string().chars().take(120).collect()
}

/// The focused tool loop (mirrors `chargen_compile::run_compile_loop`), with
/// one deliberate difference: a MALFORMED submit (missing / non-array
/// `mechanics_catalog`) is NOT treated as an empty catalog — the loop reminds
/// the model and keeps going.
async fn run_mech_loop(
    client: &dyn LlmClient,
    seed: &str,
    tool_schemas: &[Value],
    ctx: &MechCompileCtx<'_>,
    budget: usize,
) -> Option<MechSubmission> {
    let mut msgs = vec![
        json!({"role":"system","content":MECH_SYS}),
        json!({"role":"user","content":seed}),
    ];
    for _ in 0..(budget + 8) {
        let resp = client
            .complete_with_tools(msgs.clone(), tool_schemas.to_vec())
            .await
            .ok()?;
        let message = resp
            .pointer("/choices/0/message")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let tcs = message
            .get("tool_calls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if tcs.is_empty() {
            msgs.push(message);
            msgs.push(
                json!({"role":"user","content":"Use the tools, then call submit_mechanics."}),
            );
            continue;
        }
        msgs.push(message);
        for tc in &tcs {
            let name = tc
                .pointer("/function/name")
                .and_then(Value::as_str)
                .unwrap_or("");
            let args: Value = tc
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_else(|| json!({}));
            let id = tc.get("id").and_then(Value::as_str).unwrap_or("");
            if name == "submit_mechanics" {
                match args.get("mechanics_catalog").and_then(Value::as_array) {
                    Some(arr) => {
                        let opt = |k: &str| {
                            args.get(k)
                                .and_then(Value::as_array)
                                .cloned()
                                .unwrap_or_default()
                        };
                        return Some(MechSubmission {
                            catalog: arr.clone(),
                            upgrades: MechUpgrades {
                                thresholds: opt("thresholds_upgrades"),
                                bands: opt("success_bands_upgrades"),
                                field_notes: opt("field_notes"),
                            },
                        });
                    }
                    None => {
                        msgs.push(json!({"role":"tool","tool_call_id":id,"content":"submit_mechanics requires {mechanics_catalog: [...]} — an ARRAY of entry objects. Call it again with the array."}));
                        continue;
                    }
                }
            }
            let out = mech_dispatch(ctx, name, &args);
            msgs.push(json!({"role":"tool","tool_call_id":id,"content":out}));
        }
    }
    None
}

fn mech_dispatch(ctx: &MechCompileCtx<'_>, name: &str, args: &Value) -> String {
    let cap = |s: String| {
        if s.len() <= 3000 {
            s
        } else {
            s.chars().take(3000).collect()
        }
    };
    match name {
        "get_toc" => tools::toc(ctx.units, 40),
        "search" => {
            let kws: Vec<String> = args
                .get("keywords")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            cap(tools::search(ctx.units, &kws, 8))
        }
        "read" => cap(tools::read(ctx.units, args.get("pages").and_then(Value::as_str).unwrap_or(""))),
        "read_layout" => match &ctx.sidecar_text {
            Some(s) => cap(chargen_compile::read_layout(s, args.get("pages").and_then(Value::as_str).unwrap_or(""))),
            None => "[no layout view available for this book — rely on read(); tables are usually inline]".into(),
        },
        _ => format!("unknown tool {name}"),
    }
}

#[cfg(test)]
#[path = "mechanics_compile_tests.rs"]
mod tests;
