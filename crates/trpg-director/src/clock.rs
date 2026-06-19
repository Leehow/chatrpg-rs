use trpg_model::{ClockTick, ConflictIntent, FrameRelation, SituationActionKind};

use crate::{env_bool, DirectorInput};

/// World-pressure clock ticks, derived from the UPSTREAM semantic situation router
/// output (`conflict.intent`) — NOT from scanning `user_input` for literal keywords
/// (semantic-over-keyword). The router already classifies the player's move; when
/// that classification is "pausing/observing" or "waiting/holding" the player is
/// not advancing the frame, so the world keeps moving and we tick scene pressure.
///
/// Fail-closed: no conflict, no intent, or a non-stalling intent ⇒ no tick. Because
/// we never literal-match, paraphrases the old keyword list missed are caught
/// (semantic), and bare keywords with no semantic signal no longer false-fire.
pub(crate) fn maybe_tick_clocks(input: DirectorInput<'_>) -> Vec<ClockTick> {
    if !env_bool("TRPG_DIRECTOR_CLOCK_ON_STALL", true) {
        return vec![];
    }
    let Some(intent) = input.conflict.and_then(|c| c.intent.as_ref()) else {
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
fn is_world_pressure_intent(intent: &ConflictIntent) -> bool {
    matches!(
        intent.relation_to_active_frame,
        FrameRelation::PauseAndObserve
    ) || matches!(intent.action_kind, SituationActionKind::WaitOrHoldAction)
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_combat::ConflictTurnResult;
    use trpg_model::{
        CompiledContext, ConflictIntent, ContextRequest, FrameRelation, RuntimeState,
        SituationActionKind, TokenBudget, VisibilityProfile,
    };

    // The literal list the OLD detector matched. The semantic detector must not
    // depend on any of these strings.
    const OLD_KEYWORDS: &[&str] = &[
        "继续等",
        "继续讨论",
        "不行动",
        "犹豫",
        "等着",
        "继续开火",
        "继续攻击",
        "还是打",
        "wait",
        "keep discussing",
        "do nothing",
        "keep firing",
        "continue attacking",
    ];

    fn req() -> ContextRequest {
        ContextRequest {
            ruleset_id: "coc".into(),
            module_id: None,
            session_id: "s1".into(),
            turn_id: "t1".into(),
            viewer: VisibilityProfile::gm(),
            token_budget: TokenBudget::default(),
        }
    }
    fn state() -> RuntimeState {
        RuntimeState {
            ruleset_id: "coc".into(),
            ..Default::default()
        }
    }
    fn compiled() -> CompiledContext {
        CompiledContext::default()
    }

    fn intent(rel: FrameRelation, act: SituationActionKind) -> ConflictIntent {
        ConflictIntent {
            relation_to_active_frame: rel,
            action_kind: act,
            ..Default::default()
        }
    }
    fn conflict_with(intent: ConflictIntent) -> ConflictTurnResult {
        ConflictTurnResult {
            intent: Some(intent),
            ..Default::default()
        }
    }

    fn input<'a>(
        r: &'a ContextRequest,
        s: &'a RuntimeState,
        c: &'a CompiledContext,
        user_input: &'a str,
        conflict: Option<&'a ConflictTurnResult>,
    ) -> DirectorInput<'a> {
        DirectorInput {
            request: r,
            state: s,
            compiled: c,
            user_input,
            conflict,
            module_config: None,
            participants: &[],
            prior_spotlights: &[],
        }
    }

    // Paraphrased stall the old keyword list would miss, but the semantic router
    // classified as PauseAndObserve → must tick world pressure.
    #[test]
    fn fires_on_semantic_stall_paraphrase_old_list_would_miss() {
        let paraphrase = "我们暂时按兵不动，先摸清楚周围的情况";
        assert!(
            !OLD_KEYWORDS.iter().any(|k| paraphrase.contains(k)),
            "paraphrase must evade the old keyword list"
        );
        let r = req();
        let s = state();
        let c = compiled();
        let conflict = conflict_with(intent(
            FrameRelation::PauseAndObserve,
            SituationActionKind::InvestigateDuringConflict,
        ));
        let ticks = maybe_tick_clocks(input(&r, &s, &c, paraphrase, Some(&conflict)));
        assert_eq!(
            ticks.len(),
            1,
            "semantic stall must tick even though no keyword matched"
        );
        assert_eq!(ticks[0].clock_id, "clock.scene_pressure");
    }

    // Explicit wait/hold action (semantic), arbitrary wording.
    #[test]
    fn fires_on_wait_or_hold_action_intent() {
        let r = req();
        let s = state();
        let c = compiled();
        let conflict = conflict_with(intent(
            FrameRelation::InsideFrameAction,
            SituationActionKind::WaitOrHoldAction,
        ));
        let ticks = maybe_tick_clocks(input(
            &r,
            &s,
            &c,
            "（任意措辞，不含旧关键词）",
            Some(&conflict),
        ));
        assert_eq!(
            ticks.len(),
            1,
            "semantic wait/hold must tick world pressure"
        );
    }

    // The keyword list is gone: a bare old keyword with NO semantic signal must
    // not fire (no false positive from literal matching).
    #[test]
    fn fail_closed_keyword_without_semantic_signal_does_not_fire() {
        let r = req();
        let s = state();
        let c = compiled();
        let ticks = maybe_tick_clocks(input(&r, &s, &c, "继续等", None));
        assert!(
            ticks.is_empty(),
            "no semantic signal → fail-closed, no keyword fallback"
        );
    }

    // Advancing the frame (attack) is not stalling → no world-pressure tick.
    #[test]
    fn does_not_fire_on_advancing_intent() {
        let r = req();
        let s = state();
        let c = compiled();
        let conflict = conflict_with(intent(
            FrameRelation::InsideFrameAction,
            SituationActionKind::Attack,
        ));
        let ticks = maybe_tick_clocks(input(&r, &s, &c, "我开枪还击", Some(&conflict)));
        assert!(
            ticks.is_empty(),
            "advancing the frame is not world-pressure stalling"
        );
    }
}
