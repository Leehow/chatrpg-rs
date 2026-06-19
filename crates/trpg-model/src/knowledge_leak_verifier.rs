//! Knowledge Leak Verifier v1 (TC-P2-02): deterministic, pure checks that flag
//! (a) player-visible leakage of **player-unknown** facts and (b) NPC output that
//! contradicts the speaking NPC's own [`NpcMindView`] / [`NpcBehaviorPlan`].
//!
//! Design contract (foundation-only, ready for later GM turn-loop integration):
//! - **Pure + deterministic.** Every function is a pure function of its inputs: no
//!   DB, no network, no provider/API key, no stream mutation. Same inputs → same
//!   findings, in a stable order.
//! - **Projection-driven.** Player-known gating uses the caller-provided durable
//!   player-knowledge projection ([`scan_player_visible_leak`]'s
//!   `player_known_fact_ids`), never a raw context-visibility guess. NPC checks
//!   consult only the NPC's own [`NpcMindView`] / [`NpcBehaviorPlan`]; GM omniscient
//!   world truth is never read here.
//! - **Fail-closed.** When a fact cannot be proven player-known (or an NPC cannot be
//!   proven permitted to reveal it), it is flagged rather than allowed.
//! - **Redaction.** A [`KnowledgeLeakFinding`] references fact ids and holder ids and
//!   a safe summary — it NEVER echoes the raw secret term, even when the scan matched
//!   on that term. When no fact id is available it uses a redacted placeholder.
//! - **Bounded scanning.** Text scanning only ever matches caller-provided
//!   [`FactSurfaceMarker`] terms (or caller-extracted fact-id lists). No NLP, no
//!   inference, no raw-source-text secret extraction.
//! - **Belief is not truth.** A fact the NPC merely believes (incl. a false belief)
//!   is never promoted to known truth and never becomes a revealable fact; it can be
//!   identified separately (see [`scan_npc_asserted_facts`]) without leaking.
//!
//! This module owns the pure model-level types and logic. `trpg-model` has no
//! `trpg-agent` dependency, so the [`KnowledgeLeakFinding`] DTO is intentionally
//! additive and bridgeable to `trpg_agent::VerifierFinding` in the runtime layer
//! (see `trpg_runtime::knowledge_leak_verifier`).
use crate::npc_behavior::NpcBehaviorPlan;
use crate::npc_mind::NpcMindView;
use serde::{Deserialize, Serialize};

/// What kind of knowledge leak / inconsistency a finding represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeLeakKind {
    /// Player-visible narration mentions a fact the player party does not yet know.
    PlayerUnknownFactLeak,
    /// NPC output discloses a fact that is not in the NPC's `facts_can_reveal`
    /// because the NPC does not even know it as true (outside its plan entirely).
    NpcRevealsUnplannedFact,
    /// NPC output discloses a known fact the behavior plan explicitly withholds
    /// (a protected secret below the reveal threshold).
    NpcRevealsWithheldSecret,
    /// NPC output asserts as known truth a fact the NPC only believes (possibly a
    /// false belief) or does not hold at all — a consistency issue, not a secret leak.
    NpcStatesUnknownAsFact,
}

impl KnowledgeLeakKind {
    /// Stable snake_case token (for traces / bridging).
    pub fn as_token(&self) -> &'static str {
        match self {
            KnowledgeLeakKind::PlayerUnknownFactLeak => "player_unknown_fact_leak",
            KnowledgeLeakKind::NpcRevealsUnplannedFact => "npc_reveals_unplanned_fact",
            KnowledgeLeakKind::NpcRevealsWithheldSecret => "npc_reveals_withheld_secret",
            KnowledgeLeakKind::NpcStatesUnknownAsFact => "npc_states_unknown_as_fact",
        }
    }
}

/// Finding severity. `Blocker` mirrors the existing verifier "must not ship" gate;
/// `Warning` is a consistency note the GM may surface without hard-stopping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeakSeverity {
    Blocker,
    Warning,
}

