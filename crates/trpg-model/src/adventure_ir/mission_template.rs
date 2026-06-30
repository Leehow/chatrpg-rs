//! Adventure IR — mission-template induction (P2-1). The Vault is a mission
//! anthology: ~12 self-contained missions that all follow ONE repeated template
//! (`ANOMALY PROFILE → PRE-INVESTIGATION → CHAOS EFFECTS → INVESTIGATION →
//! ENCOUNTER → AFTERMATH`). The reader currently flattens these into a flat scene
//! list with `missions = 0`, deleting the mission/score/aftermath semantics.
//!
//! This module recognizes the REPEATED TEMPLATE *structurally* (purely from the
//! page-anchored source text — never `if module == "the_vault"`) and slices N
//! first-class [`MissionSpec`]s, each carrying its phases, scored objectives,
//! Chaos ability ladder and branching aftermath. Fail-closed: a document that
//! does not exhibit a repeated multi-phase template yields nothing (so non-
//! anthology modules — e.g. Homecoming — are untouched).
use super::mission_scoring::{
    parse_aftermath_outcomes, parse_chaos_tracker, parse_optional_objectives,
};
use super::mission_text::{columns_of, line_marker, slug, PHASE_MARKERS};
use super::{ContentUnit, FacetKind, ObjectiveSpec, TrackerSpec, UnitKind, VisibilityPolicy};
use crate::{ScenarioNode, SourceRef};
use serde::{Deserialize, Serialize};

/// A repeated-template anthology needs at least this many template instances
/// before we treat the document as a mission anthology (guards against two
/// coincidental headings in an otherwise linear module).
const MIN_MISSIONS: usize = 3;
/// A mission window must contain at least this many distinct template phases.
const MIN_PHASES: usize = 3;
/// `ANOMALY PROFILE` markers closer than this many pages are the same mission's
/// profile spilling across a page, not two missions.
const MIN_MISSION_GAP: u32 = 4;

/// One page of page-anchored source text.
#[derive(Debug, Clone)]
pub struct MissionPage {
    pub page: u32,
    pub text: String,
}

impl MissionPage {
    pub fn new(page: u32, text: impl Into<String>) -> Self {
        MissionPage {
            page,
            text: text.into(),
        }
    }
}

/// A first-class mission sliced from the repeated template. Bundling the scored
/// mechanics WITH the unit is the proof the IR did not collapse the anthology
/// into a flat scene list.
/// (No `Eq`: embeds `Vec<SourceRef>` via its members.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MissionSpec {
    pub unit: ContentUnit,
    #[serde(default)]
    pub phases: Vec<ContentUnit>,
    #[serde(default)]
    pub objectives: Vec<ObjectiveSpec>,
    #[serde(default)]
    pub chaos: Option<TrackerSpec>,
    #[serde(default)]
    pub aftermath: Vec<ContentUnit>,
}

/// Map a line to its canonical template phase marker, if any. Matches the marker
/// in EITHER column segment, so a right-column header is not missed.
fn marker_of(line: &str) -> Option<&'static str> {
    line_marker(line, PHASE_MARKERS)
}

/// An ALL-CAPS, non-marker line that can serve as a mission running header.
fn is_header_candidate(lc: &str) -> bool {
    let letters = lc.chars().filter(|c| c.is_alphabetic()).count();
    if letters < 3 || lc.len() > 40 {
        return false;
    }
    if lc.chars().any(|c| c.is_lowercase()) {
        return false;
    }
    !PHASE_MARKERS.iter().any(|m| lc.eq_ignore_ascii_case(m))
}

