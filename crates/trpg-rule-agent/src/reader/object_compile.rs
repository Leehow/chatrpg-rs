//! Object/ability schema compiler — the reader's focused pass that turns a
//! ruleset's item/weapon/spell/ability categories into machine-hooked SCHEMAS +
//! 2 worked examples each. Mirrors `chargen_compile` (loop / tools / dispatch +
//! a round-trip guardrail), applied to objects instead of character math.
//! Design: docs/superpowers/specs/2026-06-09-object-ability-schema-extraction-design.md
//!
//! Multi-pass and fully data-driven (rulesets differ wildly — Triangle/CoC have
//! NO spells, item params may not couple):
//!   DISCOVER — which object/ability categories this game ACTUALLY has, with
//!              typed schema_slots + the character params/tracks each couples to.
//!   EXTRACT  — per category: the typed schema + exactly 2 source-backed example
//!              instances whose formula slots HOOK into the engine the same way
//!              chargen derived_values do (damage→dice, attack→skill, ammo→track).
//! The 2 examples are the exact pattern the play-time materialization mimics, so a
//! new item is "fill THESE slots like THESE examples", not free interpretation.

use super::chargen_compile::read_layout;
use super::tools;
use super::units::Unit;
use serde_json::{json, Value};
use trpg_llm::LlmClient;

/// Shared context for the object/ability compile passes (mirrors `CompileCtx`).
pub struct ObjectCtx<'a> {
    pub units: &'a [Unit],
    /// The duotext merged `.md` (page-anchored) — read_layout's aligned table view.
    pub sidecar_text: Option<String>,
    /// Character skill field_ids, so an `attack_skill` hook reuses the real id.
    pub skills: Vec<String>,
    /// Resource-track ids, so `ammo` / `cost` / `power` hooks reuse the real id.
    pub resource_tracks: Vec<String>,
}

const DISCOVER_SYS: &str = r#"You map a tabletop RPG's MECHANICAL OBJECT/ABILITY categories so a deterministic engine can later extract them as machine-hooked schemas. Using the tools, find EVERY category of thing-with-parameters this game has, then OMIT only the families it genuinely lacks. Be data-driven: DISCOVER, never assume — but be THOROUGH.

DO NOT stop after the first one or two categories. A "category" is any group of entries that share a STAT BLOCK or parameter list (a damage+range table, a spell list with cost/effect, a gear list, a vehicle table, a signature item type). Systematically CHECK for EACH of these common families and SEARCH the book for it before concluding it's absent:
 - WEAPONS / firearms / melee — a damage (+ range / ammo) table; present in almost every game with combat. Search "weapons","firearms","damage","melee" and read that chapter.
 - ARMOR / protection — armor value / soak / SP.
 - SPELLS / magic / psychic powers / disciplines / programs — a list with cost + effect. MANY games have NONE — omit if so.
 - GEAR / equipment / items that carry mechanical params.
 - VEHICLES / mounts.
 - any SIGNATURE category this game centers on (cyberware, mythos tomes, requisitions, relics, …).
Include a category EVEN IF its entries are mundane — a plain weapons table is a category just as much as an exotic one; do not skip the ordinary ones in favor of only the flavorful ones. Only omit a family AFTER you searched and confirmed the game truly lacks it.

For EACH real category return an object:
 - category_id (snake_case) and kind (weapon|armor|spell|psychic|cyberware|gear|tool|vehicle|tome|…)
 - source_pages: where its master list/table lives (you must have located it by reading)
 - schema_slots: the per-entry parameter fields, each {slot, type (dice|number|skill_ref|resource_ref|range_ref|text), hook}. The HOOK is how the slot plugs into play: a damage/heal slot → rolled via the core dice; an attack_skill slot → which character skill resolves it; an ammo/cost slot → which resource track it draws; a range slot → the range / check model.
 - couples_to: the character skill / resource-track ids this category references (so coupled categories can share one vocabulary). Empty if it stands alone.

Use get_toc / search / read / read_layout to ground EVERY category in a page you actually read; omit any you cannot locate. Then call submit_categories with {categories:[...]}. Budget ~12 tool calls — spend them CHECKING each common family above, not just the first you find."#;

const EXTRACT_SYS: &str = r#"You compile ONE object/ability category of a tabletop RPG into a machine-hooked SCHEMA + exactly 2 EXAMPLE instances, for a deterministic engine — the same idea as the character-creation formula compiler, applied to items/spells/abilities.

