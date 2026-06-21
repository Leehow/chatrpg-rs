//! Director request vocabulary (L0.2): the multi-scale request envelope the spine
//! extends. ADDITIVE, type-only — no caller is wired here. Mirrors the proposal-type
//! discipline of [`crate::director_plan`]:
//!
//! - **Read-only snapshot, no handle.** [`StorySnapshot`] carries a clone of the story
//!   data the Director reads; it never holds a DB handle and commits nothing.
//! - **Round-trips from partial JSON.** Every field `#[serde(default)]` so a partial
//!   payload deserializes to a fail-closed request at the default ([`DirectorHorizon::Beat`])
//!   altitude with an empty snapshot.

use crate::story::StoryState;
use serde::{Deserialize, Serialize};

/// The altitude at which the Director is being asked to plan. `Beat` is the fail-closed
/// default — the only altitude the shipped turn currently executes (design R4; the wider
/// scales are greenfield). An unknown/absent horizon reads as `Beat`, never as a wider,
/// more intrusive scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectorHorizon {
    /// Whole-campaign planning (active/dormant/emerging threads, thematic focus).
    Campaign,
    /// One scene's plan (purpose, beats, forbidden reveals).
    Scene,
    /// One turn's beat selection — the shipped altitude. Fail-closed default.
    #[default]
    Beat,
    /// The immediate actionable situation (facilitation; currently dormant on the turn).
    Situation,
}

/// A read-only projection of the story data the Director consumes for one request. Carries a
/// clone — never a handle — so it stays DB-free and cheap to pass across the proposal-only
/// boundary. Extend additively (module anchors, scene context) as wider altitudes land.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StorySnapshot {
    /// The story state the Director reads (threads, promises, arcs, pacing, interests).
    #[serde(default)]
    pub story_state: StoryState,
    /// The turn this request belongs to, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    /// The scene this request is scoped to, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene_id: Option<String>,
}

/// One Director request: the altitude + the read-only snapshot it plans against. Commit
/// nothing — this is the input the [`crate::director_plan::DirectorPlan`] is produced from.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DirectorRequest {
    /// Which altitude to plan at. Defaults to [`DirectorHorizon::Beat`].
    #[serde(default)]
    pub horizon: DirectorHorizon,
    /// The read-only story snapshot to plan against.
    #[serde(default)]
    pub snapshot: StorySnapshot,
    /// L8.2 — module-extracted [`crate::NarrativeAnchor`]s (L8.1) the Director MAY seed threads
    /// from (proposal-only). Additive: empty ⇒ no seeding ⇒ byte-identical to the snapshot path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub module_anchors: Vec<crate::NarrativeAnchor>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::story::{StoryState, StoryThread};

    #[test]
    fn director_horizon_default_is_beat() {
        assert_eq!(DirectorHorizon::default(), DirectorHorizon::Beat);
    }

    #[test]
    fn director_horizon_round_trips_each_variant() {
        for h in [
            DirectorHorizon::Campaign,
            DirectorHorizon::Scene,
            DirectorHorizon::Beat,
            DirectorHorizon::Situation,
        ] {
            let json = serde_json::to_string(&h).unwrap();
            let back: DirectorHorizon = serde_json::from_str(&json).unwrap();
            assert_eq!(h, back, "horizon {h:?} must round-trip");
        }
    }

    #[test]
    fn story_snapshot_round_trips() {
        let snap = StorySnapshot {
            story_state: StoryState {
                active_threads: vec![StoryThread {
                    thread_id: "t1".into(),
                    premise: "the missing professor".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            turn_id: Some("turn_12".into()),
            scene_id: Some("scene_2".into()),
        };
        let json = serde_json::to_string(&snap).unwrap();
        let back: StorySnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(snap, back);
    }

    #[test]
    fn director_request_round_trips_and_defaults_to_beat() {
        let req = DirectorRequest {
            horizon: DirectorHorizon::Scene,
            snapshot: StorySnapshot {
                turn_id: Some("turn_3".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: DirectorRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);

        // Partial JSON ⇒ fail-closed default (Beat, empty snapshot, no anchors).
        let partial: DirectorRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(partial.horizon, DirectorHorizon::Beat);
        assert_eq!(partial.snapshot, StorySnapshot::default());
        assert!(partial.module_anchors.is_empty());
    }

    #[test]
    fn module_anchors_round_trip_and_default_empty() {
        use crate::{NarrativeAnchor, NarrativeAnchorKind};
        // Empty anchors serialize away (skip_serializing_if) ⇒ byte-stable for pre-L8.2 payloads.
        let bare = DirectorRequest::default();
        let bare_json = serde_json::to_string(&bare).unwrap();
        assert!(
            !bare_json.contains("module_anchors"),
            "empty module_anchors must not appear in the wire form: {bare_json}"
        );

        let req = DirectorRequest {
            module_anchors: vec![NarrativeAnchor {
                anchor_id: "anchor_thread_s1_s2".into(),
                kind: NarrativeAnchorKind::PotentialThread,
                summary: "顺着日记线索深入地窖".into(),
                related_ids: vec!["s2".into()],
                ..Default::default()
            }],
            ..Default::default()
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: DirectorRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }
}
