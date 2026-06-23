//! EV-4 `progress_claims_shadow_v1` — the GM's **turn-scoped progress claim**
//! sidecar types (GPT Pro design `GPTpro-progression-evidence-layer.md` §3 GM
//! TurnDocument + §核心方案 两阶段协议 step 2).
//!
//! Design law enforced **at the type level** here:
//! - **`LLM output ∩ ProgressSignal = ∅`** — an [`EvidenceClaim`] carries ONLY an
//!   opaque capability handle ([`CapId`]) + a non-empty basis of [`TurnLocalRef`]s.
//!   It *cannot* carry `objective_id` / `atom_id` / `completed` / `reward` /
//!   `scene_transition` / a free-text reason, so the LLM literally cannot construct
//!   a progress conclusion. `#[serde(deny_unknown_fields)]` makes a forged
//!   `{cap_id, completed:true}` (or any extra field) **fail to parse** — the wire
//!   schema is closed.
//! - **basis is turn-local + positional** — a [`TurnLocalRef`] is an *index* into
//!   THIS turn's adjudicated outcomes / committed events (`outcome:0` / `commit:1`),
//!   never a fact id or event id the LLM could fabricate. The Rust gateway resolves
//!   it against the turn's real committed [`crate::DomainEvent`]s.
//! - **never empty** — `basis` is a [`NonEmpty`] so a claim with zero basis is
//!   unrepresentable (a claim must cite ≥1 committed thing).
//!
//! These types are **additive** and unreferenced by the live pipeline until the
//! flag-gated gateway (`progress_claims_shadow_v1`, in `trpg-runtime::
//! evidence_gateway`) parses + admits them. The engine does NOT consume the
//! resulting ledger yet (shadow; that is EV-6).

use super::evidence_offer::CapId;
use serde::de::{self, Deserializer};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

/// A reference into THIS turn's adjudicated **outcomes** / committed **commits** —
/// the ONLY thing a claim's basis may cite. Positional (`outcome:<i>` / `commit:<i>`),
/// never a free-text fact/event id. Both variants resolve to the turn's committed
/// [`crate::DomainEvent`]s by index in the gateway; the variant is the GM's hint of
/// *which kind* of turn-local slot it is citing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnLocalRef {
    Outcome(usize),
    Commit(usize),
}

impl TurnLocalRef {
    /// The positional index this ref points at (within the turn's outcomes/commits).
    pub fn index(self) -> usize {
        match self {
            TurnLocalRef::Outcome(i) | TurnLocalRef::Commit(i) => i,
        }
    }

    /// Stable wire token (`outcome:0` / `commit:1`).
    pub fn as_token(self) -> String {
        match self {
            TurnLocalRef::Outcome(i) => format!("outcome:{i}"),
            TurnLocalRef::Commit(i) => format!("commit:{i}"),
        }
    }

    /// Parse a wire token. Fail-closed: an unknown prefix / non-numeric index / no
    /// colon ⇒ `None` (the parser drops the malformed ref, never guesses).
    pub fn parse(s: &str) -> Option<Self> {
        let (kind, idx) = s.split_once(':')?;
        let i: usize = idx.trim().parse().ok()?;
        match kind.trim() {
            "outcome" => Some(TurnLocalRef::Outcome(i)),
            "commit" => Some(TurnLocalRef::Commit(i)),
            _ => None,
        }
    }
}

impl Serialize for TurnLocalRef {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.as_token())
    }
}

impl<'de> Deserialize<'de> for TurnLocalRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        TurnLocalRef::parse(&raw)
            .ok_or_else(|| de::Error::custom(format!("invalid TurnLocalRef token: {raw:?}")))
    }
}

/// A list guaranteed **non-empty at the type level** (`head` + optional `tail`).
/// Used for a claim's `basis` so "a claim with no basis" is structurally
/// unrepresentable. Serializes/deserializes as a plain JSON array; deserializing an
/// empty array is a hard error (fail-closed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonEmpty<T> {
    head: T,
    tail: Vec<T>,
}

impl<T> NonEmpty<T> {
    pub fn new(head: T, tail: Vec<T>) -> Self {
        NonEmpty { head, tail }
    }

    /// Build from a `Vec`; `None` if the vec is empty (the only way to get a
    /// `NonEmpty` is to prove ≥1 element).
    pub fn from_vec(mut v: Vec<T>) -> Option<Self> {
        if v.is_empty() {
            return None;
        }
        let head = v.remove(0);
        Some(NonEmpty { head, tail: v })
    }

    pub fn first(&self) -> &T {
        &self.head
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        std::iter::once(&self.head).chain(self.tail.iter())
    }

    pub fn len(&self) -> usize {
        1 + self.tail.len()
    }

    /// Always ≥1 — provided for API symmetry / clippy (`is_empty` is never true).
    pub fn is_empty(&self) -> bool {
        false
    }
}

impl<T: Serialize> Serialize for NonEmpty<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = s.serialize_seq(Some(self.len()))?;
        seq.serialize_element(&self.head)?;
        for t in &self.tail {
            seq.serialize_element(t)?;
        }
        seq.end()
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for NonEmpty<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Vec::<T>::deserialize(d)?;
        NonEmpty::from_vec(v).ok_or_else(|| de::Error::custom("basis must be non-empty"))
    }
}

