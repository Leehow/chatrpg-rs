//! NPC Behavior Consistency Verifier v1 (设计3 §13-P3): deterministic, pure checks
//! that flag NPC narration which **contradicts the speaking NPC's own
//! [`NpcBehaviorPlan`]** — i.e. the NPC exhibits an action its plan marks
//! `forbidden_actions`. This is the behavioral sibling of the knowledge-leak /
//! knowledge-consistency verifiers ([`crate::knowledge_leak_verifier`]): those guard
//! *what the NPC knows / may reveal*; this guards *how the NPC is allowed to act*.
//!
//! Design contract (mirrors the knowledge verifier's discipline):
//! - **Pure + deterministic.** Every function is a pure function of its inputs: no DB,
//!   no network, no provider/API key, no stream mutation. Same inputs → same findings,
//!   in a stable order.
//! - **Plan-driven, no omniscience.** Checks consult only the NPC's own derived
//!   [`NpcBehaviorPlan`] (`forbidden_actions` / `stance` / `npc_id`). GM world truth is
//!   never read here.
//! - **Advisory, fail-soft.** Findings are `Warning` severity: a behavior contradiction
//!   is a consistency note the GM may surface, not a hard ship-blocker. No markers / no
//!   forbidden actions → no findings.
//! - **Bounded scanning.** Text scanning only ever matches caller-provided
//!   [`BehaviorSurfaceMarker`] terms bound to a plan action label. No NLP, no inference.
//! - **Redaction-safe.** A [`BehaviorConsistencyFinding`] references the `npc_id` and the
//!   plan action label only; it never echoes the matched narration substring.
//!
//! `trpg-model` has no `trpg-agent` dependency, so the finding DTO is additive and
//! bridgeable to `trpg_agent::VerifierFinding` in the runtime layer (see
//! `trpg_runtime::knowledge_leak_verifier`).
use crate::npc_behavior::NpcBehaviorPlan;
use serde::{Deserialize, Serialize};

/// What kind of behavior contradiction a finding represents. v1 has one kind; the enum
/// leaves room for later additive kinds (e.g. stance/goal violations).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BehaviorContradictionKind {
    /// NPC narration exhibits an action the plan explicitly lists under
    /// `forbidden_actions`.
    ForbiddenActionExhibited,
}

impl BehaviorContradictionKind {
    /// Stable snake_case token (for traces / bridging).
    pub fn as_token(&self) -> &'static str {
        match self {
            BehaviorContradictionKind::ForbiddenActionExhibited => "forbidden_action_exhibited",
        }
    }
}

/// A caller-provided, deterministic marker binding a plan **action label** (one of the
/// plan's `forbidden_actions` entries) to the exact surface terms whose presence in the
/// NPC's narration evidences that the NPC exhibited that forbidden action. The terms are
/// the ONLY thing scanned for — no NLP, no inference. Findings report the `action_label`,
/// never the matched term.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BehaviorSurfaceMarker {
    /// The forbidden-action label this marker witnesses. Matched against the plan's
    /// `forbidden_actions` (exact string) before any finding is produced.
    pub action_label: String,
    /// Exact substrings whose presence in narration evidences the forbidden action.
    #[serde(default)]
    pub terms: Vec<String>,
}

impl BehaviorSurfaceMarker {
    pub fn new(action_label: impl Into<String>, terms: Vec<String>) -> Self {
        Self {
            action_label: action_label.into(),
            terms,
        }
    }
}

/// A single NPC behavior-consistency finding. Carries the `npc_id` + the plan action
/// label + a safe summary only; it never contains the raw narration substring that
/// triggered the match. Always `Warning` severity (advisory).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BehaviorConsistencyFinding {
    pub kind: BehaviorContradictionKind,
    /// The speaking NPC's stable actor id.
    pub npc_id: String,
    /// The plan `forbidden_actions` label the narration contradicted.
    pub action_label: String,
    /// Redacted, human-readable summary. NEVER contains the matched narration substring.
    pub detail: String,
}

