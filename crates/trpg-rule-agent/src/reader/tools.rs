//! Deterministic book-navigation tools the reader agent calls via OpenAI
//! function-calling: toc / search / read. Ported from the hand-validated
//! prototype. Zero LLM. `tool_schemas()` exposes them (plus submit_run_kit).

use super::units::Unit;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Collapse OCR column-doubling: drop a line equal to the previous one.
pub fn dedup(text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut prev = "";
    for ln in text.lines() {
        let s = ln.trim();
        if !s.is_empty() && s == prev {
            continue;
        }
        out.push(ln);
        prev = s;
    }
    out.join("\n")
}

/// Section map: heading | pages | size | top categories/mech_tags. The "shape".
pub fn toc(units: &[Unit], max: usize) -> String {
    struct Sec { pages: Vec<u32>, chars: usize, n: usize, cats: BTreeMap<String, usize>, tags: BTreeMap<String, usize> }
    let mut secs: BTreeMap<String, Sec> = BTreeMap::new();
    for u in units {
        if u.is_noise() {
            continue;
        }
        let key = u.heading_context.first().cloned().filter(|s| !s.is_empty()).unwrap_or_else(|| u.title.clone());
        let key = if key.is_empty() { "?".to_string() } else { key };
        let s = secs.entry(key).or_insert_with(|| Sec { pages: vec![], chars: 0, n: 0, cats: BTreeMap::new(), tags: BTreeMap::new() });
        s.pages.extend(&u.page_numbers);
        s.chars += u.content_text.len();
        s.n += 1;
        *s.cats.entry(u.category.clone()).or_default() += 1;
        for t in u.mech_tags() {
            *s.tags.entry(t).or_default() += 1;
        }
    }
    let mut rows: Vec<(u32, String, String, usize, usize, String, String)> = secs
        .into_iter()
        .map(|(k, s)| {
            let mut pp = s.pages.clone();
            pp.sort_unstable();
            pp.dedup();
            let rng = if pp.is_empty() { "?".into() } else { format!("{}-{}", pp[0], pp[pp.len() - 1]) };
            let cats = top_n(&s.cats, 2);
            let tags = top_n(&s.tags, 4);
            (pp.first().copied().unwrap_or(9999), k, rng, s.chars, s.n, cats, tags)
        })
        .collect();
    rows.sort();
    let mut out = String::from(" pages    chars units  heading | cats | mech_tags\n");
    for (_, k, rng, ch, n, cats, tags) in rows.iter().take(max) {
        out.push_str(&format!("{:>9}  {:>6} {:>5}  {} | {} | {}\n", rng, ch, n, trunc(k, 46), cats, tags));
    }
    out.push_str(&format!("[{} sections]\n", rows.len()));
    out
}

fn top_n(m: &BTreeMap<String, usize>, n: usize) -> String {
    let mut v: Vec<(&String, &usize)> = m.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1));
    v.into_iter().take(n).map(|(k, c)| format!("{}:{}", k, c)).collect::<Vec<_>>().join(",")
}
fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { s.chars().take(n).collect() }
}

fn score(u: &Unit, kws: &[String]) -> f64 {
    let hay = u.haystack();
    let hits: usize = kws.iter().map(|k| hay.matches(&k.to_lowercase()).count()).sum();
    if hits == 0 { 0.0 } else { hits as f64 * (0.5 + u.index_weight()) }
}

/// Ranked hits: page | category | heading | snippet (+unit_id). Use to locate.
pub fn search(units: &[Unit], keywords: &[String], n: usize) -> String {
    let mut scored: Vec<(f64, &Unit)> = units.iter().filter(|u| !u.is_noise()).map(|u| (score(u, keywords), u)).filter(|x| x.0 > 0.0).collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut out = format!("search {:?} -> {} hits (top {}):\n", keywords, scored.len(), n);
    for (_, u) in scored.iter().take(n) {
        let snip: String = dedup(&u.content_text).replace('\n', " ").chars().take(140).collect();
        out.push_str(&format!("  p{:<4} [{:<14}] {:<34} :: {}\n", u.first_page().map(|p| p.to_string()).unwrap_or_else(|| "?".into()), trunc(&u.category, 14), trunc(&u.head_path(), 34), snip));
    }
    out
}