/// A single knowledge-leak / NPC-inconsistency finding. Carries ids + a safe
/// summary only; it never contains the raw secret term that triggered the match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeLeakFinding {
    pub kind: KnowledgeLeakKind,
    pub severity: LeakSeverity,
    /// The fact id at issue, when one is available (preferred over any text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact_id: Option<String>,
    /// The holder/NPC id at issue, for NPC-consistency findings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder_id: Option<String>,
    /// Redacted, human-readable summary. NEVER contains the raw secret term.
    pub detail: String,
}

/// A caller-provided, deterministic marker binding a fact id to the exact surface
/// terms whose presence in player-visible text would reveal that fact. The terms
/// are the ONLY thing scanned for — no NLP, no inference. Supplying this is how a
/// caller keeps secret text out of the verifier's own output: the verifier reports
/// the `fact_id`, never the matched term.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactSurfaceMarker {
    pub fact_id: String,
    /// Exact substrings that would expose the fact in player-visible text.
    #[serde(default)]
    pub terms: Vec<String>,
}

impl FactSurfaceMarker {
    pub fn new(fact_id: impl Into<String>, terms: Vec<String>) -> Self {
        Self {
            fact_id: fact_id.into(),
            terms,
        }
    }

    /// `Some(id)` when the fact id is non-empty, else `None` (so findings prefer a
    /// real id and fall back to a redacted placeholder rather than echoing text).
    fn fact_id_opt(&self) -> Option<String> {
        let id = self.fact_id.trim();
        if id.is_empty() {
            None
        } else {
            Some(id.to_string())
        }
    }
}

fn known(player_known_fact_ids: &[String], fact_id: &str) -> bool {
    player_known_fact_ids.iter().any(|k| k == fact_id)
}

/// Scan player-visible `narration` for terms bound to **player-unknown** facts.
///
/// For each [`FactSurfaceMarker`] whose `fact_id` is NOT in the durable
/// `player_known_fact_ids` projection, if any of its terms appears in the narration
/// a `Blocker` [`KnowledgeLeakKind::PlayerUnknownFactLeak`] is produced. Facts the
/// player already knows are allowed (already-revealed facts may be freely restated).
///
/// Redaction: the finding reports the `fact_id` (or a redacted placeholder when the
/// marker has no id), never the matched term. Deterministic: markers are scanned in
/// order and at most one finding is produced per marker.
pub fn scan_player_visible_leak(
    narration: &str,
    markers: &[FactSurfaceMarker],
    player_known_fact_ids: &[String],
) -> Vec<KnowledgeLeakFinding> {
    if narration.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for marker in markers {
        let fact_id = marker.fact_id_opt();
        // Player already knows this fact → allowed (cannot "leak" what is known).
        if let Some(id) = &fact_id {
            if known(player_known_fact_ids, id) {
                continue;
            }
        }
        // Bounded scan: only caller-provided terms, ignoring empty ones.
        let hit = marker
            .terms
            .iter()
            .any(|t| !t.is_empty() && narration.contains(t.as_str()));
        if !hit {
            continue;
        }
        let detail = match &fact_id {
            Some(id) => format!("player-visible narration exposes player-unknown fact '{id}'"),
            None => "player-visible narration exposes a player-unknown private fact \
                     (no fact id bound)"
                .to_string(),
        };
        out.push(KnowledgeLeakFinding {
            kind: KnowledgeLeakKind::PlayerUnknownFactLeak,
            severity: LeakSeverity::Blocker,
            fact_id,
            holder_id: None,
            detail,
        });
    }
    out
}

