//! Parallel reader: plan (1 call on the toc) -> fan out 3 loosely-coupled
//! slices concurrently (resolution / character / gm+world) -> synthesize.
//! Each slice has a small focused context (better depth, bounded tokens); the
//! shared hypothesis from the plan keeps them coherent.
//!
//! The three slices are independently-callable (`read_character_slice`,
//! `read_resolution_and_gm`) so the staged orchestrator can emit the character
//! slice (Stage 1) before resolution/gm (Stage 2). `run_reader_parallel` is a
//! thin wrapper over plan + both pieces, preserving the legacy behavior.

use super::agent::{run_loop, ReaderResult};
use super::run_kit::GmRunKit;
use super::tools;
use super::units::Unit;
use anyhow::Result;
use serde_json::{json, Value};
use trpg_llm::{system, user, LlmClient};

/// Identity/hypothesis + per-slice planner hints from the cheap TOC plan call.
pub struct Plan {
    pub identity: String,
    pub hypothesis: String,
    pub res_hint: String,
    pub char_hint: String,
    pub gm_hint: String,
}

const BASE: &str = "You are an experienced GM reading an UNKNOWN tabletop RPG rulebook to learn to RUN it, via the tools. You never get the whole book. Do NOT be combat-centric — find THIS game's actual center of gravity. The table of contents has already been fetched for you (in the seed). Use search(keywords) to LOCATE pages, then read(pages). Fill ONLY your slice's fields; leave the rest empty (other agents handle them).";

pub async fn plan_phase(client: &dyn LlmClient, ruleset: &str, toc: &str) -> Result<Plan> {
    let prompt = format!(
        "Planning to read the TRPG \"{ruleset}\". Table of contents (sections / sizes / tags):\n{toc}\n\nFrom the toc, give your best hypothesis and a 3-slice reading plan. Return JSON only:\n{{\"identity\":\"genre/tone/premise + center of gravity (1-2 sentences)\",\"hypothesis\":\"likely core dice mechanic + signature track (1 sentence)\",\"hints\":{{\"resolution\":\"keywords/page-ranges to find the dice procedure, combat, damage, and state tracks\",\"character\":\"keywords/pages for character creation / minimal PC\",\"gm\":\"keywords/pages for GM guidance + the list of subsystems\"}}}}"
    );
    let v = client
        .complete_json(vec![system("You plan how to read an unknown TRPG rulebook. Concise, operational. Output only JSON."), user(prompt)], 0.2)
        .await
        .unwrap_or_else(|_| json!({}));
    Ok(Plan {
        identity: v.get("identity").and_then(Value::as_str).unwrap_or("").into(),
        hypothesis: v.get("hypothesis").and_then(Value::as_str).unwrap_or("").into(),
        res_hint: v.pointer("/hints/resolution").and_then(Value::as_str).unwrap_or("").into(),
        char_hint: v.pointer("/hints/character").and_then(Value::as_str).unwrap_or("").into(),
        gm_hint: v.pointer("/hints/gm").and_then(Value::as_str).unwrap_or("").into(),
    })
}

fn slice_tools(submit_name: &str, props: Value, required: &[&str]) -> Vec<Value> {
    let mut t = tools::nav_tools();
    t.push(tools::submit_tool(submit_name, "Submit your slice's fields, grounded in pages you read.", props, required));
    t
}

fn seed(ruleset: &str, toc: &str, hypothesis: &str, slice: &str, hint: &str) -> String {
    format!("Rulebook \"{ruleset}\". Shared hypothesis: {hypothesis}\n\nYour slice: {slice}. Planner hints: {hint}\n\nTOC (already fetched):\n{toc}\n\nUse search/read to ground your fields, then call your submit tool.")
}

// --- Slice schemas + system prompts (shared by the slice fns and the wrapper) ---

fn res_props() -> Value {
    json!({"core_resolution":{"type":"string"},"state_tracks":{"type":"string"},"core":tools::core_schema(),"source_pages":{"type":"string"}})
}

