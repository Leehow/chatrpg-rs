//! Adventure IR — P0-2: derive a Contains hierarchy (`ContentUnit`) from the
//! ScenarioNodes the reader already segmented. The parser persisted 0 chapters /
//! 0 missions (flat scene list); this fills the IR substrate from the reader's
//! structural fingerprints (`node_type`) + authored heading titles, **fail-closed
//! and evidence-backed**, so the runtime has a real unit tree to progress over.
//!
//! Pure, deterministic, unit-tested. **Structure-fingerprint ONLY** — the role of
//! a unit is decided by its `node_type`/title, never by inspecting
//! `ruleset_id`/`module_id` (no name-branching). `ModuleGraph` stays a compat
//! projection; these units are the hierarchy fact source.
use crate::{ScenarioNode, SourceRef};

use super::content_unit::{ContentUnit, UnitKind, VisibilityPolicy};

/// Structure-fingerprint map: a scene's `node_type` → publishing [`UnitKind`].
/// The string is the structural role the reader already assigned — this is NOT a
/// ruleset branch. Unknown types fall back to `Scene` (still a real unit, never
/// dropped).
pub fn unit_kind_for(node_type: &str) -> UnitKind {
    match node_type {
        "scene" | "transition_scene" => UnitKind::Scene,
        // a decision/branch node is a Beat (an authored choice point), not a place.
        "branch_scene" => UnitKind::Beat,
        "encounter" => UnitKind::Encounter,
        // golden: NET architecture / hacking / chase subroutine → Procedure.
        "location_procedure" | "procedure" => UnitKind::Procedure,
        "map_index" | "index" | "table" => UnitKind::Table,
        "appendix" | "handout" => UnitKind::Handout,
        // GM-facing front matter (a word to the GM, background/synopsis).
        "guidance" | "setup" | "front" => UnitKind::Handout,
        "ending" => UnitKind::Ending,
        _ => UnitKind::Scene,
    }
}