Read the category's pages (use read_layout for the aligned TABLE — most weapon/spell stats live in a table). Produce:
 - schema_slots: the category's typed slots, each {slot, type (dice|number|skill_ref|resource_ref|range_ref|text), hook}. A dice slot (damage/heal) carries `expr` in machine dice form (e.g. "1d10", "1d6+1d4") so the SAME engine rolls it; a skill_ref slot names the character skill that resolves it (pick from the provided skill list when one matches); a resource_ref slot names the resource track (ammo / cost / power); a range/number/text slot is copied as-is.
 - examples: EXACTLY 2 real entries from the source (e.g. ".38 or 9mm Revolver" and "Shotgun"; or two real spells), each {name, slots:{<slot>:<value>}, source_pages}. COPY each value VERBATIM from the table — damage "1D10" not "some dice"; range "15 yards" not "short". The 2 examples must hook to the SAME schema_slots.

GUARDRAIL: ground every value in a page you READ; NEVER invent a value, a row, or a spell. If a slot's value is unreadable, set it null and list the slot in `provisional_slots`. Pick attack_skill / resource hooks from the provided character skills + resource tracks when one matches; otherwise name what the book literally says and list that slot in `provisional_slots`.

TOOLS: get_toc, search(keywords), read(pages), read_layout(pages). Then call submit_category with {category_id, kind, schema_slots:[…], examples:[…], provisional_slots:[…]}. Budget ~8 tool calls; the category's pages were located for you."#;

/// Discover this ruleset's object/ability categories, then extract a schema + 2
/// examples for each. Returns one object per category (schema + examples +
/// validation), suitable for `RuleKernel.object_schemas`. Empty when the game
/// has no such categories (e.g. a freeform-only system).
pub async fn compile_object_schemas(client: &dyn LlmClient, ctx: &ObjectCtx<'_>, budget: usize) -> Vec<Value> {
    let cats = discover_object_categories(client, ctx, budget).await;
    let mut out = Vec::new();
    for cat in cats.iter().take(8) {
        if let Some(schema) = extract_object_category(client, ctx, cat, budget).await {
            out.push(schema);
        }
    }
    out
}

/// Stage-1 of the object compile, callable on its own (staged parsing,
/// Optimization 2): discover this ruleset's object/ability categories WITHOUT
/// extracting per-category schemas. Each returned category is annotated
/// `status:"discovered"` (a stub carrying `category_id`/`kind`/`source_pages`/
/// `couples_to`, but no `schema_slots`/`examples`) so the play-time
/// materialization can fill it lazily on first use. Empty when the game has no
/// such categories.
pub async fn discover_object_categories(client: &dyn LlmClient, ctx: &ObjectCtx<'_>, budget: usize) -> Vec<Value> {
    let toc = tools::toc(ctx.units, 40);
    let submit = tools::submit_tool(
        "submit_categories",
        "Submit the discovered object/ability categories (omit ones this game lacks).",
        json!({"categories": {"type": "array", "items": {"type": "object"}}}),
        &["categories"],
    );
    let schemas = object_tool_schemas(submit);
    // Cap the coupling vocab shown so a long skills list can't crowd out the
    // discovery instruction (it biased the pass into skipping the mundane weapons
    // table). The full skills list is still used per-category in EXTRACT.
    let skills_hint: Vec<&String> = ctx.skills.iter().take(24).collect();
    let seed = format!(
        "Character skills (coupling sample): {:?}\nResource tracks: {:?}\n\nTOC (already fetched):\n{toc}\n\nDiscover this game's mechanical object/ability categories — omit any it lacks — grounding each in a page you read, then call submit_categories.",
        skills_hint, ctx.resource_tracks
    );
    let round1 = run_object_loop(client, DISCOVER_SYS, &seed, &schemas, ctx, budget, "submit_categories")
        .await
        .and_then(|v| v.get("categories").and_then(Value::as_array).cloned())
        .unwrap_or_default();

    // Round 2 — completeness critique (mirrors the chargen compiler's 2nd round):
    // a single discover call is lossy + run-to-run inconsistent (it skips mundane
    // categories like a plain weapons table even when present). Show round 1's
    // result and RE-SCAN for the common families it may have missed, then UNION.
    // Generic: names families to RE-VERIFY, never assumes one exists.
    let kinds: Vec<String> = round1.iter().filter_map(|c| c.get("kind").and_then(Value::as_str).map(String::from)).collect();
    let ids: Vec<String> = round1.iter().filter_map(|c| c.get("category_id").and_then(Value::as_str).map(String::from)).collect();
    let seed2 = format!(
        "A first pass already found these categories: kinds={kinds:?} ids={ids:?}.\nRE-SCAN the book for categories it may have MISSED. Check EACH of these and SEARCH for it before deciding it's absent: a WEAPONS / firearms / melee damage table; an ARMOR table; a SPELL / power / psychic list; a GEAR / equipment list; a VEHICLE table; any signature item type. For each family NOT already listed above, search the relevant chapter and ADD it if present (skip only if truly absent). Re-submit the COMPLETE list — INCLUDING the first-pass categories — via submit_categories.\n\nTOC (already fetched):\n{toc}"
    );
    let round2 = run_object_loop(client, DISCOVER_SYS, &seed2, &schemas, ctx, budget.min(8), "submit_categories")
        .await
        .and_then(|v| v.get("categories").and_then(Value::as_array).cloned())
        .unwrap_or_default();

    // Annotate each surviving category as a discovered stub (no schema_slots yet)
    // so the play-time materialization can detect + lazily extract it on first use.
    annotate_discovered(merge_categories(round1, round2))
}

