//! TC-P2-01 Memory Extraction Proposals v1 — a proposal-only extraction layer.
//!
//! Extractors (background memory jobs, later turn-loop hooks) **propose** candidate
//! world facts, holder knowledge updates, and NPC relationship deltas from turn
//! evidence. Runtime-owned commit paths — *not* this module — decide what becomes a
//! domain event, a [`crate::KnowledgeEdge`], an [`crate::NpcRelationship`] row, or a
//! [`crate::MemoryFact`]. Everything here is inert data plus fail-closed validation.
//!
//! Hard contract:
//! - **No commits.** Nothing writes a DB row, appends a domain event, or mutates a
//!   relationship / knowledge edge. Proposals only describe.
//! - **Fact identity ≠ holder knowledge.** A [`WorldFactCandidate`] carries the truth
//!   (subject / predicate / object); a [`KnowledgeUpdateCandidate`] carries only *who*
//!   holds *which* `fact_id` in *what* [`crate::KnowledgeState`]. They never collapse.
//! - **Identity-gated.** NPC / PC / faction holders and relationship ids route through
//!   the TC-KNOW-00 actor-identity contract ([`crate::KnowledgeHolder`]); display names,
//!   placeholders, and ad-hoc strings fail closed and are never invented.
//! - **Evidence-required.** Relationship delta candidates preserve the TC-NPC-02
//!   non-empty-evidence gate; a proposal may *represent* a delta, but applying it stays a
//!   separate runtime-owned step ([`crate::NpcRelationshipDelta::apply_to`]).
//! - **Evidence-preserving.** Every variant carries source/evidence refs for later
//!   commit review; a candidate with no provenance fails closed.
use crate::{
    FactTruthStatus, KnowledgeHolder, KnowledgeHolderKind, KnowledgeState, MemoryFact,
    NpcRelationshipDelta, NpcRelationshipTarget, RelationshipError, UnresolvedHolder,
};
use serde::{Deserialize, Serialize};

/// Fail-closed rejection from validating a proposal. Never a half-accepted state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProposalError {
    /// A candidate carried no source/evidence refs (or a relationship delta had no
    /// `evidence_event_ids`). The hard gate: no evidence, no proposal.
    MissingEvidence,
    /// A holder / relationship id was not a stable actor id (display name / placeholder /
    /// ad-hoc). Carries the underlying actor-identity reason for review.
    UnstableHolder(String),
    /// A relationship target carried an unrecognized `target_kind` token.
    UnknownRelationshipTarget(String),
    /// A world-fact / knowledge-update candidate was missing its fact identity fields.
    MissingFactIdentity,
    /// A proposal payload could not be decoded into a known proposal kind.
    UnsupportedKind(String),
}

impl std::fmt::Display for ProposalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProposalError::MissingEvidence => {
                write!(f, "proposal requires non-empty source/evidence references")
            }
            ProposalError::UnstableHolder(why) => write!(f, "unstable proposal holder: {why}"),
            ProposalError::UnknownRelationshipTarget(k) => {
                write!(f, "unknown relationship target_kind: {k:?}")
            }
            ProposalError::MissingFactIdentity => {
                write!(
                    f,
                    "proposal missing fact identity (fact_id / subject / predicate / object)"
                )
            }
            ProposalError::UnsupportedKind(k) => write!(f, "unsupported proposal kind: {k}"),
        }
    }
}

impl std::error::Error for ProposalError {}

fn map_rel_err(e: RelationshipError) -> ProposalError {
    match e {
        RelationshipError::MissingEvidence => ProposalError::MissingEvidence,
        RelationshipError::UnstableId(s) => ProposalError::UnstableHolder(s),
        RelationshipError::UnknownTargetKind(k) => ProposalError::UnknownRelationshipTarget(k),
    }
}

/// A serde-friendly knowledge holder for proposals. Collective / runtime-owned holders
/// (`gm` / `player_party` / `system`) carry no id; `pc` / `npc` / `faction` carry a
/// stable actor id validated through the TC-KNOW-00 contract. Kept distinct from the
/// non-serde [`crate::KnowledgeHolder`] so proposals stay round-trippable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalHolder {
    pub holder_kind: KnowledgeHolderKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder_id: Option<String>,
}

