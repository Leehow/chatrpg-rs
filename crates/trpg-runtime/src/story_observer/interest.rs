//! L7.2 — evolve player-interest signals + the `PacingState` history from the committed turn.
//!
//! The positive write-side counterpart of [`crate::story_write::merge_rejections`]: where that
//! persists a player REFUSAL, this persists a player ENGAGEMENT (signal strength rises, the
//! interest level climbs Engaged → Invested) and advances the pacing read (tension / relief /
//! escalation history) from the beat the Director committed. PURE, forward-only on strength
//! (engagement never silently decays here — disengagement is a separate proposed signal), and
//! GENERIC (the beat → tension mapping is structural, never branched on ruleset/module §二-⑪).
//! A REJECTED interest is never re-engaged by the observer (anti-railroad respected).

use trpg_model::story::BeatKind;
use trpg_model::{InterestSignal, PlayerInterestSignal, StoryState};

/// How much one engaged turn lifts a thread's interest strength (clamped to `1.0`).
const ENGAGE_STEP: f32 = 0.25;
/// Strength at/above which a sustained engagement reads as `Invested` rather than `Engaged`.
const INVESTED_THRESHOLD: f32 = 0.75;

/// Persist this turn's player engagement into `story.player_interests`. For each engaged thread:
/// a non-rejected signal has its strength raised by [`ENGAGE_STEP`] (capped) and its level lifted
/// toward `Invested`; a thread with no signal gets a fresh `Engaged` one; a REJECTED signal is
/// left untouched (the observer never re-engages a refused thread). Returns whether anything
/// changed.
pub(super) fn evolve_interest(story: &mut StoryState, engaged_thread_ids: &[String]) -> bool {
    let mut changed = false;
    for tid in engaged_thread_ids {
        if tid.is_empty() {
            continue;
        }
        if let Some(sig) = story
            .player_interests
            .iter_mut()
            .find(|s| &s.thread_id == tid)
        {
            if sig.rejected {
                continue; // never re-engage a refused thread
            }
            let new_strength = (sig.strength + ENGAGE_STEP).min(1.0);
            let new_signal = if new_strength >= INVESTED_THRESHOLD {
                InterestSignal::Invested
            } else {
                InterestSignal::Engaged
            };
            if new_strength != sig.strength || new_signal != sig.signal {
                sig.strength = new_strength;
                sig.signal = new_signal;
                changed = true;
            }
        } else {
            story.player_interests.push(PlayerInterestSignal {
                thread_id: tid.clone(),
                signal: InterestSignal::Engaged,
                strength: ENGAGE_STEP,
                ..Default::default()
            });
            changed = true;
        }
    }
    changed
}

/// Structural classification of a beat's effect on dramatic tension (generic; no ruleset branch).
/// `> 0` raises tension (an escalation beat), `< 0` releases it (a relief/payoff beat), `0` is a
/// neutral beat that lets tension drift.
fn beat_tension_delta(beat: BeatKind) -> f32 {
    match beat {
        BeatKind::Escalate | BeatKind::Complicate | BeatKind::Consequence => 0.2,
        BeatKind::Relief | BeatKind::Payoff => -0.3,
        _ => 0.0,
    }
}

/// Coarse pacing phase token derived from tension. Free-form snake_case (the producer's contract).
fn phase_for(tension: f32) -> &'static str {
    if tension >= 0.75 {
        "climax"
    } else if tension >= 0.4 {
        "rising"
    } else {
        "calm"
    }
}

/// Advance the `PacingState` history from the committed beat. An escalation beat raises tension and
/// resets `beats_since_escalation`; a relief/payoff beat lowers tension and resets
/// `time_since_relief`; any beat that is NOT a relief ages `time_since_relief`, and any beat that is
/// NOT an escalation ages `beats_since_escalation`. `None` ⇒ no change. Returns whether it changed.
pub(super) fn evolve_pacing(story: &mut StoryState, committed_beat: Option<BeatKind>) -> bool {
    let Some(beat) = committed_beat else {
        return false;
    };
    let pacing = &mut story.pacing;
    let before = pacing.clone();

    let delta = beat_tension_delta(beat);
    pacing.tension = (pacing.tension + delta).clamp(0.0, 1.0);

    let is_escalation = delta > 0.0;
    let is_relief = delta < 0.0;
    if is_escalation {
        pacing.beats_since_escalation = 0;
    } else {
        pacing.beats_since_escalation = pacing.beats_since_escalation.saturating_add(1);
    }
    if is_relief {
        pacing.time_since_relief = 0.0;
    } else {
        pacing.time_since_relief = (pacing.time_since_relief + 0.25).min(1.0);
    }
    pacing.phase = phase_for(pacing.tension).to_string();

    *pacing != before
}

