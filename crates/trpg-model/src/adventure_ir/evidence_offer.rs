//! EV-3 `progress_offers_v1` — the Progression Evidence Layer's **capability
//! offer** types (GPT Pro design `GPTpro-progression-evidence-layer.md` §6 "回合
//! 开始" / §5.4 / §Q2).
//!
//! At turn start the frontier additionally emits a machine-readable
//! [`EvidenceOfferSet`] = the closed set of evidence atoms the GM is **allowed to
//! claim this turn**, each behind a turn-scoped **opaque** [`CapId`] handle. Next
//! slice (EV-4) the GM returns `progress_claims` referencing only these handles.
//!
//! Design law honored here:
//! - **GM never sees AtomId or objective_id** (design §Q2 "GM 实际只输出 opaque
//!   capability handle … 输出 objective ID 本质=把 guard eval 伪装成 tag"). The
//!   [`CapId`] is a content hash of `session|turn|atom_id` so it is opaque AND
//!   turn-scoped (a different turn ⇒ a different handle), and is structurally
//!   distinct from both an [`AtomId`] (`atom:…` prefix) and any authored objective
//!   id (a dotted slug like `obj.neutralize_threat`).
//! - **single-turn capability** — `expires_at` is the turn the offer was minted in;
//!   a claim in a later turn can never resolve it (design §核心方案 "未过期").
//! - **source-grounded** — every offer is derived from an [`EvidenceAtomSpec`] that
//!   already carries ≥1 `SourceRef`; the derivation (in `trpg-runtime`) only ever
//!   offers atoms whose authored source is *surfaced this turn* (fail-closed).
//!
//! These types are **additive** and unreferenced by the live pipeline until the
//! flag-gated derivation (`progress_offers_v1`) wires them in
//! (`trpg-runtime::evidence_offers`). The engine does NOT consume offers/claims yet
//! (that is EV-4+).

use super::evidence::{AtomId, EvidenceKind};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The closed set of **domain-event basis kinds** a later [`EvidenceClaim`] (EV-4)
/// must cite to have an offer admitted. NOT a guard expression and NOT a progress
/// conclusion — only the *category of committed event* the gateway will resolve the
/// claim's basis against (design §5 admission "basis 解析为本回合真实提交事件").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BasisKind {
    /// A check/adjudication committed this turn with a success outcome.
    CheckResolved,
    /// A `PlayerLearnedFact`/`FactRevealed` DomainEvent committed this turn.
    FactCommitted,
    /// An action outcome / world-fact commit this turn.
    OutcomeCommitted,
    /// A player-driven `LocationEntered`/`SceneTransitioned` this turn.
    LocationEntered,
    /// A recorded player choice this turn.
    ChoiceCommitted,
}

impl BasisKind {
    /// Stable token (for prompt rendering + EV-4 claim resolution).
    pub fn as_str(self) -> &'static str {
        match self {
            BasisKind::CheckResolved => "check_resolved",
            BasisKind::FactCommitted => "fact_committed",
            BasisKind::OutcomeCommitted => "outcome_committed",
            BasisKind::LocationEntered => "location_entered",
            BasisKind::ChoiceCommitted => "choice_committed",
        }
    }
}

/// A **turn-scoped opaque capability handle** the GM may claim against
/// (`cap_<short hash of session|turn|atom_id>`). It is deliberately NOT the
/// [`AtomId`] and NOT an objective id — the GM never sees or emits either
/// (design §Q2). Same `(session, turn, atom)` ⇒ same handle (deterministic within a
/// turn); a different turn ⇒ a different handle (single-turn capability).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CapId(pub String);