impl ProposalHolder {
    pub fn gm() -> Self {
        Self {
            holder_kind: KnowledgeHolderKind::Gm,
            holder_id: None,
        }
    }
    pub fn player_party() -> Self {
        Self {
            holder_kind: KnowledgeHolderKind::PlayerParty,
            holder_id: None,
        }
    }
    pub fn system() -> Self {
        Self {
            holder_kind: KnowledgeHolderKind::System,
            holder_id: None,
        }
    }
    pub fn npc(id: impl Into<String>) -> Self {
        Self {
            holder_kind: KnowledgeHolderKind::Npc,
            holder_id: Some(id.into()),
        }
    }
    pub fn pc(id: impl Into<String>) -> Self {
        Self {
            holder_kind: KnowledgeHolderKind::Pc,
            holder_id: Some(id.into()),
        }
    }
    pub fn faction(id: impl Into<String>) -> Self {
        Self {
            holder_kind: KnowledgeHolderKind::Faction,
            holder_id: Some(id.into()),
        }
    }

    /// Fail-closed validation + normalization. Collective holders (gm / player_party /
    /// system) drop any id (they are id-less runtime-owned identities). pc / npc / faction
    /// route through the kind-specific actor-identity contract; an unstable / missing id
    /// is rejected with [`ProposalError::UnstableHolder`] — never invented.
    pub fn validated(&self) -> Result<ProposalHolder, ProposalError> {
        match self.holder_kind {
            KnowledgeHolderKind::Gm
            | KnowledgeHolderKind::PlayerParty
            | KnowledgeHolderKind::System => Ok(ProposalHolder {
                holder_kind: self.holder_kind,
                holder_id: None,
            }),
            KnowledgeHolderKind::Npc => self.resolve_id(KnowledgeHolder::npc_from_actor_id),
            KnowledgeHolderKind::Pc => {
                self.resolve_id(KnowledgeHolder::player_character_from_actor_id)
            }
            KnowledgeHolderKind::Faction => self.resolve_id(KnowledgeHolder::faction_from_id),
        }
    }

    fn resolve_id(
        &self,
        ctor: impl Fn(&str) -> Result<KnowledgeHolder, UnresolvedHolder>,
    ) -> Result<ProposalHolder, ProposalError> {
        let raw = self.holder_id.as_deref().unwrap_or("");
        let holder = ctor(raw).map_err(|u| ProposalError::UnstableHolder(u.to_string()))?;
        Ok(ProposalHolder {
            holder_kind: self.holder_kind,
            holder_id: holder.holder_id().map(str::to_string),
        })
    }

    /// Stable token: collective holder = its kind token; id-bearing holder = `kind:id`.
    pub fn token(&self) -> String {
        match &self.holder_id {
            None => self.holder_kind.as_token().to_string(),
            Some(id) => format!("{}:{}", self.holder_kind.as_token(), id),
        }
    }
}

/// A candidate **world fact** (the truth half). Carries fact identity + evidence, and
/// deliberately NO holder: who knows it is a separate [`KnowledgeUpdateCandidate`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldFactCandidate {
    /// Proposed stable fact id; the commit path may keep or remap it.
    pub fact_id: String,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    #[serde(default)]
    pub confidence: Option<f64>,
    /// Evidence: source event ids justifying this candidate (non-empty when valid).
    #[serde(default)]
    pub source_event_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    /// 设计3 §4 — 该事实命题**自身**的真假分类（与「谁相信它」的 [`KnowledgeState`] 正交）。
    /// 加性可空：缺省 `None`（未标），对齐 `memory_facts.truth_status` 可空列。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truth_status: Option<FactTruthStatus>,
}

