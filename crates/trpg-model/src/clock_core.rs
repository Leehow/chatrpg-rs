//! Clock pure core (P4.5, codex F8a): the single source of truth for deriving
//! world-pressure [`ClockTick`]s from an upstream semantic [`ConflictIntent`].
//!
//! Why it lives in trpg-model (the leaf), not trpg-runtime::world:
//! - `trpg-director` rewrites its `maybe_tick_clocks` as a thin shim over this core,
//!   and `trpg-runtime` already depends on `trpg-director`. Putting the core in
//!   `trpg-runtime` would force `trpg-director → trpg-runtime`, a dependency cycle.
//!   Hosting it in `trpg-model` keeps the model a leaf and lets both director and
//!   runtime delegate to one byte-identical implementation.
//!
//! Behavior contract (must stay byte-identical to the old director logic):
//! - Pure function of its single `ConflictIntent` input — no env reads, no IO, no
//!   state. The env gate (`TRPG_DIRECTOR_CLOCK_ON_STALL`) stays in the director shim.
//! - **Semantic over keyword.** The predicate consumes the router's classification
//!   (`relation_to_active_frame` / `action_kind`); it never literal-matches user text.
//! - **Fail-closed.** No intent, or a non-stalling intent ⇒ no tick (empty vec).
use crate::{ClockTick, ConflictIntent, FrameRelation, SituationActionKind};

/// Derive world-pressure clock ticks from an optional upstream [`ConflictIntent`].
///
/// This is the pure core extracted VERBATIM from the director's `maybe_tick_clocks`
/// (minus the env gate, which is an environment concern the shim retains). Same input
/// ⇒ same output, byte-for-byte. `None` or a non-stalling intent ⇒ empty.
pub fn derive_clock_proposals(intent: Option<&ConflictIntent>) -> Vec<ClockTick> {
    let Some(intent) = intent else {
        return vec![];
    };
    if !is_world_pressure_intent(intent) {
        return vec![];
    }
    vec![ClockTick {
        clock_id: "clock.scene_pressure".into(),
        label: "局势压力".into(),
        previous: 0,
        current: 1,
        max: 4,
        reason: "语义判定玩家本回合停顿/观望/等待、未推进局势，世界继续行动。".into(),
        visible_to_players: true,
    }]
}

/// The player is not advancing the active frame, so the world keeps moving.
/// Consumes the semantic router's classification; extend with new non-advancing
/// categories here — never with literal-string checks.
pub fn is_world_pressure_intent(intent: &ConflictIntent) -> bool {
    matches!(
        intent.relation_to_active_frame,
        FrameRelation::PauseAndObserve
    ) || matches!(intent.action_kind, SituationActionKind::WaitOrHoldAction)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent(rel: FrameRelation, act: SituationActionKind) -> ConflictIntent {
        ConflictIntent {
            relation_to_active_frame: rel,
            action_kind: act,
            ..Default::default()
        }
    }

    #[test]
    fn none_intent_fails_closed() {
        assert!(derive_clock_proposals(None).is_empty());
    }

    #[test]
    fn pause_and_observe_ticks_scene_pressure() {
        let i = intent(
            FrameRelation::PauseAndObserve,
            SituationActionKind::InvestigateDuringConflict,
        );
        let ticks = derive_clock_proposals(Some(&i));
        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].clock_id, "clock.scene_pressure");
        assert_eq!(ticks[0].label, "局势压力");
        assert_eq!(ticks[0].previous, 0);
        assert_eq!(ticks[0].current, 1);
        assert_eq!(ticks[0].max, 4);
        assert!(ticks[0].visible_to_players);
    }

    #[test]
    fn wait_or_hold_action_ticks() {
        let i = intent(
            FrameRelation::InsideFrameAction,
            SituationActionKind::WaitOrHoldAction,
        );
        assert_eq!(derive_clock_proposals(Some(&i)).len(), 1);
    }

    #[test]
    fn advancing_intent_does_not_tick() {
        let i = intent(
            FrameRelation::InsideFrameAction,
            SituationActionKind::Attack,
        );
        assert!(derive_clock_proposals(Some(&i)).is_empty());
    }

    #[test]
    fn is_world_pressure_intent_matches_both_channels() {
        assert!(is_world_pressure_intent(&intent(
            FrameRelation::PauseAndObserve,
            SituationActionKind::Attack
        )));
        assert!(is_world_pressure_intent(&intent(
            FrameRelation::InsideFrameAction,
            SituationActionKind::WaitOrHoldAction
        )));
        assert!(!is_world_pressure_intent(&intent(
            FrameRelation::InsideFrameAction,
            SituationActionKind::Attack
        )));
    }
}
