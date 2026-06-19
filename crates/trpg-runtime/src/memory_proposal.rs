//! Runtime adapter for TC-P2-01 Memory Extraction Proposals + the TC-D3-03 commit
//! pipeline.
//!
//! Two layers live here, both runtime-owned:
//! - **Proposal helpers** (no DB / no LLM / no IO): turn turn-evidence into validated
//!   [`MemoryExtractionProposal`]s. The model layer ([`trpg_model::memory_proposal`]) owns
//!   the proposal types and their fail-closed `validated()` gate; this layer only parses
//!   deterministic payloads and bridges the existing relationship-triple path.
//! - **Commit pipeline** ([`review_and_commit_proposals`]): the *only* place a validated
//!   proposal becomes durable. Plugins / agents / proposal model types never write — they
//!   propose; runtime validates, decides the durable path per variant, and applies. The
//!   batch is pre-validated in a pure [`plan_batch`] phase before any write: a proposal that
//!   fails the model gate OR the runtime session-authority guard aborts the whole batch and
//!   commits nothing (the pre-write atomicity gate). DB errors during execution are reported
//!   per action and are NOT rolled back across actions — see [`CommitReport`].
use serde::Serialize;
use serde_json::Value;
use trpg_model::{
    KnowledgeHolderKind, KnowledgeState, MemoryExtractionProposal, MemoryFact, MemoryFactCandidate,
    MemoryStatus, NpcRelationshipDelta, NpcRelationshipTarget, ProposalError, Scope, ScopeType,
    Visibility,
};

/// Bridge already-extracted relationship-triple [`MemoryFact`]s (see
/// [`crate::relationship_extraction`]) into proposals, WITHOUT changing the extractor or
/// committing anything. Pure mapping that preserves each fact's `source_event_ids`; a
/// fact lacking provenance / triple shape is dropped (fail-closed) so a proposal never
/// loses its evidence trail.
pub fn relationship_facts_to_proposals(facts: &[MemoryFact]) -> Vec<MemoryExtractionProposal> {
    facts
        .iter()
        .filter_map(|f| {
            MemoryExtractionProposal::MemoryFact(MemoryFactCandidate { fact: f.clone() })
                .validated()
                .ok()
        })
        .collect()
}

/// Parse a deterministic JSON proposal payload, validating every entry through the
/// model's fail-closed gate. Shape: `{"proposals":[ <proposal>, ... ]}`. Lenient: a
/// missing/garbage payload yields an empty list, and individual malformed or invalid
/// proposals are dropped (never invented). Use [`try_proposals_from_json`] when a typed
/// error is required instead.
pub fn proposals_from_json(raw: &Value) -> Vec<MemoryExtractionProposal> {
    let Some(arr) = raw.get("proposals").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|v| {
            serde_json::from_value::<MemoryExtractionProposal>(v.clone())
                .ok()
                .and_then(|p| p.validated().ok())
        })
        .collect()
}

/// Strict counterpart of [`proposals_from_json`]: fail closed with a typed
/// [`ProposalError`] on the first unsupported / malformed / invalid proposal (or a
/// payload missing its `proposals` array). For callers that must reject a bad batch
/// outright rather than silently drop entries.
pub fn try_proposals_from_json(
    raw: &Value,
) -> Result<Vec<MemoryExtractionProposal>, ProposalError> {
    let arr = raw
        .get("proposals")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            ProposalError::UnsupportedKind("payload missing `proposals` array".into())
        })?;
    arr.iter()
        .map(|v| {
            let parsed: MemoryExtractionProposal = serde_json::from_value(v.clone())
                .map_err(|e| ProposalError::UnsupportedKind(e.to_string()))?;
            parsed.validated()
        })
        .collect()
}

// ───────────────────────── TC-D3-03 commit pipeline ──────────────────────────
//
// Runtime-owned review/apply surface. Proposals only *describe*; this is where a
// validated proposal becomes a durable `memory_facts` row, a `knowledge_edges` /
// domain-event write, or an `npc_relationships` mutation — always through the existing
// state-owner DB APIs (never raw SQL here, never a plugin write).

use trpg_db::{Db, KnowledgeEdgeInput};

/// Default confidence for a committed world-fact candidate that carries none. A neutral
/// midpoint: the proposal asserted the fact (with evidence) but gave no explicit score.
const WORLD_FACT_DEFAULT_CONFIDENCE: f32 = 0.5;

/// Where/when a batch of proposals is being committed. `turn_id` is the commit turn used
/// for domain-event provenance when a candidate carries none of its own.
#[derive(Debug, Clone, Copy)]
pub struct CommitContext<'a> {
    pub session_id: &'a str,
    pub turn_id: &'a str,
}