impl WorldFactCandidate {
    fn validated(&self) -> Result<WorldFactCandidate, ProposalError> {
        let fact_id = self.fact_id.trim();
        let subject = self.subject.trim();
        let predicate = self.predicate.trim();
        let object = self.object.trim();
        if fact_id.is_empty() || subject.is_empty() || predicate.is_empty() || object.is_empty() {
            return Err(ProposalError::MissingFactIdentity);
        }
        if self.source_event_ids.is_empty() {
            return Err(ProposalError::MissingEvidence);
        }
        Ok(WorldFactCandidate {
            fact_id: fact_id.to_string(),
            subject: subject.to_string(),
            predicate: predicate.to_string(),
            object: object.to_string(),
            summary: self.summary.trim().to_string(),
            confidence: self.confidence,
            source_event_ids: self.source_event_ids.clone(),
            turn_id: self.turn_id.clone(),
            truth_status: self.truth_status,
        })
    }
}

/// A candidate **holder knowledge update** (the who-knows half). References a world fact
/// by `fact_id` only — never the fact body — so identity and knowledge stay separate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeUpdateCandidate {
    /// The world fact this holder learns/believes (identity ref, not the fact itself).
    pub fact_id: String,
    pub holder: ProposalHolder,
    pub knowledge_state: KnowledgeState,
    #[serde(default)]
    pub confidence: Option<f64>,
    #[serde(default)]
    pub source_event_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learned_at_turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl KnowledgeUpdateCandidate {
    fn validated(&self) -> Result<KnowledgeUpdateCandidate, ProposalError> {
        let fact_id = self.fact_id.trim();
        if fact_id.is_empty() {
            return Err(ProposalError::MissingFactIdentity);
        }
        let holder = self.holder.validated()?;
        if self.source_event_ids.is_empty() {
            return Err(ProposalError::MissingEvidence);
        }
        Ok(KnowledgeUpdateCandidate {
            fact_id: fact_id.to_string(),
            holder,
            knowledge_state: self.knowledge_state,
            confidence: self.confidence,
            source_event_ids: self.source_event_ids.clone(),
            learned_at_turn_id: self.learned_at_turn_id.clone(),
            reason: self.reason.clone(),
        })
    }
}

/// P3 WorldFact↔KnowledgeEdge 引用契约的**强度**（架构师拍板：本阶段默认 [`Warn`]）。
///
/// 一个 [`KnowledgeUpdateCandidate`] 用 `fact_id` 引用一个世界事实身份。本阶段该引用契约
/// 为 **WEAK**：孤儿边（`fact_id` 查无 world_fact 身份）只 warn+trace、不阻断提交——历史
/// player-reveal 边可能引用尚无 world_fact 身份的 fact_id。seam 可被参数化为 [`Enforce`]
/// （fail-closed 拒绝孤儿边），供后续阶段回填存量数据后翻转，无需改调用点结构。
///
/// [`Warn`]: WorldFactRefStrength::Warn
/// [`Enforce`]: WorldFactRefStrength::Enforce
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorldFactRefStrength {
    /// 弱：孤儿边只 warn（不阻断）。本阶段默认。
    #[default]
    Warn,
    /// 强：孤儿边 fail-closed 拒绝（留待存量回填后启用）。
    Enforce,
}

/// 一次 WorldFact↔KnowledgeEdge 引用契约校验的结果（纯数据，无 IO）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorldFactRefOutcome {
    /// 引用可解析（world_fact 身份存在）→ 放行。
    Resolved,
    /// 孤儿引用（world_fact 身份缺失）但强度为 [`WorldFactRefStrength::Warn`] → 放行 + 警告。
    /// 携带稳定的 `fact_id`（非事实正文）供 trace。
    OrphanWarned { fact_id: String },
    /// 孤儿引用且强度为 [`WorldFactRefStrength::Enforce`] → 阻断（fail-closed）。
    OrphanBlocked { fact_id: String },
}

impl WorldFactRefOutcome {
    /// 该结果是否放行提交（`Resolved` / `OrphanWarned` 放行；`OrphanBlocked` 阻断）。
    pub fn admits(&self) -> bool {
        !matches!(self, WorldFactRefOutcome::OrphanBlocked { .. })
    }

