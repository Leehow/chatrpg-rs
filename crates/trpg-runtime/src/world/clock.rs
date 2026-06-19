//! World-layer clock delegate (P4.5, codex F8a).
//!
//! Single source of truth is the pure core in `trpg_model::clock_core`
//! ([`trpg_model::derive_clock_proposals`]). This runtime-side function is a thin,
//! zero-logic delegate so the World layer can derive world-pressure
//! [`trpg_model::ClockTick`]s from an upstream [`trpg_model::ConflictIntent`] without
//! duplicating the predicate.
//!
//! Why a delegate and not duplicated logic: the director already shims the same core
//! (it owns the env gate `TRPG_DIRECTOR_CLOCK_ON_STALL`); hosting the logic only in the
//! model keeps it byte-identical for every caller and avoids a `trpg-director →
//! trpg-runtime` dependency cycle. This delegate is NOT yet wired into the turn loop
//! (P4.6); it adds no behavior to existing turns.
use trpg_model::{ClockTick, ConflictIntent};

/// Delegate to [`trpg_model::derive_clock_proposals`] — the pure core. No logic here:
/// same input ⇒ byte-identical output to the model. `None`/non-stalling intent ⇒ empty
/// (fail-closed).
pub fn derive_clock_proposals(intent: Option<&ConflictIntent>) -> Vec<ClockTick> {
    trpg_model::derive_clock_proposals(intent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::{FrameRelation, SituationActionKind};

    fn intent(rel: FrameRelation, act: SituationActionKind) -> ConflictIntent {
        ConflictIntent {
            relation_to_active_frame: rel,
            action_kind: act,
            ..Default::default()
        }
    }

    /// The runtime delegate must produce output identical to the model pure core for the
    /// same input — it is a single-source-of-truth passthrough, never a re-implementation.
    #[test]
    fn delegate_equals_model_core() {
        let cases = [
            None,
            Some(intent(
                FrameRelation::PauseAndObserve,
                SituationActionKind::InvestigateDuringConflict,
            )),
            Some(intent(
                FrameRelation::InsideFrameAction,
                SituationActionKind::WaitOrHoldAction,
            )),
            Some(intent(
                FrameRelation::InsideFrameAction,
                SituationActionKind::Attack,
            )),
        ];
        for case in &cases {
            // ClockTick has no PartialEq; compare canonical JSON (Serialize) — this is the
            // byte-identical equivalence the delegation contract requires.
            let via_delegate = serde_json::to_value(derive_clock_proposals(case.as_ref())).unwrap();
            let via_model =
                serde_json::to_value(trpg_model::derive_clock_proposals(case.as_ref())).unwrap();
            assert_eq!(
                via_delegate, via_model,
                "world::clock delegate must match model pure core byte-for-byte"
            );
        }
    }

    #[test]
    fn none_intent_fails_closed() {
        assert!(derive_clock_proposals(None).is_empty());
    }

    #[test]
    fn stalling_intent_proposes_scene_pressure_tick() {
        let i = intent(
            FrameRelation::PauseAndObserve,
            SituationActionKind::WaitOrHoldAction,
        );
        let ticks = derive_clock_proposals(Some(&i));
        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].clock_id, "clock.scene_pressure");
    }
}