/// Per-proposal disposition. `Done` = a durable write happened; `Rejected` = invalid (the
/// batch committed nothing); `Skipped` = valid but intentionally not committed (the gating
/// hook — currently latent, since every holder kind is durable after pc/faction opened).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitStatus {
    Done,
    Rejected,
    Skipped,
}

/// Audit record for one proposal. Carries only identity tokens (fact_id / holder / target)
/// and the durable path taken — never player-unknown fact prose, so dumping a report can
/// never leak a hidden fact body into a player-facing log.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CommitOutcome {
    pub kind_token: &'static str,
    pub status: CommitStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub durable_path: Option<&'static str>,
    /// Identity ref of what was committed (fact_id, or `npc_id→target` for a relationship).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fact_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Result of reviewing + committing a proposal batch, one [`CommitOutcome`] per input in
/// order.
///
/// **Atomicity is pre-write only.** If any proposal fails a pre-write gate (model
/// `validated()` or the runtime session-authority guard) the *whole* batch is aborted
/// before the first DB call: every outcome is `Rejected` and nothing wrote. Once execution
/// starts, each action commits through its own state-owner API independently — there is no
/// cross-action transaction, so a DB error mid-batch yields a per-action `Rejected` outcome
/// that *may follow* earlier `Done` writes. In short: a `Rejected` from the pre-write gates
/// guarantees nothing wrote (no `Done` present); a `Rejected` from execution does not.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CommitReport {
    pub outcomes: Vec<CommitOutcome>,
}

impl CommitReport {
    pub fn committed(&self) -> usize {
        self.count(CommitStatus::Done)
    }
    pub fn rejected(&self) -> usize {
        self.count(CommitStatus::Rejected)
    }
    pub fn skipped(&self) -> usize {
        self.count(CommitStatus::Skipped)
    }
    /// True when no proposal produced a durable write.
    pub fn committed_nothing(&self) -> bool {
        self.committed() == 0
    }
    fn count(&self, s: CommitStatus) -> usize {
        self.outcomes.iter().filter(|o| o.status == s).count()
    }
}

/// One planned durable action for a validated proposal. Holds owned data so the pure plan
/// phase needs no DB; the executor turns each into exactly one state-owner call.
enum CommitAction {
    /// New world fact → `memory_facts` (first-stage Fact Store).
    WorldFact(MemoryFact),
    /// Existing relationship-triple fact, committed as-is.
    LegacyFact(MemoryFact),
    /// player_party learned a fact as true → reveal path (event + knowledge edge).
    PlayerLearned {
        fact_id: String,
        reason: Option<String>,
    },
    /// player_party non-true belief about a fact → player_party knowledge edge.
    PlayerPartyEdge {
        fact_id: String,
        state: KnowledgeState,
        reason: Option<String>,
    },
    /// A specific NPC learned a fact as true → NpcLearnedFact event + durable npc edge.
    NpcLearned {
        npc_id: String,
        fact_id: String,
        reason: Option<String>,
    },
    /// A durable holder (gm / system / npc) holds a fact in a non-true state → edge.
    DurableEdge {
        holder_kind: &'static str,
        holder_id: String,
        fact_id: String,
        state: KnowledgeState,
        confidence: Option<f64>,
        learned_at_turn_id: Option<String>,
        source_event_id: Option<String>,
        reason: Option<String>,
    },
    /// Bounded, evidence-gated NPC relationship delta → `npc_relationships`.
    Relationship {
        session_id: String,
        npc_id: String,
        target: NpcRelationshipTarget,
        delta: NpcRelationshipDelta,
    },
    /// Valid but intentionally not committed — the typed "no durable write" path. Currently
    /// latent: every holder kind is durable after pc/faction opened (this task), so no arm
    /// constructs it; the report's `Skipped` status + executor handling are retained as the
    /// commit-pipeline's gating hook for any holder/state a future task wants to defer.
    #[allow(dead_code)]
    Skip { reason: String },
}

impl CommitAction {
    /// Identity ref recorded in the audit outcome — never fact prose.
    fn fact_ref(&self) -> Option<String> {
        match self {
            CommitAction::WorldFact(f) | CommitAction::LegacyFact(f) => Some(f.fact_id.clone()),
            CommitAction::PlayerLearned { fact_id, .. }
            | CommitAction::PlayerPartyEdge { fact_id, .. }
            | CommitAction::NpcLearned { fact_id, .. }
            | CommitAction::DurableEdge { fact_id, .. } => Some(fact_id.clone()),
            CommitAction::Relationship { npc_id, target, .. } => Some(format!(
                "{npc_id}→{}:{}",
                target.kind_token(),
                target.target_id()
            )),
            CommitAction::Skip { .. } => None,
        }
    }