/// Stamp each merged category as a `discovered` stub. A stub keeps its
/// `category_id`/`kind`/`source_pages`/`couples_to` but no `schema_slots`/
/// `examples` — the play-time materialization fills those lazily, flipping the
/// marker to `compiled`. Pure (no LLM) so it is unit-testable on a fixture.
fn annotate_discovered(cats: Vec<Value>) -> Vec<Value> {
    cats.into_iter()
        .map(|mut c| {
            if let Some(obj) = c.as_object_mut() {
                obj.insert("status".into(), json!("discovered"));
            }
            c
        })
        .collect()
}

/// Union categories from two discover passes, de-duped by (kind, category_id),
/// preserving first-seen order. On a key collision keep the better-formed entry
/// (more schema_slots + a located source_pages).
fn merge_categories(a: Vec<Value>, b: Vec<Value>) -> Vec<Value> {
    let key = |c: &Value| {
        let kind = c.get("kind").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
        let id = c.get("category_id").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
        format!("{kind}|{id}")
    };
    let score = |c: &Value| {
        let slots = c.get("schema_slots").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);
        let has_pages = c.get("source_pages").map(|p| !p.is_null()).unwrap_or(false) as usize;
        slots + has_pages
    };
    let mut order: Vec<String> = Vec::new();
    let mut map: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
    for c in a.into_iter().chain(b.into_iter()) {
        let k = key(&c);
        match map.get(&k) {
            Some(existing) if score(existing) >= score(&c) => {}
            Some(_) => { map.insert(k, c); }
            None => { order.push(k.clone()); map.insert(k, c); }
        }
    }
    order.into_iter().filter_map(|k| map.remove(&k)).collect()
}

/// Extract ONE object/ability category into a machine-hooked schema + exactly 2
/// worked examples (the per-category EXTRACT pass). Callable on its own so the
/// play-time materialization can fill a `discovered` stub lazily on first use.
/// The returned schema is annotated `status:"compiled"`.
pub async fn extract_object_category(client: &dyn LlmClient, ctx: &ObjectCtx<'_>, cat: &Value, budget: usize) -> Option<Value> {
    let submit = tools::submit_tool(
        "submit_category",
        "Submit this category's schema_slots + exactly 2 example instances.",
        json!({
            "category_id": {"type": "string"},
            "kind": {"type": "string"},
            "schema_slots": {"type": "array", "items": {"type": "object"}},
            "examples": {"type": "array", "items": {"type": "object"}},
            "provisional_slots": {"type": "array", "items": {"type": "string"}}
        }),
        &["category_id", "kind", "schema_slots", "examples"],
    );
    let schemas = object_tool_schemas(submit);
    let cat_json = serde_json::to_string(cat).unwrap_or_default();
    let pages = cat.get("source_pages").and_then(Value::as_str).unwrap_or("");
    let seed = format!(
        "Category to compile:\n{cat_json}\n\nCharacter skills (pick attack_skill hooks from these when one matches): {:?}\nResource tracks (pick ammo / cost hooks from these): {:?}\n\nIts list/table is around pages: {pages}. read_layout those pages for the aligned table, then submit_category with schema_slots + exactly 2 examples (values copied verbatim).",
        ctx.skills, ctx.resource_tracks
    );
    let out = run_object_loop(client, EXTRACT_SYS, &seed, &schemas, ctx, budget, "submit_category").await?;
    // finalize FIRST so confirmed-slot info exists, THEN regrab can target the
    // confirmed-but-null gaps (fix (e), VALDROP last-cell re-grab).
    let finalized = finalize_category(out, ctx);
    Some(super::object_regrab::regrab_missing(client, ctx, finalized, budget).await)
}

