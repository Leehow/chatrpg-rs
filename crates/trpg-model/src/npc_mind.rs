//! NPC Mind View v1 (TC-NPC-03): a prompt-safe projected view for ONE specific NPC.
//!
//! Combines three already-accepted, durable inputs and NOTHING else:
//!   1. that NPC's own knowledge/beliefs ([`crate::KnowledgeEdge`] projected to
//!      `(session, holder_kind='npc', holder_id=<this npc>)`);
//!   2. the static persona SAFE view ([`NpcProfile::safe_view`]) — GM-only secrets
//!      are STRUCTURALLY absent, never copied here;
//!   3. a compact relationship summary ([`RelationshipSummary`]) per stored target.
//!
//! Design contract (fail-closed, foundation-only):
//! - **Anchored to a stable NPC actor id.** [`NpcMindView::build`] validates the id
//!   through the TC-KNOW-00 actor-identity contract and refuses display names /
//!   placeholders / ad-hoc strings, and refuses a profile anchored to a different NPC.
//! - **Knowledge vs belief is explicit.** `knows_true` projects as
//!   [`NpcFactStanding::Known`]; the belief state (`believes_false`, P0b's only belief
//!   token) projects as [`NpcFactStanding::Belief`] and is NEVER reported as known truth
//!   or GM world truth. Every other state (`unknown` / `exposed`) is excluded.
//! - **No GM truth, no other holders.** The view only ever sees this NPC's own edges
//!   (the DB query filters by holder); GM-only world truth and player_party/other-NPC
//!   edges cannot enter. The speech context is built purely from this view, so it
//!   cannot leak a fact the NPC does not hold.
//!
//! This is the foundation for later NPC behavior planning — it deliberately does NOT
//! generate dialogue, plan behavior, or touch the GM turn loop.
use crate::knowledge::KnowledgeState;
use crate::npc_profile::{NpcProfile, NpcProfileSafeView};
use crate::npc_relationship::{NpcRelationship, RelationshipStance};
use crate::KnowledgeHolder;
use serde::{Deserialize, Serialize};

/// Whether a projected fact is known truth (for this NPC) or a belief. A belief is
/// never world truth — `believes_false` is an honest mistake the NPC holds, and must be
/// labeled as such so a prompt never treats it as fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NpcFactStanding {
    /// `knows_true`: known truth for this NPC.
    Known,
    /// `believes_false` (P0b's belief token): a belief, NOT world truth.
    Belief,
}

/// One fact in an NPC's mind, with its standing preserved. `state` keeps the exact
/// underlying [`KnowledgeState`] for fidelity; `standing` is the coarse known-vs-belief
/// label callers usually branch on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NpcMindFact {
    pub fact_id: String,
    pub standing: NpcFactStanding,
    pub state: KnowledgeState,
}

/// A raw `(fact_id, knowledge_state)` entry for one NPC, as projected from durable
/// `knowledge_edges`. [`NpcMindView::build`] filters these down to known + believed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NpcKnowledgeEntry {
    pub fact_id: String,
    pub state: KnowledgeState,
}

/// Compact, prompt-safe summary of one stored [`NpcRelationship`]. Carries the derived
/// stance + interaction posture and the few channels that most shape behavior, not the
/// full evidence ledger — enough to ground a speech prompt without redesigning channels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationshipSummary {
    pub target_kind: String,
    pub target_id: String,
    pub stance: RelationshipStance,
    pub interaction_desire: i16,
    pub trust: i16,
    pub fear: i16,
    pub hostility: i16,
}

impl RelationshipSummary {
    fn from_relationship(rel: &NpcRelationship) -> Self {
        RelationshipSummary {
            target_kind: rel.target.kind_token().to_string(),
            target_id: rel.target.target_id().to_string(),
            stance: rel.stance,
            interaction_desire: rel.interaction_desire,
            trust: rel.trust,
            fear: rel.fear,
            hostility: rel.hostility,
        }
    }

    /// One compact prompt line, e.g. `toward player_party: wary (desire -7, trust -15, fear 18, hostility 8)`.
    pub fn to_prompt_line(&self) -> String {
        let stance = serde_json::to_value(self.stance)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| "neutral".into());
        let who = if self.target_id.is_empty() {
            self.target_kind.clone()
        } else {
            format!("{}:{}", self.target_kind, self.target_id)
        };
        format!(
            "toward {who}: {stance} (desire {}, trust {}, fear {}, hostility {})",
            self.interaction_desire, self.trust, self.fear, self.hostility
        )
    }
}