    fn durable_path(&self) -> &'static str {
        match self {
            CommitAction::WorldFact(_) => "memory_facts.upsert(world_fact)",
            CommitAction::LegacyFact(_) => "memory_facts.upsert(legacy_triple)",
            CommitAction::PlayerLearned { .. } => "record_revealed_fact(player_party,knows_true)",
            CommitAction::PlayerPartyEdge { .. } => "knowledge_edges(player_party)",
            CommitAction::NpcLearned { .. } => "record_npc_learned_fact(npc,knows_true)",
            CommitAction::DurableEdge { .. } => "knowledge_edges(durable_holder)",
            CommitAction::Relationship { .. } => "npc_relationships.upsert(evidence_gated)",
            CommitAction::Skip { .. } => "",
        }
    }
}

/// One validated, planned proposal awaiting execution.
struct PlannedCommit {
    kind_token: &'static str,
    action: CommitAction,
}

/// Outcome of pre-validating + planning a whole batch (pure, no DB).
enum BatchPlan {
    /// A proposal failed validation: the batch is poisoned and commits nothing. Carries the
    /// finished all-`Rejected` report so the caller can return it without touching the DB.
    Aborted(CommitReport),
    /// Every proposal validated; ready to execute (some entries may be `Skip`).
    Ready(Vec<PlannedCommit>),
}

/// Pure pre-validation + planning. Two fail-closed gates run BEFORE any write, so a batch
/// that trips either commits nothing:
/// 1. the model's `validated()` gate (evidence / identity / fact shape), and
/// 2. the runtime session-authority guard ([`plan_action`] rejects a proposal whose own
///    `session_id` disagrees with the commit context — the runtime, not the proposal,
///    decides the destination session).
/// The moment one proposal fails either gate the whole batch is `Aborted` with an
/// all-`Rejected` report and zero executable plans — provable without a database. Surviving
/// proposals are routed to a durable [`CommitAction`] (or `Skip`).
fn plan_batch(proposals: &[MemoryExtractionProposal], ctx: &CommitContext<'_>) -> BatchPlan {
    let mut plans = Vec::with_capacity(proposals.len());
    for (idx, p) in proposals.iter().enumerate() {
        let planned = p
            .validated()
            .map_err(|e| format!("invalid proposal: {e}"))
            .and_then(|valid| {
                let kind_token = valid.kind_token();
                plan_action(&valid, ctx).map(|action| PlannedCommit { kind_token, action })
            });
        match planned {
            Ok(planned) => plans.push(planned),
            // Failed a pre-write gate → abort: all-Rejected report, commit nothing.
            Err(reason) => return BatchPlan::Aborted(aborted_report(proposals, idx, reason)),
        }
    }
    BatchPlan::Ready(plans)
}

/// Build the all-`Rejected` report for an aborted batch: the offending proposal carries the
/// concrete reason, every sibling is rejected with a "no writes" note.
fn aborted_report(
    proposals: &[MemoryExtractionProposal],
    offending: usize,
    reason: String,
) -> CommitReport {
    let outcomes = proposals
        .iter()
        .enumerate()
        .map(|(i, q)| CommitOutcome {
            kind_token: q.kind_token(),
            status: CommitStatus::Rejected,
            durable_path: None,
            fact_ref: None,
            reason: Some(if i == offending {
                reason.clone()
            } else {
                "batch aborted: a sibling proposal failed a pre-write gate (no writes)".to_string()
            }),
        })
        .collect();
    CommitReport { outcomes }
}

