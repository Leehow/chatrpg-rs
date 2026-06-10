//! Starter-character onboarding compiler — the reader's focused Stage-2 pass that
//! extracts a ruleset's STARTER GENERATION recipes (recommended archetypes +
//! quick-build shortcuts) so `create-character` builds a stat-complete PC instead
//! of a narrative draft. Mirrors `object_compile` (loop / nav_tools / read_layout /
//! submit + a fail-closed guardrail), applied to character onboarding.
//! Design: docs/superpowers/specs/2026-06-09-staged-onboarding-completion-design.md
//!
//! Data-driven + fail-closed: recipes are EXTRACTED from the book per ruleset; an
//! archetype/shortcut not grounded in a read page (or referencing a non-existent
//! template field / option-catalog id) is DROPPED — never fabricated. An empty
//! extraction returns an empty pack `{}` (create-character degrades to a draft,
//! never invents a build). The LLM loop is LIVE-VALIDATED (parse-staged +
//! create-character); only the deterministic guardrail is unit-tested here.

use super::chargen_compile::read_layout;
use super::tools;
use super::units::Unit;
use serde_json::{json, Value};
use trpg_llm::LlmClient;

/// Context for the starter-pack compile pass (mirrors `ObjectCtx`). The template's
/// role/class field + skill fields + option catalogs give the guardrail the legal
/// ids an archetype/shortcut may reference.
pub struct OnboardingCtx<'a> {
    pub units: &'a [Unit],
    /// The duotext merged `.md` (page-anchored) — read_layout's aligned table view.
    pub sidecar_text: Option<String>,
    /// The template's role/class field_id, if any (e.g. "occupation", "class").
    pub role_field: Option<String>,
    /// The template's skill field_ids — legal `signature skill` references.
    pub skill_fields: Vec<String>,
    /// The reader's option catalogs (Roles/classes, skills, starter gear) — legal
    /// option ids an archetype/shortcut may reference.
    pub option_catalogs: Value,
}

const STARTER_SYS: &str = r#"You compile a tabletop RPG's STARTER CHARACTER GENERATION recipes for a deterministic engine, so a new player can build a stat-complete starter character fast. Using the tools, read the character-creation chapter (the part that tells a NEW player how to make a first character), then produce two lists:

 - archetypes: 2-4 recommended STARTER builds. Each = a concrete starting concept tied to this game's REAL options: {archetype_id (snake_case), title, summary (1-2 sentences: which role/class, the stat priorities, the signature skills), fit_tags (e.g. combat/investigation/social), required_option_refs (the role/class + key skill ids it needs — use the REAL template field ids and option-catalog ids you were given), source_pages (where you read it)}.
 - creation_shortcuts: 1-3 quick-build recipes. Each = {shortcut_id (snake_case), title, mode (quick_start|guided|pregenerated), description (the quick path: pick role -> stat method (point-buy budget N, or roll XdY) -> starting skill picks -> starting gear), source_pages}.
 - pregens (OPTIONAL): only if the book prints ready-to-play sample characters; each {pregen_id, title, summary, source_pages}.

GUARDRAIL (fail-closed): GROUND every recipe in a page you READ — record source_pages. Reference ONLY the role/class field, skill fields, and option-catalog ids you were given; do NOT invent a role, class, skill, or option that is not in those lists. If you cannot read a recipe's basis, OMIT it — never fabricate a build, a stat, or a number. If the game has NO printed starter guidance you can ground, submit EMPTY lists.

TOOLS: get_toc, search(keywords), read(pages), read_layout(pages) for tables. Then call submit_starter_pack with {archetypes:[...], creation_shortcuts:[...], pregens:[...]}. Budget ~10 tool calls; be targeted — find the "creating a character" / "character creation" chapter."#;