fn char_props() -> Value {
    json!({"character":{"type":"string","description":"prose: what a PC is + the minimal path to have one"},"source_pages":{"type":"string"},
        "character_template":{"type":"object","description":"COMPLETE sheet schema from the blank character sheet + creation chapter","properties":{
            "fields":{"type":"array","description":"every printed sheet field","items":{"type":"object","properties":{
                "field_id":{"type":"string","description":"snake_case e.g. ref, handgun_skill, hit_points, sanity"},
                "title":{"type":"string"},
                "field_type":{"type":"string","description":"stat|skill|derived|resource|text|number|choice"},
                "choices_material_id":{"type":"string","description":"for choice fields: locator/material id of the option list"},
                "notes":{"type":"string"}},"required":["field_id","title","field_type"]}},
            "sections":{"type":"array","description":"sheet sections grouping fields","items":{"type":"object","properties":{"section_id":{"type":"string"},"title":{"type":"string"},"field_ids":{"type":"array","items":{"type":"string"}}},"required":["section_id","title"]}},
            "derived_values":{"type":"array","description":"how derived fields are COMPUTED at chargen, e.g. hit_points = 10 + 5*ceil((BODY+WILL)/2)","items":{"type":"object","properties":{"field_id":{"type":"string"},"formula":{"type":"string"},"depends_on":{"type":"array","items":{"type":"string"}},"evaluator":{"type":"string"},"notes":{"type":"string"}},"required":["field_id","formula"]}},
            "creation_flow":{"type":"array","description":"the full ordered steps to make a playable PC","items":{"type":"object","properties":{
                "step_id":{"type":"string"},"title":{"type":"string"},"prompt":{"type":"string"},"inputs":{"type":"array","items":{"type":"string"}},"outputs":{"type":"array","items":{"type":"string"}}},"required":["step_id","title"]}},
            "validation_rules":{"type":"array","description":"legality checks, e.g. point-buy totals, max stat","items":{"type":"object","properties":{"rule_id":{"type":"string"},"severity":{"type":"string"},"description":{"type":"string"},"expression":{"type":"string"}},"required":["rule_id","description"]}}}},
        "option_catalogs":{"type":"array","description":"starter-complete option lists (Roles/classes, skills, starting gear). Enumerate options needed to build a STARTER character; for long lists (every weapon/spell/cyberware) give a locator entry instead.","items":{"type":"object","properties":{
            "group_id":{"type":"string"},"title":{"type":"string"},"category":{"type":"string","description":"role|class|skill|gear|background|spell|cyberware|..."},"starter_legal":{"type":"boolean"},
            "options":{"type":"array","items":{"type":"object","properties":{"option_id":{"type":"string"},"title":{"type":"string"},"summary":{"type":"string"}},"required":["option_id","title"]}},
            "locators":{"type":"array","description":"for long lists: where to find the full catalog (page/section)","items":{"type":"object","properties":{"title":{"type":"string"},"pages":{"type":"string"}}}}},"required":["group_id","title","category"]}}})
}

fn gm_props() -> Value {
    json!({"game_identity":{"type":"string"},"subsystem_map":{"type":"string"},"gm_procedures":{"type":"string"},"source_pages":{"type":"string"}})
}

fn res_sys_prompt() -> String {
    format!("{BASE}\n\nFOCUS = RESOLUTION ENGINE. Fill: core_resolution (prose), state_tracks, and the machine-readable `core` (dice, direction roll_high/roll_under/pool_count, compare_to, success_rule, resource_tracks, derived_formulas in {{field_id,formula,depends_on,evaluator,notes}}).\nYou MUST nail the EXACT dice procedure. The planner hints and the TOC may be UNRELIABLE on poorly-formatted books — do NOT trust them blindly. Hunt the dice yourself: search dice notations (\"6d4\",\"2d6\",\"d100\",\"1d10\",\"d20\",\"d6\") AND phrases (\"roll the dice\",\"how to play\",\"the basics\",\"making a roll\",\"how to resolve\",\"count\",\"success\",\"under your\"), and read the EARLY chapters (a game's core roll is usually explained in the first 10-30 pages). Never submit with dice unknown/unclear — keep searching until you can state the concrete dice (e.g. \"6d4, count the 3s\").")
}

