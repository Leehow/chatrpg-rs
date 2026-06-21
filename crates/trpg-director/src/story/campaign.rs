//! L5.1 — Campaign-scale plan + the deterministic dormancy/emergence classifier.
//!
//! The campaign horizon ([`trpg_model::DirectorHorizon::Campaign`]) reasons over the WHOLE story
//! at session/chapter boundaries — NOT per turn (cost guard). This module lands the typed
//! [`CampaignPlan`], the missing deterministic dormancy/emergence classifier
//! ([`classify_campaign`]), and the boundary gate ([`should_run_campaign`]) + flag reader
//! ([`campaign_plan_enabled`]).
//!
//! PURE / proposal-only / DB-free / deterministic / GENERIC — classification is a function of the
//! typed thread/arc SIGNALS (status, momentum, player_interest, urgency, spotlight_debt), NEVER
//! branched on `ruleset_id`/`module_id` (§二-⑪). Nothing here is wired into the per-turn path, so
//! OFF is byte-identical (no caller); the rare boundary invocation is the owning runtime's job,
//! gated by [`should_run_campaign`].

use trpg_model::{CharacterArcState, StoryThread, StoryThreadStatus};

/// Flag gate for the campaign-scale plan. Default OFF. Mirrors the other director flags.
pub fn campaign_plan_enabled() -> bool {
    std::env::var("TRPG_DIRECTOR_CAMPAIGN_PLAN")
        .ok()
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// Cost guard: the Campaign plan runs ONLY at a session/chapter boundary AND only when the flag is
/// on. A per-turn caller passing `boundary = false` is a no-op — the expensive whole-story pass
/// never runs mid-scene.
pub fn should_run_campaign(boundary: bool) -> bool {
    boundary && campaign_plan_enabled()
}

/// A whole-campaign read: which threads are live / fading / surfacing, the recurring themes, the
/// long-horizon pressures, and which characters are owed spotlight. Proposal-only.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CampaignPlan {
    /// Threads carrying live pressure right now.
    pub active_thread_ids: Vec<String>,
    /// Threads that have gone quiet (status Dormant, or a live thread that lost all momentum AND
    /// interest) — candidates to retire or re-seed.
    pub dormant_thread_ids: Vec<String>,
    /// Threads surfacing into play (freshly Introduced, or a Dormant thread the player is showing
    /// interest in) — candidates to promote.
    pub emerging_thread_ids: Vec<String>,
    /// Recurring dramatic questions across the active threads (deduped, order-stable) — the
    /// campaign's thematic focus.
    pub thematic_focus: Vec<String>,
    /// High-urgency active threads (urgency ≥ 0.6) — pressures that should pay off over the long arc.
    pub long_horizon_pressures: Vec<String>,
    /// Character ids ordered by spotlight debt (desc, id-asc tiebreak) — who is owed the spotlight.
    pub character_arc_priorities: Vec<String>,
}

/// How a thread reads at the campaign scale once SIGNALS (not just raw status) are considered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThreadBucket {
    Active,
    Dormant,
    Emerging,
    /// Terminal (Resolved/Abandoned/Transformed) — excluded from the campaign read.
    Retired,
}

/// The deterministic dormancy/emergence classifier. Overrides raw status with signal-derived
/// dormancy/emergence (the gap the design flags as missing):
/// - terminal statuses ⇒ Retired (excluded);
/// - `Dormant` + player interest ≥ 0.5 ⇒ Emerging (re-surfacing on player pull);
/// - `Dormant` otherwise ⇒ Dormant;
/// - `Introduced` ⇒ Emerging (freshly surfaced, not yet driving);
/// - a live thread (Active/Escalating/ReadyForPayoff) that has lost momentum AND interest
///   (both < 0.2) ⇒ Dormant (it has faded despite its status);
/// - any other live thread ⇒ Active.
fn classify_thread(t: &StoryThread) -> ThreadBucket {
    use StoryThreadStatus::*;
    match t.status {
        Resolved | Abandoned | Transformed => ThreadBucket::Retired,
        Dormant if t.player_interest >= 0.5 => ThreadBucket::Emerging,
        Dormant => ThreadBucket::Dormant,
        Introduced => ThreadBucket::Emerging,
        Active | Escalating | ReadyForPayoff => {
            if t.momentum < 0.2 && t.player_interest < 0.2 {
                ThreadBucket::Dormant
            } else {
                ThreadBucket::Active
            }
        }
    }
}