/// The GM's per-capability progress claim (design §3 `progress_claims:
/// Vec<EvidenceClaim>`). **Closed schema** (`deny_unknown_fields`): the GM may emit
/// only the opaque `cap_id` it was offered + a non-empty `basis` of turn-local refs.
/// There is no field through which the LLM could name an objective, an atom, a
/// completion, a reward, or a free-text fact — admission + the cap→atom resolution
/// are Rust's alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceClaim {
    pub cap_id: CapId,
    pub basis: NonEmpty<TurnLocalRef>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_local_ref_roundtrips_token() {
        assert_eq!(TurnLocalRef::Outcome(0).as_token(), "outcome:0");
        assert_eq!(TurnLocalRef::Commit(1).as_token(), "commit:1");
        assert_eq!(TurnLocalRef::parse("outcome:0"), Some(TurnLocalRef::Outcome(0)));
        assert_eq!(TurnLocalRef::parse("commit:1"), Some(TurnLocalRef::Commit(1)));
        assert_eq!(TurnLocalRef::Outcome(3).index(), 3);
        assert_eq!(TurnLocalRef::Commit(2).index(), 2);
    }

    #[test]
    fn turn_local_ref_parse_is_fail_closed() {
        assert_eq!(TurnLocalRef::parse("bogus:0"), None, "unknown kind ⇒ None");
        assert_eq!(TurnLocalRef::parse("commit:x"), None, "non-numeric index ⇒ None");
        assert_eq!(TurnLocalRef::parse("commit"), None, "no colon ⇒ None");
        assert_eq!(TurnLocalRef::parse(""), None, "empty ⇒ None");
        // Crucially: a fabricated fact name is NOT a legal basis ref.
        assert_eq!(TurnLocalRef::parse("fact:clue_aquifer"), None);
    }

    #[test]
    fn turn_local_ref_serializes_as_string() {
        let v = serde_json::to_value(TurnLocalRef::Commit(1)).unwrap();
        assert_eq!(v, serde_json::json!("commit:1"));
        let back: TurnLocalRef = serde_json::from_value(serde_json::json!("outcome:2")).unwrap();
        assert_eq!(back, TurnLocalRef::Outcome(2));
    }

    #[test]
    fn nonempty_requires_at_least_one() {
        assert!(NonEmpty::<u8>::from_vec(vec![]).is_none(), "empty vec ⇒ no NonEmpty");
        let ne = NonEmpty::from_vec(vec![1u8, 2, 3]).unwrap();
        assert_eq!(ne.len(), 3);
        assert_eq!(ne.first(), &1);
        assert_eq!(ne.iter().copied().collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    #[test]
    fn nonempty_deserialize_rejects_empty_array() {
        let ok: NonEmpty<TurnLocalRef> =
            serde_json::from_value(serde_json::json!(["commit:0"])).unwrap();
        assert_eq!(ok.len(), 1);
        let err = serde_json::from_value::<NonEmpty<TurnLocalRef>>(serde_json::json!([]));
        assert!(err.is_err(), "empty basis array is a hard parse error (fail-closed)");
    }

    #[test]
    fn evidence_claim_roundtrips_serde() {
        let claim = EvidenceClaim {
            cap_id: CapId("cap_abc123".into()),
            basis: NonEmpty::from_vec(vec![TurnLocalRef::Commit(0), TurnLocalRef::Outcome(1)])
                .unwrap(),
        };
        let v = serde_json::to_value(&claim).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"cap_id": "cap_abc123", "basis": ["commit:0", "outcome:1"]})
        );
        let back: EvidenceClaim = serde_json::from_value(v).unwrap();
        assert_eq!(back, claim);
    }

    #[test]
    fn evidence_claim_schema_cannot_carry_a_progress_conclusion() {
        // The whole point of EV-4: the GM cannot smuggle a guard verdict through the
        // claim. Any field that would let it name an objective / completion / atom /
        // reward / a free-text fact is rejected at parse time (deny_unknown_fields).
        for forged in [
            serde_json::json!({"cap_id": "cap_x", "basis": ["commit:0"], "objective_id": "obj.win"}),
            serde_json::json!({"cap_id": "cap_x", "basis": ["commit:0"], "completed": true}),
            serde_json::json!({"cap_id": "cap_x", "basis": ["commit:0"], "atom_id": "atom:deadbeef"}),
            serde_json::json!({"cap_id": "cap_x", "basis": ["commit:0"], "reward": "xp"}),
            serde_json::json!({"cap_id": "cap_x", "basis": ["commit:0"], "scene_transition": "scene_02"}),
            serde_json::json!({"cap_id": "cap_x", "basis": ["commit:0"], "reason": "because I said so"}),
        ] {
            assert!(
                serde_json::from_value::<EvidenceClaim>(forged.clone()).is_err(),
                "forged field must be rejected: {forged}"
            );
        }
        // A bare, well-formed claim still parses.
        assert!(serde_json::from_value::<EvidenceClaim>(
            serde_json::json!({"cap_id": "cap_x", "basis": ["commit:0"]})
        )
        .is_ok());
    }

    #[test]
    fn evidence_claim_requires_a_basis() {
        // No basis field at all ⇒ parse error; empty basis ⇒ parse error.
        assert!(serde_json::from_value::<EvidenceClaim>(
            serde_json::json!({"cap_id": "cap_x"})
        )
        .is_err());
        assert!(serde_json::from_value::<EvidenceClaim>(
            serde_json::json!({"cap_id": "cap_x", "basis": []})
        )
        .is_err());
    }
}
