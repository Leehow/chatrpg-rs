//! Narrative anchors (L8.1): deterministic, additive MATERIAL extracted from a parsed module.
//!
//! A [`NarrativeAnchor`] is director-usable raw material — a potential thread, a stake, a
//! character goal, a reveal candidate, a climax condition, a setup→payoff pair, or a scene's
//! dramatic function. It is **material, not a script** (discipline rule 1): the Director MAY pull
//! on an anchor when it serves the live fiction, but anchors never railroad the turn.
//!
//! [`extract_narrative_anchors`] is a PURE, deterministic, idempotent projection of the parsed
//! [`ScenarioNode`]s — no LLM, no DB, no ruleset/module name-branch (rule 11). It keys only on the
//! STRUCTURE of the parsed scenes (references, links, terminality), so re-parsing the same module
//! yields byte-identical anchors. Additive: nothing consumes anchors until L8.2.

use crate::ScenarioNode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// The dramatic role a [`NarrativeAnchor`] plays. Generic vocabulary; the deterministic extractor
/// grounds a subset structurally, the rest are available for richer (L8.2+) seeding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NarrativeAnchorKind {
    /// A latent storyline the Director could promote into a `StoryThread`. Fail-closed default.
    #[default]
    PotentialThread,
    /// What is at risk in the fiction (a stake the Director can raise).
    Stakes,
    /// A character's goal/want the Director can spotlight.
    CharacterGoal,
    /// A fact the module holds that could become a reveal (a clue).
    RevealCandidate,
    /// A condition under which the story converges/climaxes (a terminal scene).
    ClimaxCondition,
    /// A setup whose payoff is gated elsewhere (a clue-gated transition).
    SetupPayoff,
    /// A scene's dramatic function (every parsed scene has one).
    SceneFunction,
}

/// One unit of director material extracted from the module. All non-id fields `#[serde(default)]`
/// so a partial payload round-trips; empty collections serialize away (byte-stable for old data).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct NarrativeAnchor {
    /// Deterministic, stable id derived from kind + source + reference (idempotency key).
    pub anchor_id: String,
    /// The dramatic role of this anchor.
    #[serde(default)]
    pub kind: NarrativeAnchorKind,
    /// One-line, human-facing material (a summary / reason / label) — never a verbatim script.
    #[serde(default)]
    pub summary: String,
    /// The module node this anchor was extracted from, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_node_id: Option<String>,
    /// Referenced module entity ids (clue / target node) that ground this anchor.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_ids: Vec<String>,
}

fn push_unique(
    out: &mut Vec<NarrativeAnchor>,
    seen: &mut HashSet<String>,
    anchor: NarrativeAnchor,
) {
    if seen.insert(anchor.anchor_id.clone()) {
        out.push(anchor);
    }
}