/// Full text of a page range like "129-130" (OCR-deduped).
pub fn read(units: &[Unit], pages: &str) -> String {
    let (a, b) = parse_range(pages);
    let mut out = String::new();
    let mut total = 0usize;
    for u in units.iter().filter(|u| !u.is_noise()) {
        if u.page_numbers.iter().any(|&p| a <= p && p <= b) {
            let body = dedup(&u.content_text);
            total += body.len();
            out.push_str(&format!("--- p{} [{}] {} ---\n{}\n\n", u.first_page().unwrap_or(0), u.category, u.head_path(), body));
        }
    }
    out.push_str(&format!("[read, {} chars]\n", total));
    out
}

fn parse_range(s: &str) -> (u32, u32) {
    let s = s.trim();
    if let Some((a, b)) = s.split_once('-') {
        (a.trim().parse().unwrap_or(0), b.trim().parse().unwrap_or(u32::MAX))
    } else {
        let p = s.parse().unwrap_or(0);
        (p, p)
    }
}

/// The 3 navigation tools every reader/slice agent shares.
pub fn nav_tools() -> Vec<Value> {
    vec![
        json!({"type":"function","function":{"name":"get_toc","description":"Table of contents: sections with page ranges, sizes, categories, mechanics tags. Call once first to see the game's shape.","parameters":{"type":"object","properties":{},"required":[]}}}),
        json!({"type":"function","function":{"name":"search","description":"Keyword search over the rulebook. Returns ranked hits with page numbers, section, snippets. Use to LOCATE the core rule, signature track, character creation, GM guidance.","parameters":{"type":"object","properties":{"keywords":{"type":"array","items":{"type":"string"}}},"required":["keywords"]}}}),
        json!({"type":"function","function":{"name":"read","description":"Read full text of a page range, e.g. \"129-130\". Use on pages search returned.","parameters":{"type":"object","properties":{"pages":{"type":"string"}},"required":["pages"]}}}),
    ]
}