/// L5.1 — derive a [`CampaignPlan`] from the whole story's threads + character arcs (PURE,
/// deterministic, generic). Order-stable (follows input order; arc priorities sorted by debt).
pub fn classify_campaign(threads: &[StoryThread], arcs: &[CharacterArcState]) -> CampaignPlan {
    let mut active_thread_ids = Vec::new();
    let mut dormant_thread_ids = Vec::new();
    let mut emerging_thread_ids = Vec::new();
    let mut thematic_focus: Vec<String> = Vec::new();
    let mut long_horizon_pressures = Vec::new();

    for t in threads {
        match classify_thread(t) {
            ThreadBucket::Active => {
                active_thread_ids.push(t.thread_id.clone());
                if !t.dramatic_question.is_empty()
                    && !thematic_focus.contains(&t.dramatic_question)
                {
                    thematic_focus.push(t.dramatic_question.clone());
                }
                if t.urgency >= 0.6 {
                    long_horizon_pressures.push(t.thread_id.clone());
                }
            }
            ThreadBucket::Dormant => dormant_thread_ids.push(t.thread_id.clone()),
            ThreadBucket::Emerging => emerging_thread_ids.push(t.thread_id.clone()),
            ThreadBucket::Retired => {}
        }
    }

    // Character arc priorities: by spotlight debt desc, then id asc. Only characters with a real id.
    let mut prioritized: Vec<&CharacterArcState> =
        arcs.iter().filter(|a| !a.character_id.is_empty()).collect();
    prioritized.sort_by(|a, b| {
        b.spotlight_debt
            .partial_cmp(&a.spotlight_debt)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.character_id.cmp(&b.character_id))
    });
    let character_arc_priorities = prioritized.iter().map(|a| a.character_id.clone()).collect();

    CampaignPlan {
        active_thread_ids,
        dormant_thread_ids,
        emerging_thread_ids,
        thematic_focus,
        long_horizon_pressures,
        character_arc_priorities,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread(id: &str, status: StoryThreadStatus) -> StoryThread {
        StoryThread {
            thread_id: id.into(),
            status,
            momentum: 0.6,
            player_interest: 0.6,
            ..Default::default()
        }
    }

    fn arc(id: &str, debt: f32) -> CharacterArcState {
        CharacterArcState {
            character_id: id.into(),
            spotlight_debt: debt,
            ..Default::default()
        }
    }

    // Boundary gate: campaign never runs per-turn; it needs BOTH a boundary AND the flag on.
    #[test]
    fn should_run_only_at_boundary_with_flag() {
        std::env::set_var("TRPG_DIRECTOR_CAMPAIGN_PLAN", "1");
        assert!(should_run_campaign(true), "boundary + flag ⇒ run");
        assert!(!should_run_campaign(false), "mid-scene ⇒ never run");
        std::env::remove_var("TRPG_DIRECTOR_CAMPAIGN_PLAN");
        assert!(!should_run_campaign(true), "flag off ⇒ never run");
    }

    // Raw status buckets: live→active, Introduced→emerging, Dormant→dormant, Resolved→retired.
    #[test]
    fn classifier_buckets_by_status() {
        let mut t_dorm = thread("t_dorm", StoryThreadStatus::Dormant);
        t_dorm.player_interest = 0.1; // genuinely quiet — below the re-emergence threshold
        let threads = vec![
            thread("t_active", StoryThreadStatus::Active),
            thread("t_intro", StoryThreadStatus::Introduced),
            t_dorm,
            thread("t_done", StoryThreadStatus::Resolved),
        ];
        let plan = classify_campaign(&threads, &[]);
        assert_eq!(plan.active_thread_ids, vec!["t_active"]);
        assert_eq!(plan.emerging_thread_ids, vec!["t_intro"]);
        assert_eq!(plan.dormant_thread_ids, vec!["t_dorm"]);
        // Resolved is retired ⇒ in none of the buckets.
        assert!(!plan.active_thread_ids.contains(&"t_done".to_string()));
    }

    // Signal override (the dormancy classifier's reason for existing): a still-Active thread that
    // has lost momentum AND interest fades to dormant; a Dormant thread the player wants re-emerges.
    #[test]
    fn classifier_overrides_status_by_signal() {
        let mut faded = thread("t_faded", StoryThreadStatus::Active);
        faded.momentum = 0.1;
        faded.player_interest = 0.1;
        let mut rewanted = thread("t_rewanted", StoryThreadStatus::Dormant);
        rewanted.player_interest = 0.8;
        let plan = classify_campaign(&[faded, rewanted], &[]);
        assert_eq!(
            plan.dormant_thread_ids,
            vec!["t_faded"],
            "live-but-faded ⇒ dormant"
        );
        assert_eq!(
            plan.emerging_thread_ids,
            vec!["t_rewanted"],
            "dormant-but-wanted ⇒ emerging"
        );
    }

    // Thematic focus dedupes dramatic questions; long-horizon pressures pick high-urgency actives.
    #[test]
    fn thematic_focus_and_pressures_derived() {
        let mut t1 = thread("t1", StoryThreadStatus::Active);
        t1.dramatic_question = "who betrayed the crew?".into();
        t1.urgency = 0.9;
        let mut t2 = thread("t2", StoryThreadStatus::Escalating);
        t2.dramatic_question = "who betrayed the crew?".into(); // duplicate ⇒ deduped
        t2.urgency = 0.3;
        let plan = classify_campaign(&[t1, t2], &[]);
        assert_eq!(plan.thematic_focus, vec!["who betrayed the crew?"]);
        assert_eq!(
            plan.long_horizon_pressures,
            vec!["t1"],
            "only the urgency≥0.6 active thread is a long-horizon pressure"
        );
    }

    // Arc priorities sort by spotlight debt desc, id asc tiebreak; empty ids dropped.
    #[test]
    fn arc_priorities_sorted_by_debt() {
        let arcs = vec![
            arc("npc_low", 0.2),
            arc("npc_high", 0.9),
            arc("", 1.0), // empty id dropped
            arc("npc_mid_b", 0.5),
            arc("npc_mid_a", 0.5), // tie ⇒ id asc
        ];
        let plan = classify_campaign(&[], &arcs);
        assert_eq!(
            plan.character_arc_priorities,
            vec!["npc_high", "npc_mid_a", "npc_mid_b", "npc_low"]
        );
    }
}
