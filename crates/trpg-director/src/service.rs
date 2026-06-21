//! `StoryDirectorService` façade (L0.4): a thin, additive dispatch layer over today's two
//! live director engines — `build_director_brief_packet` (Beat altitude) and
//! `ActionableSituationDirector` (Situation altitude, dormant on the shipped turn per L0.1) —
//! keyed by [`trpg_model::DirectorHorizon`]. Campaign + Scene are greenfield and return an
//! explicit [`DirectorUnsupported`] (NEVER `todo!`/panic). No behavior change: the Beat path
//! is byte-identical to calling `build_director_brief_packet` directly.
//!
//! The crate stays DB-free (proposal-only boundary); a grep-guard test asserts
//! `trpg-director/Cargo.toml` carries no `trpg-db`/`sqlx` dep.

use crate::{ActionableSituationDirector, DirectorInput, DirectorMode};
use trpg_model::{
    DirectorHorizon, DirectorPlan, DirectorRequest, DirectorTurnResult, SpotlightState,
    WorldReactionCandidate,
};

/// The requested altitude is not implemented by this façade yet (Campaign/Scene). Explicit
/// error — the spine handles it deterministically rather than panicking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectorUnsupported {
    pub horizon: DirectorHorizon,
}

/// Inputs the Beat altitude needs beyond the [`DirectorRequest`] snapshot. These are exactly
/// the trailing arguments of `build_director_brief_packet`; the façade reads the story state
/// from `req.snapshot.story_state` and forwards the rest verbatim.
pub struct BeatPlanInputs<'a> {
    pub mode: DirectorMode,
    pub candidates: &'a [WorldReactionCandidate],
    pub player_known: Option<&'a [String]>,
    pub gm_truth: Option<&'a [String]>,
    pub spotlights: &'a [SpotlightState],
    pub rejected_thread_ids: &'a [String],
    pub acting_actor_id: &'a str,
}

/// Stateless façade. Holds no story state and no handle (proposal-only boundary).
#[derive(Debug, Clone, Copy, Default)]
pub struct StoryDirectorService;

impl StoryDirectorService {
    pub fn new() -> Self {
        Self
    }

    /// Which altitudes this façade serves today. Campaign/Scene are not yet implemented;
    /// Beat + Situation delegate to existing engines.
    pub fn supports(horizon: DirectorHorizon) -> bool {
        matches!(horizon, DirectorHorizon::Beat | DirectorHorizon::Situation)
    }

    /// Beat altitude: delegate to `build_director_brief_packet` with the snapshot's story
    /// state + the forwarded inputs. Byte-identical to the direct call. Any non-Beat horizon
    /// returns [`DirectorUnsupported`] (no `todo!`).
    pub fn plan_beat(
        &self,
        req: &DirectorRequest,
        inputs: &BeatPlanInputs<'_>,
    ) -> Result<DirectorPlan, DirectorUnsupported> {
        if req.horizon != DirectorHorizon::Beat {
            return Err(DirectorUnsupported {
                horizon: req.horizon,
            });
        }
        Ok(crate::build_director_brief_packet(
            inputs.mode,
            inputs.candidates,
            &req.snapshot.story_state,
            inputs.player_known,
            inputs.gm_truth,
            inputs.spotlights,
            inputs.rejected_thread_ids,
            inputs.acting_actor_id,
        ))
    }

    /// L1.2 SPINE — Beat altitude, POST-adjudication: build the base Beat plan, then re-shape it
    /// so the beat reflects the REAL committed result (`results`, the L1.1 projection). This is
    /// the load-bearing call at the `resolution_commit_boundary` seam — the first time the
    /// Director can see the committed `check_result`/effects. Fail-closed: empty/all-`Unresolved`
    /// `results` ⇒ identical to [`Self::plan_beat`] (no invented outcome). Any non-Beat horizon
    /// returns [`DirectorUnsupported`].
    pub fn plan_beat_post_adjudication(
        &self,
        req: &DirectorRequest,
        inputs: &BeatPlanInputs<'_>,
        results: &[trpg_model::MechanicalResultView],
    ) -> Result<DirectorPlan, DirectorUnsupported> {
        let base = self.plan_beat(req, inputs)?;
        Ok(crate::apply_committed_outcome(base, results))
    }