/// nav tools + the read_layout table view + the given submit tool.
pub(super) fn object_tool_schemas(submit: Value) -> Vec<Value> {
    let mut t = tools::nav_tools();
    t.push(json!({"type": "function", "function": {
        "name": "read_layout",
        "description": "aligned-table view of page(s) like \"401\" or \"400-402\" — use for weapon/spell stat tables that look misaligned in read().",
        "parameters": {"type": "object", "properties": {"pages": {"type": "string"}}, "required": ["pages"]}
    }}));
    t.push(submit);
    t
}

/// The focused tool loop (mirrors `run_compile_loop`). Returns the submit args.
pub(super) async fn run_object_loop(
    client: &dyn LlmClient,
    system: &str,
    seed: &str,
    tool_schemas: &[Value],
    ctx: &ObjectCtx<'_>,
    budget: usize,
    submit_name: &str,
) -> Option<Value> {
    let mut msgs = vec![
        json!({"role": "system", "content": system}),
        json!({"role": "user", "content": seed}),
    ];
    for _ in 0..(budget + 8) {
        let resp = client.complete_with_tools(msgs.clone(), tool_schemas.to_vec()).await.ok()?;
        let message = resp.pointer("/choices/0/message").cloned().unwrap_or_else(|| json!({}));
        let tcs = message.get("tool_calls").and_then(Value::as_array).cloned().unwrap_or_default();
        if tcs.is_empty() {
            msgs.push(message);
            msgs.push(json!({"role": "user", "content": format!("Use the tools, then call {submit_name}.")}));
            continue;
        }
        msgs.push(message);
        for tc in &tcs {
            let name = tc.pointer("/function/name").and_then(Value::as_str).unwrap_or("");
            let args: Value = tc
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_else(|| json!({}));
            let id = tc.get("id").and_then(Value::as_str).unwrap_or("");
            if name == submit_name {
                return Some(args);
            }
            let out = object_dispatch(ctx, name, &args);
            msgs.push(json!({"role": "tool", "tool_call_id": id, "content": out}));
        }
    }
    None
}

fn object_dispatch(ctx: &ObjectCtx<'_>, name: &str, args: &Value) -> String {
    let cap = |s: String| if s.len() <= 3000 { s } else { s.chars().take(3000).collect() };
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
            Some(s) => cap(read_layout(s, args.get("pages").and_then(Value::as_str).unwrap_or(""))),
            None => "[no layout view available — mark table-based values provisional]".into(),
        },
        _ => format!("unknown tool {name}"),
    }
}

// ---------------------------------------------------------------------------
// Round-trip guardrail — type-aware, data-driven (objects, not chargen math):
//   a dice slot value must look like dice; a skill_ref/resource_ref must resolve
//   to a known id (when the vocab was provided). Unresolved -> provisional.
// ---------------------------------------------------------------------------

fn finalize_category(mut cat: Value, ctx: &ObjectCtx<'_>) -> Value {
    let slot_types = slot_type_map(&cat);
    let mut provisional: Vec<String> = cat
        .get("provisional_slots")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let mut checked = 0usize;
    if let Some(examples) = cat.get("examples").and_then(Value::as_array) {
        for ex in examples {
            let Some(slots) = ex.get("slots").and_then(Value::as_object) else { continue };
            for (slot, val) in slots {
                let ty = slot_types.get(slot.as_str()).map(String::as_str).unwrap_or("text");
                checked += 1;
                if !slot_value_ok(ty, val, ctx) && !provisional.contains(slot) {
                    provisional.push(slot.clone());
                }
            }
        }
    }
    provisional.sort();
    provisional.dedup();
    // Fix (f): confirmed-slot tagging. A schema slot is "confirmed" iff at least
    // ONE example fills it with a meaningful (non-empty) value — i.e. it is a real
    // source column, not a discover over-spec (SCHEMA_OVERSPEC). This lets fill
    // ratio be judged over applicable slots instead of the declared superset.
    let confirmed = confirmed_slots(&cat);
    let schema_total = cat.get("schema_slots").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);
    if let Some(slots) = cat.get_mut("schema_slots").and_then(Value::as_array_mut) {
        for s in slots.iter_mut() {
            let name = s.get("slot").and_then(Value::as_str).unwrap_or("").to_string();
            if let Some(obj) = s.as_object_mut() {
                obj.insert("confirmed".into(), json!(confirmed.contains(&name)));
            }
        }
    }
    if let Some(obj) = cat.as_object_mut() {
        obj.insert("provisional_slots".into(), json!(provisional));
        obj.insert(
            "validation".into(),
            json!({
                "slot_values_checked": checked,
                "provisional": provisional.len(),
                "confirmed_slots": confirmed.len(),
                "schema_slots": schema_total,
            }),
        );
        // A fully extracted category flips the stub marker discovered -> compiled.
        obj.insert("status".into(), json!("compiled"));
    }
    cat
}