/// Check NPC output against the speaking NPC's own [`NpcBehaviorPlan`].
///
/// `disclosed_fact_ids` are fact ids the caller has deterministically extracted from
/// the NPC's generated speech/action (e.g. via fact-id markers — this function does
/// no text scanning itself). For each disclosed id that is NOT in `facts_can_reveal`:
/// - if the plan explicitly lists it under `facts_will_withhold`, emit a `Blocker`
///   [`KnowledgeLeakKind::NpcRevealsWithheldSecret`];
/// - otherwise emit a `Blocker` [`KnowledgeLeakKind::NpcRevealsUnplannedFact`] (the
///   NPC does not even know the fact as true, so it is outside the plan entirely).
///
/// Fail-closed: anything not provably permitted (`facts_can_reveal`) is flagged. The
/// plan is derived purely from the NPC's mind view, so this never consults GM truth.
pub fn scan_npc_disclosure(
    plan: &NpcBehaviorPlan,
    disclosed_fact_ids: &[String],
) -> Vec<KnowledgeLeakFinding> {
    let mut out = Vec::new();
    for fact_id in disclosed_fact_ids {
        if plan.facts_can_reveal.iter().any(|f| f == fact_id) {
            continue; // permitted disclosure
        }
        let withheld = plan.facts_will_withhold.iter().any(|f| f == fact_id);
        let (kind, detail) = if withheld {
            (
                KnowledgeLeakKind::NpcRevealsWithheldSecret,
                format!(
                    "NPC '{}' disclosed withheld protected fact '{}' (below reveal threshold)",
                    plan.npc_id, fact_id
                ),
            )
        } else {
            (
                KnowledgeLeakKind::NpcRevealsUnplannedFact,
                format!(
                    "NPC '{}' disclosed fact '{}' outside its behavior plan (not in facts_can_reveal)",
                    plan.npc_id, fact_id
                ),
            )
        };
        out.push(KnowledgeLeakFinding {
            kind,
            severity: LeakSeverity::Blocker,
            fact_id: Some(fact_id.clone()),
            holder_id: Some(plan.npc_id.clone()),
            detail,
        });
    }
    out
}

/// Whether an NPC may state `fact_id` as **known truth**. Only the NPC's
/// `knows_true` facts qualify; beliefs (incl. false beliefs) and unheld facts return
/// `false`, so a belief is never promoted to truth.
pub fn npc_may_assert_as_known_truth(view: &NpcMindView, fact_id: &str) -> bool {
    view.known_fact_ids().iter().any(|k| *k == fact_id)
}

