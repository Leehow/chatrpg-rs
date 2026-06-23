//! EV-P1 `progress_evidence_audit_required_v1` — the GM's **mandatory per-offer
//! EvidenceAudit** sidecar types (GPT Pro producer-firing follow-up
//! `design/GPTpro-producer-firing-followup.md` §B).
//!
//! EV-4R let the GM emit an OPTIONAL `[progress_claims]` tag — which the GM
//! silently omitted (producer-recall failure). EV-P1 replaces that with a
//! **mandatory observation audit**: for EVERY offered capability this turn the GM
//! must return exactly one [`WitnessDecision`] —
//! [`WitnessDecision::Observed`]`{basis}` or [`WitnessDecision::NotObserved`]`{reason}`.
//! Rust then enforces completeness: a missing / extra / malformed decision becomes a
//! logged `ProducerProtocolFailure` (in `trpg-gm`), never a silent "none". This makes
//! "the GM checked and found nothing" distinguishable from "the GM forgot".
//!
//! Design law enforced **at the type level** (same invariant as [`super::EvidenceClaim`]):
//! - **`LLM output ∩ ProgressSignal = ∅`** — an audit decision carries ONLY a status
//!   + (Observed) a non-empty basis of [`TurnLocalRef`]s or (NotObserved) a
//!   closed-vocabulary [`NotObservedReason`]. It *cannot* carry `objective_id` /
//!   `atom_id` / `completed` / `reward` / a free-text reason. `deny_unknown_fields`
//!   on the wire form makes any forged extra field **fail to parse** — the schema is
//!   closed, so the GM literally cannot construct a progress conclusion.
//! - **Observed ⇔ basis, NotObserved ⇔ reason** — enforced in the custom
//!   `Deserialize`: an Observed without a (non-empty) basis, or a NotObserved
//!   without a reason (or with a basis), is a hard parse error (fail-closed).
//! - **cap→atom resolution stays Rust-side** — the decision is keyed by the opaque
//!   [`CapId`] handle the GM was offered; the GM never names the atom.
//!
//! These types are **additive** and unreferenced by the live pipeline until the
//! flag-gated audit wiring (`progress_evidence_audit_required_v1`, in `trpg-gm`)
//! parses + admits them. The engine does NOT consume the resulting ledger (shadow).

use super::evidence_claim::{NonEmpty, TurnLocalRef};
use super::evidence_offer::CapId;
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Why an offered capability was **not** observed this turn. A **closed vocabulary**
/// (no free text) so the GM cannot smuggle a guard verdict / progress conclusion
/// through a "reason" string. Maps to the EV-P1 boundary few-shot cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotObservedReason {
    /// The capability's action simply did not happen this turn.
    NotAttempted,
    /// Only intent / plan / a partial attempt — the result was not established.
    Attempted,
    /// Attempted and FAILED this turn (e.g. a failed lock pick).
    Failed,
    /// Explicitly deferred to a later turn ("I'll go to the back alley next turn").
    NotYet,
    /// Alluded to / traced but not actually presented or committed (e.g. the player
    /// traced the ad chain but the storyboard was never shown).
    MerelyImplied,
}

impl NotObservedReason {
    /// Stable wire token.
    pub fn as_str(self) -> &'static str {
        match self {
            NotObservedReason::NotAttempted => "not_attempted",
            NotObservedReason::Attempted => "attempted",
            NotObservedReason::Failed => "failed",
            NotObservedReason::NotYet => "not_yet",
            NotObservedReason::MerelyImplied => "merely_implied",
        }
    }
}

/// One GM decision for one offered capability. `Observed` MUST carry a non-empty
/// basis (the turn-local outcome/commit that established it); `NotObserved` MUST
/// carry a closed-vocabulary reason. The two are mutually exclusive — neither can
/// carry the other's payload (enforced in `Deserialize`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WitnessDecision {
    Observed { basis: NonEmpty<TurnLocalRef> },
    NotObserved { reason: NotObservedReason },
}