    /// L8.2 — Beat altitude with module-anchor seeding. When `TRPG_DIRECTOR_ANCHOR_SEED` is ON and
    /// `req.module_anchors` is non-empty, promote the request's `PotentialThread` anchors into
    /// proposal threads (proposal-only, DB-free) and plan over the augmented story so an
    /// anchor-seeded thread is selectable by the scorer. Fail-closed + flag-gated: OFF, or no
    /// anchors, ⇒ byte-identical to [`Self::plan_beat`] (no augmentation). Non-Beat ⇒
    /// [`DirectorUnsupported`].
    pub fn plan_beat_with_anchor_seeds(
        &self,
        req: &DirectorRequest,
        inputs: &BeatPlanInputs<'_>,
    ) -> Result<DirectorPlan, DirectorUnsupported> {
        if req.horizon != DirectorHorizon::Beat {
            return Err(DirectorUnsupported {
                horizon: req.horizon,
            });
        }
        if !crate::anchor_seed_enabled() || req.module_anchors.is_empty() {
            return self.plan_beat(req, inputs);
        }
        let augmented =
            crate::augment_story_with_anchor_seeds(&req.snapshot.story_state, &req.module_anchors);
        Ok(crate::build_director_brief_packet(
            inputs.mode,
            inputs.candidates,
            &augmented,
            inputs.player_known,
            inputs.gm_truth,
            inputs.spotlights,
            inputs.rejected_thread_ids,
            inputs.acting_actor_id,
        ))
    }