/// The set of schema-slot names that at least ONE example fills with a meaningful
/// value (across all examples' `slots`). A confirmed slot is a real source column;
/// an unconfirmed one is discover over-reach (SCHEMA_OVERSPEC) — every example
/// left it empty. Pure (no LLM) so fix (e)'s gap computation can reuse it.
pub(super) fn confirmed_slots(cat: &Value) -> std::collections::HashSet<String> {
    let mut confirmed = std::collections::HashSet::new();
    let Some(examples) = cat.get("examples").and_then(Value::as_array) else { return confirmed };
    for ex in examples {
        let Some(slots) = ex.get("slots").and_then(Value::as_object) else { continue };
        for (slot, val) in slots {
            if is_meaningful(val) {
                confirmed.insert(slot.clone());
            }
        }
    }
    confirmed
}

/// A "meaningful" value = not null, not "" (empty/whitespace), not {} / [].
/// Mirrors the emptiness checks `slot_value_ok` makes for null/empty strings.
pub(super) fn is_meaningful(val: &Value) -> bool {
    match val {
        Value::Null => false,
        Value::String(s) => !s.trim().is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        _ => true, // number / bool
    }
}

fn slot_type_map(cat: &Value) -> std::collections::HashMap<String, String> {
    let mut m = std::collections::HashMap::new();
    if let Some(slots) = cat.get("schema_slots").and_then(Value::as_array) {
        for s in slots {
            if let (Some(name), Some(ty)) = (
                s.get("slot").and_then(Value::as_str),
                s.get("type").and_then(Value::as_str),
            ) {
                m.insert(name.to_string(), ty.to_string());
            }
        }
    }
    m
}

fn slot_value_ok(ty: &str, val: &Value, ctx: &ObjectCtx<'_>) -> bool {
    let s = match val {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        Value::Null => return false,
        other => other.to_string(),
    };
    if s.is_empty() {
        return false;
    }
    match ty {
        "dice" => looks_like_dice(&s),
        "skill_ref" => ctx.skills.is_empty() || ctx.skills.iter().any(|k| fuzzy_match(k, &s)),
        "resource_ref" => ctx.resource_tracks.is_empty() || ctx.resource_tracks.iter().any(|k| fuzzy_match(k, &s)),
        _ => true, // number / range_ref / text: non-empty is enough
    }
}

/// A dice token: digits-then-d-then-digits anywhere (1D10, d100, 2d6+1d4).
fn looks_like_dice(s: &str) -> bool {
    let b = s.as_bytes();
    for i in 0..b.len() {
        if b[i] == b'd' || b[i] == b'D' {
            let next_digit = i + 1 < b.len() && b[i + 1].is_ascii_digit();
            let prev_ok = i == 0 || !b[i - 1].is_ascii_alphabetic();
            if next_digit && prev_ok {
                return true;
            }
        }
    }
    false
}