/// The machine-readable `core` object schema (shared by full + resolution-slice submit).
pub fn core_schema() -> Value {
    json!({"type":"object","description":"machine-readable resolution core, grounded in the pages you read","properties":{
        "dice":{"type":"string","description":"core dice e.g. 1d10, 1d100, 6d4, 2d6"},
        "direction":{"type":"string","enum":["roll_high","roll_under","pool_count"]},
        "compare_to":{"type":"string","description":"e.g. DV (GM-set/range table), skill%, count of target faces"},
        "success_rule":{"type":"string","description":"e.g. total >= DV; tie -> defender"},
        "compare":{"type":"string","enum":["meet_or_beat","roll_under","count_faces"],"description":"MACHINE-READABLE success operator the engine resolves with: meet_or_beat=total>=target; roll_under=total<=target; count_faces=count dice showing target_face, succeed when count>=success_threshold"},
        "target_face":{"type":"integer","description":"count_faces only: which die face counts as a success, e.g. 3 for Triangle 6d4-count-3s"},
        "success_threshold":{"type":"integer","description":"count_faces only: how many counted dice are needed to succeed, e.g. 1"},
        "target_number":{"type":"integer","description":"meet_or_beat/roll_under with a FIXED number: the static target; omit when target is per-skill or a DV table"},
        "resource_tracks":{"type":"array","description":"signature state tracks; include on_outcome so the engine updates them from a roll's result without hardcoding","items":{"type":"object","properties":{
            "id":{"type":"string","description":"snake_case key e.g. chaos, sanity, hp, harm"},
            "name":{"type":"string"},"kind":{"type":"string"},
            "owner_kind":{"type":"string","enum":["actor","scene"],"description":"who holds it: a shared scene pool (e.g. Chaos) or the actor (e.g. SAN/HP)"},
            "zero_means":{"type":"string"},
            "initial":{"type":"integer"},"max":{"type":"integer"},
            "on_outcome":{"type":"array","description":"how a resolved roll updates this track — covers Chaos gain, Sanity loss, HP damage, etc.","items":{"type":"object","properties":{
                "trigger":{"type":"string","enum":["on_success","on_failure","always"],"description":"when this rule fires (default always)"},
                "check_match":{"type":"string","description":"OPTIONAL scope: only fire on checks whose intent/label/tested-parameter contains one of these pipe-separated alternatives. Include the term in the RULEBOOK'S OWN LANGUAGE plus the common English term and any in-text synonyms players use, so the same check matches across languages — e.g. a Sanity track: sanity|san|理智|恐惧|horror. Derive these from how the rulebook actually names the check; do NOT invent. Omit entirely for rules that apply to every roll (e.g. Triangle Chaos)."},
                "op":{"type":"string","enum":["add","subtract","set"]},
                "amount":{"type":"string","description":"a dice expr like 1d6 (rolled), =value (uses the `when` outcome field), =field (reads a check-outcome field directly — ONLY total|target|success|success_count|pool_miss_count|success_tier_rank exist; write it bare, e.g. =pool_miss_count, never with <> brackets), or a fixed int. NEVER reference a derived_formulas field_id or an invented name (=damage, =damage_after_armor, =sanity_loss read NOTHING — the rule is dead and gets dropped); an amount from a separate roll (weapon damage) is NOT expressible here — omit the rule, the combat/effect path applies it. e.g. CoC Sanity loss on a failed roll = 1d6"},
                "default_amount":{"type":"string","description":"OPTIONAL fallback dice/int used when amount's =field is absent on the outcome AND this track is the check's tested parameter, e.g. CoC default Sanity loss 1d6. Lets a bare 'make a Sanity roll' still cost SAN."},
                "when":{"type":"string","description":"for amount==value: which outcome field — total|target|success|success_count|pool_miss_count|success_tier_rank"},
                "mitigation":{"type":"string","description":"optional, e.g. armor.sp — subtract target armor before applying (HP damage)"}}}},
            "thresholds":{"type":"array","description":"consequences when the track crosses a value, or when one roll changes it a lot","items":{"type":"object","properties":{
                "at":{"type":"integer","description":"cumulative threshold value"},"direction":{"type":"string","enum":["at_or_above","at_or_below"]},
                "loss_in_one_go":{"type":"integer","description":"fires when a single subtract removes >= this much (e.g. CoC lose >=5 Sanity at once -> temporary insanity)"},
                "consequence":{"type":"string"}},"required":["consequence"]}}
        },"required":["id"]}},
        "derived_formulas":{"type":"array","description":"executable formulas grounded in the rules","items":{"type":"object","properties":{
            "field_id":{"type":"string","description":"snake_case e.g. ranged_attack_roll"},
            "formula":{"type":"string","description":"e.g. 1d10 + REF + ranged_weapon_skill, or 1d100 (roll under skill)"},
            "depends_on":{"type":"array","items":{"type":"string"}},
            "evaluator":{"type":"string","description":"e.g. attack_vs_dv, percentile_roll_under, pool_count, damage_minus_armor"},
            "notes":{"type":"string"}},"required":["field_id","formula"]}},
        "success_bands":{"type":"array","description":"graded success LEVELS (CoC critical/extreme/hard/regular/fumble, D&D nat20/nat1, PbtA 10+/7-9/6-). Omit for pure pass/fail games. The engine reports the matching band as success_tier.","items":{"type":"object","properties":{
            "id":{"type":"string","description":"snake_case e.g. critical, extreme, hard, regular, failure, fumble"},
            "label":{"type":"string"},
            "rank":{"type":"integer","description":"higher = better; the engine returns the best-ranked matching band"},
            "test":{"type":"object","description":"predicate vs the resolved roll and its target","properties":{
                "kind":{"type":"string","enum":["roll_under_or_equal","roll_under_fraction","meet_or_beat_fraction","exact","in_range","otherwise"]},
                "denominator":{"type":"integer","description":"roll_under_fraction/meet_or_beat_fraction: total compared to target*numerator/denominator (CoC hard=2, extreme=5)"},
                "numerator":{"type":"integer","description":"fraction numerator (default 1)"},
                "value":{"type":"integer","description":"exact: the exact roll total (e.g. 1 for a CoC critical)"},
                "min":{"type":"integer"},"max":{"type":"integer"},
                "when":{"type":"object","description":"guard: apply only if e.g. {target_lt:50} or {target_gte:50}"},
                "unless":{"type":"object","description":"guard: skip if the condition holds"}},"required":["kind"]}},"required":["id","rank","test"]}}},
        "required":["dice","direction","success_rule"]})
}