fn titlecase(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => {
                    f.to_uppercase().collect::<String>()
                        + &c.flat_map(|x| x.to_lowercase()).collect::<String>()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn phase_kind(marker: &str) -> UnitKind {
    match marker {
        "INVESTIGATION" => UnitKind::Scene,
        "ENCOUNTER" => UnitKind::Encounter,
        "AFTERMATH" => UnitKind::Ending,
        _ => UnitKind::Phase,
    }
}

/// Recognize the repeated mission template in `pages` and slice first-class
/// [`MissionSpec`]s. Returns empty when the document is not a mission anthology.
pub fn induct_missions(
    module_id: &str,
    source_id: &str,
    pages: &[MissionPage],
) -> Vec<MissionSpec> {
    // 1. Mission starts = `ANOMALY PROFILE` markers, merging ones within a page or
    //    two (the same profile spilling across a page).
    let mut starts: Vec<usize> = Vec::new();
    for (i, p) in pages.iter().enumerate() {
        if p.text
            .lines()
            .any(|l| marker_of(l) == Some("ANOMALY PROFILE"))
        {
            if let Some(&last) = starts.last() {
                if p.page.saturating_sub(pages[last].page) < MIN_MISSION_GAP {
                    continue;
                }
            }
            starts.push(i);
        }
    }
    if starts.is_empty() {
        return Vec::new();
    }
    let root = format!("campaign.{}", slug(module_id));

    // 2. Each start..next-start is a candidate mission window.
    let mut missions = Vec::new();
    for (k, &s) in starts.iter().enumerate() {
        let end = starts.get(k + 1).copied().unwrap_or(pages.len());
        let window = &pages[s..end];
        let window_text = window
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let start_page = window[0].page;

        // distinct phase markers present (structural template signature)
        let mut markers_seen: Vec<&'static str> = Vec::new();
        let mut header_freq: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for p in window {
            for l in p.text.lines() {
                if let Some(m) = marker_of(l) {
                    if !markers_seen.contains(&m) {
                        markers_seen.push(m);
                    }
                } else {
                    let lc = columns_of(l).into_iter().next().unwrap_or("");
                    if is_header_candidate(lc) {
                        *header_freq.entry(lc.to_string()).or_insert(0) += 1;
                    }
                }
            }
        }
        if markers_seen.len() < MIN_PHASES {
            continue; // not a real template instance — skip (fail-closed)
        }

        // 3. Title from the most frequent running header (fallback: ordinal).
        let title_raw = header_freq
            .iter()
            .max_by_key(|(_, c)| **c)
            .map(|(h, _)| h.clone())
            .unwrap_or_else(|| format!("MISSION {}", k + 1));
        let title = titlecase(&title_raw);
        let mid = slug(&title);

        let base = SourceRef {
            source_id: source_id.to_string(),
            page: Some(start_page),
            anchor_id: None,
            section_path: vec![title.clone()],
            char_start: None,
            char_end: None,
            text_hash: None,
            note: None,
        };

        let objectives = parse_optional_objectives(&mid, &base, &window_text);
        let chaos = parse_chaos_tracker(&mid, &base, &window_text);
        let aftermath = parse_aftermath_outcomes(&mid, &base, &window_text);

        // 4. The Mission unit; facets reflect the structures actually present.
        let mut unit = ContentUnit::new(format!("mission.{mid}"), UnitKind::Mission, title);
        unit.parent_id = Some(root.clone());
        unit.source_evidence = vec![base.clone()];
        let mut facets = vec![FacetKind::MissionLifecycle];
        if !objectives.is_empty() {
            facets.push(FacetKind::Objective);
        }
        if chaos.is_some() {
            facets.push(FacetKind::Chaos);
        }
        if markers_seen.contains(&"INVESTIGATION") {
            facets.push(FacetKind::ClueWeb);
        }
        if markers_seen.contains(&"ENCOUNTER") {
            facets.push(FacetKind::SocialEncounter);
        }
        if !aftermath.is_empty() {
            facets.push(FacetKind::Outcome);
        }
        unit.facets = facets;

        // 5. Phase children (one per distinct template marker).
        let phases = markers_seen
            .iter()
            .map(|m| {
                let mut u = ContentUnit::new(
                    format!("phase.{mid}.{}", slug(m)),
                    phase_kind(m),
                    titlecase(m),
                );
                u.parent_id = Some(unit.id.clone());
                u.source_evidence = vec![base.clone()];
                u
            })
            .collect();

        missions.push(MissionSpec {
            unit,
            phases,
            objectives,
            chaos,
            aftermath,
        });
    }

    // 6. Anthology gate: a repeated template needs >= MIN_MISSIONS instances.
    if missions.len() < MIN_MISSIONS {
        return Vec::new();
    }
    missions
}

/// The Campaign root unit for a mission anthology (parent of every Mission unit).
pub fn campaign_root(module_id: &str, title: &str) -> ContentUnit {
    let mut u = ContentUnit::new(
        format!("campaign.{}", slug(module_id)),
        UnitKind::Campaign,
        title,
    );
    u.visibility = VisibilityPolicy::Public;
    u
}

/// Compat projection: one `ScenarioNode` per induced Mission, so the persisted
/// `ModuleGraph.missions` is no longer empty for a mission anthology (mirrors
/// [`super::project_chapters`]). The scored mechanics live on the richer
/// [`MissionSpec`] / `content_units`; this is only the legacy node view.
pub fn project_missions(missions: &[MissionSpec]) -> Vec<ScenarioNode> {
    missions
        .iter()
        .map(|m| ScenarioNode {
            node_id: m.unit.id.clone(),
            title: m.unit.title.clone(),
            node_type: "mission".to_string(),
            summary: format!(
                "Mission inducted from repeated Vault template ({} scored objectives, {} chaos rungs, {} aftermath outcomes)",
                m.objectives.len(),
                m.chaos.as_ref().map(|c| c.rungs.len()).unwrap_or(0),
                m.aftermath.len(),
            ),
            ..Default::default()
        })
        .collect()
}

/// Flatten every induced mission's structural units (the Mission itself, its
/// phase children, and its Aftermath outcomes) into the `ContentUnit` substrate
/// so the anthology's Contains hierarchy persists alongside the linear scenes.
pub fn mission_content_units(missions: &[MissionSpec]) -> Vec<ContentUnit> {
    let mut out = Vec::new();
    for m in missions {
        out.push(m.unit.clone());
        out.extend(m.phases.iter().cloned());
        out.extend(m.aftermath.iter().cloned());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // A compact 3-mission anthology that mirrors the real Vault template shape:
    // each mission = a running header (its NAME, ALL CAPS, repeated per page) +
    // ANOMALY PROFILE / PRE-INVESTIGATION+Optional Objectives / CHAOS EFFECTS /
    // INVESTIGATION / ENCOUNTER / AFTERMATH.
    fn anthology_pages() -> Vec<MissionPage> {
        let mission = |name: &str, p0: u32| {
            vec![
                MissionPage::new(
                    p0,
                    format!(
                        "{name}\nANOMALY PROFILE\nDomain\nWater and youth.\n{name}\nImpulse\nRejuvenate."
                    ),
                ),
                MissionPage::new(
                    p0 + 1,
                    format!(
                        "{name}\nPRE-INVESTIGATION\nOptional Objectives\nf +3 Commendations if you conduct an experiment\nf +1 Demerit each time you reminisce\n{name}"
                    ),
                ),
                MissionPage::new(
                    p0 + 2,
                    format!("{name}\nCHAOS EFFECTS\n      2 Chaos       Refresh\n      6 Chaos       Expand\n{name}"),
                ),
                MissionPage::new(
                    p0 + 3,
                    format!("{name}\nINVESTIGATION\nClues are scattered around town.\n{name}"),
                ),
                MissionPage::new(p0 + 4, format!("{name}\nENCOUNTER\nA fight breaks out.\n{name}")),
                MissionPage::new(
                    p0 + 5,
                    format!("{name}\nAFTERMATH\n\nAnomaly Captured\n\nThings settle down and the city slowly returns to normal life.\n\nAnomaly Escaped\n\nIf it gets away it will cause serious problems for the Agency later on.\n{name}"),
                ),
            ]
        };
        let mut pages = Vec::new();
        pages.extend(mission("SPRINGS ETERNAL", 8));
        pages.extend(mission("DEAD QUIET", 22));
        pages.extend(mission("ROM DUMP", 36));
        pages
    }

    #[test]
    fn inducts_one_mission_per_template_instance() {
        let m = induct_missions("triangle_agency.the_vault", "the_vault", &anthology_pages());
        assert_eq!(m.len(), 3, "one Mission per repeated template instance");
        assert!(m.iter().all(|x| x.unit.kind == UnitKind::Mission));
    }

    #[test]
    fn mission_title_comes_from_repeated_running_header() {
        let m = induct_missions("triangle_agency.the_vault", "the_vault", &anthology_pages());
        let titles: Vec<&str> = m.iter().map(|x| x.unit.title.as_str()).collect();
        assert!(
            titles
                .iter()
                .any(|t| t.eq_ignore_ascii_case("Springs Eternal")),
            "got {titles:?}"
        );
        assert!(
            titles.iter().any(|t| t.eq_ignore_ascii_case("Dead Quiet")),
            "got {titles:?}"
        );
    }

    #[test]
    fn mission_carries_scored_objectives_chaos_and_aftermath() {
        let m = induct_missions("triangle_agency.the_vault", "the_vault", &anthology_pages());
        let first = &m[0];
        // scored objectives (Commendation + Demerit)
        assert!(
            !first.objectives.is_empty(),
            "mission must carry scored objectives"
        );
        assert!(first
            .objectives
            .iter()
            .any(|o| o.score_effects.iter().any(|s| s.label == "Commendation")));
        // chaos ability ladder
        let chaos = first.chaos.as_ref().expect("chaos tracker");
        assert_eq!(chaos.rungs.len(), 2);
        // branching aftermath
        assert!(first.aftermath.len() >= 2, "branching aftermath outcomes");
        // facets reflect the structures actually present (not collapsed to Scene).
        assert!(first.unit.has_facet(FacetKind::MissionLifecycle));
        assert!(first.unit.has_facet(FacetKind::Objective));
        assert!(first.unit.has_facet(FacetKind::Chaos));
        assert!(first.unit.has_facet(FacetKind::Outcome));
    }

    #[test]
    fn mission_has_phase_children_parented_to_it() {
        let m = induct_missions("triangle_agency.the_vault", "the_vault", &anthology_pages());
        let first = &m[0];
        assert!(first.phases.len() >= MIN_PHASES);
        assert!(first
            .phases
            .iter()
            .all(|p| p.parent_id.as_deref() == Some(first.unit.id.as_str())));
    }

    #[test]
    fn non_anthology_document_yields_nothing() {
        // Homecoming-like: linear scenes, no repeated multi-phase template.
        let pages = vec![
            MissionPage::new(1, "LAWMEN IN TROUBLE\nThe opening scene of a linear story."),
            MissionPage::new(2, "FOXWELL SERVICES\nA second scene with a fixer."),
            MissionPage::new(3, "THE CONDO\nThe climax floor by floor."),
        ];
        assert!(
            induct_missions("cyberpunk_red.homecoming", "homecoming", &pages).is_empty(),
            "fail-closed: a non-anthology module must induct no missions"
        );
    }

    #[test]
    fn too_few_instances_is_not_an_anthology() {
        let mut pages = Vec::new();
        // only 2 template instances < MIN_MISSIONS
        for (name, p0) in [("SPRINGS ETERNAL", 8u32), ("DEAD QUIET", 22)] {
            pages.push(MissionPage::new(
                p0,
                format!("{name}\nANOMALY PROFILE\nDomain"),
            ));
            pages.push(MissionPage::new(
                p0 + 1,
                format!("{name}\nCHAOS EFFECTS\n2 Chaos Refresh"),
            ));
            pages.push(MissionPage::new(
                p0 + 2,
                format!("{name}\nAFTERMATH\n\nDone\n\nThe end of this little tale arrives."),
            ));
        }
        assert!(induct_missions("m", "s", &pages).is_empty());
    }
}