    /// Situation altitude: pure delegation to the existing facilitation engine. Dormant on
    /// the shipped turn (L0.1) — exposed for completeness, no behavior change.
    pub fn plan_situation(
        &self,
        director: &ActionableSituationDirector,
        input: DirectorInput<'_>,
    ) -> DirectorTurnResult {
        director.prepare(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::{StorySnapshot, StoryState, StoryThread, StoryThreadStatus};

    fn candidate(npc: &str) -> WorldReactionCandidate {
        WorldReactionCandidate {
            npc_id: npc.into(),
            stance: String::new(),
            emotional_state: String::new(),
            urgency: 0.8,
            feasibility: 0.7,
            risk: 0.3,
            action_intent: None,
            knowledge_basis: vec![],
            source_event_ids: vec![],
        }
    }

    fn story_with_thread() -> StoryState {
        StoryState {
            active_threads: vec![StoryThread {
                thread_id: "t.alpha".into(),
                premise: "the buried ledger".into(),
                status: StoryThreadStatus::Active,
                urgency: 0.9,
                momentum: 0.5,
                participant_ids: vec!["npc.broker".into()],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn beat_plan_is_byte_identical_to_direct_call() {
        let story = story_with_thread();
        let candidates = vec![candidate("npc.broker")];
        let req = DirectorRequest {
            horizon: DirectorHorizon::Beat,
            snapshot: StorySnapshot {
                story_state: story.clone(),
                ..Default::default()
            },
            ..Default::default()
        };
        let inputs = BeatPlanInputs {
            mode: DirectorMode::OnDemand,
            candidates: &candidates,
            player_known: None,
            gm_truth: None,
            spotlights: &[],
            rejected_thread_ids: &[],
            acting_actor_id: "pc.current",
        };

        let via_service = StoryDirectorService::new().plan_beat(&req, &inputs).unwrap();
        let direct = crate::build_director_brief_packet(
            DirectorMode::OnDemand,
            &candidates,
            &story,
            None,
            None,
            &[],
            &[],
            "pc.current",
        );

        // Byte-identical (serde) AND structurally equal.
        assert_eq!(
            serde_json::to_vec(&via_service).unwrap(),
            serde_json::to_vec(&direct).unwrap(),
            "façade Beat plan must be byte-identical to the direct call"
        );
        assert_eq!(via_service, direct);
        // The story state actually drove the plan (proves the snapshot was forwarded, not
        // a default StoryState): a thread was selected.
        assert!(
            via_service.primary_thread_id.is_some(),
            "snapshot story should have produced a thread selection"
        );
    }

    #[test]
    fn post_adjudication_failed_check_differs_from_pre_adjudication() {
        use trpg_model::{CheckOutcomeView, MechanicalResultView};
        let story = story_with_thread();
        let candidates = vec![candidate("npc.broker")];
        let req = DirectorRequest {
            horizon: DirectorHorizon::Beat,
            snapshot: StorySnapshot {
                story_state: story,
                ..Default::default()
            },
            ..Default::default()
        };
        let inputs = BeatPlanInputs {
            mode: DirectorMode::OnDemand,
            candidates: &candidates,
            player_known: None,
            gm_truth: None,
            spotlights: &[],
            rejected_thread_ids: &[],
            acting_actor_id: "pc.current",
        };
        let svc = StoryDirectorService::new();

        // Pre-adjudication: the Director guesses (no committed result seen).
        let pre = svc.plan_beat(&req, &inputs).unwrap();

        // Post-adjudication on a FAILED committed check: beat responds to the real failure.
        let failed = vec![MechanicalResultView {
            check_id: "c.dodge".into(),
            outcome: CheckOutcomeView::Failed,
            ..Default::default()
        }];
        let post = svc.plan_beat_post_adjudication(&req, &inputs, &failed).unwrap();

        assert_eq!(post.beat_kind, trpg_model::BeatKind::Complicate);
        assert_eq!(post.desired_change, crate::DESIRED_CHANGE_FAIL_FORWARD);
        // Provably different from the pre-adjudication plan on the SAME fixture.
        assert_ne!(post.beat_kind, pre.beat_kind);
        assert_ne!(post.desired_change, pre.desired_change);
        // Selection guarantees preserved (same thread spotlighted).
        assert_eq!(post.primary_thread_id, pre.primary_thread_id);

        // Fail-closed: no committed result ⇒ identical to pre-adjudication.
        let none = svc.plan_beat_post_adjudication(&req, &inputs, &[]).unwrap();
        assert_eq!(none, pre, "no committed result ⇒ no override");
    }

    #[test]
    fn campaign_and_scene_are_explicit_unsupported() {
        let svc = StoryDirectorService::new();
        let inputs = BeatPlanInputs {
            mode: DirectorMode::OnDemand,
            candidates: &[],
            player_known: None,
            gm_truth: None,
            spotlights: &[],
            rejected_thread_ids: &[],
            acting_actor_id: "pc.current",
        };
        for horizon in [DirectorHorizon::Campaign, DirectorHorizon::Scene] {
            let req = DirectorRequest {
                horizon,
                ..Default::default()
            };
            assert_eq!(
                svc.plan_beat(&req, &inputs),
                Err(DirectorUnsupported { horizon }),
                "{horizon:?} must be explicit Unsupported"
            );
            assert!(!StoryDirectorService::supports(horizon));
        }
        assert!(StoryDirectorService::supports(DirectorHorizon::Beat));
        assert!(StoryDirectorService::supports(DirectorHorizon::Situation));
    }

    #[test]
    fn anchor_seeds_off_is_byte_identical_to_plain_beat() {
        // L8.2 OFF byte-equal: with the flag default-OFF, even a request carrying module anchors
        // plans EXACTLY as plan_beat (no augmentation). (The flag is not set in this test ⇒ OFF.)
        use trpg_model::{NarrativeAnchor, NarrativeAnchorKind};
        let story = story_with_thread();
        let candidates = vec![candidate("npc.broker")];
        let req = DirectorRequest {
            horizon: DirectorHorizon::Beat,
            snapshot: StorySnapshot {
                story_state: story,
                ..Default::default()
            },
            module_anchors: vec![NarrativeAnchor {
                anchor_id: "anchor_thread_s1_s2".into(),
                kind: NarrativeAnchorKind::PotentialThread,
                summary: "顺着线索通向 s2".into(),
                related_ids: vec!["s2".into()],
                ..Default::default()
            }],
        };
        let inputs = BeatPlanInputs {
            mode: DirectorMode::OnDemand,
            candidates: &candidates,
            player_known: None,
            gm_truth: None,
            spotlights: &[],
            rejected_thread_ids: &[],
            acting_actor_id: "pc.current",
        };
        let svc = StoryDirectorService::new();
        assert_eq!(
            svc.plan_beat_with_anchor_seeds(&req, &inputs).unwrap(),
            svc.plan_beat(&req, &inputs).unwrap(),
            "flag OFF ⇒ anchor seeding is a no-op (byte-equal to plan_beat)"
        );
    }

    #[test]
    fn crate_has_no_db_or_sqlx_dependency() {
        // Proposal-only boundary: trpg-director must never gain a DB dep.
        let manifest = include_str!("../Cargo.toml");
        assert!(
            !manifest.contains("trpg-db"),
            "trpg-director must not depend on trpg-db"
        );
        assert!(
            !manifest.contains("sqlx"),
            "trpg-director must not depend on sqlx"
        );
    }
}