/// Build a terminal submit function tool with the given owned properties.
pub fn submit_tool(name: &str, desc: &str, properties: Value, required: &[&str]) -> Value {
    json!({"type":"function","function":{"name":name,"description":desc,"parameters":{"type":"object","properties":properties,"required":required}}})
}

/// Full single-loop tool set (nav + the whole run-kit submit).
pub fn tool_schemas() -> Vec<Value> {
    let mut t = nav_tools();
    t.push(submit_tool(
        "submit_run_kit",
        "Submit the final GM run-kit when all 6 questions are answered from pages you read. Also fill the machine-readable `core` from the same pages.",
        json!({
            "game_identity":{"type":"string"},"core_resolution":{"type":"string"},"state_tracks":{"type":"string"},
            "character":{"type":"string"},"subsystem_map":{"type":"string"},"gm_procedures":{"type":"string"},
            "source_pages":{"type":"string","description":"pages you read, comma-separated"},
            "core": core_schema()}),
        &["game_identity", "core_resolution", "state_tracks", "character", "subsystem_map", "gm_procedures", "source_pages", "core"],
    ));
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn unit(id: &str, head: &str, body: &str, page: u32, cat: &str, w: f64) -> Unit {
        Unit {
            unit_id: id.into(),
            title: head.into(),
            content_text: body.into(),
            heading_context: vec![head.into()],
            page_numbers: vec![page],
            category: cat.into(),
            signal_class: "signal".into(),
            metadata: json!({"index_weight": w}),
        }
    }

    #[test]
    fn dedup_collapses_doubled_lines() {
        assert_eq!(dedup("a\na\nb\nb\na"), "a\nb\na");
    }

    #[test]
    fn search_ranks_by_keyword_and_weight() {
        let units = vec![
            unit("u1", "Skill Checks", "add your STAT + Skill + 1d10 vs the Difficulty Value DV", 129, "rule_or_procedure", 0.9),
            unit("u2", "Lore", "night city is a dangerous place", 5, "lore_or_guidance", 0.3),
        ];
        let out = search(&units, &["1d10".into(), "DV".into()], 5);
        assert!(out.contains("p129"));
        assert!(out.find("p129").unwrap() < out.find("hits").map(|_| usize::MAX).unwrap_or(0).min(out.len()));
        assert!(!out.contains("p5  ")); // lore not matched
    }

    #[test]
    fn read_returns_page_range_text() {
        let units = vec![
            unit("u1", "A", "alpha body", 10, "rule_or_procedure", 0.5),
            unit("u2", "B", "bravo body", 12, "rule_or_procedure", 0.5),
            unit("u3", "C", "charlie body", 20, "rule_or_procedure", 0.5),
        ];
        let out = read(&units, "10-12");
        assert!(out.contains("alpha body") && out.contains("bravo body"));
        assert!(!out.contains("charlie body"));
    }

    #[test]
    fn toc_groups_and_drops_noise() {
        let mut n = unit("n", "junk", "noise noise", 1, "noise", 0.1);
        n.signal_class = "noise".into();
        let units = vec![unit("u1", "Combat", "fight", 100, "rule_or_procedure", 0.6), n];
        let out = toc(&units, 20);
        assert!(out.contains("Combat"));
        assert!(!out.contains("junk")); // noise dropped
    }

    #[test]
    fn schemas_have_four_tools() {
        let s = tool_schemas();
        assert_eq!(s.len(), 4);
        assert_eq!(s[0]["function"]["name"], "get_toc");
        assert_eq!(s[3]["function"]["name"], "submit_run_kit");
    }

    #[test]
    fn on_outcome_amount_docs_carry_the_engine_outcome_vocabulary() {
        // The amount AND when descriptions must teach exactly the legal
        // `=field` vocabulary (trpg-model AMOUNT_RESOLVABLE) — drift here
        // re-teaches the extractor to invent dead fields like
        // "=damage_after_armor" (silent no-op rules, dropped by the
        // finalize guard).
        let vocab = trpg_model::outcome_fields::AMOUNT_RESOLVABLE.join("|");
        let docs = serde_json::to_string(&tool_schemas()).unwrap();
        assert_eq!(
            docs.matches(vocab.as_str()).count(),
            2,
            "amount + when descriptions must both list the vocabulary: {vocab}"
        );
    }
}