/// Route ONE already-validated proposal to its durable action. Returns `Err(reason)` only
/// for the runtime session-authority guard — a proposal that tries to choose its own
/// destination session instead of the commit context's: a relationship candidate naming a
/// different `session_id`, or a legacy `MemoryFact` whose `session_id` (or session-scope id)
/// disagrees with the context. That aborts the batch like a validation failure. Every
/// durable holder (gm / player_party / system / npc / pc / faction) routes to a write; the
/// [`CommitAction::Skip`] gating path is retained but currently unused.
fn plan_action(
    p: &MemoryExtractionProposal,
    ctx: &CommitContext<'_>,
) -> Result<CommitAction, String> {
    Ok(match p {
        MemoryExtractionProposal::WorldFact(c) => {
            let now = chrono::Utc::now();
            let summary = if c.summary.is_empty() {
                format!("{} {} {}", c.subject, c.predicate, c.object)
            } else {
                c.summary.clone()
            };
            CommitAction::WorldFact(MemoryFact {
                fact_id: c.fact_id.clone(),
                session_id: ctx.session_id.to_string(),
                scope: Scope {
                    scope_type: ScopeType::Session,
                    scope_id: ctx.session_id.to_string(),
                },
                visibility: Visibility::GmOnly,
                subject: c.subject.clone(),
                predicate: c.predicate.clone(),
                object: Value::String(c.object.clone()),
                summary,
                status: MemoryStatus::Active,
                confidence: c
                    .confidence
                    .map(|v| v as f32)
                    .unwrap_or(WORLD_FACT_DEFAULT_CONFIDENCE),
                source_event_ids: c.source_event_ids.clone(),
                tags: vec!["memory".into(), "proposal".into(), "world_fact".into()],
                importance: 1,
                turn_id: c.turn_id.clone().or_else(|| Some(ctx.turn_id.to_string())),
                created_at: now,
                updated_at: now,
            })
        }
        MemoryExtractionProposal::MemoryFact(c) => {
            // Session-authority guard (same invariant as every other variant): the commit
            // context — never the proposal — decides the durable destination. A legacy
            // `MemoryFact` carries its own `session_id` (and, for a session-scoped fact, a
            // scope id); a fact naming a different session is rejected before any write.
            let fact = &c.fact;
            if fact.session_id != ctx.session_id {
                return Err(format!(
                    "legacy memory_fact session `{}` ≠ commit context session `{}` (runtime is authoritative)",
                    fact.session_id, ctx.session_id
                ));
            }
            if matches!(fact.scope.scope_type, ScopeType::Session)
                && fact.scope.scope_id != ctx.session_id
            {
                return Err(format!(
                    "legacy memory_fact session-scope id `{}` ≠ commit context session `{}` (runtime is authoritative)",
                    fact.scope.scope_id, ctx.session_id
                ));
            }
            // Normalize to the context session as authoritative before the write (a no-op
            // once the guards above pass, but it makes the authority explicit so the durable
            // row can never carry a proposal-chosen session/scope id).
            let mut fact = fact.clone();
            fact.session_id = ctx.session_id.to_string();
            if matches!(fact.scope.scope_type, ScopeType::Session) {
                fact.scope.scope_id = ctx.session_id.to_string();
            }
            CommitAction::LegacyFact(fact)
        }
        MemoryExtractionProposal::KnowledgeUpdate(c) => {
            let fact_id = c.fact_id.clone();
            let reason = c.reason.clone();
            let source_event_id = c.source_event_ids.first().cloned();
            let learned_at_turn_id = c
                .learned_at_turn_id
                .clone()
                .or_else(|| Some(ctx.turn_id.to_string()));
            let is_true = matches!(c.knowledge_state, KnowledgeState::KnowsTrue);
            match c.holder.holder_kind {
                KnowledgeHolderKind::PlayerParty if is_true => {
                    CommitAction::PlayerLearned { fact_id, reason }
                }
                KnowledgeHolderKind::PlayerParty => CommitAction::PlayerPartyEdge {
                    fact_id,
                    state: c.knowledge_state,
                    reason,
                },
                KnowledgeHolderKind::Npc if is_true => {
                    // holder_id was normalized by ProposalHolder::validated().
                    let npc_id = c.holder.holder_id.clone().unwrap_or_default();
                    CommitAction::NpcLearned {
                        npc_id,
                        fact_id,
                        reason,
                    }
                }
                // Every durable holder that has no dedicated learned-event path (gm / system,
                // any non-true npc, and now pc / faction) commits one durable edge. The
                // holder_id was already normalized by ProposalHolder::validated(); the DB
                // upsert re-validates pc/faction/npc ids and fails closed on an unstable id.
                kind @ (KnowledgeHolderKind::Npc
                | KnowledgeHolderKind::Gm
                | KnowledgeHolderKind::System
                | KnowledgeHolderKind::Pc
                | KnowledgeHolderKind::Faction) => CommitAction::DurableEdge {
                    holder_kind: kind.as_token(),
                    holder_id: c.holder.holder_id.clone().unwrap_or_default(),
                    fact_id,
                    state: c.knowledge_state,
                    confidence: c.confidence,
                    learned_at_turn_id,
                    source_event_id,
                    reason,
                },
            }
        }
        MemoryExtractionProposal::NpcRelationshipDelta(c) => {
            // Session-authority guard: the runtime commit context — never the proposal —
            // decides which session receives the write. A proposal naming a different
            // session is rejected before any write (fail-closed).
            if c.session_id != ctx.session_id {
                return Err(format!(
                    "relationship proposal session `{}` ≠ commit context session `{}` (runtime is authoritative)",
                    c.session_id, ctx.session_id
                ));
            }
            CommitAction::Relationship {
                // Use the context's authoritative session, not the proposal's.
                session_id: ctx.session_id.to_string(),
                npc_id: c.npc_id.clone(),
                target: c.target.clone(),
                delta: c.delta.clone(),
            }
        }
    })
}

