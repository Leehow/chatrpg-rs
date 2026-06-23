//! EV-P5 `progress_post_turn_witness_v1` — the **PostTurnWitnessProducer** (GPT Pro
//! follow-up `design/GPTpro-producer-firing-followup.md` §B "post-turn 抽取 pass" +
//! §运行顺序 step 4). DECISIVE: EV-4/EV-4R/EV-P1/EV-P4 all confirmed the MAIN GM is
//! unreliable at emitting structured side-channel signals inline (progress_claims=0,
//! evidence_audit ~50%, evidence_attempts=0). This slice stops depending on the GM
//! remembering a tag: a SEPARATE focused single-task LLM call, AFTER the turn's
//! checks/commits are final, proposes which *offered* observations actually occurred.
//!
//! Three deterministic guarantees enforced here (the LLM only ever PROPOSES):
//!  1. **WitnessExtractorView** is the ONLY thing the extractor sees — offer meanings +
//!     player intent + committed outcomes + staged event kinds + materialized content
//!     refs. It carries **no objective_id / guard / reward / next-scene / atom_id**, so
//!     the extractor literally cannot judge progression.
//!  2. **Deterministic candidate narrowing FIRST** (Rust, no LLM): an offer is a
//!     *structural candidate* only if this turn committed a `CheckResolved(success|
//!     partial)` AND the offer is a check-bindable (action/state) kind. No candidate ⇒
//!     the offer is NOT shown to the extractor (shrinks the space + prevents fabrication;
//!     fail-closed).
//!  3. **Rust admission** reuses EV-P4 `bind_capability_evidence` (action/state) /
//!     EV-4 `EvidenceGateway::admit` (everything else) UNCHANGED — the extractor's output
//!     is a proposal, never auto-admitted. Still **shadow** (the engine does not consume
//!     the ledger — that is EV-APPLY).

use crate::evidence_binding::{
    bind_capability_evidence, is_check_bindable_kind, is_committed_success_check,
};
use crate::evidence_gateway::{EvidenceGateway, GatewayInputs};
use async_trait::async_trait;
use serde::Serialize;
use std::sync::Arc;
use trpg_llm::{self, LlmClient};
use trpg_model::adventure_ir::{
    CapId, EvidenceAtomCatalog, EvidenceClaim, EvidenceLedger, EvidenceOfferSet,
};
use trpg_model::{DomainEvent, DomainEventKind};

const PROGRESS_POST_TURN_WITNESS_V1_ENV: &str = "TRPG_PROGRESS_POST_TURN_WITNESS_V1";
const PROGRESS_EVIDENCE_V1_ENV: &str = "TRPG_PROGRESS_EVIDENCE_V1";

fn flag_on(raw: &str) -> bool {
    let v = raw.trim().to_ascii_lowercase();
    v == "1" || v == "true" || v == "on"
}

/// Whether the EV-P5 post-turn witness is active. Default OFF == byte-identical
/// baseline (no second LLM call, no prompt, no candidate narrowing). ON when EITHER the
/// master `progress_evidence_v1` OR the `progress_post_turn_witness_v1` slice flag is truthy.
pub fn progress_post_turn_witness_enabled() -> bool {
    std::env::var(PROGRESS_POST_TURN_WITNESS_V1_ENV)
        .map(|v| flag_on(&v))
        .unwrap_or(false)
        || std::env::var(PROGRESS_EVIDENCE_V1_ENV)
            .map(|v| flag_on(&v))
            .unwrap_or(false)
}

/// One committed adjudication outcome the extractor may cite as a basis. Positional
/// `ref_token` (`commit:<i>`) = the index of the event within THIS turn's committed
/// events (the same indexing the gateway resolves a [`trpg_model::adventure_ir::TurnLocalRef`]
/// against). NO guard/objective/reward — only the dice result the player produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommittedOutcome {
    pub ref_token: String,
    pub success: bool,
    pub success_tier: String,
}

/// A summary of a committed domain event (kind only — no payload that could leak an
/// authored fact id / guard / objective). Lets the extractor see "a check resolved /
/// a fact committed" without ever seeing what it advances.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StagedEvent {
    pub kind: String,
}