/// Compile this ruleset's starter-character pack (recommended archetypes +
/// quick-build shortcuts; pregens optional). Returns the StarterCharacterPack as a
/// JSON OBJECT, fail-closed: every recipe is grounded + references only legal ids,
/// or it is dropped; an empty extraction returns `{}`. The LLM loop is
/// live-validated — only `finalize_starter_pack` is unit-tested.
pub async fn compile_starter_pack(client: &dyn LlmClient, ctx: &OnboardingCtx<'_>, budget: usize) -> Value {
    let toc = tools::toc(ctx.units, 40);
    let submit = tools::submit_tool(
        "submit_starter_pack",
        "Submit the starter-character recipes (empty lists if the game has no groundable starter guidance).",
        json!({
            "archetypes": {"type": "array", "items": {"type": "object"}},
            "creation_shortcuts": {"type": "array", "items": {"type": "object"}},
            "pregens": {"type": "array", "items": {"type": "object"}}
        }),
        &["archetypes", "creation_shortcuts"],
    );
    let schemas = onboarding_tool_schemas(submit);
    let option_ids = legal_option_ids(ctx);
    let seed = format!(
        "Role/class field: {:?}\nSkill fields (legal signature-skill refs): {:?}\nOption-catalog ids (legal required_option_refs): {:?}\n\nTOC (already fetched):\n{toc}\n\nRead this game's character-creation chapter and submit_starter_pack with 2-4 grounded archetypes + 1-3 quick-build shortcuts (pregens only if the book prints sample characters).",
        ctx.role_field, ctx.skill_fields, option_ids
    );
    let raw = run_onboarding_loop(client, STARTER_SYS, &seed, &schemas, ctx, budget, "submit_starter_pack")
        .await
        .unwrap_or_else(|| json!({}));
    let pack = finalize_starter_pack(raw.clone(), ctx);
    let n = |v: &Value, k: &str| v.get(k).and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);
    tracing::info!(
        raw_archetypes = n(&raw, "archetypes"), raw_shortcuts = n(&raw, "creation_shortcuts"),
        kept_archetypes = n(&pack, "archetypes"), kept_shortcuts = n(&pack, "creation_shortcuts"),
        "starter pack compiled"
    );
    pack
}

/// nav tools + the read_layout table view + the given submit tool (mirrors
/// `object_tool_schemas`).
fn onboarding_tool_schemas(submit: Value) -> Vec<Value> {
    let mut t = tools::nav_tools();
    t.push(json!({"type": "function", "function": {
        "name": "read_layout",
        "description": "aligned-table view of page(s) like \"30\" or \"30-32\" — use for point-buy / starting-gear tables that look misaligned in read().",
        "parameters": {"type": "object", "properties": {"pages": {"type": "string"}}, "required": ["pages"]}
    }}));
    t.push(submit);
    t
}

/// The focused tool loop (mirrors `run_object_loop`). Returns the submit args.
async fn run_onboarding_loop(
    client: &dyn LlmClient,
    system: &str,
    seed: &str,
    tool_schemas: &[Value],
    ctx: &OnboardingCtx<'_>,
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
            let out = onboarding_dispatch(ctx, name, &args);
            msgs.push(json!({"role": "tool", "tool_call_id": id, "content": out}));
        }
    }
    None
}

fn onboarding_dispatch(ctx: &OnboardingCtx<'_>, name: &str, args: &Value) -> String {
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
            None => "[no layout view available — read() the page instead]".into(),
        },
        _ => format!("unknown tool {name}"),
    }
}

/// The legal id vocabulary an archetype/shortcut may reference: the role/class
/// field, every skill field, and every option-catalog option/group/locator id.
/// Pure (no LLM) so the guardrail is unit-testable. Lowercased.
fn legal_option_ids(ctx: &OnboardingCtx<'_>) -> std::collections::HashSet<String> {
    let mut ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let Some(rf) = &ctx.role_field {
        ids.insert(rf.to_ascii_lowercase());
    }
    for s in &ctx.skill_fields {
        ids.insert(s.to_ascii_lowercase());
    }
    if let Some(cats) = ctx.option_catalogs.as_array() {
        for c in cats {
            collect_ids(c, &mut ids);
        }
    }
    ids
}