/// Review and commit a batch of memory-extraction proposals to durable state — the single
/// runtime-owned entry point. Pre-validates the whole batch in a pure phase; if any proposal
/// fails the model gate or the runtime session-authority guard, returns an all-`Rejected`
/// report and writes nothing. Otherwise each surviving proposal is committed through its
/// state-owner DB path (or recorded `Skipped`), and the per-proposal [`CommitReport`] records
/// which durable path ran. Execution is per-action with no cross-action transaction (see
/// [`CommitReport`] for the exact atomicity guarantee).
///
/// The commit context is authoritative for the destination session: a relationship proposal
/// naming a different session is rejected before any write.
///
/// Plugins / agents / proposal model types cannot reach this — they only build proposals.
pub async fn review_and_commit_proposals(
    db: &Db,
    ctx: &CommitContext<'_>,
    proposals: &[MemoryExtractionProposal],
) -> anyhow::Result<CommitReport> {
    let plans = match plan_batch(proposals, ctx) {
        BatchPlan::Aborted(report) => return Ok(report),
        BatchPlan::Ready(plans) => plans,
    };
    let mut outcomes = Vec::with_capacity(plans.len());
    for planned in plans {
        let fact_ref = planned.action.fact_ref();
        let durable_path = planned.action.durable_path();
        let (status, reason, path) = match execute_action(db, ctx, planned.action).await {
            Ok(ExecOutcome::Done) => (CommitStatus::Done, None, Some(durable_path)),
            Ok(ExecOutcome::Skipped(why)) => (CommitStatus::Skipped, Some(why), None),
            Err(e) => (
                CommitStatus::Rejected,
                Some(format!("commit failed: {e}")),
                None,
            ),
        };
        outcomes.push(CommitOutcome {
            kind_token: planned.kind_token,
            status,
            durable_path: path,
            fact_ref,
            reason,
        });
    }
    Ok(CommitReport { outcomes })
}

enum ExecOutcome {
    Done,
    Skipped(String),
}

