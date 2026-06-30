//! Adventure IR — mission scoring extraction (P2-1). Deterministic, source-
//! grounded text parsers that pull the scored mechanics out of one mission's
//! window of authored prose: Optional Objectives (→ Commendation/Demerit
//! [`ScoreEffect`]s), the Chaos ability ladder (→ [`TrackerSpec`] with rungs),
//! and the branching Aftermath (→ Outcome [`ContentUnit`]s).
//!
//! These are NOT LLM calls and NOT ruleset/module-name branches — they recognize
//! the repeated structural shape of the text and extract only what is literally
//! present (fail-closed: nothing recognized → nothing emitted, never fabricated).
//! The `success_when` of a scored objective is stored as
//! [`PredicateExpr::OpaqueAuthoredText`] (GM-facing, never auto-fired) because the
//! condition ("if you conduct an experiment") is authored free-form.
use super::mission_text::{columns_of, section_slice, slug, PHASE_MARKERS};
use super::{
    ContentUnit, FacetKind, ObjectiveSpec, PredicateExpr, ScoreEffect, TrackerKind, TrackerRung,
    TrackerSpec, UnitKind, VisibilityPolicy,
};
use crate::SourceRef;

fn evidence(base: &SourceRef, section: &str) -> Vec<SourceRef> {
    let mut e = base.clone();
    let mut path = base.section_path.clone();
    path.push(section.to_string());
    e.section_path = path;
    vec![e]
}

