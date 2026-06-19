//! NPC Relationship v1 (TC-NPC-02): a structured, bounded, evidence-gated model of
//! an NPC's dynamic attitude toward the player party, a PC, another NPC, or a faction.
//!
//! Design contract:
//! - **Bounded.** Every numeric channel is clamped to its declared range on every
//!   applied delta (signed bipolar channels to [-100, 100]; unipolar channels to
//!   [0, 100]). Values can never drift out of band, however large a delta is.
//! - **Evidence-gated.** A relationship is never mutated without explicit evidence:
//!   [`NpcRelationshipDelta::apply_to`] returns [`RelationshipError::MissingEvidence`]
//!   when `evidence_event_ids` is empty. No evidence, no mutation. An LLM cannot set
//!   final values directly — it can only propose a delta, which still passes through
//!   this bounded, evidence-required path.
//! - **Stable identity, fail-closed.** Identity is the stable NPC `actor_id` and a
//!   typed target. Construction validates ids through the actor-identity contract
//!   ([`crate::KnowledgeHolder`]); display-name / ad-hoc / placeholder ids fail closed.
//! - **Deterministic derivation.** `stance` and `interaction_desire` are recomputed
//!   from the affect channels after every apply, so they stay consistent with state.
//!
//! Storage is serde/JSON round-trippable; the DB/runtime layer maps the flat fields
//! to columns. This module is pure: no IO, no combat/behavior logic.
use crate::KnowledgeHolder;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Inclusive bound for signed bipolar channels (trust/respect/affection/debt and the
/// derived interaction_desire): negative = the opposing pole (distrust / contempt /
/// dislike / owed-to-NPC / avoidance), positive = the affirming pole.
pub const CHANNEL_SIGNED_MIN: i16 = -100;
pub const CHANNEL_SIGNED_MAX: i16 = 100;
/// Inclusive bound for unipolar channels (fear/suspicion/hostility/leverage/
/// talkativeness): 0 = absent, 100 = maximal. These have no meaningful negative pole.
pub const CHANNEL_UNIPOLAR_MIN: i16 = 0;
pub const CHANNEL_UNIPOLAR_MAX: i16 = 100;

#[inline]
fn clamp_signed(v: i64) -> i16 {
    v.clamp(CHANNEL_SIGNED_MIN as i64, CHANNEL_SIGNED_MAX as i64) as i16
}

#[inline]
fn clamp_unipolar(v: i64) -> i16 {
    v.clamp(CHANNEL_UNIPOLAR_MIN as i64, CHANNEL_UNIPOLAR_MAX as i64) as i16
}

/// What an NPC's relationship points at. Couples the kind token with its id so the
/// pairing is always valid; `PlayerParty` is the collective player holder (no id).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "target_kind", content = "target_id", rename_all = "snake_case")]
pub enum NpcRelationshipTarget {
    PlayerParty,
    Pc(String),
    Npc(String),
    Faction(String),
}

impl NpcRelationshipTarget {
    /// DB-aligned kind token.
    pub fn kind_token(&self) -> &'static str {
        match self {
            NpcRelationshipTarget::PlayerParty => "player_party",
            NpcRelationshipTarget::Pc(_) => "pc",
            NpcRelationshipTarget::Npc(_) => "npc",
            NpcRelationshipTarget::Faction(_) => "faction",
        }
    }

    /// Stable target id; the collective player party has none (empty string in storage).
    pub fn target_id(&self) -> &str {
        match self {
            NpcRelationshipTarget::PlayerParty => "",
            NpcRelationshipTarget::Pc(id)
            | NpcRelationshipTarget::Npc(id)
            | NpcRelationshipTarget::Faction(id) => id.as_str(),
        }
    }

    /// Fail-closed validation + normalization of this target's id through the
    /// TC-KNOW-00 actor-identity contract, using the **kind-specific** constructor
    /// (PC → `player_character_from_actor_id`, NPC → `npc_from_actor_id`,
    /// faction → `faction_from_id`). `PlayerParty` is the collective holder and has no
    /// id to validate. Returns a fresh target carrying the normalized (trimmed) id.
    ///
    /// This is the single gate every public construction/write path routes through, so
    /// a public enum variant or a serde-deserialized value can never smuggle a
    /// display-name / placeholder / ad-hoc target id past the contract.
    pub fn validated(&self) -> Result<NpcRelationshipTarget, RelationshipError> {
        match self {
            NpcRelationshipTarget::PlayerParty => Ok(NpcRelationshipTarget::PlayerParty),
            NpcRelationshipTarget::Pc(id) => Ok(NpcRelationshipTarget::Pc(validate_target_id(
                KnowledgeHolder::player_character_from_actor_id,
                id,
            )?)),
            NpcRelationshipTarget::Npc(id) => Ok(NpcRelationshipTarget::Npc(validate_target_id(
                KnowledgeHolder::npc_from_actor_id,
                id,
            )?)),
            NpcRelationshipTarget::Faction(id) => Ok(NpcRelationshipTarget::Faction(
                validate_target_id(KnowledgeHolder::faction_from_id, id)?,
            )),
        }
    }

    /// Resolve a `(kind_token, raw_id)` pair into a validated, normalized target.
    /// Fail-closed: pc/npc/faction ids go through the kind-specific actor-identity
    /// contract (display names / placeholders / ad-hoc strings rejected);
    /// `player_party` ignores the id. Unknown kind tokens are rejected.
    pub fn from_parts(kind_token: &str, raw_id: &str) -> Result<Self, RelationshipError> {
        let raw = match kind_token {
            "player_party" => NpcRelationshipTarget::PlayerParty,
            "pc" => NpcRelationshipTarget::Pc(raw_id.to_string()),
            "npc" => NpcRelationshipTarget::Npc(raw_id.to_string()),
            "faction" => NpcRelationshipTarget::Faction(raw_id.to_string()),
            other => return Err(RelationshipError::UnknownTargetKind(other.to_string())),
        };
        raw.validated()
    }
}