/// Execute one planned action through the matching state-owner DB API. Each arm is exactly
/// one durable call; fail-closed gates (holder identity, evidence) live in those APIs.
async fn execute_action(
    db: &Db,
    ctx: &CommitContext<'_>,
    action: CommitAction,
) -> anyhow::Result<ExecOutcome> {
    match action {
        CommitAction::WorldFact(fact) | CommitAction::LegacyFact(fact) => {
            db.upsert_memory_fact(&fact).await?;
        }
        CommitAction::PlayerLearned { fact_id, reason } => {
            db.record_revealed_fact(ctx.session_id, ctx.turn_id, &fact_id, reason.as_deref())
                .await?;
        }
        CommitAction::PlayerPartyEdge {
            fact_id,
            state,
            reason,
        } => {
            db.upsert_knowledge_edge_player_party(
                ctx.session_id,
                ctx.turn_id,
                &fact_id,
                state.as_token(),
                reason.as_deref(),
            )
            .await?;
        }
        CommitAction::NpcLearned {
            npc_id,
            fact_id,
            reason,
        } => {
            db.record_npc_learned_fact(
                ctx.session_id,
                ctx.turn_id,
                &npc_id,
                &fact_id,
                reason.as_deref(),
            )
            .await?;
        }
        CommitAction::DurableEdge {
            holder_kind,
            holder_id,
            fact_id,
            state,
            confidence,
            learned_at_turn_id,
            source_event_id,
            reason,
        } => {
            db.upsert_knowledge_edge(KnowledgeEdgeInput {
                session_id: ctx.session_id,
                holder_kind,
                holder_id: &holder_id,
                fact_id: &fact_id,
                knowledge_state: state.as_token(),
                confidence,
                learned_at_turn_id: learned_at_turn_id.as_deref(),
                disclosure_policy: None,
                source_event_id: source_event_id.as_deref(),
                reason: reason.as_deref(),
            })
            .await?;
        }
        CommitAction::Relationship {
            session_id,
            npc_id,
            target,
            delta,
        } => {
            crate::npc_relationship::apply_npc_relationship_delta(
                db,
                &session_id,
                &npc_id,
                target,
                &delta,
            )
            .await?;
        }
        CommitAction::Skip { reason } => return Ok(ExecOutcome::Skipped(reason)),
    }
    Ok(ExecOutcome::Done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx() -> CommitContext<'static> {
        CommitContext {
            session_id: "sess_commit",
            turn_id: "turn_commit",
        }
    }

    fn parse(raw: Value) -> Vec<MemoryExtractionProposal> {
        // Lenient parse keeps only well-formed entries; tests below feed valid JSON and
        // assert the routing, so this mirrors how a real extractor payload arrives.
        proposals_from_json(&raw)
    }

    fn plans_of(raw: Value) -> Vec<PlannedCommit> {
        match plan_batch(&parse(raw), &ctx()) {
            BatchPlan::Ready(p) => p,
            BatchPlan::Aborted(_) => panic!("expected a ready plan"),
        }
    }

    /// Build a parseable list of proposals straight from JSON (bypassing the lenient parser
    /// so an authority-violating-but-otherwise-valid entry still reaches `plan_batch`).
    fn proposals(raw: Value) -> Vec<MemoryExtractionProposal> {
        raw.as_array()
            .unwrap()
            .iter()
            .map(|v| serde_json::from_value(v.clone()).unwrap())
            .collect()
    }

    /// A legacy relationship-triple `MemoryFact` proposal carrying its own session + scope.
    fn legacy_memory_fact(session: &str, scope_type: &str, scope_id: &str) -> Value {
        json!({
            "proposal_kind": "memory_fact",
            "fact": {
                "fact_id": "mf_legacy_1", "session_id": session,
                "scope": {"scope_type": scope_type, "scope_id": scope_id},
                "visibility": "gm_only",
                "subject": "raul", "predicate": "wrote", "object": "letter",
                "summary": "Raul wrote the letter.", "status": "active", "confidence": 0.9,
                "source_event_ids": ["ev1"], "tags": ["relationship"], "importance": 1,
                "turn_id": "turn7",
                "created_at": "2026-06-18T00:00:00Z", "updated_at": "2026-06-18T00:00:00Z"
            }
        })
    }

    #[test]
    fn legacy_memory_fact_session_mismatch_aborts_batch() {
        // A legacy memory_fact naming a different session passes the model's validated()
        // gate but must be rejected by the runtime session-authority guard before any write,
        // aborting the whole batch so the valid sibling cannot write either.
        let batch = proposals(json!([
            {
                "proposal_kind": "world_fact",
                "fact_id": "f_ok", "subject": "a", "predicate": "rel", "object": "b",
                "source_event_ids": ["ev1"]
            },
            legacy_memory_fact("sess_OTHER", "session", "sess_OTHER")
        ]));
        match plan_batch(&batch, &ctx()) {
            BatchPlan::Aborted(report) => {
                assert_eq!(
                    report.rejected(),
                    2,
                    "mismatched legacy fact poisons the batch"
                );
                assert!(report.committed_nothing(), "no durable write planned");
            }
            BatchPlan::Ready(_) => panic!("session mismatch must abort, not be ready"),
        }
    }

    #[test]
    fn legacy_memory_fact_session_scope_mismatch_aborts_batch() {
        // session_id matches the context, but a session-scoped fact whose scope id points at
        // another session is the same authority violation → abort before writes.
        let batch = proposals(json!([legacy_memory_fact(
            "sess_commit",
            "session",
            "sess_OTHER"
        )]));
        assert!(matches!(plan_batch(&batch, &ctx()), BatchPlan::Aborted(_)));
    }

    #[test]
    fn legacy_memory_fact_matching_session_plans_with_context_session() {
        // Matching session → plans a LegacyFact whose session + session-scope id are the
        // authoritative context session.
        let batch = proposals(json!([legacy_memory_fact(
            "sess_commit",
            "session",
            "sess_commit"
        )]));
        match plan_batch(&batch, &ctx()) {
            BatchPlan::Ready(plans) => match &plans[0].action {
                CommitAction::LegacyFact(fact) => {
                    assert_eq!(fact.session_id, "sess_commit");
                    assert_eq!(fact.scope.scope_id, "sess_commit");
                    assert_eq!(fact.fact_id, "mf_legacy_1");
                }
                other => panic!("expected LegacyFact, got {}", other.durable_path()),
            },
            BatchPlan::Aborted(_) => panic!("matching session must plan Ready"),
        }
        // A non-session scope is left intact (only the session id is forced to the context).
        let batch = proposals(json!([legacy_memory_fact(
            "sess_commit",
            "global",
            "global"
        )]));
        match plan_batch(&batch, &ctx()) {
            BatchPlan::Ready(plans) => match &plans[0].action {
                CommitAction::LegacyFact(fact) => {
                    assert_eq!(fact.session_id, "sess_commit");
                    assert_eq!(fact.scope.scope_type, ScopeType::Global);
                    assert_eq!(fact.scope.scope_id, "global");
                }
                other => panic!("expected LegacyFact, got {}", other.durable_path()),
            },
            BatchPlan::Aborted(_) => panic!("matching session must plan Ready"),
        }
    }

    #[test]
    fn world_fact_plans_a_memory_fact_write() {
        let plans = plans_of(json!({"proposals": [{
            "proposal_kind": "world_fact",
            "fact_id": "f_world_1",
            "subject": "raul", "predicate": "wrote", "object": "letter",
            "source_event_ids": ["ev1"]
        }]}));
        assert_eq!(plans.len(), 1);
        match &plans[0].action {
            CommitAction::WorldFact(fact) => {
                assert_eq!(fact.fact_id, "f_world_1");
                assert_eq!(fact.session_id, "sess_commit");
                assert_eq!(fact.scope.scope_type, ScopeType::Session);
                assert_eq!(fact.object, Value::String("letter".into()));
                assert!(fact.tags.contains(&"world_fact".to_string()));
                assert_eq!(fact.confidence, WORLD_FACT_DEFAULT_CONFIDENCE);
            }
            other => panic!("expected WorldFact action, got {}", other.durable_path()),
        }
    }

    #[test]
    fn knowledge_update_routes_by_holder_and_state() {
        // player_party + knows_true → reveal path.
        let p = plans_of(json!({"proposals": [{
            "proposal_kind": "knowledge_update",
            "fact_id": "f1", "holder": {"holder_kind": "player_party"},
            "knowledge_state": "knows_true", "source_event_ids": ["ev1"]
        }]}));
        assert!(matches!(p[0].action, CommitAction::PlayerLearned { .. }));

        // npc + knows_true → NpcLearnedFact path with normalized id.
        let p = plans_of(json!({"proposals": [{
            "proposal_kind": "knowledge_update",
            "fact_id": "f1", "holder": {"holder_kind": "npc", "holder_id": "npc_alice"},
            "knowledge_state": "knows_true", "source_event_ids": ["ev1"]
        }]}));
        match &p[0].action {
            CommitAction::NpcLearned { npc_id, .. } => assert_eq!(npc_id, "npc_alice"),
            _ => panic!("expected NpcLearned"),
        }

        // gm + non-true belief → durable knowledge edge.
        let p = plans_of(json!({"proposals": [{
            "proposal_kind": "knowledge_update",
            "fact_id": "f1", "holder": {"holder_kind": "gm"},
            "knowledge_state": "suspects", "source_event_ids": ["ev1"]
        }]}));
        assert!(matches!(
            p[0].action,
            CommitAction::DurableEdge {
                holder_kind: "gm",
                ..
            }
        ));
    }

    #[test]
    fn pc_and_faction_holders_route_to_durable_edge() {
        // pc / faction holders are now durable: a valid one plans a DurableEdge carrying the
        // normalized id, never a Skip (no dedicated learned-event path, so knows_true too
        // commits straight to a durable edge).
        let p = plans_of(json!({"proposals": [{
            "proposal_kind": "knowledge_update",
            "fact_id": "f1", "holder": {"holder_kind": "pc", "holder_id": "pc_hero"},
            "knowledge_state": "knows_true", "source_event_ids": ["ev1"]
        }]}));
        match &p[0].action {
            CommitAction::DurableEdge {
                holder_kind,
                holder_id,
                ..
            } => {
                assert_eq!(*holder_kind, "pc");
                assert_eq!(holder_id, "pc_hero");
            }
            other => panic!("expected DurableEdge, got {}", other.durable_path()),
        }

        let p = plans_of(json!({"proposals": [{
            "proposal_kind": "knowledge_update",
            "fact_id": "f1", "holder": {"holder_kind": "faction", "holder_id": "guild_thieves"},
            "knowledge_state": "suspects", "source_event_ids": ["ev1"]
        }]}));
        match &p[0].action {
            CommitAction::DurableEdge {
                holder_kind,
                holder_id,
                ..
            } => {
                assert_eq!(*holder_kind, "faction");
                assert_eq!(holder_id, "guild_thieves");
            }
            other => panic!("expected DurableEdge, got {}", other.durable_path()),
        }
    }

    #[test]
    fn pc_faction_unstable_id_aborts_batch_no_write() {
        // An unstable pc/faction id fails the model `validated()` gate (display-name shape),
        // so the whole batch is Aborted before any write — fail-closed preserved.
        for holder in [
            json!({"holder_kind": "pc", "holder_id": "The Hero"}),
            json!({"holder_kind": "faction", "holder_id": "Thieves Guild"}),
        ] {
            let batch = proposals(json!([{
                "proposal_kind": "knowledge_update",
                "fact_id": "f1", "holder": holder,
                "knowledge_state": "knows_true", "source_event_ids": ["ev1"]
            }]));
            match plan_batch(&batch, &ctx()) {
                BatchPlan::Aborted(report) => {
                    assert!(report.committed_nothing(), "unstable id writes nothing");
                    assert_eq!(report.rejected(), 1);
                }
                BatchPlan::Ready(_) => panic!("unstable pc/faction id must abort, not be ready"),
            }
        }
    }

    #[test]
    fn relationship_delta_plans_evidence_gated_apply() {
        let p = plans_of(json!({"proposals": [{
            "proposal_kind": "npc_relationship_delta",
            "session_id": "sess_commit", "npc_id": "npc_lars",
            "target": {"target_kind": "player_party"},
            "delta": {"trust": 10, "evidence_event_ids": ["ev1"]}
        }]}));
        assert!(matches!(p[0].action, CommitAction::Relationship { .. }));
    }

    #[test]
    fn relationship_delta_session_mismatch_aborts_batch() {
        // The commit context is authoritative for the destination session. A relationship
        // proposal naming a different session must abort the whole batch (fail-closed) — even
        // though it passes the model's `validated()` gate — so a proposal can never redirect
        // a write to another session. A valid sibling in the batch commits nothing.
        let proposals: Vec<MemoryExtractionProposal> = json!([
            {
                "proposal_kind": "world_fact",
                "fact_id": "f_ok", "subject": "a", "predicate": "rel", "object": "b",
                "source_event_ids": ["ev1"]
            },
            {
                "proposal_kind": "npc_relationship_delta",
                "session_id": "sess_OTHER", "npc_id": "npc_lars",
                "target": {"target_kind": "player_party"},
                "delta": {"trust": 10, "evidence_event_ids": ["ev1"]}
            }
        ])
        .as_array()
        .unwrap()
        .iter()
        .map(|v| serde_json::from_value(v.clone()).unwrap())
        .collect();
        // ctx session is "sess_commit"; the relationship names "sess_OTHER" → mismatch.
        match plan_batch(&proposals, &ctx()) {
            BatchPlan::Aborted(report) => {
                assert_eq!(
                    report.rejected(),
                    2,
                    "both proposals rejected on session mismatch"
                );
                assert!(report.committed_nothing(), "no durable write planned");
            }
            BatchPlan::Ready(_) => panic!("session mismatch must abort, not be ready"),
        }
        // Sanity: the same relationship with a matching session plans fine.
        let matching: Vec<MemoryExtractionProposal> = json!([{
            "proposal_kind": "npc_relationship_delta",
            "session_id": "sess_commit", "npc_id": "npc_lars",
            "target": {"target_kind": "player_party"},
            "delta": {"trust": 10, "evidence_event_ids": ["ev1"]}
        }])
        .as_array()
        .unwrap()
        .iter()
        .map(|v| serde_json::from_value(v.clone()).unwrap())
        .collect();
        assert!(matches!(plan_batch(&matching, &ctx()), BatchPlan::Ready(_)));
    }

    #[test]
    fn invalid_proposal_batch_commits_nothing() {
        // A batch with one invalid proposal (missing evidence) aborts wholesale: the plan
        // is Aborted, every outcome is Rejected, and there is nothing to execute — proving
        // atomicity without a database.
        let raw = json!({"proposals": [
            {
                "proposal_kind": "world_fact",
                "fact_id": "f_ok", "subject": "a", "predicate": "rel", "object": "b",
                "source_event_ids": ["ev1"]
            },
            {
                "proposal_kind": "world_fact",
                "fact_id": "f_bad", "subject": "a", "predicate": "rel", "object": "b",
                "source_event_ids": []
            }
        ]});
        // try_proposals_from_json fails closed on the bad entry, so plan from the typed
        // batch directly to exercise the in-pipeline gate on a mixed valid/invalid batch.
        let proposals: Vec<MemoryExtractionProposal> = raw["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| serde_json::from_value(v.clone()).unwrap())
            .collect();
        match plan_batch(&proposals, &ctx()) {
            BatchPlan::Aborted(report) => {
                assert_eq!(report.outcomes.len(), 2);
                assert_eq!(report.rejected(), 2, "both proposals rejected");
                assert!(report.committed_nothing(), "no durable write planned");
            }
            BatchPlan::Ready(_) => panic!("invalid batch must abort, not be ready"),
        }
    }
}