    /// secret-safe trace 摘要：只露稳定 fact_id，绝不夹事实正文。
    pub fn trace_summary(&self) -> Option<String> {
        match self {
            WorldFactRefOutcome::Resolved => None,
            WorldFactRefOutcome::OrphanWarned { fact_id } => {
                Some(format!("world_fact_ref orphan (warn): {fact_id}"))
            }
            WorldFactRefOutcome::OrphanBlocked { fact_id } => {
                Some(format!("world_fact_ref orphan (blocked): {fact_id}"))
            }
        }
    }
}

/// 纯校验 seam（无 IO）：判定一个 KnowledgeUpdate 的 `fact_id` 是否指向一个**已知**世界事实
/// 身份。`fact_exists` 由调用方注入（runtime 查 world_facts；离线/单测可注入 closure），
/// 使本函数纯净可测、零 DB 依赖。强度 [`WorldFactRefStrength`] 决定孤儿引用是 warn 还是阻断。
///
/// 本阶段调用点（runtime commit）以默认 [`WorldFactRefStrength::Warn`] 调用：孤儿边放行 +
/// trace，绝不阻断（呼应 must_not_break：历史 player-reveal 边可能引用尚无 world_fact 身份的
/// fact_id）。绝不绕过 TC-KNOW-00 actor identity（本 seam 只看 fact_id，不发明 holder）。
pub fn check_world_fact_ref(
    fact_id: &str,
    fact_exists: impl FnOnce(&str) -> bool,
    strength: WorldFactRefStrength,
) -> WorldFactRefOutcome {
    let fact_id = fact_id.trim();
    if fact_exists(fact_id) {
        return WorldFactRefOutcome::Resolved;
    }
    match strength {
        WorldFactRefStrength::Warn => WorldFactRefOutcome::OrphanWarned {
            fact_id: fact_id.to_string(),
        },
        WorldFactRefStrength::Enforce => WorldFactRefOutcome::OrphanBlocked {
            fact_id: fact_id.to_string(),
        },
    }
}

/// A candidate **NPC relationship delta** (bounded, evidence-backed). Wraps the model's
/// [`NpcRelationshipDelta`] so the TC-NPC-02 evidence gate and bounded clamp are reused
/// verbatim. Validation checks identity + evidence but DOES NOT apply the delta.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NpcRelationshipDeltaCandidate {
    pub session_id: String,
    /// Stable NPC actor id (the relationship holder). NOT a display name.
    pub npc_id: String,
    pub target: NpcRelationshipTarget,
    pub delta: NpcRelationshipDelta,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl NpcRelationshipDeltaCandidate {
    fn validated(&self) -> Result<NpcRelationshipDeltaCandidate, ProposalError> {
        let npc_id = KnowledgeHolder::npc_from_actor_id(&self.npc_id)
            .map(|h| h.holder_id().expect("validated id present").to_string())
            .map_err(|u| ProposalError::UnstableHolder(u.to_string()))?;
        let target = self.target.validated().map_err(map_rel_err)?;
        // Preserve the TC-NPC-02 hard gate: a representable delta still needs evidence.
        if self.delta.evidence_event_ids.is_empty() {
            return Err(ProposalError::MissingEvidence);
        }
        Ok(NpcRelationshipDeltaCandidate {
            session_id: self.session_id.clone(),
            npc_id,
            target,
            delta: self.delta.clone(),
            reason: self.reason.clone(),
        })
    }
}

/// Adapter bridging the existing relationship-triple path: wraps an extracted
/// [`MemoryFact`] (see runtime `relationship_extraction`) as a proposal without changing
/// the extractor's behavior. Validation only checks provenance + triple shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryFactCandidate {
    pub fact: MemoryFact,
}

impl MemoryFactCandidate {
    fn validated(&self) -> Result<MemoryFactCandidate, ProposalError> {
        if self.fact.subject.trim().is_empty() || self.fact.predicate.trim().is_empty() {
            return Err(ProposalError::MissingFactIdentity);
        }
        if self.fact.source_event_ids.is_empty() {
            return Err(ProposalError::MissingEvidence);
        }
        Ok(self.clone())
    }
}