fn char_sys_prompt() -> String {
    format!("{BASE}\n\nFOCUS = COMPLETE CHARACTER CREATION (this is the deep branch — read thoroughly, a player must be able to build a full legal starter character). Read the BLANK CHARACTER SHEET, the character-creation chapter, the skills list, and the starter gear/role lists. The character sheet IS the game's data schema — list EVERY printed field. Fill:\n- `character`: prose (what a PC is + the minimal path).\n- `character_template`: COMPLETE — `fields` (every stat/skill/derived/resource field with type), `sections` (sheet groupings), `derived_values` (HOW each derived field is COMPUTED at chargen, e.g. hit_points = 10 + 5*ceil((BODY+WILL)/2); humanity = 10*EMP; skill_base = STAT + skill_level), `creation_flow` (full ordered steps with inputs/outputs), `validation_rules` (point-buy totals, max stat, legality).\n- `option_catalogs`: starter-complete — enumerate the options a player needs to build a STARTER character (the Roles/classes, the skills list, starting gear/packages). For LONG lists (every weapon/spell/cyberware) DON'T enumerate — give a `locators` entry (title + pages) so they're looked up on demand.\nGround everything in pages you read.")
}

fn gm_sys_prompt() -> String {
    format!("{BASE}\n\nFOCUS = GM + WORLD. Fill: game_identity (refine), subsystem_map (the modes this game has + each one's centrality none/minor/one_of_several/central), gm_procedures (scene framing / when to roll / minimal one-session prep). Read the GM chapter + each subsystem's overview.")
}

/// The character slice in isolation — yields the buildable character template +
/// option catalogs (Stage 1 of staged parsing). Mirrors the char branch of
/// run_reader_parallel but does not run resolution/gm.
pub struct CharacterSlice {
    pub character: String,
    pub character_template: serde_json::Value,
    pub option_catalogs: serde_json::Value,
    pub source_pages: String,
}

pub async fn read_character_slice(client: &dyn LlmClient, units: &[Unit], ruleset: &str, plan: &Plan, budget: usize) -> Result<CharacterSlice> {
    let toc = tools::toc(units, 40);
    let char_tools = slice_tools("submit_character", char_props(), &["character", "character_template", "source_pages"]);
    let char_sys = char_sys_prompt();
    let char_seed = seed(ruleset, &toc, &plan.hypothesis, "character", &plan.char_hint);
    let c = run_loop(client, units, &char_sys, &char_seed, &char_tools, budget + 6, false).await?;
    Ok(CharacterSlice {
        character: c.run_kit.character,
        character_template: c.run_kit.character_template,
        option_catalogs: c.run_kit.option_catalogs,
        source_pages: c.run_kit.source_pages,
    })
}

/// The resolution + gm slices (Stage 2 of staged parsing). Runs both concurrently.
pub struct ResolutionGm {
    pub core_resolution: String,
    pub state_tracks: String,
    pub core: super::run_kit::CoreRules,
    pub game_identity: String,
    pub subsystem_map: String,
    pub gm_procedures: String,
    pub source_pages: String,
}

pub async fn read_resolution_and_gm(client: &dyn LlmClient, units: &[Unit], ruleset: &str, plan: &Plan, budget: usize) -> Result<ResolutionGm> {
    let toc = tools::toc(units, 40);
    let res_tools = slice_tools("submit_resolution", res_props(), &["core_resolution", "core", "source_pages"]);
    let gm_tools = slice_tools("submit_gm", gm_props(), &["game_identity", "subsystem_map", "gm_procedures", "source_pages"]);
    let res_sys = res_sys_prompt();
    let gm_sys = gm_sys_prompt();
    let res_seed = seed(ruleset, &toc, &plan.hypothesis, "resolution", &plan.res_hint);
    let gm_seed = seed(ruleset, &toc, &plan.hypothesis, "gm+world", &plan.gm_hint);
    let (r, g) = tokio::join!(
        run_loop(client, units, &res_sys, &res_seed, &res_tools, budget + 5, true),
        run_loop(client, units, &gm_sys, &gm_seed, &gm_tools, budget, false),
    );
    let (r, g) = (r?, g?);
    Ok(ResolutionGm {
        core_resolution: r.run_kit.core_resolution,
        state_tracks: r.run_kit.state_tracks,
        core: r.run_kit.core,
        game_identity: g.run_kit.game_identity,
        subsystem_map: g.run_kit.subsystem_map,
        gm_procedures: g.run_kit.gm_procedures,
        source_pages: [r.run_kit.source_pages, g.run_kit.source_pages].join(" ; "),
    })
}