/// Check fact ids the NPC asserted **as known truth** against its mind view.
///
/// For each asserted id the NPC does not hold as `knows_true`, emit a `Warning`
/// [`KnowledgeLeakKind::NpcStatesUnknownAsFact`]. A belief the NPC holds is
/// identified here (the finding's `fact_id` lets the caller see it is a belief via
/// [`NpcMindView::belief_fact_ids`]) WITHOUT promoting it to a revealable fact. This
/// keeps false beliefs honestly labeled rather than treated as world truth.
pub fn scan_npc_asserted_facts(
    view: &NpcMindView,
    asserted_as_known_fact_ids: &[String],
) -> Vec<KnowledgeLeakFinding> {
    let mut out = Vec::new();
    for fact_id in asserted_as_known_fact_ids {
        if npc_may_assert_as_known_truth(view, fact_id) {
            continue; // genuinely known truth
        }
        let is_belief = view.belief_fact_ids().iter().any(|b| *b == fact_id);
        let detail = if is_belief {
            format!(
                "NPC '{}' asserted belief '{}' as known truth (belief is not world truth)",
                view.npc_id, fact_id
            )
        } else {
            format!(
                "NPC '{}' asserted fact '{}' as known truth but does not hold it",
                view.npc_id, fact_id
            )
        };
        out.push(KnowledgeLeakFinding {
            kind: KnowledgeLeakKind::NpcStatesUnknownAsFact,
            severity: LeakSeverity::Warning,
            fact_id: Some(fact_id.clone()),
            holder_id: Some(view.npc_id.clone()),
            detail,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        KnowledgeState, NpcBehaviorContext, NpcKnowledgeEntry, NpcMindView, NpcProfile,
        NpcRelationship, NpcRelationshipDelta, NpcRelationshipTarget,
    };

    const SECRET_TEXT: &str = "管家其实是幕后真凶";

    fn profile() -> NpcProfile {
        NpcProfile {
            actor_id: "npc_lars".into(),
            name: "Lars".into(),
            ..Default::default()
        }
    }

    fn view(entries: &[NpcKnowledgeEntry]) -> NpcMindView {
        NpcMindView::build("s", "npc_lars", &profile(), &[], entries).unwrap()
    }

    fn trusting_rel() -> NpcRelationship {
        // High trust so the default reveal willingness clears the secret threshold.
        let mut r =
            NpcRelationship::new("s", "npc_lars", NpcRelationshipTarget::PlayerParty).unwrap();
        r.apply_delta(&NpcRelationshipDelta {
            trust: 90,
            evidence_event_ids: vec!["e".into()],
            ..Default::default()
        })
        .unwrap();
        r
    }

    fn hostile_rel() -> NpcRelationship {
        let mut r =
            NpcRelationship::new("s", "npc_lars", NpcRelationshipTarget::PlayerParty).unwrap();
        r.apply_delta(&NpcRelationshipDelta {
            hostility: 80,
            evidence_event_ids: vec!["e".into()],
            ..Default::default()
        })
        .unwrap();
        r
    }

    fn view_with_rel(rel: NpcRelationship, entries: &[NpcKnowledgeEntry]) -> NpcMindView {
        NpcMindView::build("s", "npc_lars", &profile(), &[rel], entries).unwrap()
    }

    #[test]
    fn player_unknown_fact_leak_is_blocker() {
        let markers = vec![FactSurfaceMarker::new(
            "fact_villain",
            vec![SECRET_TEXT.into()],
        )];
        let findings = scan_player_visible_leak(
            "旁白：管家其实是幕后真凶，你却毫不知情。",
            &markers,
            &[], // player knows nothing
        );
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, KnowledgeLeakKind::PlayerUnknownFactLeak);
        assert_eq!(findings[0].severity, LeakSeverity::Blocker);
        assert_eq!(findings[0].fact_id.as_deref(), Some("fact_villain"));
    }

    #[test]
    fn player_known_fact_is_allowed() {
        let markers = vec![FactSurfaceMarker::new(
            "fact_villain",
            vec![SECRET_TEXT.into()],
        )];
        // Same narration, but the fact is already player-known → freely restated.
        let findings = scan_player_visible_leak(
            "旁白：管家其实是幕后真凶，正如你早已查明的那样。",
            &markers,
            &["fact_villain".into()],
        );
        assert!(findings.is_empty(), "already-known fact must be allowed");
    }

    #[test]
    fn npc_cannot_reveal_fact_outside_behavior_plan() {
        // NPC knows only "k1"; the plan therefore can reveal at most "k1".
        let entries = vec![NpcKnowledgeEntry {
            fact_id: "k1".into(),
            state: KnowledgeState::KnowsTrue,
        }];
        let plan = NpcBehaviorPlan::derive(
            &view_with_rel(trusting_rel(), &entries),
            &NpcBehaviorContext::default(),
        );
        // The NPC's speech tried to disclose "fact_secret_plot" it does not know.
        let findings = scan_npc_disclosure(&plan, &["fact_secret_plot".into()]);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, KnowledgeLeakKind::NpcRevealsUnplannedFact);
        assert_eq!(findings[0].severity, LeakSeverity::Blocker);
        assert_eq!(findings[0].fact_id.as_deref(), Some("fact_secret_plot"));
        assert_eq!(findings[0].holder_id.as_deref(), Some("npc_lars"));
    }

    #[test]
    fn npc_withheld_secret_reveal_is_flagged() {
        // NPC knows "s1" but it is a secret and the relationship is hostile → withheld.
        let entries = vec![NpcKnowledgeEntry {
            fact_id: "s1".into(),
            state: KnowledgeState::KnowsTrue,
        }];
        let ctx = NpcBehaviorContext {
            secret_fact_ids: vec!["s1".into()],
            ..Default::default()
        };
        let plan = NpcBehaviorPlan::derive(&view_with_rel(hostile_rel(), &entries), &ctx);
        assert!(
            plan.facts_will_withhold.iter().any(|f| f == "s1"),
            "precondition: secret is withheld"
        );
        let findings = scan_npc_disclosure(&plan, &["s1".into()]);
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].kind,
            KnowledgeLeakKind::NpcRevealsWithheldSecret
        );
        assert_eq!(findings[0].severity, LeakSeverity::Blocker);
        assert_eq!(findings[0].fact_id.as_deref(), Some("s1"));
    }

    #[test]
    fn npc_false_belief_is_not_treated_as_known_truth() {
        let entries = vec![
            NpcKnowledgeEntry {
                fact_id: "k_true".into(),
                state: KnowledgeState::KnowsTrue,
            },
            NpcKnowledgeEntry {
                fact_id: "b_false".into(),
                state: KnowledgeState::BelievesFalse,
            },
        ];
        let v = view(&entries);
        // Belief is never known truth.
        assert!(npc_may_assert_as_known_truth(&v, "k_true"));
        assert!(!npc_may_assert_as_known_truth(&v, "b_false"));
        // Belief never becomes a revealable / known fact in the derived plan.
        let plan = NpcBehaviorPlan::derive(&v, &NpcBehaviorContext::default());
        assert!(!plan.facts_knows.iter().any(|f| f == "b_false"));
        assert!(!plan.facts_can_reveal.iter().any(|f| f == "b_false"));
        // But the false belief IS identifiable separately, without promotion.
        assert!(v.belief_fact_ids().iter().any(|b| *b == "b_false"));
        let findings = scan_npc_asserted_facts(&v, &["b_false".into()]);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, KnowledgeLeakKind::NpcStatesUnknownAsFact);
        assert_eq!(findings[0].fact_id.as_deref(), Some("b_false"));
    }

    #[test]
    fn verifier_finding_does_not_echo_secret_text() {
        let markers = vec![FactSurfaceMarker::new(
            "fact_villain",
            vec![SECRET_TEXT.into()],
        )];
        let narration = format!("旁白：{SECRET_TEXT}。");
        let findings = scan_player_visible_leak(&narration, &markers, &[]);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert!(
            !f.detail.contains(SECRET_TEXT),
            "finding detail must not echo the raw secret term"
        );
        assert_eq!(
            f.fact_id.as_deref(),
            Some("fact_villain"),
            "finding must reference the fact id instead"
        );
        // Full serialized finding must also be free of the secret term.
        let json = serde_json::to_string(f).unwrap();
        assert!(
            !json.contains(SECRET_TEXT),
            "serialized finding must not carry secret text"
        );
    }

    #[test]
    fn scanning_is_deterministic_and_pure() {
        let markers = vec![
            FactSurfaceMarker::new("f1", vec!["alpha".into()]),
            FactSurfaceMarker::new("f2", vec!["beta".into()]),
        ];
        let n = "alpha then beta";
        let a = scan_player_visible_leak(n, &markers, &[]);
        let b = scan_player_visible_leak(n, &markers, &[]);
        assert_eq!(a, b, "same inputs must yield identical findings in order");
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].fact_id.as_deref(), Some("f1"));
        assert_eq!(a[1].fact_id.as_deref(), Some("f2"));
    }

    #[test]
    fn marker_without_fact_id_is_failclosed_and_redacted() {
        let markers = vec![FactSurfaceMarker::new("", vec![SECRET_TEXT.into()])];
        let narration = format!("{SECRET_TEXT}!");
        let findings = scan_player_visible_leak(&narration, &markers, &[]);
        assert_eq!(findings.len(), 1, "unbound marker still fail-closed flags");
        assert!(findings[0].fact_id.is_none());
        assert!(
            !findings[0].detail.contains(SECRET_TEXT),
            "redaction holds even without a fact id"
        );
    }
}