/// The closed wire form of a decision. `deny_unknown_fields` rejects any forged
/// extra field (e.g. `completed`, `objective_id`, `atom_id`, `reward`). The typed
/// `WitnessDecision` is reconstructed (and validated) from this.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireDecision {
    status: AuditStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    basis: Option<NonEmpty<TurnLocalRef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<NotObservedReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AuditStatus {
    Observed,
    NotObserved,
}

impl Serialize for WitnessDecision {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let wire = match self {
            WitnessDecision::Observed { basis } => WireDecision {
                status: AuditStatus::Observed,
                basis: Some(basis.clone()),
                reason: None,
            },
            WitnessDecision::NotObserved { reason } => WireDecision {
                status: AuditStatus::NotObserved,
                basis: None,
                reason: Some(*reason),
            },
        };
        wire.serialize(s)
    }
}

impl<'de> Deserialize<'de> for WitnessDecision {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let wire = WireDecision::deserialize(d)?;
        match wire.status {
            // Observed ⇔ basis present (and NonEmpty enforces ≥1); a reason must NOT
            // be smuggled alongside it.
            AuditStatus::Observed => {
                if wire.reason.is_some() {
                    return Err(de::Error::custom(
                        "observed decision must not carry a reason",
                    ));
                }
                let basis = wire
                    .basis
                    .ok_or_else(|| de::Error::custom("observed decision must carry a basis"))?;
                Ok(WitnessDecision::Observed { basis })
            }
            // NotObserved ⇔ reason present; a basis must NOT be smuggled alongside it.
            AuditStatus::NotObserved => {
                if wire.basis.is_some() {
                    return Err(de::Error::custom(
                        "not_observed decision must not carry a basis",
                    ));
                }
                let reason = wire
                    .reason
                    .ok_or_else(|| de::Error::custom("not_observed decision must carry a reason"))?;
                Ok(WitnessDecision::NotObserved { reason })
            }
        }
    }
}