pub async fn run_reader_parallel(client: &dyn LlmClient, units: &[Unit], ruleset: &str, budget: usize) -> Result<ReaderResult> {
    let toc = tools::toc(units, 40);
    let plan = plan_phase(client, ruleset, &toc).await?;

    // The three slices run concurrently (as before). The wrapper drives `run_loop`
    // directly (via the SAME shared schema/prompt helpers used by the public slice
    // fns) so it can aggregate per-slice telemetry field-for-field, while the
    // public `read_character_slice` / `read_resolution_and_gm` fns expose just the
    // staged orchestrator's needs.
    let res_tools = slice_tools("submit_resolution", res_props(), &["core_resolution", "core", "source_pages"]);
    let char_tools = slice_tools("submit_character", char_props(), &["character", "character_template", "source_pages"]);
    let gm_tools = slice_tools("submit_gm", gm_props(), &["game_identity", "subsystem_map", "gm_procedures", "source_pages"]);

    let res_sys = res_sys_prompt();
    let char_sys = char_sys_prompt();
    let gm_sys = gm_sys_prompt();

    let res_seed = seed(ruleset, &toc, &plan.hypothesis, "resolution", &plan.res_hint);
    let char_seed = seed(ruleset, &toc, &plan.hypothesis, "character", &plan.char_hint);
    let gm_seed = seed(ruleset, &toc, &plan.hypothesis, "gm+world", &plan.gm_hint);
    // Resolution must nail the dice; the character branch goes DEEP for a
    // complete buildable sheet -> both get more budget than the lean gm slice.
    let res_fut = run_loop(client, units, &res_sys, &res_seed, &res_tools, budget + 5, true);
    let char_fut = run_loop(client, units, &char_sys, &char_seed, &char_tools, budget + 6, false);
    let gm_fut = run_loop(client, units, &gm_sys, &gm_seed, &gm_tools, budget, false);

    let (r, c, g) = tokio::join!(res_fut, char_fut, gm_fut);
    let (r, c, g) = (r?, c?, g?);

    let pick = |a: String, b: &str| if a.trim().is_empty() { b.to_string() } else { a };
    let run_kit = GmRunKit {
        game_identity: pick(g.run_kit.game_identity, &plan.identity),
        core_resolution: r.run_kit.core_resolution,
        state_tracks: r.run_kit.state_tracks,
        character: c.run_kit.character,
        subsystem_map: g.run_kit.subsystem_map,
        gm_procedures: g.run_kit.gm_procedures,
        source_pages: [r.run_kit.source_pages, c.run_kit.source_pages, g.run_kit.source_pages].join(" ; "),
        core: r.run_kit.core,
        character_template: c.run_kit.character_template,
        option_catalogs: c.run_kit.option_catalogs,
    };
    let mut trace = r.trace;
    trace.extend(c.trace);
    trace.extend(g.trace);
    Ok(ReaderResult {
        run_kit,
        tool_calls: r.tool_calls + c.tool_calls + g.tool_calls,
        llm_calls: r.llm_calls + c.llm_calls + g.llm_calls + 1, // +1 plan
        prompt_tokens: r.prompt_tokens + c.prompt_tokens + g.prompt_tokens,
        completion_tokens: r.completion_tokens + c.completion_tokens + g.completion_tokens,
        trace,
    })
}