/// Why a mind view could not be built. Fail-closed: the view is never produced from an
/// unstable identity or a profile that belongs to a different NPC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NpcMindError {
    /// The NPC id was not a stable actor id (display name / placeholder / ad-hoc).
    UnstableNpcId(String),
    /// The supplied profile is anchored to a different NPC than the requested id.
    ProfileMismatch { expected: String, got: String },
}

impl std::fmt::Display for NpcMindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NpcMindError::UnstableNpcId(why) => write!(f, "unstable NPC id: {why}"),
            NpcMindError::ProfileMismatch { expected, got } => write!(
                f,
                "profile actor_id {got:?} does not match requested NPC {expected:?}"
            ),
        }
    }
}

impl std::error::Error for NpcMindError {}

/// A prompt-safe mind view for one NPC: its own knowledge/beliefs, persona safe view,
/// and compact relationship summaries. Construct via [`Self::build`]; the type carries
/// no GM-only secret text and no facts the NPC does not hold.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NpcMindView {
    pub session_id: String,
    /// Stable NPC actor id this view is anchored to (normalized).
    pub npc_id: String,
    pub persona: NpcProfileSafeView,
    pub relationships: Vec<RelationshipSummary>,
    /// Known + believed facts only (unknown / weak states excluded).
    pub facts: Vec<NpcMindFact>,
}

impl NpcMindView {
    /// Project a mind view for `npc_actor_id` from its persona, stored relationships, and
    /// durable knowledge entries. Fail-closed: the id is validated through the
    /// actor-identity contract and the profile must be anchored to the same NPC.
    ///
    /// `entries` are this NPC's own `knowledge_edges` rows; only `knows_true` and the
    /// belief states survive (see [`KnowledgeState::is_true_knowledge`] /
    /// [`KnowledgeState::is_belief`]). `relationships` are filtered to those actually
    /// held by this NPC (defense in depth — a caller cannot smuggle another NPC's state).
    pub fn build(
        session_id: &str,
        npc_actor_id: &str,
        profile: &NpcProfile,
        relationships: &[NpcRelationship],
        entries: &[NpcKnowledgeEntry],
    ) -> Result<Self, NpcMindError> {
        let npc_id = KnowledgeHolder::npc_from_actor_id(npc_actor_id)
            .map(|h| h.holder_id().expect("validated id present").to_string())
            .map_err(|u| NpcMindError::UnstableNpcId(u.to_string()))?;
        // The profile must anchor to the same NPC. Validate its id too so a display-name
        // profile can't pass, then compare normalized ids.
        let profile_id = KnowledgeHolder::npc_from_actor_id(&profile.actor_id)
            .map(|h| h.holder_id().expect("validated id present").to_string())
            .map_err(|u| NpcMindError::UnstableNpcId(u.to_string()))?;
        if profile_id != npc_id {
            return Err(NpcMindError::ProfileMismatch {
                expected: npc_id,
                got: profile_id,
            });
        }

        let facts = entries
            .iter()
            .filter_map(|e| {
                let standing = if e.state.is_true_knowledge() {
                    NpcFactStanding::Known
                } else if e.state.is_belief() {
                    NpcFactStanding::Belief
                } else {
                    return None; // unknown / heard_about / suspects / ... excluded
                };
                Some(NpcMindFact {
                    fact_id: e.fact_id.clone(),
                    standing,
                    state: e.state,
                })
            })
            .collect();

        // Only this NPC's own relationships (compare normalized npc_id; skip others).
        let relationships = relationships
            .iter()
            .filter(|r| {
                KnowledgeHolder::npc_from_actor_id(&r.npc_id)
                    .map(|h| h.holder_id() == Some(npc_id.as_str()))
                    .unwrap_or(false)
            })
            .map(RelationshipSummary::from_relationship)
            .collect();

        Ok(NpcMindView {
            session_id: session_id.to_string(),
            npc_id,
            persona: profile.safe_view(),
            relationships,
            facts,
        })
    }

    /// Fact ids this NPC knows as true (standing == Known).
    pub fn known_fact_ids(&self) -> Vec<&str> {
        self.facts
            .iter()
            .filter(|f| f.standing == NpcFactStanding::Known)
            .map(|f| f.fact_id.as_str())
            .collect()
    }