/// Chapter-tier structural role (fingerprint) a scene belongs to. Groups the flat
/// scene list into the three sections every published adventure actually has:
/// front matter, the playable adventure, and reference/appendix material.
fn chapter_role(node_type: &str) -> ChapterRole {
    match node_type {
        "guidance" | "setup" | "front" => ChapterRole::Front,
        "map_index" | "index" | "table" | "appendix" | "handout" => ChapterRole::Reference,
        _ => ChapterRole::Adventure,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChapterRole {
    Front,
    Adventure,
    Reference,
}

impl ChapterRole {
    /// Stable id/title/visibility for the synthesized chapter unit.
    fn meta(self) -> (&'static str, &'static str, VisibilityPolicy) {
        match self {
            ChapterRole::Front => ("front", "Front Matter", VisibilityPolicy::GmOnly),
            ChapterRole::Adventure => ("adventure", "Adventure", VisibilityPolicy::OnUnlock),
            ChapterRole::Reference => ("reference", "Reference", VisibilityPolicy::GmOnly),
        }
    }
}

const ROLE_ORDER: [ChapterRole; 3] = [
    ChapterRole::Front,
    ChapterRole::Adventure,
    ChapterRole::Reference,
];

fn evidence(source_id: &str, page: Option<u32>, heading: &str) -> SourceRef {
    SourceRef {
        source_id: source_id.to_string(),
        page,
        anchor_id: None,
        section_path: if heading.is_empty() {
            Vec::new()
        } else {
            vec![heading.to_string()]
        },
        char_start: None,
        char_end: None,
        text_hash: None,
        note: None,
    }
}

/// Derive the `Contains` hierarchy: a `Campaign` root → structural `Chapter`s
/// (only the non-empty ones) → `Scene`/`Procedure`/... units → authored `Beat`s
/// split from multi-segment titles. Every non-root unit carries `parent_id`
/// (Contains) and verbatim `source_evidence` (page + heading), so nothing is
/// invented: empty input yields an empty Vec (fail-closed, OFF==baseline).
pub fn derive_content_units(
    module_id: &str,
    source_id: &str,
    title: &str,
    scenes: &[ScenarioNode],
) -> Vec<ContentUnit> {
    if scenes.is_empty() {
        return Vec::new();
    }
    let root_id = format!("unit.{module_id}");
    let mut out: Vec<ContentUnit> = Vec::new();

    let mut root = ContentUnit::new(&root_id, UnitKind::Campaign, title);
    root.visibility = VisibilityPolicy::Public;
    out.push(root);

    for role in ROLE_ORDER {
        let members: Vec<&ScenarioNode> = scenes
            .iter()
            .filter(|s| chapter_role(&s.node_type) == role)
            .collect();
        if members.is_empty() {
            continue; // fail-closed: never emit an empty chapter tier.
        }
        let (slug, ch_title, vis) = role.meta();
        let chapter_id = format!("{root_id}.ch.{slug}");
        let mut chapter =
            ContentUnit::new(&chapter_id, UnitKind::Chapter, ch_title).with_parent(&root_id);
        chapter.visibility = vis;
        out.push(chapter);

        for s in members {
            let scene_id = format!("unit.{}", s.node_id);
            let mut unit = ContentUnit::new(&scene_id, unit_kind_for(&s.node_type), &s.title)
                .with_parent(&chapter_id);
            unit.visibility = vis;
            unit.source_evidence
                .push(evidence(source_id, s.page_start, &s.title));
            out.push(unit);

            // Authored sub-beats: a "A / B / C" title is several merged headings.
            let segments: Vec<&str> = s
                .title
                .split('/')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .collect();
            if segments.len() > 1 {
                for (i, seg) in segments.iter().enumerate().skip(1) {
                    let beat_id = format!("{scene_id}.beat.{i}");
                    let mut beat =
                        ContentUnit::new(&beat_id, UnitKind::Beat, *seg).with_parent(&scene_id);
                    beat.visibility = vis;
                    beat.source_evidence
                        .push(evidence(source_id, s.page_start, seg));
                    out.push(beat);
                }
            }
        }
    }
    out
}

/// Compat projection: the synthesized `Chapter` units → `ScenarioNode`s for the
/// legacy `ModuleGraph.chapters` view (so `chapters` is no longer always 0). The
/// fact source remains `content_units`; this is a read-only view.
pub fn project_chapters(units: &[ContentUnit]) -> Vec<ScenarioNode> {
    units
        .iter()
        .filter(|u| u.kind == UnitKind::Chapter)
        .map(|u| ScenarioNode {
            node_id: u.id.clone(),
            title: u.title.clone(),
            node_type: "chapter".to_string(),
            summary: format!(
                "Contains tier derived from scene fingerprints ({})",
                u.title
            ),
            ..Default::default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, node_type: &str, title: &str, page: u32) -> ScenarioNode {
        ScenarioNode {
            node_id: id.to_string(),
            node_type: node_type.to_string(),
            title: title.to_string(),
            page_start: Some(page),
            ..Default::default()
        }
    }

    /// A faithful slice of the live homecoming scene list (node_type + title +
    /// page taken verbatim from the parsed bundle at :54347).
    fn homecoming_scenes() -> Vec<ScenarioNode> {
        vec![
            node("front_01_word_to_gm", "guidance", "A Word to the GM", 3),
            node(
                "front_02_background",
                "setup",
                "Background, Synopsis, Interests",
                4,
            ),
            node(
                "scene_01_lawmen",
                "scene",
                "Lawmen in Trouble / Scavv's Warehouse",
                5,
            ),
            node(
                "scene_02_athena",
                "scene",
                "Questions for Athena / Getting Her to a Charger",
                7,
            ),
            node(
                "scene_03_foxwell",
                "scene",
                "Foxwell Services / Charging Athena",
                7,
            ),
            node(
                "scene_04_scavvs",
                "encounter",
                "Scavvs Are Here / Taking Care of the Scavvs",
                9,
            ),
            node(
                "scene_05_coords",
                "transition_scene",
                "Coordinates to Apartment 3012",
                10,
            ),
            node(
                "scene_06_condo",
                "scene",
                "Condominium Tora No Ko / Meet Quil and Hisako",
                11,
            ),
            node(
                "scene_07_hisako",
                "branch_scene",
                "Hisako's Rewards and Promises",
                12,
            ),
            node("scene_08_quil", "branch_scene", "Taking Quil's Side", 12),
            node(
                "scene_09_net",
                "location_procedure",
                "Condo Floors / Hacking the Condo / Escape",
                13,
            ),
            node("index_01_maps", "map_index", "Maps", 14),
            node(
                "appendix_01_npc",
                "appendix",
                "How to Use the NPC Cards",
                27,
            ),
        ]
    }

    #[test]
    fn empty_scenes_yield_no_units() {
        assert!(derive_content_units("m", "src", "T", &[]).is_empty());
    }

    #[test]
    fn derives_three_tier_contains_hierarchy() {
        let scenes = homecoming_scenes();
        let units = derive_content_units("cyberpunk_red.homecoming", "src", "Homecoming", &scenes);

        // root present, exactly one, Campaign.
        let roots: Vec<_> = units.iter().filter(|u| u.parent_id.is_none()).collect();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].kind, UnitKind::Campaign);

        // three structural chapters: front / adventure / reference all non-empty here.
        let chapters: Vec<_> = units
            .iter()
            .filter(|u| u.kind == UnitKind::Chapter)
            .collect();
        assert_eq!(chapters.len(), 3, "front+adventure+reference");
        assert!(chapters
            .iter()
            .all(|c| c.parent_id.as_deref() == Some("unit.cyberpunk_red.homecoming")));

        // every non-root unit has a parent → real Contains tree.
        assert!(units.iter().filter(|u| u.parent_id.is_some()).count() == units.len() - 1);
    }

    #[test]
    fn net_procedure_scene_is_procedure_not_scene() {
        // golden: the condo hacking/escape subroutine → Procedure.
        let scenes = homecoming_scenes();
        let units = derive_content_units("m", "src", "T", &scenes);
        let net = units.iter().find(|u| u.id == "unit.scene_09_net").unwrap();
        assert_eq!(net.kind, UnitKind::Procedure);
        assert_ne!(net.kind, UnitKind::Scene);
    }

    #[test]
    fn scene_units_carry_verbatim_evidence() {
        let scenes = homecoming_scenes();
        let units = derive_content_units("m", "cpr_homecoming", "T", &scenes);
        let athena = units
            .iter()
            .find(|u| u.id == "unit.scene_02_athena")
            .unwrap();
        let ev = &athena.source_evidence[0];
        assert_eq!(ev.source_id, "cpr_homecoming");
        assert_eq!(ev.page, Some(7));
        assert_eq!(
            ev.section_path,
            vec!["Questions for Athena / Getting Her to a Charger".to_string()]
        );
    }

    #[test]
    fn multi_segment_title_yields_beat_children() {
        let scenes = homecoming_scenes();
        let units = derive_content_units("m", "src", "T", &scenes);
        // scene_01 "Lawmen in Trouble / Scavv's Warehouse" → 1 beat child "Scavv's Warehouse".
        let beats: Vec<_> = units
            .iter()
            .filter(|u| {
                u.kind == UnitKind::Beat && u.parent_id.as_deref() == Some("unit.scene_01_lawmen")
            })
            .collect();
        assert_eq!(beats.len(), 1);
        assert_eq!(beats[0].title, "Scavv's Warehouse");
        assert_eq!(beats[0].source_evidence[0].page, Some(5));
    }

    #[test]
    fn fingerprint_not_ruleset_branch() {
        // same node_type → same kind regardless of module_id (no name-branching).
        let a = derive_content_units("module.alpha", "s", "A", &[node("x", "encounter", "X", 1)]);
        let b = derive_content_units("module.beta", "s", "B", &[node("x", "encounter", "X", 1)]);
        let ka = a.iter().find(|u| u.id == "unit.x").unwrap().kind;
        let kb = b.iter().find(|u| u.id == "unit.x").unwrap().kind;
        assert_eq!(ka, kb);
        assert_eq!(ka, UnitKind::Encounter);
    }

    #[test]
    fn empty_content_units_omitted_off_is_baseline() {
        // OFF (flag off → empty content_units) must serialize byte-identically to
        // the pre-P0-2 baseline: the field key is absent entirely.
        let g = crate::ModuleGraph::default();
        let json = serde_json::to_value(&g).unwrap();
        assert!(
            json.get("content_units").is_none(),
            "empty content_units must be skipped on serialize (OFF==baseline)"
        );
    }

    #[test]
    fn project_chapters_makes_compat_nodes() {
        let scenes = homecoming_scenes();
        let units = derive_content_units("m", "src", "T", &scenes);
        let chapters = project_chapters(&units);
        assert_eq!(chapters.len(), 3);
        assert!(chapters.iter().all(|c| c.node_type == "chapter"));
    }
}