/// A structurally-narrowed candidate offer shown to the extractor: the opaque cap
/// handle + its human meaning + kind token. NEVER the atom_id / objective_id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CandidateOffer {
    pub cap_id: String,
    pub kind: String,
    pub meaning: String,
}

/// The ONLY thing the focused extractor sees (design follow-up §B). By construction it
/// contains **no** objective_id / guard AST / reward / success-condition / next-scene /
/// atom_id field — so the LLM cannot judge progression; it can only say "this offered
/// observation occurred, cite this committed basis". Serialized verbatim into the
/// focused prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WitnessExtractorView {
    pub player_intent: String,
    pub committed_outcomes: Vec<CommittedOutcome>,
    pub staged_domain_events: Vec<StagedEvent>,
    pub materialized_content_refs: Vec<String>,
    pub candidates: Vec<CandidateOffer>,
}

/// Why (or whether) the post-turn witness fires this turn (the recall backstop trigger).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WitnessTrigger {
    /// Do not run the extractor (no structural candidate, or the GM audit already found
    /// the observations — the witness is a backstop, not a default producer).
    NotTriggered,
    /// The main GM audit was missing/incomplete (a `ProducerProtocolFailure`).
    AuditFailed,
    /// The audit was complete but marked everything not_observed WHILE structural
    /// candidates exist (the GM said "none" but a successful check on an offered atom
    /// happened) — the recall backstop.
    AllNotObservedWithCandidates,
}

impl WitnessTrigger {
    pub fn fires(self) -> bool {
        !matches!(self, WitnessTrigger::NotTriggered)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            WitnessTrigger::NotTriggered => "not_triggered",
            WitnessTrigger::AuditFailed => "audit_failed",
            WitnessTrigger::AllNotObservedWithCandidates => "all_not_observed_with_candidates",
        }
    }
}

/// Decide whether the post-turn witness should run. **Fail-closed**: zero structural
/// candidates ⇒ never run (nothing the extractor could legitimately confirm). With
/// candidates: run when the audit failed the completeness protocol OR the audit was
/// complete but observed nothing (the recall backstop). If the audit already observed
/// ≥1, the witness stays out (it is a backstop, not a second guesser).
pub fn witness_trigger(
    audit_protocol_failed: bool,
    observed: usize,
    candidate_count: usize,
) -> WitnessTrigger {
    if candidate_count == 0 {
        return WitnessTrigger::NotTriggered;
    }
    if audit_protocol_failed {
        return WitnessTrigger::AuditFailed;
    }
    if observed == 0 {
        return WitnessTrigger::AllNotObservedWithCandidates;
    }
    WitnessTrigger::NotTriggered
}

/// Deterministic candidate narrowing (Rust, NO LLM). An offer is a *structural
/// candidate* iff (a) it is a check-bindable (action/state) kind AND (b) this turn
/// committed ≥1 successful/partial `CheckResolved` for this (session, turn). Reuses the
/// SAME predicates as EV-P4 binding so a confirmed candidate is exactly what
/// `bind_capability_evidence` can admit. fail-closed: no committed success ⇒ empty.
pub fn structural_candidates(
    session_id: &str,
    turn_id: &str,
    offer_set: &EvidenceOfferSet,
    committed_events: &[DomainEvent],
) -> Vec<CapId> {
    let has_committed_success = committed_events.iter().any(|e| {
        e.session_id == session_id
            && e.turn_id == turn_id
            && !e.event_id.trim().is_empty()
            && is_committed_success_check(e)
    });
    if !has_committed_success {
        return Vec::new();
    }
    offer_set
        .offers()
        .iter()
        .filter(|o| is_check_bindable_kind(o.kind))
        .map(|o| o.cap_id.clone())
        .collect()
}

