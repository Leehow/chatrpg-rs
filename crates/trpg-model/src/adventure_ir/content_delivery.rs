//! EV-P2 `progress_exact_projectors_v1` — the GM's **authored-content
//! materialization** sidecar type (GPT Pro follow-up
//! `design/GPTpro-producer-firing-followup.md` §A "FactLearned 用显式 authored-content
//! materialization" + §谁生产 `AuthoredContentDeliveryProducer`).
//!
//! GPT Pro's reframe: a free `progress_claim` after the fact is unreliable; the
//! sturdier producer is an **explicit content delivery** — when the GM actually
//! *presents an authored clue's content to the player*, it emits a tightly
//! constrained reference, and **Rust** (not the GM) decides whether that constitutes
//! [`super::AcceptedEvidence`]`(FactLearned, ExactDomain)`.
//!
//! Design law enforced **at the type level** here (identical philosophy to
//! [`super::EvidenceClaim`]):
//! - **`LLM output ∩ ProgressSignal = ∅`** — a [`ContentDelivery`] carries ONLY an
//!   opaque [`CapId`] (the offered authored-clue handle), a closed [`DeliveryRecipient`],
//!   and a non-empty basis of [`super::TurnLocalRef`]s. It *cannot* carry
//!   `objective_id` / `atom_id` / `fact_id` / `completed` / `reward`, so the GM
//!   literally cannot construct a progress conclusion or name the authored fact —
//!   Rust resolves `cap → authored fact` from the OfferSet/catalog.
//!   `#[serde(deny_unknown_fields)]` makes any forged extra field fail to parse.
//! - **recipient is closed** — [`DeliveryRecipient`] has the single `Player` variant,
//!   so a delivery to anyone but the player is *unrepresentable* (the projector
//!   requires recipient=player; the type makes the check structural / fail-closed).
//! - **basis is non-empty + turn-local** — a delivery must cite ≥1 committed
//!   turn-local thing (`commit:<i>` / `outcome:<i>`); the Rust projector resolves it
//!   against THIS turn's real committed [`crate::DomainEvent`]s.
//!
//! These types are **additive** and unreferenced by the live pipeline until the
//! flag-gated projector (`progress_exact_projectors_v1`, in `trpg-runtime::
//! exact_projectors`) parses + verifies them. The engine does NOT consume the
//! resulting ledger yet (shadow; that is EV-APPLY).

use super::evidence_claim::{NonEmpty, TurnLocalRef};
use super::evidence_offer::CapId;
use serde::{Deserialize, Serialize};

/// Who an authored clue's content was delivered to. **Closed** to the single
/// `Player` variant: the content-delivery projector only ever produces evidence for
/// player-facing materialization, and a delivery naming any other recipient is
/// unrepresentable (so a forged `"recipient":"npc"` fails to parse — fail-closed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryRecipient {
    Player,
}

/// The GM's per-delivery authored-content materialization reference (design follow-up
/// §A). **Closed schema** (`deny_unknown_fields`): the GM may emit only the opaque
/// `delivery_cap` it was offered + a closed `recipient` + a non-empty `basis` of
/// turn-local refs. There is no field through which the GM could name the authored
/// fact, an atom, an objective, a completion, or a reward — the Rust projector
/// resolves the cap→authored-fact mapping and decides admission alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentDelivery {
    /// The offered authored-clue capability handle (turn-scoped, opaque). Rust maps
    /// it back to the authored fact via the turn's OfferSet — the GM never names the
    /// fact/atom.
    pub delivery_cap: CapId,
    /// Who the content was presented to. Closed to `Player` (fail-closed by type).
    pub recipient: DeliveryRecipient,
    /// The committed turn-local basis (≥1) that presented the content this turn.
    pub basis: NonEmpty<TurnLocalRef>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_delivery_roundtrips_serde() {
        let cd = ContentDelivery {
            delivery_cap: CapId("cap_abc123def456".into()),
            recipient: DeliveryRecipient::Player,
            basis: NonEmpty::from_vec(vec![TurnLocalRef::Commit(0)]).unwrap(),
        };
        let v = serde_json::to_value(&cd).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "delivery_cap": "cap_abc123def456",
                "recipient": "player",
                "basis": ["commit:0"]
            })
        );
        let back: ContentDelivery = serde_json::from_value(v).unwrap();
        assert_eq!(back, cd);
    }

    #[test]
    fn recipient_is_closed_to_player_only() {
        // The only legal recipient is the player; any other value fails to parse
        // (a delivery to an NPC/GM is structurally unrepresentable — fail-closed).
        assert!(
            serde_json::from_value::<ContentDelivery>(serde_json::json!({
                "delivery_cap": "cap_x", "recipient": "player", "basis": ["commit:0"]
            }))
            .is_ok()
        );
        for bad in ["npc", "gm", "world", ""] {
            assert!(
                serde_json::from_value::<ContentDelivery>(serde_json::json!({
                    "delivery_cap": "cap_x", "recipient": bad, "basis": ["commit:0"]
                }))
                .is_err(),
                "recipient {bad:?} must be rejected (closed to player)"
            );
        }
    }

    #[test]
    fn content_delivery_schema_cannot_carry_a_progress_conclusion() {
        // The whole point: the GM cannot smuggle a guard verdict / fact name through
        // the delivery. Any field that would let it name a fact / objective /
        // completion / atom / reward is rejected at parse time (deny_unknown_fields).
        for forged in [
            serde_json::json!({"delivery_cap": "cap_x", "recipient": "player", "basis": ["commit:0"], "fact_id": "clue_aquifer"}),
            serde_json::json!({"delivery_cap": "cap_x", "recipient": "player", "basis": ["commit:0"], "objective_id": "obj.win"}),
            serde_json::json!({"delivery_cap": "cap_x", "recipient": "player", "basis": ["commit:0"], "completed": true}),
            serde_json::json!({"delivery_cap": "cap_x", "recipient": "player", "basis": ["commit:0"], "atom_id": "atom:deadbeef"}),
            serde_json::json!({"delivery_cap": "cap_x", "recipient": "player", "basis": ["commit:0"], "reward": "xp"}),
        ] {
            assert!(
                serde_json::from_value::<ContentDelivery>(forged.clone()).is_err(),
                "forged field must be rejected: {forged}"
            );
        }
    }

    #[test]
    fn content_delivery_requires_a_nonempty_basis() {
        // No basis ⇒ parse error; empty basis ⇒ parse error (a delivery must cite ≥1
        // committed turn-local thing).
        assert!(
            serde_json::from_value::<ContentDelivery>(serde_json::json!({
                "delivery_cap": "cap_x", "recipient": "player"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<ContentDelivery>(serde_json::json!({
                "delivery_cap": "cap_x", "recipient": "player", "basis": []
            }))
            .is_err()
        );
    }
}
