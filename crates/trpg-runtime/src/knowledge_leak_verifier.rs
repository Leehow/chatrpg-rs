//! Runtime bridge for the Knowledge Leak Verifier v1 (TC-P2-02).
//!
//! The model ([`trpg_model::knowledge_leak_verifier`]) owns the pure, deterministic
//! detection logic and a model-level [`KnowledgeLeakFinding`] DTO (`trpg-model` has no
//! `trpg-agent` dependency). This adapter is the thin, also-pure seam that maps those
//! findings onto the existing [`trpg_agent::VerifierFinding`] so later GM turn-loop
//! consumers can fold them into the same errata / verifier path used by the NoSpoiler
//! plugin — without this layer redefining detection or wiring into the live stream.
//!
//! Mapping is fixed and deterministic:
//! - all three leakage kinds (player-unknown leak, NPC reveals an unplanned fact, NPC
//!   reveals a withheld secret) → [`VerifierFindingKind::SecretLeak`], matching the
//!   NoSpoiler `SecretLeak` semantics;
//! - the consistency kind (NPC states a belief/unheld fact as known truth) →
//!   [`VerifierFindingKind::InventedEffect`], the closest existing "asserted something
//!   not grounded in truth" semantic.
//!
//! Redaction is preserved: the model finding's `detail` already carries ids + a safe
//! summary and never the raw secret term, so the bridge copies it verbatim.
use trpg_agent::{VerifierFinding, VerifierFindingKind, VerifierSeverity};
use trpg_model::knowledge_leak_verifier::{
    scan_npc_asserted_facts, scan_npc_disclosure, scan_player_visible_leak, FactSurfaceMarker,
    KnowledgeLeakFinding, KnowledgeLeakKind, LeakSeverity,
};

use crate::knowledge_projection::{NpcOwnedProjection, PlayerNarrationProjection};

/// Map a model-level [`KnowledgeLeakFinding`] to the existing
/// [`trpg_agent::VerifierFinding`]. Pure and deterministic; copies the already-redacted
/// `detail` without re-introducing secret text.
pub fn to_verifier_finding(finding: &KnowledgeLeakFinding) -> VerifierFinding {
    let kind = match finding.kind {
        KnowledgeLeakKind::PlayerUnknownFactLeak
        | KnowledgeLeakKind::NpcRevealsUnplannedFact
        | KnowledgeLeakKind::NpcRevealsWithheldSecret => VerifierFindingKind::SecretLeak,
        KnowledgeLeakKind::NpcStatesUnknownAsFact => VerifierFindingKind::InventedEffect,
    };
    let severity = match finding.severity {
        LeakSeverity::Blocker => VerifierSeverity::Blocker,
        LeakSeverity::Warning => VerifierSeverity::Warning,
    };
    VerifierFinding {
        kind,
        severity,
        detail: finding.detail.clone(),
    }
}

/// Convenience: map a batch of findings, preserving order.
pub fn to_verifier_findings(findings: &[KnowledgeLeakFinding]) -> Vec<VerifierFinding> {
    findings.iter().map(to_verifier_finding).collect()
}

// =================== TC-D3-05 projection-driven verifier helpers ===================
//
// These thin, pure helpers consume the runtime viewer/speaker projections and call the
// existing pure model checks, then bridge findings to `VerifierFinding`. They add no
// detection logic of their own and never echo secret text (the model findings carry ids
// + a safe summary only). Production `AfterLlmStream` calls them with deterministic
// markers/plans; with no markers/plan they fail-soft to no findings.

/// Player-visible leak check via the **player narration projection**: scan `narration`
/// for caller-provided [`FactSurfaceMarker`] terms bound to facts the player does NOT yet
/// know. Bridged to `VerifierFinding::SecretLeak`. The marker terms are the only thing
/// scanned and are never copied into a finding (findings reference the `fact_id`).
pub fn verify_player_narration_leak(
    narration: &str,
    markers: &[FactSurfaceMarker],
    projection: &PlayerNarrationProjection,
) -> Vec<VerifierFinding> {
    let player_known = projection.known_fact_ids_sorted();
    to_verifier_findings(&scan_player_visible_leak(narration, markers, &player_known))
}

/// NPC disclosure consistency check via an NPC speech/action projection: each id in
/// `disclosed_fact_ids` (deterministically extracted by the caller) that is not in the
/// NPC's `facts_can_reveal` is flagged. Bridged to `VerifierFinding::SecretLeak`. Empty
/// `disclosed_fact_ids` → no findings (fail-soft).
pub fn verify_npc_disclosure(
    projection: &NpcOwnedProjection,
    disclosed_fact_ids: &[String],
) -> Vec<VerifierFinding> {
    to_verifier_findings(&scan_npc_disclosure(&projection.plan, disclosed_fact_ids))
}