/// Deterministically extract director material from parsed scenes. Pure / idempotent / generic.
///
/// Structural rules (no name-branch, order-stable, deduped by `anchor_id`):
/// - every scene → a `SceneFunction` anchor (its dramatic role);
/// - each `referenced_clue_id` → a `RevealCandidate` (a clue is potential reveal material);
/// - each outgoing link with a non-empty `reason` → a `PotentialThread` (the dramatic connection);
/// - each outgoing link carrying a `clue_id` → a `SetupPayoff` (a clue-gated transition);
/// - a terminal scene (no outgoing links) → a `ClimaxCondition` (a convergence point).
pub fn extract_narrative_anchors(scenes: &[ScenarioNode]) -> Vec<NarrativeAnchor> {
    let mut out: Vec<NarrativeAnchor> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for s in scenes {
        let label = if s.summary.trim().is_empty() {
            s.title.clone()
        } else {
            s.summary.clone()
        };
        push_unique(
            &mut out,
            &mut seen,
            NarrativeAnchor {
                anchor_id: format!("anchor_scene_{}", s.node_id),
                kind: NarrativeAnchorKind::SceneFunction,
                summary: label.clone(),
                source_node_id: Some(s.node_id.clone()),
                related_ids: Vec::new(),
            },
        );

        for clue in &s.referenced_clue_ids {
            push_unique(
                &mut out,
                &mut seen,
                NarrativeAnchor {
                    anchor_id: format!("anchor_reveal_{}_{}", s.node_id, clue),
                    kind: NarrativeAnchorKind::RevealCandidate,
                    summary: format!("可揭示线索 {clue}（出自场景 {}）", s.node_id),
                    source_node_id: Some(s.node_id.clone()),
                    related_ids: vec![clue.clone()],
                },
            );
        }

        for link in &s.links {
            if !link.reason.trim().is_empty() {
                push_unique(
                    &mut out,
                    &mut seen,
                    NarrativeAnchor {
                        anchor_id: format!("anchor_thread_{}_{}", s.node_id, link.to_node_id),
                        kind: NarrativeAnchorKind::PotentialThread,
                        summary: link.reason.clone(),
                        source_node_id: Some(s.node_id.clone()),
                        related_ids: vec![link.to_node_id.clone()],
                    },
                );
            }
            if let Some(clue) = &link.clue_id {
                push_unique(
                    &mut out,
                    &mut seen,
                    NarrativeAnchor {
                        anchor_id: format!(
                            "anchor_setup_{}_{}_{}",
                            s.node_id, link.to_node_id, clue
                        ),
                        kind: NarrativeAnchorKind::SetupPayoff,
                        summary: format!("线索 {clue} 铺垫通向 {} 的回收", link.to_node_id),
                        source_node_id: Some(s.node_id.clone()),
                        related_ids: vec![clue.clone(), link.to_node_id.clone()],
                    },
                );
            }
        }

        if s.links.is_empty() {
            push_unique(
                &mut out,
                &mut seen,
                NarrativeAnchor {
                    anchor_id: format!("anchor_climax_{}", s.node_id),
                    kind: NarrativeAnchorKind::ClimaxCondition,
                    summary: format!("收束/高潮汇聚点：{label}"),
                    source_node_id: Some(s.node_id.clone()),
                    related_ids: Vec::new(),
                },
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ScenarioLink, ScenarioNode};

    fn node(id: &str, summary: &str) -> ScenarioNode {
        ScenarioNode {
            node_id: id.to_string(),
            title: format!("场景 {id}"),
            node_type: "scene".to_string(),
            summary: summary.to_string(),
            ..Default::default()
        }
    }

    fn two_scene_module() -> Vec<ScenarioNode> {
        let mut a = node("s1", "调查员抵达废弃宅邸");
        a.referenced_clue_ids = vec!["clue_diary".into(), "clue_blood".into()];
        a.links = vec![ScenarioLink {
            to_node_id: "s2".into(),
            reason: "顺着日记线索深入地窖".into(),
            clue_id: Some("clue_diary".into()),
            ..Default::default()
        }];
        let b = node("s2", "地窖中的真相"); // terminal scene (no links)
        vec![a, b]
    }

    #[test]
    fn parsed_module_yields_anchors() {
        let anchors = extract_narrative_anchors(&two_scene_module());
        assert!(!anchors.is_empty(), "a parsed module must yield anchors");
        // every scene → a SceneFunction anchor
        assert_eq!(
            anchors
                .iter()
                .filter(|a| a.kind == NarrativeAnchorKind::SceneFunction)
                .count(),
            2
        );
        // two referenced clues → two RevealCandidate anchors
        assert_eq!(
            anchors
                .iter()
                .filter(|a| a.kind == NarrativeAnchorKind::RevealCandidate)
                .count(),
            2
        );
        // the s1→s2 link has a reason → a PotentialThread; and a clue_id → a SetupPayoff
        assert!(anchors
            .iter()
            .any(|a| a.kind == NarrativeAnchorKind::PotentialThread
                && a.related_ids == vec!["s2".to_string()]));
        assert!(anchors
            .iter()
            .any(|a| a.kind == NarrativeAnchorKind::SetupPayoff));
        // s2 is terminal → a ClimaxCondition
        assert!(anchors
            .iter()
            .any(|a| a.kind == NarrativeAnchorKind::ClimaxCondition
                && a.source_node_id.as_deref() == Some("s2")));
    }

    #[test]
    fn extraction_is_idempotent_and_deduped() {
        let scenes = two_scene_module();
        let first = extract_narrative_anchors(&scenes);
        let second = extract_narrative_anchors(&scenes);
        assert_eq!(
            first, second,
            "pure extraction must be deterministic/idempotent"
        );

        // a duplicate clue reference must not create a duplicate anchor (dedup by anchor_id).
        let mut dup = node("s1", "重复线索引用");
        dup.referenced_clue_ids = vec!["c".into(), "c".into()];
        let anchors = extract_narrative_anchors(&[dup]);
        let reveal_ids: Vec<&str> = anchors
            .iter()
            .filter(|a| a.kind == NarrativeAnchorKind::RevealCandidate)
            .map(|a| a.anchor_id.as_str())
            .collect();
        assert_eq!(
            reveal_ids.len(),
            1,
            "duplicate clue ref must dedup to one anchor"
        );
    }

    #[test]
    fn empty_module_yields_no_anchors() {
        assert!(extract_narrative_anchors(&[]).is_empty());
    }

    #[test]
    fn anchor_serde_round_trip() {
        let anchors = extract_narrative_anchors(&two_scene_module());
        let json = serde_json::to_string(&anchors).unwrap();
        let back: Vec<NarrativeAnchor> = serde_json::from_str(&json).unwrap();
        assert_eq!(anchors, back);
    }

    #[test]
    fn anchor_kind_defaults_to_potential_thread() {
        assert_eq!(
            NarrativeAnchorKind::default(),
            NarrativeAnchorKind::PotentialThread
        );
    }
}