/// The proposal family. Serde-tagged on `proposal_kind` for deterministic JSON payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "proposal_kind", rename_all = "snake_case")]
pub enum MemoryExtractionProposal {
    WorldFact(WorldFactCandidate),
    KnowledgeUpdate(KnowledgeUpdateCandidate),
    NpcRelationshipDelta(NpcRelationshipDeltaCandidate),
    MemoryFact(MemoryFactCandidate),
}

impl MemoryExtractionProposal {
    /// Stable kind token (aligned with the serde `proposal_kind` tag).
    pub fn kind_token(&self) -> &'static str {
        match self {
            MemoryExtractionProposal::WorldFact(_) => "world_fact",
            MemoryExtractionProposal::KnowledgeUpdate(_) => "knowledge_update",
            MemoryExtractionProposal::NpcRelationshipDelta(_) => "npc_relationship_delta",
            MemoryExtractionProposal::MemoryFact(_) => "memory_fact",
        }
    }

    /// Fail-closed validation + normalization, dispatched per variant. Returns a fresh
    /// validated proposal or a typed [`ProposalError`]; never mutates external state.
    pub fn validated(&self) -> Result<MemoryExtractionProposal, ProposalError> {
        Ok(match self {
            MemoryExtractionProposal::WorldFact(c) => {
                MemoryExtractionProposal::WorldFact(c.validated()?)
            }
            MemoryExtractionProposal::KnowledgeUpdate(c) => {
                MemoryExtractionProposal::KnowledgeUpdate(c.validated()?)
            }
            MemoryExtractionProposal::NpcRelationshipDelta(c) => {
                MemoryExtractionProposal::NpcRelationshipDelta(c.validated()?)
            }
            MemoryExtractionProposal::MemoryFact(c) => {
                MemoryExtractionProposal::MemoryFact(c.validated()?)
            }
        })
    }

    /// The source/evidence refs this proposal preserves for later commit review.
    pub fn evidence_refs(&self) -> Vec<String> {
        match self {
            MemoryExtractionProposal::WorldFact(c) => c.source_event_ids.clone(),
            MemoryExtractionProposal::KnowledgeUpdate(c) => c.source_event_ids.clone(),
            MemoryExtractionProposal::NpcRelationshipDelta(c) => c.delta.evidence_event_ids.clone(),
            MemoryExtractionProposal::MemoryFact(c) => c.fact.source_event_ids.clone(),
        }
    }
}

#[cfg(test)]
mod world_fact_ref_tests {
    use super::*;

    /// 引用可解析（world_fact 身份存在）→ Resolved，放行，无 trace。
    #[test]
    fn resolvable_ref_admits_without_warning() {
        let out = check_world_fact_ref("f1", |id| id == "f1", WorldFactRefStrength::Warn);
        assert_eq!(out, WorldFactRefOutcome::Resolved);
        assert!(out.admits());
        assert!(out.trace_summary().is_none());
    }

    /// 孤儿引用 + 默认 Warn 强度 → OrphanWarned：仍放行（不阻断）但产 secret-safe trace。
    #[test]
    fn orphan_ref_warns_but_admits_by_default() {
        let out = check_world_fact_ref("f_orphan", |_| false, WorldFactRefStrength::default());
        assert_eq!(
            out,
            WorldFactRefOutcome::OrphanWarned {
                fact_id: "f_orphan".into()
            }
        );
        assert!(out.admits(), "弱契约：孤儿边放行不阻断");
        let summary = out.trace_summary().expect("孤儿边须有 trace");
        assert!(summary.contains("f_orphan"));
        assert!(summary.contains("warn"));
    }

    /// 孤儿引用 + Enforce 强度 → OrphanBlocked：fail-closed 阻断（供后续阶段回填后启用）。
    #[test]
    fn orphan_ref_blocked_under_enforce() {
        let out = check_world_fact_ref("f_orphan", |_| false, WorldFactRefStrength::Enforce);
        assert_eq!(
            out,
            WorldFactRefOutcome::OrphanBlocked {
                fact_id: "f_orphan".into()
            }
        );
        assert!(!out.admits(), "强契约：孤儿边 fail-closed 阻断");
    }
}