/// Validate a stable target id through the supplied kind-specific actor-identity
/// constructor, returning the normalized id. The constructed holder is discarded — the
/// target enum carries the kind; the constructor is used purely for its fail-closed
/// `validate_actor_id` shape-check.
fn validate_target_id(
    ctor: impl Fn(&str) -> Result<KnowledgeHolder, crate::UnresolvedHolder>,
    raw: &str,
) -> Result<String, RelationshipError> {
    ctor(raw)
        .map(|h| h.holder_id().expect("validated id present").to_string())
        .map_err(|u| RelationshipError::UnstableId(u.to_string()))
}

/// Derived, low-cardinality summary of where the relationship sits. Recomputed from
/// the affect channels after every apply; never set directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipStance {
    Hostile,
    Wary,
    Neutral,
    Cordial,
    Friendly,
    Allied,
}

impl Default for RelationshipStance {
    fn default() -> Self {
        RelationshipStance::Neutral
    }
}

/// Errors from constructing or mutating a relationship. All are fail-closed refusals;
/// none leave a half-applied state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelationshipError {
    /// A delta carried no `evidence_event_ids`. The hard gate: no evidence, no mutation.
    MissingEvidence,
    /// The NPC or target id was not a stable actor id (display name / placeholder / ad-hoc).
    UnstableId(String),
    /// Unrecognized target kind token.
    UnknownTargetKind(String),
}

impl std::fmt::Display for RelationshipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RelationshipError::MissingEvidence => {
                write!(
                    f,
                    "relationship delta requires non-empty evidence_event_ids"
                )
            }
            RelationshipError::UnstableId(why) => write!(f, "unstable relationship id: {why}"),
            RelationshipError::UnknownTargetKind(k) => {
                write!(f, "unknown relationship target_kind: {k:?}")
            }
        }
    }
}

impl std::error::Error for RelationshipError {}