    /// Fact ids this NPC merely believes (standing == Belief), which may be false.
    pub fn belief_fact_ids(&self) -> Vec<&str> {
        self.facts
            .iter()
            .filter(|f| f.standing == NpcFactStanding::Belief)
            .map(|f| f.fact_id.as_str())
            .collect()
    }

    /// Build a compact, prompt-safe speech context for this NPC. Contains ONLY persona
    /// safe-view descriptors, compact relationship lines, and fact ids labeled
    /// known-vs-belief. It never carries GM-only secret text or any fact the NPC does
    /// not hold. Fact ids (not full text) are used — a safe source-backed fact-text path
    /// can be layered on later without changing this contract.
    pub fn speech_context(&self) -> String {
        let p = &self.persona;
        let mut lines: Vec<String> = Vec::new();
        let header = match p.role.as_deref().filter(|s| !s.trim().is_empty()) {
            Some(role) => format!("NPC: {} ({role})", p.name),
            None => format!("NPC: {}", p.name),
        };
        lines.push(header);
        if !p.personality_traits.is_empty() {
            lines.push(format!("Personality: {}", p.personality_traits.join(", ")));
        }
        if !p.drives.is_empty() {
            lines.push(format!("Drives: {}", p.drives.join(", ")));
        }
        if !p.goals.is_empty() {
            lines.push(format!("Goals: {}", p.goals.join(", ")));
        }
        let speech = p.speech_style.to_prompt_block();
        if !speech.is_empty() {
            lines.push(speech);
        }

        if !self.relationships.is_empty() {
            lines.push("Attitude:".to_string());
            for r in &self.relationships {
                lines.push(format!("- {}", r.to_prompt_line()));
            }
        }

        let known = self.known_fact_ids();
        if !known.is_empty() {
            lines.push(format!("Known facts (true to this NPC): {}", known.join(", ")));
        }
        let beliefs = self.belief_fact_ids();
        if !beliefs.is_empty() {
            lines.push(format!(
                "Beliefs (may be false — do not treat as fact): {}",
                beliefs.join(", ")
            ));
        }
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NpcRelationshipDelta, NpcRelationshipTarget};

    fn profile() -> NpcProfile {
        NpcProfile {
            actor_id: "npc_lars".into(),
            name: "Lars".into(),
            role: Some("mechanic".into()),
            ..Default::default()
        }
    }

    #[test]
    fn only_known_and_belief_states_project() {
        // P0b 4-态：knows_true → Known，believes_false → Belief，unknown/exposed 排除。
        let entries = vec![
            NpcKnowledgeEntry { fact_id: "k".into(), state: KnowledgeState::KnowsTrue },
            NpcKnowledgeEntry { fact_id: "bf".into(), state: KnowledgeState::BelievesFalse },
            NpcKnowledgeEntry { fact_id: "u".into(), state: KnowledgeState::Unknown },
            NpcKnowledgeEntry { fact_id: "ex".into(), state: KnowledgeState::Exposed },
        ];
        let v = NpcMindView::build("s", "npc_lars", &profile(), &[], &entries).unwrap();
        assert_eq!(v.known_fact_ids(), vec!["k"]);
        assert_eq!(v.belief_fact_ids(), vec!["bf"]);
    }

    #[test]
    fn foreign_relationship_is_filtered_out() {
        let mine =
            NpcRelationship::new("s", "npc_lars", NpcRelationshipTarget::PlayerParty).unwrap();
        let other =
            NpcRelationship::new("s", "npc_other", NpcRelationshipTarget::PlayerParty).unwrap();
        let v = NpcMindView::build("s", "npc_lars", &profile(), &[mine, other], &[]).unwrap();
        assert_eq!(v.relationships.len(), 1, "only this NPC's relationship survives");
    }

    #[test]
    fn summary_reflects_threatened_relationship() {
        let mut rel =
            NpcRelationship::new("s", "npc_lars", NpcRelationshipTarget::PlayerParty).unwrap();
        rel.apply_delta(&NpcRelationshipDelta::threat(vec!["e".into()])).unwrap();
        let s = RelationshipSummary::from_relationship(&rel);
        assert_eq!(s.target_kind, "player_party");
        assert!(s.fear > 0 && s.trust < 0);
        assert!(s.to_prompt_line().contains("toward player_party"));
    }
}
