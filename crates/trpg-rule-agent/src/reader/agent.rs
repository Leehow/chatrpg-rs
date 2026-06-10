//! The rulebook-reader ReAct loop, driven by the LLM via OpenAI function-calling
//! (text pseudo-tool protocols make gpt refuse — real tool-calling is required).
//! Mirrors the hand-validated prototype: get_toc -> search -> read -> submit.

use super::run_kit::GmRunKit;
use super::tools;
use super::units::Unit;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use trpg_llm::LlmClient;

const SYSTEM: &str = r#"You are an experienced GM reading an UNKNOWN tabletop RPG rulebook to learn to RUN it, using the provided tools. You never get the whole book — navigate it like a human GM: get the shape, find the core, stop when you could run a session. Do NOT be combat-centric: some games barely have combat (Call of Cthulhu is investigation/horror; some indie games have none). Find THIS game's actual center of gravity.

METHOD: get_toc once to see the shape and form a hypothesis (identity, core resolution mechanic, signature state track, subsystems). Then search(keywords) to LOCATE the core resolution rule, the signature track, character creation, and GM guidance (search returns page numbers). Then read(pages) the located pages for the real text. Prefer search -> read(pages). Read targeted.

GOAL: gather enough to answer 6 questions, EACH with a source page, then call submit_run_kit:
 1 identity (genre/tone/premise, what players do, center_of_gravity)
 2 core_resolution (dice; roll-high or roll-under; vs what; modifiers; success rule)
 3 state_tracks (what depletes/matters: hp/sanity/chaos/humanity/...; what zero means)
 4 character (what a PC is + the minimal path to have one: pregen/quickstart/build)
 5 subsystem_map (the modes this game has + each one's centrality: none/minor/one_of_several/central)
 6 gm_procedures (how the GM frames scenes / when to roll / minimal one-session prep)

CRITICAL — you MUST nail core_resolution: the exact dice procedure (which dice, e.g. d100 / 2d6 / 6d4 / 1d10+stat; roll-high or roll-under; vs what; how success is counted). If the toc is unstructured/garbage, hunt it AGGRESSIVELY with search: dice notations ("6d4","2d6","d100","1d10","d20","d6") AND phrases ("roll the dice","how to play","resolve","success","count","succeed","under your"). NEVER submit with core_resolution unknown/unclear — keep searching until you can state the concrete dice mechanic from a page you read.

Since you already read the rules pages, ALSO fill the machine-readable `core` in the same pass (it feeds the deterministic engine): core.dice, core.direction (roll_high/roll_under/pool_count), core.compare_to, core.success_rule, AND the TYPED success operator so the engine resolves WITHOUT re-reading prose: core.compare (meet_or_beat|roll_under|count_faces) plus the numbers it needs — for count_faces give core.target_face (which die face counts, e.g. 3) and core.success_threshold (how many needed, e.g. 1); for a FIXED meet_or_beat/roll_under target give core.target_number (omit when target is per-skill or a DV table). Examples: Triangle 6d4-count-3s-on-1+ => {dice:"6d4",direction:"pool_count",compare:"count_faces",target_face:3,success_threshold:1}; CoC d100-under-skill => {dice:"1d100",direction:"roll_under",compare:"roll_under"} (no target_number, it's the skill). If the game has graded success LEVELS (not just pass/fail), ALSO fill core.success_bands = [{id,label,rank(int,higher=better),test:{kind, ...}}] so the engine reports the tier: test.kind ∈ roll_under_or_equal | roll_under_fraction(denominator) | meet_or_beat_fraction(numerator,denominator) | exact(value) | in_range(min,max) | otherwise, with optional guard when/unless {target_lt|target_gte}. CoC example: [{id:"critical",label:"critical",rank:5,test:{kind:"exact",value:1}},{id:"extreme",rank:4,test:{kind:"roll_under_fraction",denominator:5}},{id:"hard",rank:3,test:{kind:"roll_under_fraction",denominator:2}},{id:"regular",rank:2,test:{kind:"roll_under_or_equal"}},{id:"fumble",rank:0,test:{kind:"in_range",min:96,max:100,when:{target_lt:50}}},{id:"fumble",rank:0,test:{kind:"in_range",min:100,max:100,when:{target_gte:50}}},{id:"failure",rank:1,test:{kind:"otherwise"}}]. Omit success_bands entirely for pure pass/fail games. Also fill core.resource_tracks for the signature tracks (HP/Sanity/Chaos/Humanity/...) WITH the engine-update rule so they change without hardcoding: each = {id, name, owner_kind (actor|scene), initial, max, zero_means, on_outcome:[{trigger, op, amount, when?, mitigation?}], thresholds:[{at|loss_in_one_go, consequence}]}. on_outcome.trigger = on_success|on_failure|always; amount = a dice expr like "1d6" (rolled), or "=value" (uses the `when` outcome field: success_count/pool_miss_count/total/success), or a fixed int. Examples — Triangle Chaos: {id:"chaos", owner_kind:"scene", initial:0, max:12, on_outcome:[{trigger:"always", when:"pool_miss_count", op:"add", amount:"=value"}], thresholds:[{at:12, consequence:"Chaos overflows; anomaly escalates"}]}. CoC Sanity: {id:"sanity", owner_kind:"actor", initial:50, max:99, on_outcome:[{trigger:"on_failure", op:"subtract", amount:"1d6"}], thresholds:[{loss_in_one_go:5, consequence:"temporary insanity"},{at:0, direction:"at_or_below", consequence:"permanent insanity"}]}. Cyberpunk HP: {id:"hp", owner_kind:"actor", on_outcome:[{trigger:"always", when:"total", op:"subtract", amount:"=value", mitigation:"armor.sp"}]}. Also fill core.derived_formulas — executable formulas grounded in the pages, engine shape {field_id, formula, depends_on[], evaluator, notes}. Examples: ranged_attack_roll formula "1d10 + REF + ranged_weapon_skill"; skill_check formula "1d100 (succeed if <= skill)"; damage_after_armor formula "max(0, weapon_damage - SP)". Only include formulas you can ground in a page you read; omit what the game lacks (e.g. a no-combat game has no attack/damage formulas).

EFFICIENCY (strict): you have a budget of ~12 tool calls. Each search already returns enough to know where to read; read a page range ONCE and never re-read it. Do NOT gather extra detail "to be safe" — as soon as you can answer all 6 questions, call submit_run_kit IMMEDIATELY. A good run is ~1 toc + ~4-6 searches + ~3-5 reads + submit. Prefer batching keywords into one search over many searches."#;

pub struct ReaderResult {
    pub run_kit: GmRunKit,
    pub tool_calls: usize,
    pub llm_calls: usize,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub trace: Vec<Value>,
}

/// Single-loop reader (full run-kit in one ReAct loop). Fallback / baseline.
pub async fn run_reader(client: &dyn LlmClient, units: &[Unit], ruleset: &str, max_tools: usize) -> Result<ReaderResult> {
    let seed = format!("The rulebook is \"{ruleset}\". Learn to run it. Start with get_toc.");
    run_loop(client, units, SYSTEM, &seed, &tools::tool_schemas(), max_tools, true).await
}

/// The shared ReAct loop. Terminates on any tool whose name starts with
/// "submit"; parses its args into a (partial) GmRunKit. `require_core` enables
/// the core-resolution quality gate (only the resolution slice needs it).
pub(crate) async fn run_loop(
    client: &dyn LlmClient,
    units: &[Unit],
    system: &str,
    seed_user: &str,
    tool_schemas: &[Value],
    max_tools: usize,
    require_core: bool,
) -> Result<ReaderResult> {
    let tool_schemas = tool_schemas.to_vec();
    let mut msgs: Vec<Value> = vec![
        json!({"role": "system", "content": system}),
        json!({"role": "user", "content": seed_user}),
    ];
    let (mut tool_calls, mut llm_calls, mut pt, mut ct) = (0usize, 0usize, 0u64, 0u64);
    let mut trace = Vec::new();

    for _ in 0..(max_tools + 8) {
        let resp = client.complete_with_tools(msgs.clone(), tool_schemas.clone()).await?;
        llm_calls += 1;
        pt += resp.pointer("/usage/prompt_tokens").and_then(Value::as_u64).unwrap_or(0);
        ct += resp.pointer("/usage/completion_tokens").and_then(Value::as_u64).unwrap_or(0);
        let message = resp.pointer("/choices/0/message").cloned().unwrap_or_else(|| json!({}));
        let tcs = message.get("tool_calls").and_then(Value::as_array).cloned().unwrap_or_default();

        if tcs.is_empty() {
            msgs.push(message);
            msgs.push(json!({"role": "user", "content": "Use the tools to gather, then call the submit tool. Continue."}));
            continue;
        }
        msgs.push(message);
        let mut rejected = false;
        for tc in &tcs {
            let name = tc.pointer("/function/name").and_then(Value::as_str).unwrap_or("");
            let args: Value = tc
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_else(|| json!({}));
            let id = tc.get("id").and_then(Value::as_str).unwrap_or("");

            if name.starts_with("submit") {
                let run_kit: GmRunKit = serde_json::from_value(args).unwrap_or_default();
                if require_core && core_unresolved(&run_kit) && tool_calls < max_tools + 10 {
                    msgs.push(json!({"role": "tool", "tool_call_id": id, "content": "REJECTED: core_resolution is unresolved — you must state the exact dice procedure (fill core.dice too). Search dice notations (6d4, 2d6, d100, 1d10, d20) and phrases (\"roll\", \"how to play\", \"success\", \"count\", \"under your\"), READ those pages, then re-submit."}));
                    rejected = true;
                    break;
                }
                return Ok(ReaderResult { run_kit, tool_calls, llm_calls, prompt_tokens: pt, completion_tokens: ct, trace });
            }
            tool_calls += 1;
            let out = dispatch(units, name, &args);
            trace.push(json!({"tool": name, "args": args}));
            msgs.push(json!({"role": "tool", "tool_call_id": id, "content": out}));
        }
        if !rejected && tool_calls >= max_tools {
            msgs.push(json!({"role": "user", "content": "Read budget reached. Call the submit tool NOW with what you have; fill any thin field from the pages you already read."}));
        }
    }
    Err(anyhow!("reader hit max tool calls ({max_tools}) without submitting"))
}

fn dispatch(units: &[Unit], name: &str, args: &Value) -> String {
    match name {
        "get_toc" => tools::toc(units, 40),
        "search" => {
            let kws: Vec<String> = args
                .get("keywords")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            cap(tools::search(units, &kws, 8), 2800)
        }
        "read" => cap(tools::read(units, args.get("pages").and_then(Value::as_str).unwrap_or("")), 2800),
        _ => format!("unknown tool {name}"),
    }
}

fn cap(s: String, n: usize) -> String {
    if s.len() <= n { s } else { s.chars().take(n).collect() }
}

/// True when the resolution core is vague/unknown — require both a concrete
/// prose core_resolution AND a structured core.dice before accepting.
fn core_unresolved(rk: &GmRunKit) -> bool {
    let cr = rk.core_resolution.to_lowercase();
    let dice = rk.core.dice.trim().to_lowercase();
    let bad_dice = dice.is_empty()
        || ["unknown", "unclear", "n/a", "na", "?", "tbd", "none", "not confirmed", "various", "see rules", "multiple"].contains(&dice.as_str());
    let vague = ["not present", "not available", "unclear", "could not", "not found", "not in the pages", "is not present", "unknown", "unable to", "no explicit", "not confirmed"];
    bad_dice
        || cr.len() < 30
        || vague.iter().any(|v| cr.contains(v))
        || rk.core.success_rule.to_lowercase().contains("not confirmed")
}