/// One NPC's bounded attitude toward a single target, plus interaction posture.
///
/// Construct with [`NpcRelationship::new`] (validates identity). Mutate only via
/// [`NpcRelationship::apply_delta`] / [`NpcRelationshipDelta::apply_to`] (bounded +
/// evidence-gated). `stance` / `interaction_desire` are derived and kept consistent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NpcRelationship {
    pub session_id: String,
    /// Stable NPC actor id (the relationship holder). NOT a display name.
    pub npc_id: String,
    #[serde(flatten)]
    pub target: NpcRelationshipTarget,

    // Signed bipolar affect channels [-100, 100].
    pub trust: i16,
    pub respect: i16,
    pub affection: i16,
    /// Positive = NPC feels indebted to the target (owes them); negative = target owes NPC.
    pub debt: i16,

    // Unipolar channels [0, 100].
    pub fear: i16,
    pub suspicion: i16,
    pub hostility: i16,
    /// Leverage the NPC perceives it holds over the target.
    pub leverage: i16,
    pub talkativeness: i16,

    /// Derived: how much the NPC wants to engage this target (-100 avoid .. 100 seek).
    pub interaction_desire: i16,
    /// Derived summary stance.
    pub stance: RelationshipStance,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_interaction_turn_id: Option<String>,
    /// Every evidence event id that has moved this relationship (append-only, deduped).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_event_ids: Vec<String>,
}

impl NpcRelationship {
    /// Construct a fresh, neutral relationship. Validates the NPC id and the target id
    /// (both fail-closed) via [`NpcRelationshipTarget::validated`]. All channels start at
    /// the neutral midpoint (0), stance Neutral, no evidence.
    pub fn new(
        session_id: impl Into<String>,
        npc_id: &str,
        target: NpcRelationshipTarget,
    ) -> Result<Self, RelationshipError> {
        let npc_id = KnowledgeHolder::npc_from_actor_id(npc_id)
            .map(|h| h.holder_id().expect("validated id present").to_string())
            .map_err(|u| RelationshipError::UnstableId(u.to_string()))?;
        // Fail-closed on the target too: a public `Pc/Npc/Faction(id)` variant must not
        // bypass the actor-identity contract. Normalizes the id (trim) in the process.
        let target = target.validated()?;
        let mut rel = NpcRelationship {
            session_id: session_id.into(),
            npc_id,
            target,
            trust: 0,
            respect: 0,
            affection: 0,
            debt: 0,
            fear: 0,
            suspicion: 0,
            hostility: 0,
            leverage: 0,
            talkativeness: 50,
            interaction_desire: 0,
            stance: RelationshipStance::Neutral,
            last_interaction_turn_id: None,
            evidence_event_ids: Vec::new(),
        };
        rel.recompute_derived();
        Ok(rel)
    }

    /// Apply a bounded, evidence-gated delta in place. See [`NpcRelationshipDelta::apply_to`].
    pub fn apply_delta(&mut self, delta: &NpcRelationshipDelta) -> Result<(), RelationshipError> {
        delta.apply_to(self)
    }

    /// Recompute the derived `stance` and `interaction_desire` from the affect channels.
    /// Deterministic and pure; called after every mutation so derived state never lags.
    fn recompute_derived(&mut self) {
        self.interaction_desire = self.derive_interaction_desire();
        self.stance = self.derive_stance();
    }

    /// interaction_desire = rapport minus aversion (averaged so it stays in band),
    /// nudged by base talkativeness. Positive = seek interaction, negative = avoid.
    fn derive_interaction_desire(&self) -> i16 {
        let rapport = self.trust as i64 + self.respect as i64 + self.affection as i64;
        let aversion = self.hostility as i64 + self.suspicion as i64 + self.fear as i64;
        let base = (rapport - aversion) / 3;
        // talkativeness 0..100 (mid 50) contributes a -25..+25 sociability bias.
        let talk_bias = (self.talkativeness as i64 - 50) / 2;
        clamp_signed(base + talk_bias)
    }

    /// Deterministic stance bands, checked most-severe first so a hostile NPC never
    /// reads as merely "wary". Thresholds are documented and covered by tests.
    fn derive_stance(&self) -> RelationshipStance {
        if self.hostility >= 60 || (self.hostility >= 40 && self.trust <= -20) {
            RelationshipStance::Hostile
        } else if self.fear >= 60 || self.suspicion >= 50 || self.trust <= -30 {
            RelationshipStance::Wary
        } else if self.trust >= 60 && self.affection >= 50 && self.hostility < 20 {
            RelationshipStance::Allied
        } else if self.affection >= 40 && self.trust >= 25 {
            RelationshipStance::Friendly
        } else if self.trust >= 20 || self.respect >= 40 {
            RelationshipStance::Cordial
        } else {
            RelationshipStance::Neutral
        }
    }
}