/// Loose id/name match — case-insensitive, snake/space-insensitive, substring.
fn fuzzy_match(a: &str, b: &str) -> bool {
    let norm = |x: &str| x.to_ascii_lowercase().replace(['_', '-', '(', ')'], " ").split_whitespace().collect::<Vec<_>>().join(" ");
    let (na, nb) = (norm(a), norm(b));
    !na.is_empty() && (na == nb || na.contains(&nb) || nb.contains(&na))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dice_detection() {
        for ok in ["1D10", "1d6", "d100", "2d6+1d4", "1D8/1D10"] {
            assert!(looks_like_dice(ok), "{ok} should be dice");
        }
        for no in ["15 yards", "6", "Firearms", "credstick"] {
            assert!(!looks_like_dice(no), "{no} should NOT be dice");
        }
    }

    #[test]
    fn finalize_flags_bad_slots() {
        let ctx = ObjectCtx { units: &[], sidecar_text: None, skills: vec!["firearms_handgun".into()], resource_tracks: vec![] };
        let cat = json!({
            "category_id": "firearms", "kind": "weapon",
            "schema_slots": [
                {"slot": "damage", "type": "dice", "hook": "core dice"},
                {"slot": "skill", "type": "skill_ref", "hook": "attack skill"},
                {"slot": "range", "type": "range_ref", "hook": "range table"}
            ],
            "examples": [
                {"name": ".38 Revolver", "slots": {"damage": "1D10", "skill": "Firearms (Handgun)", "range": "15 yards"}},
                {"name": "Shotgun", "slots": {"damage": "not-dice", "skill": "Swimming", "range": "10 yards"}}
            ]
        });
        let out = finalize_category(cat, &ctx);
        let prov: Vec<String> = out["provisional_slots"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
        assert!(prov.contains(&"damage".to_string()), "bad dice should be flagged: {prov:?}");
        assert!(prov.contains(&"skill".to_string()), "unknown skill should be flagged: {prov:?}");
        assert!(!prov.contains(&"range".to_string()), "valid range should pass: {prov:?}");
        // A fully extracted category flips the stub marker to compiled.
        assert_eq!(out["status"].as_str(), Some("compiled"), "extract should mark compiled");
    }

    // Fix (f): a schema slot that NO example fills is tagged confirmed:false
    // (SCHEMA_OVERSPEC), one that some example fills is confirmed:true, and the
    // validation block reports the confirmed/total counts. Deterministic — no LLM.
    #[test]
    fn finalize_tags_confirmed_slots() {
        let ctx = ObjectCtx { units: &[], sidecar_text: None, skills: vec![], resource_tracks: vec![] };
        let cat = json!({
            "category_id": "armor", "kind": "armor",
            "schema_slots": [
                {"slot": "soak", "type": "number", "hook": "armor value"},
                {"slot": "location", "type": "text", "hook": "body location"}
            ],
            "examples": [
                // location is null in every example -> over-spec, unconfirmed.
                {"name": "Leather", "slots": {"soak": "2", "location": null}},
                {"name": "Plate", "slots": {"soak": "8", "location": ""}}
            ]
        });
        let out = finalize_category(cat, &ctx);
        let slots = out["schema_slots"].as_array().unwrap();
        let by_name = |n: &str| slots.iter().find(|s| s["slot"] == json!(n)).unwrap().clone();
        assert_eq!(by_name("soak")["confirmed"], json!(true), "filled slot is confirmed");
        assert_eq!(by_name("location")["confirmed"], json!(false), "all-null slot is over-spec");
        assert_eq!(out["validation"]["confirmed_slots"], json!(1), "exactly one confirmed");
        assert_eq!(out["validation"]["schema_slots"], json!(2), "two schema slots total");
    }

    // The deterministic half of `discover_object_categories`: each surviving
    // category becomes a `discovered` stub carrying source_pages but no
    // schema_slots/examples. The LLM rounds themselves are live-validated via
    // `parse-staged`; here we pin the stub SHAPE on a fixture.
    #[test]
    fn discover_annotates_stubs() {
        let cats = vec![
            json!({"category_id": "firearms", "kind": "weapon", "source_pages": "401-403", "couples_to": ["firearms_handgun"]}),
            json!({"category_id": "armor", "kind": "armor", "source_pages": "410"}),
        ];
        let stubs = annotate_discovered(cats);
        assert_eq!(stubs.len(), 2);
        for s in &stubs {
            assert_eq!(s["status"].as_str(), Some("discovered"), "every category is a discovered stub: {s}");
            assert!(s.get("source_pages").map(|p| !p.is_null()).unwrap_or(false), "source_pages preserved: {s}");
            assert!(s.get("schema_slots").is_none(), "a stub has no schema_slots yet: {s}");
            assert!(s.get("examples").is_none(), "a stub has no examples yet: {s}");
        }
        // category_id / kind / couples_to survive untouched.
        assert_eq!(stubs[0]["category_id"].as_str(), Some("firearms"));
        assert_eq!(stubs[0]["kind"].as_str(), Some("weapon"));
        assert_eq!(stubs[0]["couples_to"][0].as_str(), Some("firearms_handgun"));
    }
}