#[cfg(test)]
mod tests {
    use super::*;

    // A fresh engaged thread gets an Engaged interest; a repeat engagement climbs to Invested.
    #[test]
    fn l72_engagement_creates_then_climbs_interest() {
        let mut story = StoryState::default();
        assert!(evolve_interest(&mut story, &["thr".into()]));
        assert_eq!(story.player_interests.len(), 1);
        assert_eq!(story.player_interests[0].signal, InterestSignal::Engaged);
        assert!((story.player_interests[0].strength - 0.25).abs() < f32::EPSILON);
        // Engage three more turns ⇒ strength 1.0 ⇒ Invested.
        for _ in 0..3 {
            evolve_interest(&mut story, &["thr".into()]);
        }
        assert_eq!(story.player_interests.len(), 1, "no duplicate signal");
        assert_eq!(story.player_interests[0].signal, InterestSignal::Invested);
        assert!((story.player_interests[0].strength - 1.0).abs() < f32::EPSILON);
    }

    // A rejected interest is never re-engaged by the observer.
    #[test]
    fn l72_rejected_interest_not_re_engaged() {
        let mut story = StoryState {
            player_interests: vec![PlayerInterestSignal {
                thread_id: "thr".into(),
                signal: InterestSignal::Avoidant,
                rejected: true,
                strength: 0.1,
                ..Default::default()
            }],
            ..Default::default()
        };
        let changed = evolve_interest(&mut story, &["thr".into()]);
        assert!(!changed);
        assert!(story.player_interests[0].rejected);
        assert_eq!(story.player_interests[0].signal, InterestSignal::Avoidant);
    }

    // Empty engagement ⇒ no change.
    #[test]
    fn l72_no_engagement_is_noop() {
        let mut story = StoryState::default();
        assert!(!evolve_interest(&mut story, &[]));
        assert!(story.player_interests.is_empty());
    }

    // Pacing evolves causally: escalation raises tension; a later relief lowers it and resets the
    // relief clock; escalation resets the escalation counter.
    #[test]
    fn l72_pacing_evolves_causally() {
        let mut story = StoryState::default();
        // Two escalation beats ⇒ tension climbs, beats_since_escalation stays 0.
        assert!(evolve_pacing(&mut story, Some(BeatKind::Escalate)));
        assert!((story.pacing.tension - 0.2).abs() < f32::EPSILON);
        assert_eq!(story.pacing.beats_since_escalation, 0);
        assert!(story.pacing.time_since_relief > 0.0, "no relief yet ⇒ relief clock ages");
        assert!(evolve_pacing(&mut story, Some(BeatKind::Complicate)));
        assert!((story.pacing.tension - 0.4).abs() < 1e-6);
        assert_eq!(story.pacing.phase, "rising");
        // A neutral beat ages the escalation counter without changing tension classification.
        assert!(evolve_pacing(&mut story, Some(BeatKind::Respond)));
        assert_eq!(story.pacing.beats_since_escalation, 1);
        // A relief beat lowers tension and zeroes the relief clock.
        let before_tension = story.pacing.tension;
        assert!(evolve_pacing(&mut story, Some(BeatKind::Relief)));
        assert!(story.pacing.tension < before_tension);
        assert!((story.pacing.time_since_relief).abs() < f32::EPSILON);
    }

    // No committed beat ⇒ pacing untouched.
    #[test]
    fn l72_no_beat_is_noop() {
        let mut story = StoryState::default();
        assert!(!evolve_pacing(&mut story, None));
        assert_eq!(story.pacing, Default::default());
    }
}