impl CapId {
    /// Deterministic, opaque, turn-scoped. The `atom_id` is folded in so the handle
    /// is unforgeable (the GM cannot guess it from the meaning) but the GM never
    /// sees the atom id itself.
    pub fn from_parts(session_id: &str, turn_id: &str, atom_id: &AtomId) -> Self {
        let canon = format!("{session_id}|{turn_id}|{}", atom_id.as_str());
        let mut h = Sha256::new();
        h.update(canon.as_bytes());
        let hex = format!("{:x}", h.finalize());
        CapId(format!("cap_{}", &hex[..12]))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One offered capability: "if the player's action this turn actually caused
/// `meaning`, you (GM) may note it via `cap_id`, and a later claim must cite a
/// `required_basis` committed event". Carries the (GM-hidden) `atom_id` it maps to
/// so the EV-4 gateway can resolve the opaque handle back to its authored atom.
///
/// (No `Eq`: `EvidenceKind` is `Eq` but the struct stays `PartialEq` for symmetry
/// with the rest of the evidence types.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceOffer {
    /// Turn-scoped opaque handle (the ONLY field the GM is shown alongside `meaning`).
    pub cap_id: CapId,
    /// The authored atom this capability maps to (GM-hidden; EV-4 gateway input).
    pub atom_id: AtomId,
    /// What category of observable thing this is.
    pub kind: EvidenceKind,
    /// Human-readable description shown to the GM ("the player learned clue X").
    pub meaning: String,
    /// The committed-event basis a later claim MUST cite to be admitted.
    pub required_basis: Vec<BasisKind>,
    /// The turn id this offer expires at (single-turn capability).
    pub expires_at: String,
}

/// The turn-local set of capabilities offered this turn (design §6 / §5.4
/// `EvidenceOfferSet`). Conservative: holds only atoms whose authored source is
/// surfaced this turn — never the whole catalog. Stored turn-local for EV-4; the
/// engine does not consume it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EvidenceOfferSet {
    pub turn_id: String,
    offers: Vec<EvidenceOffer>,
}

impl EvidenceOfferSet {
    pub fn new(turn_id: impl Into<String>) -> Self {
        Self {
            turn_id: turn_id.into(),
            offers: Vec::new(),
        }
    }

    pub fn push(&mut self, offer: EvidenceOffer) {
        self.offers.push(offer);
    }

    pub fn offers(&self) -> &[EvidenceOffer] {
        &self.offers
    }

    /// Resolve an opaque handle back to its offer (EV-4 admission entry point).
    pub fn find(&self, cap_id: &str) -> Option<&EvidenceOffer> {
        self.offers.iter().find(|o| o.cap_id.as_str() == cap_id)
    }

    /// A deterministic id for THIS offer set (EV-P1): folds in the turn + the sorted
    /// opaque cap handles, so a [`super::EvidenceAudit`] can echo it and Rust can
    /// reject an audit answering a stale/different OfferSet. Pure derivation — adds NO
    /// serialized field (OFF byte-identical preserved). Empty set ⇒ stable `osid_…`
    /// over just the turn.
    pub fn id(&self) -> String {
        let mut caps: Vec<&str> = self.offers.iter().map(|o| o.cap_id.as_str()).collect();
        caps.sort_unstable();
        let canon = format!("{}|{}", self.turn_id, caps.join(","));
        let mut h = Sha256::new();
        h.update(canon.as_bytes());
        format!("osid_{}", &format!("{:x}", h.finalize())[..12])
    }