/// Scan NPC `narration` against the speaking NPC's own [`NpcBehaviorPlan`].
///
/// For each [`BehaviorSurfaceMarker`] whose `action_label` IS one of the plan's
/// `forbidden_actions`, if any of its terms appears in the narration a
/// [`BehaviorContradictionKind::ForbiddenActionExhibited`] `Warning` finding is produced.
/// Markers whose label is not actually forbidden by the plan are ignored (fail-soft: we
/// only flag genuine plan contradictions). Deterministic: markers are scanned in order
/// and at most one finding is produced per forbidden action label.
///
/// Redaction: the finding reports the `action_label`, never the matched term.
pub fn scan_npc_behavior_consistency(
    plan: &NpcBehaviorPlan,
    narration: &str,
    markers: &[BehaviorSurfaceMarker],
) -> Vec<BehaviorConsistencyFinding> {
    if narration.is_empty() || markers.is_empty() || plan.forbidden_actions.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for marker in markers {
        let label = marker.action_label.trim();
        if label.is_empty() || seen.iter().any(|s| *s == label) {
            continue;
        }
        // Only flag a label the plan actually forbids (no inventing forbiddens).
        if !plan.forbidden_actions.iter().any(|f| f == label) {
            continue;
        }
        // Bounded scan: only caller-provided terms, ignoring empty ones.
        let hit = marker
            .terms
            .iter()
            .any(|t| !t.is_empty() && narration.contains(t.as_str()));
        if !hit {
            continue;
        }
        seen.push(label);
        out.push(BehaviorConsistencyFinding {
            kind: BehaviorContradictionKind::ForbiddenActionExhibited,
            npc_id: plan.npc_id.clone(),
            action_label: label.to_string(),
            detail: format!(
                "NPC '{}' narration exhibits forbidden behavior '{}'",
                plan.npc_id, label
            ),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        NpcBehaviorContext, NpcKnowledgeEntry, NpcMindView, NpcProfile, NpcRelationship,
        NpcRelationshipDelta, NpcRelationshipTarget,
    };

    fn profile() -> NpcProfile {
        NpcProfile {
            actor_id: "npc_lars".into(),
            name: "Lars".into(),
            goals: vec!["protect the workshop".into()],
            behavioral_boundaries: vec!["never harm a child".into()],
            ..Default::default()
        }
    }

    fn rel(trust: i16, fear: i16, hostility: i16) -> NpcRelationship {
        let mut r =
            NpcRelationship::new("s", "npc_lars", NpcRelationshipTarget::PlayerParty).unwrap();
        r.apply_delta(&NpcRelationshipDelta {
            trust,
            fear,
            hostility,
            evidence_event_ids: vec!["e".into()],
            ..Default::default()
        })
        .unwrap();
        r
    }

    fn plan(rel: NpcRelationship, entries: &[NpcKnowledgeEntry]) -> NpcBehaviorPlan {
        let view = NpcMindView::build("s", "npc_lars", &profile(), &[rel], entries).unwrap();
        NpcBehaviorPlan::derive(&view, &NpcBehaviorContext::default())
    }

    #[test]
    fn forbidden_action_exhibited_is_flagged() {
        // Persona boundary "never harm a child" becomes a forbidden action.
        let p = plan(rel(0, 0, 0), &[]);
        assert!(p
            .forbidden_actions
            .iter()
            .any(|a| a == "never harm a child"));
        let markers = vec![BehaviorSurfaceMarker::new(
            "never harm a child",
            vec!["strikes the child".into()],
        )];
        let findings = scan_npc_behavior_consistency(
            &p,
            "Lars strikes the child without hesitation.",
            &markers,
        );
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].kind,
            BehaviorContradictionKind::ForbiddenActionExhibited
        );
        assert_eq!(findings[0].npc_id, "npc_lars");
        assert_eq!(findings[0].action_label, "never harm a child");
    }

    #[test]
    fn consistent_behavior_passes() {
        let p = plan(rel(0, 0, 0), &[]);
        let markers = vec![BehaviorSurfaceMarker::new(
            "never harm a child",
            vec!["strikes the child".into()],
        )];
        // Narration does not contain the marker term → no contradiction.
        let findings = scan_npc_behavior_consistency(
            &p,
            "Lars kneels and offers the child a warm meal.",
            &markers,
        );
        assert!(findings.is_empty(), "non-matching narration must pass");
    }

    #[test]
    fn marker_for_non_forbidden_label_is_ignored() {
        let p = plan(rel(0, 0, 0), &[]);
        // The plan does not forbid "sing a song"; even a term hit must not flag.
        let markers = vec![BehaviorSurfaceMarker::new(
            "sing a song",
            vec!["sings".into()],
        )];
        let findings = scan_npc_behavior_consistency(&p, "Lars sings cheerfully.", &markers);
        assert!(
            findings.is_empty(),
            "only genuine plan forbidden_actions may be flagged"
        );
    }

    #[test]
    fn finding_does_not_echo_matched_substring() {
        let p = plan(rel(0, 0, 0), &[]);
        let secret_substr = "strikes the child";
        let markers = vec![BehaviorSurfaceMarker::new(
            "never harm a child",
            vec![secret_substr.into()],
        )];
        let findings = scan_npc_behavior_consistency(&p, "Lars strikes the child.", &markers);
        assert_eq!(findings.len(), 1);
        assert!(
            !findings[0].detail.contains(secret_substr),
            "detail must reference the action label, not the matched substring"
        );
        let json = serde_json::to_string(&findings[0]).unwrap();
        assert!(!json.contains(secret_substr));
    }

    #[test]
    fn scanning_is_deterministic_and_dedups_per_label() {
        let p = plan(rel(0, 0, 0), &[]);
        // Two markers for the same forbidden label → at most one finding.
        let markers = vec![
            BehaviorSurfaceMarker::new("never harm a child", vec!["hits the kid".into()]),
            BehaviorSurfaceMarker::new("never harm a child", vec!["hurts the kid".into()]),
        ];
        let narration = "Lars hits the kid and hurts the kid.";
        let a = scan_npc_behavior_consistency(&p, narration, &markers);
        let b = scan_npc_behavior_consistency(&p, narration, &markers);
        assert_eq!(a, b, "same inputs → identical findings");
        assert_eq!(a.len(), 1, "one finding per forbidden label");
    }

    #[test]
    fn empty_inputs_fail_soft() {
        let p = plan(rel(0, 0, 0), &[]);
        let markers = vec![BehaviorSurfaceMarker::new(
            "never harm a child",
            vec!["x".into()],
        )];
        assert!(scan_npc_behavior_consistency(&p, "", &markers).is_empty());
        assert!(scan_npc_behavior_consistency(&p, "text", &[]).is_empty());
        // Token round-trip for the finding kind.
        let v = serde_json::to_value(BehaviorContradictionKind::ForbiddenActionExhibited).unwrap();
        assert_eq!(v.as_str(), Some("forbidden_action_exhibited"));
        assert_eq!(
            v.as_str(),
            Some(BehaviorContradictionKind::ForbiddenActionExhibited.as_token())
        );
    }
}