/// Parse the Optional Objectives block into scored [`ObjectiveSpec`]s.
/// Recognizes bullet lines of the form `+N Commendation(s)/Demerit <condition>`.
pub fn parse_optional_objectives(
    mission_id: &str,
    base: &SourceRef,
    text: &str,
) -> Vec<ObjectiveSpec> {
    let block = match section_slice(text, &["Optional Objectives"], PHASE_MARKERS) {
        Some(b) => b,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    // A bullet may sit in EITHER column; scan every segment of every line.
    for line in block.lines() {
        for seg in columns_of(line) {
            if let Some(obj) = parse_objective_line(mission_id, base, out.len(), seg) {
                out.push(obj);
            }
        }
    }
    out
}

fn parse_objective_line(
    mission_id: &str,
    base: &SourceRef,
    n: usize,
    left: &str,
) -> Option<ObjectiveSpec> {
    // Strip a leading bullet glyph ("f", "•", "-").
    let s = left
        .trim_start_matches(|c: char| c == 'f' || c == '•' || c == '-' || c.is_whitespace())
        .trim_start();
    if !s.starts_with('+') {
        return None;
    }
    let rest = s[1..].trim_start();
    let mut it = rest.splitn(2, char::is_whitespace);
    let num: i64 = it.next()?.parse().ok()?;
    let after = it.next()?.trim_start();
    let (label, cond) = if let Some(c) = after.strip_prefix("Commendations") {
        ("Commendation", c)
    } else if let Some(c) = after.strip_prefix("Commendation") {
        ("Commendation", c)
    } else if let Some(c) = after.strip_prefix("Demerits") {
        ("Demerit", c)
    } else if let Some(c) = after.strip_prefix("Demerit") {
        ("Demerit", c)
    } else {
        return None;
    };
    let cond = cond.trim();
    Some(ObjectiveSpec {
        id: format!("obj.opt.{mission_id}.{n}"),
        mission_id: Some(mission_id.to_string()),
        mandatory: false,
        success_when: PredicateExpr::OpaqueAuthoredText {
            raw_text: cond.to_string(),
        },
        failure_when: None,
        score_effects: vec![ScoreEffect {
            label: label.to_string(),
            delta: num,
        }],
        rewards: vec![],
        deadline: None,
        source_evidence: evidence(base, "Optional Objectives"),
    })
}

/// Parse the CHAOS EFFECTS ability ladder into a [`TrackerSpec`] with rungs.
/// Two authored layouts occur: a three-column row `<cost> Chaos | <Ability> |
/// detail`, and a vertically-stacked form where a lone `<N> Chaos` line precedes
/// the ability word on its own line. Both are read; nothing else is fabricated.
pub fn parse_chaos_tracker(mission_id: &str, base: &SourceRef, text: &str) -> Option<TrackerSpec> {
    let block = section_slice(text, &["CHAOS EFFECTS"], PHASE_MARKERS)?;
    let mut rungs = Vec::new();
    let mut pending_cost: Option<i64> = None;
    for line in block.lines() {
        if let Some(r) = parse_chaos_rung(line) {
            rungs.push(r);
            pending_cost = None;
            continue;
        }
        let cols = columns_of(line);
        if cols.len() == 1 {
            if let Some(cost) = lone_chaos_cost(cols[0]) {
                pending_cost = Some(cost); // stacked: cost line, ability follows
                continue;
            }
            if let Some(cost) = pending_cost {
                if let Some(label) = lone_ability_label(cols[0]) {
                    rungs.push(TrackerRung {
                        cost,
                        label,
                        detail: String::new(),
                    });
                    pending_cost = None;
                }
            }
        }
    }
    if rungs.is_empty() {
        return None;
    }
    Some(TrackerSpec {
        id: format!("tracker.chaos.{mission_id}"),
        kind: TrackerKind::Countdown,
        label: "Chaos".to_string(),
        start: 0,
        threshold: 0,
        anchor: None,
        at_threshold: vec![],
        visible_to_players: false,
        source_evidence: evidence(base, "Chaos Effects"),
        rungs,
    })
}

fn parse_chaos_rung(line: &str) -> Option<TrackerRung> {
    // The rung is a three-column micro-row: `<cost> Chaos | <Ability> | <detail>`.
    // Read it as segments so the cost+unit stays intact (a page-level decolumnize
    // would split "2 Chaos" from its ability at the wide blank band).
    let cols = columns_of(line);
    let mut cost_seg = cols.first()?.split_whitespace();
    let cost: i64 = cost_seg.next()?.parse().ok()?;
    if !cost_seg.next()?.eq_ignore_ascii_case("Chaos") {
        return None;
    }
    let label_seg = cols.get(1)?;
    let mut words = label_seg.splitn(2, char::is_whitespace);
    let label = words.next()?.trim_end_matches('*').trim();
    if label.is_empty() || !label.chars().next()?.is_ascii_uppercase() {
        return None;
    }
    // Detail: inline remainder of the ability segment, else the next segment.
    let inline = words.next().unwrap_or("").trim();
    let detail = if !inline.is_empty() {
        inline.to_string()
    } else {
        cols.get(2).map(|s| s.trim()).unwrap_or("").to_string()
    };
    Some(TrackerRung {
        cost,
        label: label.to_string(),
        detail,
    })
}

/// A lone `<N> Chaos` cost line (the stacked CHAOS layout: the cost sits on its
/// own line and the ability follows on the next). Returns N. Fail-closed: any
/// segment that is not exactly `<integer> Chaos` yields `None`.
fn lone_chaos_cost(seg: &str) -> Option<i64> {
    let mut it = seg.split_whitespace();
    let cost: i64 = it.next()?.parse().ok()?;
    if !it.next()?.eq_ignore_ascii_case("Chaos") {
        return None;
    }
    if it.next().is_some() {
        return None; // more than "<N> Chaos" → not a lone cost line
    }
    Some(cost)
}

/// A lone ability label line in the stacked CHAOS layout (a short Title-Case word
/// such as `Refresh`/`Return`, optionally `*`-suffixed). Fail-closed: rejects
/// prose so a wrapped detail line is never mistaken for an ability rung.
fn lone_ability_label(seg: &str) -> Option<String> {
    let words: Vec<&str> = seg.split_whitespace().collect();
    if words.is_empty() || words.len() > 2 {
        return None;
    }
    let label = words[0].trim_end_matches('*').trim();
    if label.is_empty() || !label.chars().next()?.is_ascii_uppercase() {
        return None;
    }
    Some(label.to_string())
}

/// Parse the AFTERMATH branches into Outcome [`ContentUnit`]s. Branch heads are
/// short Title-Case standalone lines followed by prose. Fail-closed: if no branch
/// head is recognized, emit a single "Aftermath" outcome so the phase is never
/// silently dropped.
pub fn parse_aftermath_outcomes(
    mission_id: &str,
    base: &SourceRef,
    text: &str,
) -> Vec<ContentUnit> {
    let block = match section_slice(text, &["AFTERMATH"], &["ANOMALY PROFILE"]) {
        Some(b) => b,
        None => return Vec::new(),
    };
    // Split into blank-line-separated paragraphs of the left-column stream
    // (outcome heads/prose are left-aligned; the right column is a separate flow).
    let mut paras: Vec<Vec<String>> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    for raw in block.lines() {
        let l = columns_of(raw).into_iter().next().unwrap_or("");
        if l.is_empty() {
            if !cur.is_empty() {
                paras.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(l.to_string());
        }
    }
    if !cur.is_empty() {
        paras.push(cur);
    }
    let is_prose = |p: &Vec<String>| p.iter().any(|l| l.split_whitespace().count() > 6);
    let is_heading_block = |p: &Vec<String>| p.len() <= 2 && is_outcome_head(&p[0]) && !is_prose(p);
    let mut out = Vec::new();
    for i in 0..paras.len() {
        // A heading block (1-2 short title-case lines) followed by a prose block
        // is an Aftermath outcome branch. Title = the head's first line.
        if is_heading_block(&paras[i]) && paras.get(i + 1).map(is_prose).unwrap_or(false) {
            let title = paras[i][0].clone();
            let id = format!("outcome.{mission_id}.{}", slug(&title));
            let mut u = ContentUnit::new(id, UnitKind::Ending, title);
            u.parent_id = Some(format!("mission.{mission_id}"));
            u.facets = vec![FacetKind::Outcome];
            u.visibility = VisibilityPolicy::OnUnlock;
            u.source_evidence = evidence(base, "Aftermath");
            out.push(u);
        }
    }
    if out.is_empty() {
        let mut u = ContentUnit::new(
            format!("outcome.{mission_id}.aftermath"),
            UnitKind::Ending,
            "Aftermath",
        );
        u.parent_id = Some(format!("mission.{mission_id}"));
        u.facets = vec![FacetKind::Outcome];
        u.source_evidence = evidence(base, "Aftermath");
        out.push(u);
    }
    out
}

fn is_outcome_head(line: &str) -> bool {
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.is_empty() || words.len() > 5 {
        return false;
    }
    if line.len() > 40 || line.ends_with(['.', ',', ';', ':']) {
        return false;
    }
    // First word capitalized, and not an ALL-CAPS section marker.
    if PHASE_MARKERS.iter().any(|m| line.eq_ignore_ascii_case(m)) {
        return false;
    }
    let first = words[0];
    first
        .chars()
        .next()
        .map(|c| c.is_uppercase())
        .unwrap_or(false)
        && line.chars().any(|c| c.is_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> SourceRef {
        SourceRef {
            source_id: "the_vault".into(),
            page: Some(11),
            anchor_id: None,
            section_path: vec!["Springs Eternal".into()],
            char_start: None,
            char_end: None,
            text_hash: None,
            note: None,
        }
    }

    // Verbatim excerpt from Springs Eternal (page 11), in reading order after the
    // page-level decolumnize step (the left narrative column's Optional Objectives
    // block; the right column's narrative is dropped here, parsed separately).
    const OBJ_TEXT: &str = "PRE-INVESTIGATION
Optional Objectives
f +1 Commendation if you wear a flower
crown (once per Agent)
f +3 Commendations if you conduct an experiment
f +6 Commendations for the first to win a lawsuit
f +1 Demerit each time you reminisce about the past
CHAOS EFFECTS";

    #[test]
    fn parses_scored_optional_objectives_with_commendation_and_demerit() {
        let objs = parse_optional_objectives("springs_eternal", &base(), OBJ_TEXT);
        assert_eq!(objs.len(), 4, "four scored optional objectives");
        // +6 Commendations is the load-bearing scored mechanic — exact.
        let six = objs.iter().find(|o| o.score_effects[0].delta == 6).unwrap();
        assert_eq!(six.score_effects[0].label, "Commendation");
        assert!(matches!(
            six.success_when,
            PredicateExpr::OpaqueAuthoredText { .. }
        ));
        // the Demerit is captured with its own label and positive delta.
        let dem = objs
            .iter()
            .find(|o| o.score_effects[0].label == "Demerit")
            .unwrap();
        assert_eq!(dem.score_effects[0].delta, 1);
        // every scored objective is mission-scoped and not mandatory.
        assert!(objs
            .iter()
            .all(|o| o.mission_id.as_deref() == Some("springs_eternal")));
        assert!(objs.iter().all(|o| !o.mandatory));
    }

    #[test]
    fn objective_condition_is_source_grounded_not_fabricated() {
        let objs = parse_optional_objectives("springs_eternal", &base(), OBJ_TEXT);
        let three = objs.iter().find(|o| o.score_effects[0].delta == 3).unwrap();
        match &three.success_when {
            PredicateExpr::OpaqueAuthoredText { raw_text } => {
                assert_eq!(raw_text, "if you conduct an experiment");
            }
            _ => panic!("scored objective condition must be opaque authored text"),
        }
    }

    #[test]
    fn no_optional_objectives_section_yields_none() {
        let objs =
            parse_optional_objectives("m", &base(), "ANOMALY PROFILE\nsome prose\nCHAOS EFFECTS");
        assert!(
            objs.is_empty(),
            "fail-closed: no section → no fabricated objectives"
        );
    }

    const CHAOS_TEXT: &str = "CHAOS EFFECTS
      2 Chaos       Refresh*
                                    more like Serena Evermore.
      4 Chaos       Manifest        The Anomaly can also Manifest puddles that reflect
      12 Chaos        Return*
INVESTIGATION";

    #[test]
    fn parses_chaos_ability_ladder_into_tracker_rungs() {
        let t = parse_chaos_tracker("springs_eternal", &base(), CHAOS_TEXT).unwrap();
        assert_eq!(t.label, "Chaos");
        assert_eq!(t.kind, TrackerKind::Countdown);
        assert_eq!(t.rungs.len(), 3);
        // verbatim costs + ability names — the ladder is NOT flattened.
        assert_eq!(t.rungs[0].cost, 2);
        assert_eq!(t.rungs[0].label, "Refresh");
        let ret = t.rungs.iter().find(|r| r.cost == 12).unwrap();
        assert_eq!(ret.label, "Return");
        // an inline detail is captured when present.
        let man = t.rungs.iter().find(|r| r.label == "Manifest").unwrap();
        assert!(man.detail.contains("Manifest puddles"));
    }

    #[test]
    fn no_chaos_section_yields_none() {
        assert!(parse_chaos_tracker("m", &base(), "ANOMALY PROFILE\nprose").is_none());
    }

    // The stacked CHAOS layout: each rung is a lone `<N> Chaos` line followed by
    // its ability word on the next line, with wrapped detail prose after it.
    const CHAOS_STACKED: &str = "CHAOS EFFECTS
2 Chaos
Refresh
A mundane target's appearance is altered to look years younger.
12 Chaos
Return*
The target is fully under the Anomaly's influence.
INVESTIGATION";

    #[test]
    fn parses_stacked_chaos_layout_into_rungs() {
        let t = parse_chaos_tracker("springs_eternal", &base(), CHAOS_STACKED).unwrap();
        assert_eq!(
            t.rungs.len(),
            2,
            "stacked cost+ability pairs, prose ignored"
        );
        assert_eq!(t.rungs[0].cost, 2);
        assert_eq!(t.rungs[0].label, "Refresh");
        assert_eq!(t.rungs[1].cost, 12);
        assert_eq!(t.rungs[1].label, "Return"); // trailing '*' stripped
    }

    const AFTERMATH_TEXT: &str = "AFTERMATH

Anomaly Captured
or Neutralized

Aquifer products will spontaneously lose both their efficacy and their side effects.

Anomaly Escaped

If the Anomaly escapes and Serena is alive, it will bond with her.

Loose Ends

No matter the outcome, any surviving staff need to be dealt with.

ANOMALY PROFILE";

    #[test]
    fn parses_branching_aftermath_outcomes() {
        let outs = parse_aftermath_outcomes("springs_eternal", &base(), AFTERMATH_TEXT);
        let titles: Vec<&str> = outs.iter().map(|o| o.title.as_str()).collect();
        assert!(titles.contains(&"Anomaly Captured"), "got {titles:?}");
        assert!(titles.contains(&"Anomaly Escaped"), "got {titles:?}");
        assert!(titles.contains(&"Loose Ends"), "got {titles:?}");
        assert!(outs.iter().all(|o| o.kind == UnitKind::Ending));
        assert!(outs.iter().all(|o| o.has_facet(FacetKind::Outcome)));
    }

    #[test]
    fn aftermath_without_branch_heads_emits_one_outcome() {
        let t =
            "AFTERMATH\njust a paragraph of prose with no headings at all here.\nANOMALY PROFILE";
        let outs = parse_aftermath_outcomes("m", &base(), t);
        assert_eq!(outs.len(), 1, "fail-closed: phase captured as one outcome");
        assert_eq!(outs[0].title, "Aftermath");
    }
}