/// NPC asserted-as-known-truth consistency check via an NPC speech/action projection:
/// each id in `asserted_as_known_fact_ids` the NPC does not hold as `knows_true` (a belief
/// or unheld fact) is flagged. Bridged to `VerifierFinding::InventedEffect` (Warning).
/// Empty input → no findings (fail-soft).
pub fn verify_npc_asserted_facts(
    projection: &NpcOwnedProjection,
    asserted_as_known_fact_ids: &[String],
) -> Vec<VerifierFinding> {
    to_verifier_findings(&scan_npc_asserted_facts(
        &projection.mind,
        asserted_as_known_fact_ids,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::knowledge_leak_verifier::{
        scan_npc_asserted_facts, scan_player_visible_leak, FactSurfaceMarker,
    };
    use trpg_model::{KnowledgeState, NpcKnowledgeEntry, NpcMindView, NpcProfile};

    const SECRET_TEXT: &str = "the butler is the killer";

    fn mind_view(entries: &[NpcKnowledgeEntry]) -> NpcMindView {
        let profile = NpcProfile {
            actor_id: "npc_lars".into(),
            name: "Lars".into(),
            ..Default::default()
        };
        NpcMindView::build("s", "npc_lars", &profile, &[], entries).unwrap()
    }

    #[test]
    fn leak_finding_bridges_to_secret_leak_without_echoing_text() {
        let markers = vec![FactSurfaceMarker::new(
            "fact_villain",
            vec![SECRET_TEXT.into()],
        )];
        let narration = format!("Narration: {SECRET_TEXT}.");
        let model_findings = scan_player_visible_leak(&narration, &markers, &[]);
        assert_eq!(model_findings.len(), 1);

        let bridged = to_verifier_findings(&model_findings);
        assert_eq!(bridged.len(), 1);
        assert_eq!(bridged[0].kind, VerifierFindingKind::SecretLeak);
        assert_eq!(bridged[0].severity, VerifierSeverity::Blocker);
        assert!(
            !bridged[0].detail.contains(SECRET_TEXT),
            "bridged finding must not echo secret text"
        );
        assert!(
            bridged[0].detail.contains("fact_villain"),
            "bridged finding should reference the fact id"
        );
    }

    #[test]
    fn belief_assertion_bridges_to_invented_effect_warning() {
        let entries = vec![NpcKnowledgeEntry {
            fact_id: "b_false".into(),
            state: KnowledgeState::BelievesFalse,
        }];
        let v = mind_view(&entries);
        let model_findings = scan_npc_asserted_facts(&v, &["b_false".into()]);
        assert_eq!(model_findings.len(), 1);

        let bridged = to_verifier_finding(&model_findings[0]);
        assert_eq!(bridged.kind, VerifierFindingKind::InventedEffect);
        assert_eq!(bridged.severity, VerifierSeverity::Warning);
    }

    use crate::knowledge_projection::{npc_speech_projection_from_mind, PlayerNarrationProjection};

    /// projection player-leak helper: marker term for a player-unknown fact in narration →
    /// bridged SecretLeak; helper never echoes the secret term.
    #[test]
    fn projection_player_leak_helper_flags_unknown_and_redacts() {
        let markers = vec![FactSurfaceMarker::new(
            "fact_villain",
            vec![SECRET_TEXT.into()],
        )];
        let narration = format!("Narration: {SECRET_TEXT}.");
        // Player knows nothing → leak.
        let proj = PlayerNarrationProjection::from_player_known(Vec::<String>::new());
        let out = verify_player_narration_leak(&narration, &markers, &proj);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, VerifierFindingKind::SecretLeak);
        assert!(!out[0].detail.contains(SECRET_TEXT));
        assert!(out[0].detail.contains("fact_villain"));
        // Same fact already player-known → allowed (no finding).
        let known = PlayerNarrationProjection::from_player_known(["fact_villain"]);
        assert!(verify_player_narration_leak(&narration, &markers, &known).is_empty());
    }

    /// projection NPC helpers: disclosure outside the plan → SecretLeak; belief asserted as
    /// truth → InventedEffect; empty inputs fail-soft to no findings.
    #[test]
    fn projection_npc_helpers_flag_disclosure_and_belief() {
        let entries = vec![
            NpcKnowledgeEntry {
                fact_id: "k_known".into(),
                state: KnowledgeState::KnowsTrue,
            },
            NpcKnowledgeEntry {
                fact_id: "b_false".into(),
                state: KnowledgeState::BelievesFalse,
            },
        ];
        // Player knows nothing → the NPC's known fact is a withheld secret.
        let proj = npc_speech_projection_from_mind(mind_view(&entries), &[]);
        // Disclosing a fact outside facts_can_reveal → flagged.
        let disc = verify_npc_disclosure(&proj.0, &["k_known".into()]);
        assert_eq!(disc.len(), 1);
        assert_eq!(disc[0].kind, VerifierFindingKind::SecretLeak);
        // Asserting a false belief as known truth → InventedEffect warning.
        let belief = verify_npc_asserted_facts(&proj.0, &["b_false".into()]);
        assert_eq!(belief.len(), 1);
        assert_eq!(belief[0].kind, VerifierFindingKind::InventedEffect);
        // Empty inputs fail-soft.
        assert!(verify_npc_disclosure(&proj.0, &[]).is_empty());
        assert!(verify_npc_asserted_facts(&proj.0, &[]).is_empty());
    }

    #[test]
    fn bridge_is_deterministic_passthrough() {
        let f = KnowledgeLeakFinding {
            kind: KnowledgeLeakKind::NpcRevealsWithheldSecret,
            severity: LeakSeverity::Blocker,
            fact_id: Some("s1".into()),
            holder_id: Some("npc_lars".into()),
            detail: "NPC 'npc_lars' disclosed withheld protected fact 's1'".into(),
        };
        assert_eq!(to_verifier_finding(&f), to_verifier_finding(&f));
        assert_eq!(
            to_verifier_finding(&f).kind,
            VerifierFindingKind::SecretLeak
        );
    }
}