/// Recursively harvest every `*_id` / `option_id` / `group_id` / `locator_id` /
/// `field_id` / `catalog_id` string from a catalog value (and option titles).
fn collect_ids(v: &Value, out: &mut std::collections::HashSet<String>) {
    match v {
        Value::Object(map) => {
            for (k, val) in map {
                if (k.ends_with("_id") || k == "title") && val.is_string() {
                    if let Some(s) = val.as_str() {
                        if !s.trim().is_empty() {
                            out.insert(s.trim().to_ascii_lowercase());
                        }
                    }
                }
                collect_ids(val, out);
            }
        }
        Value::Array(arr) => {
            for val in arr {
                collect_ids(val, out);
            }
        }
        _ => {}
    }
}

/// Fail-closed guardrail (pure, no LLM): keep only archetypes/shortcuts that are
/// GROUNDED (have a non-empty `source_pages`) AND whose `required_option_refs` all
/// resolve to a legal id (role/class field, a skill field, or an option-catalog id).
/// Pregens are kept when grounded. If nothing survives, return `{}` (never
/// fabricate). Otherwise return a StarterCharacterPack-shaped object.
fn finalize_starter_pack(raw: Value, ctx: &OnboardingCtx<'_>) -> Value {
    let legal = legal_option_ids(ctx);
    let archetypes = keep_grounded(raw.get("archetypes"), |a| refs_are_legal(a, &legal));
    let shortcuts = keep_grounded(raw.get("creation_shortcuts"), |_| true);
    let pregens = keep_grounded(raw.get("pregens"), |_| true);
    if archetypes.is_empty() && shortcuts.is_empty() && pregens.is_empty() {
        return json!({});
    }
    let mut pack = json!({
        "archetypes": archetypes,
        "creation_shortcuts": shortcuts,
    });
    if !pregens.is_empty() {
        pack["pregens"] = json!(pregens);
    }
    pack
}

/// Keep array entries that are grounded (non-empty `source_pages`) AND pass `extra`.
fn keep_grounded(arr: Option<&Value>, extra: impl Fn(&Value) -> bool) -> Vec<Value> {
    arr.and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| is_grounded(item) && extra(item))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Grounded = the recipe records the page(s) it was read from.
fn is_grounded(item: &Value) -> bool {
    // Grounded = a non-empty source_pages, in WHATEVER shape the LLM emitted it —
    // a string ("32"/"32-34"), an array (["32","33"]), or a number. Only missing /
    // empty fails (the real fail-closed guard). Being format-picky here silently
    // dropped every well-grounded recipe (Cyberpunk/CoC starter packs came back 0).
    match item.get("source_pages") {
        Some(Value::String(s)) => !s.trim().is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Number(_)) => true,
        _ => false,
    }
}