    pub fn len(&self) -> usize {
        self.offers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.offers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atom(clue_id: &str) -> AtomId {
        AtomId::from_parts(
            "digest_v1",
            "p8",
            EvidenceKind::FactLearned,
            &[clue_id.to_string()],
        )
    }

    #[test]
    fn cap_id_is_opaque_and_distinct_from_atom_and_objective_ids() {
        let a = atom("clue_aquifer_commercial");
        let cap = CapId::from_parts("sess_a", "turn_3", &a);
        // Opaque handle shape, NOT the atom id, NOT an objective id.
        assert!(cap.as_str().starts_with("cap_"), "opaque cap_ handle");
        assert_ne!(cap.as_str(), a.as_str(), "cap_id ≠ atom_id");
        assert!(
            !cap.as_str().starts_with("atom:"),
            "cap_id is not an AtomId"
        );
        // Authored objective ids are dotted slugs like `obj.neutralize_threat`.
        assert_ne!(
            cap.as_str(),
            "obj.neutralize_threat",
            "cap_id ≠ objective_id"
        );
        assert!(
            !cap.as_str().contains("clue_aquifer_commercial"),
            "handle hides the authored ref"
        );
    }

    #[test]
    fn cap_id_is_turn_scoped() {
        let a = atom("clue_a");
        let t3 = CapId::from_parts("sess_a", "turn_3", &a);
        let t4 = CapId::from_parts("sess_a", "turn_4", &a);
        assert_ne!(
            t3, t4,
            "different turn ⇒ different handle (single-turn capability)"
        );
        let other_sess = CapId::from_parts("sess_b", "turn_3", &a);
        assert_ne!(t3, other_sess, "different session ⇒ different handle");
    }

    #[test]
    fn cap_id_is_deterministic_within_a_turn() {
        let a = atom("clue_a");
        assert_eq!(
            CapId::from_parts("sess_a", "turn_3", &a),
            CapId::from_parts("sess_a", "turn_3", &a),
            "same (session, turn, atom) ⇒ same handle"
        );
    }

    #[test]
    fn basis_kind_tokens_are_stable_and_distinct() {
        let all = [
            BasisKind::CheckResolved,
            BasisKind::FactCommitted,
            BasisKind::OutcomeCommitted,
            BasisKind::LocationEntered,
            BasisKind::ChoiceCommitted,
        ];
        let tokens: Vec<&str> = all.iter().map(|b| b.as_str()).collect();
        let mut uniq = tokens.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(
            uniq.len(),
            tokens.len(),
            "every BasisKind token is distinct"
        );
        assert_eq!(BasisKind::FactCommitted.as_str(), "fact_committed");
    }

    #[test]
    fn offer_set_push_find_len() {
        let a = atom("clue_a");
        let cap = CapId::from_parts("sess_a", "turn_1", &a);
        let mut set = EvidenceOfferSet::new("turn_1");
        assert!(set.is_empty());
        set.push(EvidenceOffer {
            cap_id: cap.clone(),
            atom_id: a,
            kind: EvidenceKind::FactLearned,
            meaning: "the player learned clue A".into(),
            required_basis: vec![BasisKind::FactCommitted],
            expires_at: "turn_1".into(),
        });
        assert_eq!(set.len(), 1);
        assert!(
            set.find(cap.as_str()).is_some(),
            "handle resolves to its offer"
        );
        assert!(
            set.find("cap_unknown").is_none(),
            "unknown handle does not resolve"
        );
    }

    #[test]
    fn offer_set_id_is_deterministic_and_order_independent() {
        let a1 = atom("clue_a");
        let a2 = atom("clue_b");
        let mk = |order: bool| {
            let mut set = EvidenceOfferSet::new("turn_1");
            let mut offers = vec![
                EvidenceOffer {
                    cap_id: CapId("cap_zzz".into()),
                    atom_id: a1.clone(),
                    kind: EvidenceKind::FactLearned,
                    meaning: "x".into(),
                    required_basis: vec![BasisKind::FactCommitted],
                    expires_at: "turn_1".into(),
                },
                EvidenceOffer {
                    cap_id: CapId("cap_aaa".into()),
                    atom_id: a2.clone(),
                    kind: EvidenceKind::FactLearned,
                    meaning: "y".into(),
                    required_basis: vec![BasisKind::FactCommitted],
                    expires_at: "turn_1".into(),
                },
            ];
            if order {
                offers.reverse();
            }
            for o in offers {
                set.push(o);
            }
            set
        };
        assert!(mk(false).id().starts_with("osid_"), "opaque osid_ handle");
        assert_eq!(
            mk(false).id(),
            mk(true).id(),
            "id is independent of push order"
        );
        // Different turn ⇒ different id.
        let mut t2 = EvidenceOfferSet::new("turn_2");
        t2.push(EvidenceOffer {
            cap_id: CapId("cap_aaa".into()),
            atom_id: a2.clone(),
            kind: EvidenceKind::FactLearned,
            meaning: "y".into(),
            required_basis: vec![BasisKind::FactCommitted],
            expires_at: "turn_2".into(),
        });
        assert_ne!(mk(false).id(), t2.id(), "different turn ⇒ different id");
    }

    #[test]
    fn offer_set_roundtrips_serde() {
        let a = atom("clue_a");
        let mut set = EvidenceOfferSet::new("turn_1");
        set.push(EvidenceOffer {
            cap_id: CapId::from_parts("sess_a", "turn_1", &a),
            atom_id: a,
            kind: EvidenceKind::FactLearned,
            meaning: "the player learned clue A".into(),
            required_basis: vec![BasisKind::FactCommitted],
            expires_at: "turn_1".into(),
        });
        let v = serde_json::to_value(&set).unwrap();
        let back: EvidenceOfferSet = serde_json::from_value(v).unwrap();
        assert_eq!(back, set, "EvidenceOfferSet serde round-trips");
    }
}