/// Build the extractor view from the turn's committed events + the narrowed candidates.
/// Only candidate offers are surfaced (fail-closed: an un-narrowed offer is never shown).
/// committed_outcomes carry the positional `commit:<i>` token so the extractor can cite a
/// real basis; staged_domain_events carry kinds only (no payload leak).
pub fn build_extractor_view(
    player_intent: &str,
    offer_set: &EvidenceOfferSet,
    committed_events: &[DomainEvent],
    materialized_content_refs: Vec<String>,
    candidates: &[CapId],
) -> WitnessExtractorView {
    let committed_outcomes = committed_events
        .iter()
        .enumerate()
        .filter(|(_, e)| is_committed_success_check(e))
        .map(|(i, e)| {
            let outcome = e.data.get("outcome");
            let success = outcome
                .and_then(|o| o.get("success"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let success_tier = outcome
                .and_then(|o| o.get("success_tier"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            CommittedOutcome {
                ref_token: format!("commit:{i}"),
                success,
                success_tier,
            }
        })
        .collect();
    let staged_domain_events = committed_events
        .iter()
        .map(|e| StagedEvent {
            kind: domain_event_kind_token(e.kind),
        })
        .collect();
    let candidates = candidates
        .iter()
        .filter_map(|cap| offer_set.find(cap.as_str()))
        .map(|o| CandidateOffer {
            cap_id: o.cap_id.as_str().to_string(),
            kind: o.kind.as_str().to_string(),
            meaning: o.meaning.clone(),
        })
        .collect();
    WitnessExtractorView {
        player_intent: player_intent.to_string(),
        committed_outcomes,
        staged_domain_events,
        materialized_content_refs,
        candidates,
    }
}

fn domain_event_kind_token(kind: DomainEventKind) -> String {
    format!("{kind:?}")
}

/// One witness admission decision (for shadow logging). `Ok(atom_id)` = admitted
/// (Rust-resolved atom — the extractor never named it); `Err(reason)` = rejected.
pub struct WitnessAdmission {
    pub cap_id: String,
    pub result: Result<String, String>,
}

/// Admit the extractor's PROPOSED claims by REUSING the EV-P4 binding / EV-4 gateway
/// UNCHANGED. A proposal for a check-bindable (action/state) cap routes through
/// `bind_capability_evidence` (basis = the turn's committed success — Rust decides);
/// any other kind routes through `EvidenceGateway::admit` (cap+basis). The extractor's
/// proposal is NEVER auto-admitted — Rust decides. Seeded with the turn's prior ledger
/// (exact + binding + GM-witnessed) so a witness proposal duplicating an already-admitted
/// observation is rejected. Still SHADOW.
pub fn admit_witness_proposals(
    session_id: &str,
    turn_id: &str,
    offer_set: &EvidenceOfferSet,
    catalog: &EvidenceAtomCatalog,
    committed_events: &[DomainEvent],
    proposals: &[EvidenceClaim],
    seed: &EvidenceLedger,
) -> (EvidenceLedger, Vec<WitnessAdmission>) {
    let mut ledger = seed.clone();
    let mut admissions = Vec::with_capacity(proposals.len());

    for claim in proposals {
        let cap_str = claim.cap_id.as_str().to_string();
        let Some(offer) = offer_set.find(&cap_str) else {
            admissions.push(WitnessAdmission {
                cap_id: cap_str,
                result: Err("unknown_capability".into()),
            });
            continue;
        };

        if is_check_bindable_kind(offer.kind) {
            // Reuse EV-P4 binding UNCHANGED: Rust derives the basis from the committed
            // success check (the extractor's cited basis is advisory for action atoms).
            let before = ledger.len();
            let (bound, decisions) = bind_capability_evidence(
                session_id,
                turn_id,
                offer_set,
                catalog,
                committed_events,
                &[claim.cap_id.clone()],
                &ledger,
            );
            ledger = bound;
            let result = match decisions.into_iter().next().map(|d| d.result) {
                Some(Ok(atom)) if ledger.len() > before => Ok(atom),
                Some(Ok(atom)) => Ok(atom),
                Some(Err(reason)) => Err(reason.as_str().to_string()),
                None => Err("no_decision".into()),
            };
            admissions.push(WitnessAdmission {
                cap_id: cap_str,
                result,
            });
        } else {
            // Reuse the EV-4 gateway UNCHANGED for non-check-bindable kinds (cap+basis).
            let result = {
                let inp = GatewayInputs {
                    session_id,
                    turn_id,
                    offer_set,
                    committed_events,
                    catalog,
                    ledger: &ledger,
                };
                EvidenceGateway::admit(claim, &inp)
            };
            match result {
                Ok(ev) => {
                    let atom = ev.atom_id.as_str().to_string();
                    ledger.append(ev);
                    admissions.push(WitnessAdmission {
                        cap_id: cap_str,
                        result: Ok(atom),
                    });
                }
                Err(reason) => admissions.push(WitnessAdmission {
                    cap_id: cap_str,
                    result: Err(reason.as_str().to_string()),
                }),
            }
        }
    }
    (ledger, admissions)
}

/// The focused, single-task witness producer. Given the [`WitnessExtractorView`] it
/// PROPOSES which candidate observations the final committed result actually established
/// — output is `Vec<EvidenceClaim>` (closed schema: cap_id + non-empty basis only). It
/// can never construct a progress conclusion (the schema forbids it) and never sees a
/// guard/objective/reward (the view forbids it). Mocked in unit tests.
#[async_trait]
pub trait EvidenceClaimProducer: Send + Sync {
    async fn propose(&self, view: &WitnessExtractorView) -> Vec<EvidenceClaim>;
}

/// The live LLM-backed witness producer. Reuses the GM LLM client but a SEPARATE minimal
/// single-task prompt (not the giant GM turn prompt). Fail-closed: any malformed claim
/// (extra fields / missing cap / empty basis) is dropped at parse time; a request error
/// ⇒ no proposals.
pub struct LlmWitnessProducer {
    llm: Arc<dyn LlmClient>,
}

impl LlmWitnessProducer {
    pub fn new(llm: Arc<dyn LlmClient>) -> Self {
        Self { llm }
    }
}

/// The minimal focused-witness instruction (no objective/guard/reward; only confirm
/// which CANDIDATE observations the committed result established + cite a committed basis).
pub fn render_witness_prompt() -> String {
    String::from(
        "You are an EVIDENCE WITNESS for a single turn. You are given this turn's player \
         intent, the checks/outcomes that were COMMITTED, and a list of CANDIDATE \
         observations (each an opaque cap handle + a human meaning). For EACH candidate, \
         decide ONLY whether THIS turn's final committed result actually ESTABLISHED it. \
         Return JSON: {\"claims\":[{\"cap_id\":\"<one of the candidate handles>\",\
         \"basis\":[\"commit:<i>\"]}]} citing the committed outcome(s) that establish it. \
         Include ONLY genuinely-established candidates; if none, return {\"claims\":[]}. \
         Do NOT judge any objective, completion, reward, or scene — only which listed \
         candidate observations occurred. Never invent a cap handle not listed.",
    )
}

/// Parse the focused LLM's JSON into closed-schema claims. Fail-closed: a non-object /
/// missing `claims` array / a malformed entry (extra field, missing cap, empty basis) is
/// dropped — never guessed.
pub fn parse_witness_claims(value: &serde_json::Value) -> Vec<EvidenceClaim> {
    let Some(arr) = value.get("claims").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|e| serde_json::from_value::<EvidenceClaim>(e.clone()).ok())
        .collect()
}

#[async_trait]
impl EvidenceClaimProducer for LlmWitnessProducer {
    async fn propose(&self, view: &WitnessExtractorView) -> Vec<EvidenceClaim> {
        if view.candidates.is_empty() {
            return Vec::new();
        }
        let view_json = match serde_json::to_string_pretty(view) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let messages = vec![
            trpg_llm::system(render_witness_prompt()),
            trpg_llm::user(format!("TURN WITNESS INPUT:\n{view_json}")),
        ];
        match self.llm.complete_json(messages, 0.0).await {
            Ok(value) => parse_witness_claims(&value),
            Err(_) => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::adventure_ir::{
        AtomId, BasisKind, EvidenceAtomSpec, EvidenceAuthority, EvidenceKind, EvidenceOffer,
        NonEmpty, ProgressRole, TurnLocalRef,
    };
    use trpg_model::SourceRef;

    const SESSION: &str = "sess_p5";
    const TURN: &str = "turn-uuid-p5-0001";

    fn action_atom(entity: &str) -> EvidenceAtomSpec {
        let atom_id = AtomId::from_parts(
            "the_vault",
            "p8",
            EvidenceKind::ActionResolved,
            &[entity.to_string()],
        );
        EvidenceAtomSpec {
            atom_id,
            kind: EvidenceKind::ActionResolved,
            bindings: vec![entity.to_string(), "scene:scene_001".into()],
            source_refs: vec![SourceRef {
                source_id: "the_vault".into(),
                note: Some("Conduct an experiment.".into()),
                ..Default::default()
            }],
            grounding: format!("action:{entity}"),
            progress_role: ProgressRole::GuardLeaf,
        }
    }

    fn fact_atom(clue: &str) -> EvidenceAtomSpec {
        let atom_id =
            AtomId::from_parts("the_vault", "p9", EvidenceKind::FactLearned, &[clue.to_string()]);
        EvidenceAtomSpec {
            atom_id,
            kind: EvidenceKind::FactLearned,
            bindings: vec![clue.to_string()],
            source_refs: vec![SourceRef {
                source_id: "the_vault".into(),
                page: Some(9),
                ..Default::default()
            }],
            grounding: format!("fact:{clue}"),
            progress_role: ProgressRole::CarrierOnly,
        }
    }

    fn offer_for(atom: EvidenceAtomSpec) -> (EvidenceOfferSet, EvidenceAtomCatalog, CapId) {
        let mut catalog = EvidenceAtomCatalog::new();
        let cap = CapId::from_parts(SESSION, TURN, &atom.atom_id);
        let mut set = EvidenceOfferSet::new(TURN);
        set.push(EvidenceOffer {
            cap_id: cap.clone(),
            atom_id: atom.atom_id.clone(),
            kind: atom.kind,
            meaning: "the player resolved an authored action on the anomaly".into(),
            required_basis: vec![BasisKind::OutcomeCommitted],
            expires_at: TURN.into(),
        });
        catalog.insert(atom);
        (set, catalog, cap)
    }

    fn check_event(event_id: &str, success: bool) -> DomainEvent {
        DomainEvent::new(
            event_id,
            SESSION,
            TURN,
            DomainEventKind::CheckResolved,
            serde_json::json!({"check_id": "chk_1", "outcome": {"success": success, "success_tier": "regular"}}),
        )
    }

    // ─── candidate narrowing ───────────────────────────────────────────────

    #[test]
    fn committed_success_on_action_offer_is_a_structural_candidate() {
        let (set, _catalog, cap) = offer_for(action_atom("npc_fount_anomaly"));
        let events = vec![check_event("de_chk_1", true)];
        let cands = structural_candidates(SESSION, TURN, &set, &events);
        assert_eq!(cands, vec![cap], "a committed success on a check-bindable offer ⇒ candidate");
    }

    #[test]
    fn no_committed_success_yields_no_candidate() {
        // fail-closed: a failed-only check (or no check) ⇒ the extractor gets nothing.
        let (set, _catalog, _cap) = offer_for(action_atom("npc_fount_anomaly"));
        let failed = vec![check_event("de_chk_1", false)];
        assert!(structural_candidates(SESSION, TURN, &set, &failed).is_empty());
        assert!(structural_candidates(SESSION, TURN, &set, &[]).is_empty());
    }

    #[test]
    fn fact_offer_is_not_a_check_candidate_even_with_a_success() {
        // A FactLearned cap has an exact producer, not a check binding ⇒ not narrowed in.
        let (set, _catalog, _cap) = offer_for(fact_atom("clue_aquifer_commercial"));
        let events = vec![check_event("de_chk_1", true)];
        assert!(structural_candidates(SESSION, TURN, &set, &events).is_empty());
    }

    // ─── trigger logic ─────────────────────────────────────────────────────

    #[test]
    fn trigger_fires_on_audit_failure_with_candidates() {
        assert_eq!(witness_trigger(true, 0, 1), WitnessTrigger::AuditFailed);
    }

    #[test]
    fn trigger_fires_when_all_not_observed_but_candidates_exist() {
        assert_eq!(
            witness_trigger(false, 0, 2),
            WitnessTrigger::AllNotObservedWithCandidates
        );
    }

    #[test]
    fn trigger_stays_out_when_audit_already_observed() {
        // The witness is a backstop: if the GM audit already observed ≥1, do not run it.
        assert_eq!(witness_trigger(false, 1, 3), WitnessTrigger::NotTriggered);
    }

    #[test]
    fn trigger_never_fires_without_candidates() {
        // fail-closed: zero candidates ⇒ never run the extractor (nothing to confirm).
        assert_eq!(witness_trigger(true, 0, 0), WitnessTrigger::NotTriggered);
        assert_eq!(witness_trigger(false, 0, 0), WitnessTrigger::NotTriggered);
    }

    // ─── view contains no progression fields ───────────────────────────────

    #[test]
    fn extractor_view_carries_no_objective_guard_or_atom() {
        let (set, _catalog, cap) = offer_for(action_atom("npc_fount_anomaly"));
        let events = vec![check_event("de_chk_1", true)];
        let cands = structural_candidates(SESSION, TURN, &set, &events);
        let view = build_extractor_view(
            "I experiment on the anomaly",
            &set,
            &events,
            vec![],
            &cands,
        );
        // the candidate is surfaced with its meaning + opaque handle …
        assert_eq!(view.candidates.len(), 1);
        assert_eq!(view.candidates[0].cap_id, cap.as_str());
        // … one committed success outcome with a positional commit token …
        assert_eq!(view.committed_outcomes.len(), 1);
        assert_eq!(view.committed_outcomes[0].ref_token, "commit:0");
        // … and the SERIALIZED view leaks no progression vocabulary.
        let json = serde_json::to_string(&view).unwrap();
        for forbidden in [
            "objective", "guard", "reward", "next_scene", "atom:", "atom_id",
            "success_when", "progress_role", set.offers()[0].atom_id.as_str(),
        ] {
            assert!(!json.contains(forbidden), "view leaked `{forbidden}`: {json}");
        }
    }

    // ─── admission reuses EV-P4 binding ────────────────────────────────────

    #[test]
    fn proposed_action_candidate_with_committed_success_admits_one_evidence() {
        let (set, catalog, cap) = offer_for(action_atom("npc_fount_anomaly"));
        let events = vec![check_event("de_chk_1", true)];
        let proposals = vec![EvidenceClaim {
            cap_id: cap.clone(),
            basis: NonEmpty::from_vec(vec![TurnLocalRef::Commit(0)]).unwrap(),
        }];
        let (ledger, adm) = admit_witness_proposals(
            SESSION, TURN, &set, &catalog, &events, &proposals, &EvidenceLedger::new(),
        );
        assert_eq!(ledger.len(), 1, "one admitted AcceptedEvidence via reused EV-P4 binding");
        assert_eq!(ledger.entries()[0].evidence_kind, EvidenceKind::ActionResolved);
        // Rust decided success (ExactDomain), the LLM only proposed which action.
        assert_eq!(ledger.entries()[0].authority, EvidenceAuthority::ExactDomain);
        assert!(adm[0].result.is_ok());
    }

    #[test]
    fn proposed_candidate_without_committed_success_is_not_admitted() {
        // fail-closed at admission too: a proposal whose check did NOT succeed admits nothing.
        let (set, catalog, cap) = offer_for(action_atom("npc_fount_anomaly"));
        let events = vec![check_event("de_chk_1", false)];
        let proposals = vec![EvidenceClaim {
            cap_id: cap,
            basis: NonEmpty::from_vec(vec![TurnLocalRef::Commit(0)]).unwrap(),
        }];
        let (ledger, adm) = admit_witness_proposals(
            SESSION, TURN, &set, &catalog, &events, &proposals, &EvidenceLedger::new(),
        );
        assert_eq!(ledger.len(), 0);
        assert!(adm[0].result.is_err());
    }

    #[test]
    fn forged_cap_proposal_is_rejected() {
        let (set, catalog, _cap) = offer_for(action_atom("npc_fount_anomaly"));
        let events = vec![check_event("de_chk_1", true)];
        let proposals = vec![EvidenceClaim {
            cap_id: CapId("cap_invented".into()),
            basis: NonEmpty::from_vec(vec![TurnLocalRef::Commit(0)]).unwrap(),
        }];
        let (ledger, adm) = admit_witness_proposals(
            SESSION, TURN, &set, &catalog, &events, &proposals, &EvidenceLedger::new(),
        );
        assert_eq!(ledger.len(), 0);
        assert_eq!(adm[0].result, Err("unknown_capability".to_string()));
    }

    // ─── mocked focused producer (extractor mocked, per CL-P5a) ─────────────

    struct MockProducer {
        cap: CapId,
    }
    #[async_trait]
    impl EvidenceClaimProducer for MockProducer {
        async fn propose(&self, view: &WitnessExtractorView) -> Vec<EvidenceClaim> {
            // confirm the single candidate it was shown, citing the committed outcome
            assert_eq!(view.candidates.len(), 1, "extractor only sees narrowed candidates");
            vec![EvidenceClaim {
                cap_id: self.cap.clone(),
                basis: NonEmpty::from_vec(vec![TurnLocalRef::Commit(0)]).unwrap(),
            }]
        }
    }

    #[tokio::test]
    async fn mocked_producer_proposal_flows_to_one_admitted_evidence() {
        let (set, catalog, cap) = offer_for(action_atom("npc_fount_anomaly"));
        let events = vec![check_event("de_chk_1", true)];
        let cands = structural_candidates(SESSION, TURN, &set, &events);
        let view = build_extractor_view("experiment", &set, &events, vec![], &cands);
        let producer = MockProducer { cap: cap.clone() };
        let proposals = producer.propose(&view).await;
        assert_eq!(proposals.len(), 1, "the focused extractor proposed the candidate");
        let (ledger, _adm) = admit_witness_proposals(
            SESSION, TURN, &set, &catalog, &events, &proposals, &EvidenceLedger::new(),
        );
        assert_eq!(ledger.len(), 1, "proposal → reused admission → one AcceptedEvidence");
    }

    #[test]
    fn parse_witness_claims_is_fail_closed() {
        // well-formed
        let v = serde_json::json!({"claims": [{"cap_id": "cap_a", "basis": ["commit:0"]}]});
        assert_eq!(parse_witness_claims(&v).len(), 1);
        // no `claims` ⇒ empty; non-array ⇒ empty
        assert!(parse_witness_claims(&serde_json::json!({"x": 1})).is_empty());
        assert!(parse_witness_claims(&serde_json::json!({"claims": "no"})).is_empty());
        // forged entry (a progress conclusion smuggled in) + empty basis are dropped
        let forged = serde_json::json!({"claims": [
            {"cap_id": "cap_ok", "basis": ["commit:0"]},
            {"cap_id": "cap_bad", "basis": ["commit:0"], "completed": true},
            {"cap_id": "cap_empty", "basis": []}
        ]});
        let claims = parse_witness_claims(&forged);
        assert_eq!(claims.len(), 1, "only the closed-schema, non-empty-basis claim survives");
        assert_eq!(claims[0].cap_id.as_str(), "cap_ok");
    }

    #[test]
    fn flag_defaults_off_and_prompt_never_leaks() {
        assert!(!flag_on("0") && !flag_on("false") && !flag_on("off") && !flag_on(""));
        assert!(flag_on("1") && flag_on("true") && flag_on("on"));
        let p = render_witness_prompt();
        assert!(p.contains("cap_id") && p.contains("basis"));
        assert!(!p.contains("atom:"), "no atom id leaked");
        assert!(!p.contains("obj."), "no objective id slug leaked");
    }
}