/// Every `required_option_refs` id must resolve to a legal id. Empty refs are OK
/// (a concept that names no specific option). Unknown id -> drop (fail-closed).
fn refs_are_legal(item: &Value, legal: &std::collections::HashSet<String>) -> bool {
    // When the ruleset gave us NO option-catalog vocabulary to validate against
    // (thin/locator-only catalogs — common for CoC/Cyberpunk), we cannot meaningfully
    // check refs, so don't reject on them — grounding (is_grounded) remains the guard.
    // Enforce membership only when there IS a vocabulary.
    if legal.is_empty() {
        return true;
    }
    let refs = item.get("required_option_refs").and_then(Value::as_array);
    match refs {
        None => true,
        Some(rs) => rs.iter().all(|r| {
            r.as_str()
                .map(|s| {
                    let id = s.trim().to_ascii_lowercase();
                    id.is_empty() || legal.contains(&id)
                })
                .unwrap_or(true)
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The LLM loop (`compile_starter_pack`) is LIVE-VALIDATED via parse-staged +
    // create-character (Task 7), not unit-tested — the deterministic guardrail
    // (`finalize_starter_pack`) carries the unit coverage (Task 4). This first test
    // just pins the context + empty-extraction contract so the module compiles.
    #[test]
    fn empty_records_yield_empty_pack() {
        let ctx = OnboardingCtx { units: &[], sidecar_text: None, role_field: None, skill_fields: vec![], option_catalogs: serde_json::json!([]) };
        let pack = finalize_starter_pack(serde_json::json!({}), &ctx);
        assert_eq!(pack, serde_json::json!({}), "no archetypes/shortcuts -> empty pack, never fabricated");
    }

    fn ctx_with(role: Option<&str>, skills: &[&str], catalogs: Value) -> OnboardingCtx<'static> {
        OnboardingCtx {
            units: &[],
            sidecar_text: None,
            role_field: role.map(String::from),
            skill_fields: skills.iter().map(|s| s.to_string()).collect(),
            option_catalogs: catalogs,
        }
    }

    #[test]
    fn archetype_referencing_bogus_field_is_dropped() {
        // One archetype references a real skill ("library_use"); the other references
        // a non-existent field ("warp_drive") -> dropped. The good one survives.
        let ctx = ctx_with(Some("occupation"), &["library_use", "spot_hidden"], json!([]));
        let raw = json!({
            "archetypes": [
                {"archetype_id": "scholar", "title": "Scholar", "summary": "reads things",
                 "required_option_refs": ["occupation", "library_use"], "source_pages": "30-31"},
                {"archetype_id": "pilot", "title": "Pilot", "summary": "flies",
                 "required_option_refs": ["warp_drive"], "source_pages": "30"}
            ],
            "creation_shortcuts": []
        });
        let pack = finalize_starter_pack(raw, &ctx);
        let arch = pack["archetypes"].as_array().expect("archetypes array");
        assert_eq!(arch.len(), 1, "bogus-ref archetype dropped: {pack}");
        assert_eq!(arch[0]["archetype_id"], json!("scholar"));
    }

    #[test]
    fn ungrounded_recipe_is_dropped() {
        // An archetype with no source_pages is not grounded -> dropped.
        let ctx = ctx_with(Some("class"), &["athletics"], json!([]));
        let raw = json!({
            "archetypes": [
                {"archetype_id": "fighter", "title": "Fighter", "required_option_refs": ["class"], "source_pages": "12"},
                {"archetype_id": "floater", "title": "Floater", "required_option_refs": ["class"]}
            ],
            "creation_shortcuts": [
                {"shortcut_id": "quick", "title": "Quick build", "mode": "quick_start", "description": "pick class then go", "source_pages": "12"},
                {"shortcut_id": "ghost", "title": "Ungrounded", "mode": "quick_start", "description": "no page"}
            ]
        });
        let pack = finalize_starter_pack(raw, &ctx);
        assert_eq!(pack["archetypes"].as_array().unwrap().len(), 1, "ungrounded archetype dropped");
        assert_eq!(pack["archetypes"][0]["archetype_id"], json!("fighter"));
        assert_eq!(pack["creation_shortcuts"].as_array().unwrap().len(), 1, "ungrounded shortcut dropped");
        assert_eq!(pack["creation_shortcuts"][0]["shortcut_id"], json!("quick"));
    }

    #[test]
    fn empty_after_dropping_yields_empty_pack() {
        // Everything is ungrounded -> all dropped -> empty pack {}, NOT fabricated.
        let ctx = ctx_with(Some("class"), &[], json!([]));
        let raw = json!({
            "archetypes": [{"archetype_id": "x", "title": "X", "required_option_refs": ["class"]}],
            "creation_shortcuts": [{"shortcut_id": "y", "title": "Y", "mode": "guided", "description": "no page"}]
        });
        assert_eq!(finalize_starter_pack(raw, &ctx), json!({}), "all dropped -> {{}}");
    }

    #[test]
    fn ref_validated_against_option_catalog_ids() {
        // required_option_refs may also reference option-catalog ids, not just template fields.
        let catalogs = json!([
            {"catalog_id": "coc.options.v1", "option_groups": [
                {"group_id": "occupations", "options": [
                    {"option_id": "antiquarian", "title": "Antiquarian"}
                ]}
            ]}
        ]);
        let ctx = ctx_with(Some("occupation"), &[], catalogs);
        let raw = json!({
            "archetypes": [
                {"archetype_id": "a", "title": "A", "required_option_refs": ["antiquarian"], "source_pages": "30"}
            ],
            "creation_shortcuts": []
        });
        let pack = finalize_starter_pack(raw, &ctx);
        assert_eq!(pack["archetypes"].as_array().unwrap().len(), 1, "catalog option id is legal");
    }
}