/// The GM's per-offer audit (design follow-up §B). `offer_set_id` ties the audit to
/// the exact OfferSet it answers (a stale/mismatched id is a protocol failure,
/// checked Rust-side). `decisions` maps each offered opaque [`CapId`] to its single
/// [`WitnessDecision`]. **Closed schema** (`deny_unknown_fields`): no field can name
/// an objective / atom / completion / reward.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceAudit {
    pub offer_set_id: String,
    pub decisions: BTreeMap<CapId, WitnessDecision>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observed(refs: Vec<TurnLocalRef>) -> WitnessDecision {
        WitnessDecision::Observed {
            basis: NonEmpty::from_vec(refs).unwrap(),
        }
    }

    #[test]
    fn observed_decision_roundtrips_with_basis_only() {
        let d = observed(vec![TurnLocalRef::Commit(0)]);
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v, serde_json::json!({"status": "observed", "basis": ["commit:0"]}));
        let back: WitnessDecision = serde_json::from_value(v).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn not_observed_decision_roundtrips_with_reason_only() {
        let d = WitnessDecision::NotObserved {
            reason: NotObservedReason::Failed,
        };
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v, serde_json::json!({"status": "not_observed", "reason": "failed"}));
        let back: WitnessDecision = serde_json::from_value(v).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn observed_without_basis_is_rejected() {
        // The whole point: an "observed" with no basis cannot be admitted — fail-closed.
        let err = serde_json::from_value::<WitnessDecision>(
            serde_json::json!({"status": "observed"}),
        );
        assert!(err.is_err(), "observed must carry a basis");
        // Empty basis array is also rejected (NonEmpty).
        let err2 = serde_json::from_value::<WitnessDecision>(
            serde_json::json!({"status": "observed", "basis": []}),
        );
        assert!(err2.is_err(), "observed with empty basis is rejected");
    }

    #[test]
    fn not_observed_without_reason_is_rejected() {
        let err = serde_json::from_value::<WitnessDecision>(
            serde_json::json!({"status": "not_observed"}),
        );
        assert!(err.is_err(), "not_observed must carry a reason");
    }

    #[test]
    fn decision_cannot_mix_basis_and_reason() {
        // An observed must not carry a reason; a not_observed must not carry a basis.
        assert!(serde_json::from_value::<WitnessDecision>(
            serde_json::json!({"status": "observed", "basis": ["commit:0"], "reason": "failed"})
        )
        .is_err());
        assert!(serde_json::from_value::<WitnessDecision>(
            serde_json::json!({"status": "not_observed", "reason": "failed", "basis": ["commit:0"]})
        )
        .is_err());
    }

    #[test]
    fn decision_schema_cannot_carry_a_progress_conclusion() {
        // deny_unknown_fields: any field that would name an objective / completion /
        // atom / reward is rejected at parse time — the GM cannot disguise a guard
        // verdict as an audit decision.
        for forged in [
            serde_json::json!({"status": "observed", "basis": ["commit:0"], "completed": true}),
            serde_json::json!({"status": "observed", "basis": ["commit:0"], "objective_id": "obj.win"}),
            serde_json::json!({"status": "observed", "basis": ["commit:0"], "atom_id": "atom:dead"}),
            serde_json::json!({"status": "not_observed", "reason": "failed", "reward": "xp"}),
            serde_json::json!({"status": "not_observed", "reason": "failed", "scene_transition": "scene_02"}),
        ] {
            assert!(
                serde_json::from_value::<WitnessDecision>(forged.clone()).is_err(),
                "forged field must be rejected: {forged}"
            );
        }
    }

    #[test]
    fn unknown_not_observed_reason_is_rejected() {
        // The reason vocabulary is closed — an invented reason fails (fail-closed).
        assert!(serde_json::from_value::<WitnessDecision>(
            serde_json::json!({"status": "not_observed", "reason": "vibes"})
        )
        .is_err());
    }

    #[test]
    fn evidence_audit_roundtrips_serde() {
        let mut decisions = BTreeMap::new();
        decisions.insert(CapId("cap_aaa".into()), observed(vec![TurnLocalRef::Commit(0)]));
        decisions.insert(
            CapId("cap_bbb".into()),
            WitnessDecision::NotObserved {
                reason: NotObservedReason::MerelyImplied,
            },
        );
        let audit = EvidenceAudit {
            offer_set_id: "osid_abc123def456".into(),
            decisions,
        };
        let v = serde_json::to_value(&audit).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "offer_set_id": "osid_abc123def456",
                "decisions": {
                    "cap_aaa": {"status": "observed", "basis": ["commit:0"]},
                    "cap_bbb": {"status": "not_observed", "reason": "merely_implied"}
                }
            })
        );
        let back: EvidenceAudit = serde_json::from_value(v).unwrap();
        assert_eq!(back, audit);
    }

    #[test]
    fn evidence_audit_schema_is_closed() {
        // A forged top-level field (e.g. completing an objective directly) fails to parse.
        assert!(serde_json::from_value::<EvidenceAudit>(serde_json::json!({
            "offer_set_id": "osid_x",
            "decisions": {},
            "objective_completed": "obj.win"
        }))
        .is_err());
        // A well-formed empty-decisions audit still parses (completeness is checked
        // against the OfferSet by the gm-side parser, not here).
        assert!(serde_json::from_value::<EvidenceAudit>(serde_json::json!({
            "offer_set_id": "osid_x",
            "decisions": {}
        }))
        .is_ok());
    }

    #[test]
    fn not_observed_reason_tokens_are_distinct() {
        let all = [
            NotObservedReason::NotAttempted,
            NotObservedReason::Attempted,
            NotObservedReason::Failed,
            NotObservedReason::NotYet,
            NotObservedReason::MerelyImplied,
        ];
        let mut toks: Vec<&str> = all.iter().map(|r| r.as_str()).collect();
        let n = toks.len();
        toks.sort_unstable();
        toks.dedup();
        assert_eq!(toks.len(), n, "every NotObservedReason token is distinct");
    }
}