/// A proposed bounded change to a relationship. Channels default to 0 (no-op).
/// `evidence_event_ids` MUST be non-empty for [`Self::apply_to`] to mutate anything —
/// this is the hard gate that keeps relationship state source-grounded.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NpcRelationshipDelta {
    #[serde(default)]
    pub trust: i16,
    #[serde(default)]
    pub respect: i16,
    #[serde(default)]
    pub affection: i16,
    #[serde(default)]
    pub debt: i16,
    #[serde(default)]
    pub fear: i16,
    #[serde(default)]
    pub suspicion: i16,
    #[serde(default)]
    pub hostility: i16,
    #[serde(default)]
    pub leverage: i16,
    #[serde(default)]
    pub talkativeness: i16,
    /// Optional turn id to stamp as the last interaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_interaction_turn_id: Option<String>,
    /// REQUIRED non-empty: the evidence event ids justifying this change.
    #[serde(default)]
    pub evidence_event_ids: Vec<String>,
}

impl NpcRelationshipDelta {
    /// Start a zero delta carrying the given evidence. Builder methods below nudge channels.
    pub fn with_evidence(evidence_event_ids: Vec<String>) -> Self {
        NpcRelationshipDelta {
            evidence_event_ids,
            ..Default::default()
        }
    }

    /// Deterministic "the party/target helped this NPC" delta: trust + respect + debt up
    /// (the NPC now feels it owes the helper), with a small talkativeness bump. Passes
    /// through the same bounded, evidence-required apply path.
    pub fn help(evidence_event_ids: Vec<String>) -> Self {
        NpcRelationshipDelta {
            trust: 12,
            respect: 8,
            debt: 15,
            talkativeness: 5,
            evidence_event_ids,
            ..Default::default()
        }
    }

    /// Deterministic "the target threatened this NPC" delta: fear + suspicion up, trust
    /// down (and a touch of hostility). Same bounded, evidence-required apply path.
    pub fn threat(evidence_event_ids: Vec<String>) -> Self {
        NpcRelationshipDelta {
            trust: -15,
            fear: 18,
            suspicion: 14,
            hostility: 8,
            evidence_event_ids,
            ..Default::default()
        }
    }

    /// Apply this delta to `rel`, clamping every channel to its bound and recomputing
    /// derived state. Returns [`RelationshipError::MissingEvidence`] (mutating nothing)
    /// when `evidence_event_ids` is empty — the hard evidence gate.
    pub fn apply_to(&self, rel: &mut NpcRelationship) -> Result<(), RelationshipError> {
        if self.evidence_event_ids.is_empty() {
            return Err(RelationshipError::MissingEvidence);
        }
        // Signed bipolar channels.
        rel.trust = clamp_signed(rel.trust as i64 + self.trust as i64);
        rel.respect = clamp_signed(rel.respect as i64 + self.respect as i64);
        rel.affection = clamp_signed(rel.affection as i64 + self.affection as i64);
        rel.debt = clamp_signed(rel.debt as i64 + self.debt as i64);
        // Unipolar channels.
        rel.fear = clamp_unipolar(rel.fear as i64 + self.fear as i64);
        rel.suspicion = clamp_unipolar(rel.suspicion as i64 + self.suspicion as i64);
        rel.hostility = clamp_unipolar(rel.hostility as i64 + self.hostility as i64);
        rel.leverage = clamp_unipolar(rel.leverage as i64 + self.leverage as i64);
        rel.talkativeness = clamp_unipolar(rel.talkativeness as i64 + self.talkativeness as i64);

        if let Some(turn) = &self.last_interaction_turn_id {
            rel.last_interaction_turn_id = Some(turn.clone());
        }
        for ev in &self.evidence_event_ids {
            if !rel.evidence_event_ids.iter().any(|e| e == ev) {
                rel.evidence_event_ids.push(ev.clone());
            }
        }
        rel.recompute_derived();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel() -> NpcRelationship {
        NpcRelationship::new("sess1", "npc_lars", NpcRelationshipTarget::PlayerParty).unwrap()
    }

    #[test]
    fn new_starts_neutral_and_derived_is_consistent() {
        let r = rel();
        assert_eq!(r.trust, 0);
        assert_eq!(r.stance, RelationshipStance::Neutral);
        assert_eq!(r.interaction_desire, 0);
    }

    #[test]
    fn unstable_npc_id_fails_closed() {
        // Display-name / placeholder shaped ids are rejected.
        assert!(matches!(
            NpcRelationship::new("s", "The Butler", NpcRelationshipTarget::PlayerParty),
            Err(RelationshipError::UnstableId(_))
        ));
        assert!(matches!(
            NpcRelationship::new("s", "npc", NpcRelationshipTarget::PlayerParty),
            Err(RelationshipError::UnstableId(_))
        ));
    }

    #[test]
    fn new_rejects_invalid_public_variant_target_id() {
        // REV1 regression: a public `Npc/Pc/Faction(id)` variant must NOT bypass the
        // actor-identity gate — `NpcRelationship::new` validates the target id too.
        for target in [
            NpcRelationshipTarget::Npc("Goblin #2".into()),
            NpcRelationshipTarget::Pc("The Hero".into()),
            NpcRelationshipTarget::Faction("the cult".into()),
            NpcRelationshipTarget::Npc("".into()),
        ] {
            assert!(
                matches!(
                    NpcRelationship::new("s", "npc_a", target.clone()),
                    Err(RelationshipError::UnstableId(_))
                ),
                "unstable public-variant target must fail closed: {target:?}"
            );
        }
    }

    #[test]
    fn new_normalizes_valid_public_variant_target_id() {
        // Valid (but untrimmed) ids are accepted and normalized through the contract.
        let r = NpcRelationship::new("s", "npc_a", NpcRelationshipTarget::Npc("  npc_b  ".into()))
            .unwrap();
        assert_eq!(r.target, NpcRelationshipTarget::Npc("npc_b".into()));
        assert_eq!(r.target.target_id(), "npc_b");
        let r2 =
            NpcRelationship::new("s", "npc_a", NpcRelationshipTarget::Pc("pc_42".into())).unwrap();
        assert_eq!(r2.target.kind_token(), "pc");
        assert_eq!(r2.target.target_id(), "pc_42");
    }

    #[test]
    fn unstable_target_id_fails_closed() {
        assert!(matches!(
            NpcRelationshipTarget::from_parts("npc", "Goblin #2"),
            Err(RelationshipError::UnstableId(_))
        ));
        // kind-specific routing: a pc target with a display-name id is rejected too.
        assert!(matches!(
            NpcRelationshipTarget::Pc("Sir Reginald".into()).validated(),
            Err(RelationshipError::UnstableId(_))
        ));
        assert!(matches!(
            NpcRelationshipTarget::from_parts("dragon", "x"),
            Err(RelationshipError::UnknownTargetKind(_))
        ));
    }

    #[test]
    fn empty_delta_with_evidence_dedupes_evidence() {
        let mut r = rel();
        let d = NpcRelationshipDelta::with_evidence(vec!["e1".into(), "e1".into()]);
        d.apply_to(&mut r).unwrap();
        assert_eq!(r.evidence_event_ids, vec!["e1".to_string()]);
    }

    #[test]
    fn large_delta_is_clamped_both_poles() {
        let mut r = rel();
        let d = NpcRelationshipDelta {
            trust: 1000,
            fear: -1000,
            evidence_event_ids: vec!["e".into()],
            ..Default::default()
        };
        d.apply_to(&mut r).unwrap();
        assert_eq!(r.trust, CHANNEL_SIGNED_MAX);
        assert_eq!(r.fear, CHANNEL_UNIPOLAR_MIN);
    }

    #[test]
    fn stance_bands_are_deterministic() {
        let mut r = rel();
        NpcRelationshipDelta {
            hostility: 70,
            evidence_event_ids: vec!["e".into()],
            ..Default::default()
        }
        .apply_to(&mut r)
        .unwrap();
        assert_eq!(r.stance, RelationshipStance::Hostile);
        assert!(r.interaction_desire < 0, "hostility should suppress desire");
    }

    #[test]
    fn target_round_trips_through_serde() {
        let mut r =
            NpcRelationship::new("s", "npc_a", NpcRelationshipTarget::Npc("npc_b".into())).unwrap();
        NpcRelationshipDelta::help(vec!["e1".into()])
            .apply_to(&mut r)
            .unwrap();
        let json = serde_json::to_string(&r).unwrap();
        let back: NpcRelationship = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
        assert_eq!(back.target.kind_token(), "npc");
        assert_eq!(back.target.target_id(), "npc_b");
    }
}
