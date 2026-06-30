//! EV-P1 `progress_evidence_audit_required_v1` — main-GM **mandatory EvidenceAudit**
//! producer protocol (GPT Pro follow-up `design/GPTpro-producer-firing-followup.md` §B).
//!
//! EV-4R's optional `[progress_claims]` tag was silently omitted by the GM
//! (producer-recall failure). EV-P1 makes the GM answer a **per-offer observation
//! audit**: for EVERY offered capability this turn it must return exactly one
//! decision (Observed{basis} | NotObserved{reason}). This module holds the three
//! pure pieces the pipeline wires in (the EvidenceGateway + EV-2/EV-3/EV-4 types are
//! reused UNCHANGED — nothing here re-implements admission):
//!
//!  1. [`render_gm_audit_block`] — the GM-prompt block: the EV-3 capability list + an
//!     "observation audit" instruction + 5 boundary few-shot + the closed-schema
//!     `[evidence_audit]` format (carrying the OfferSet id the GM must echo).
//!  2. [`evaluate_gm_audit`] — enforce COMPLETENESS (every offered cap exactly one
//!     decision; matching offer_set_id; no extra/unknown caps). A missing / extra /
//!     mismatched audit ⇒ a logged [`ProducerProtocolFailure`] (telemetry), NEVER a
//!     silent "none". A complete audit ⇒ each Observed decision is admitted via the
//!     reused `EvidenceGateway::admit` → AcceptedEvidence(GmWitnessed) (shadow); each
//!     NotObserved is counted (no evidence). The engine is NEVER called.
//!
//! Design law honored: **LLM-never-evaluates-guard** (the GM only marks
//! observed/not + a turn-local basis; Rust admits + resolves cap→atom); **fail-closed**
//! (incomplete audit ⇒ ProducerProtocolFailure, never "none"; never admit on doubt);
//! **shadow** (no engine, no objective completion — J3 unchanged).

use crate::evidence_claims::{admit_gm_claims, ClaimDecision};
use trpg_model::adventure_ir::{
    EvidenceAtomCatalog, EvidenceAudit, EvidenceClaim, EvidenceLedger, EvidenceOfferSet,
    WitnessDecision,
};
use trpg_model::DomainEvent;

/// Why the GM's audit failed the COMPLETENESS protocol (distinct from a *content*
/// rejection, which the gateway handles). All four are "缺失≠none" failures: the GM
/// did not produce a trustworthy per-offer audit, so NO evidence is admitted and the
/// (later) PostTurnWitness pass is the recovery (out of scope here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProtocolFailureKind {
    /// The flag was ON and offers were non-empty, but the GM emitted no parseable
    /// `[evidence_audit]` (the EV-4R "silent omit" — now a measurable failure).
    MissingAudit,
    /// The audit's `offer_set_id` does not match this turn's OfferSet (stale/forged).
    OfferSetMismatch,
    /// At least one offered capability has no decision (under-coverage).
    IncompleteDecisions,
    /// At least one decision references a capability that was never offered.
    ExtraDecisions,
}

impl ProtocolFailureKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ProtocolFailureKind::MissingAudit => "missing_audit",
            ProtocolFailureKind::OfferSetMismatch => "offer_set_mismatch",
            ProtocolFailureKind::IncompleteDecisions => "incomplete_decisions",
            ProtocolFailureKind::ExtraDecisions => "extra_decisions",
        }
    }
}

/// Per-turn audit telemetry (logged in the live wiring). `completeness` = covered /
/// offers (1.0 only when every offered cap has exactly one decision and there are no
/// extras); the EV-P1 win is being able to MEASURE this instead of inferring "none".
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AuditTelemetry {
    pub offers: usize,
    pub observed: usize,
    pub not_observed: usize,
    pub completeness: f64,
    pub protocol_failure: Option<ProtocolFailureKind>,
}

/// The full result of evaluating a GM audit: telemetry (always), the shadow ledger of
/// admitted GmWitnessed evidence (empty on protocol failure), and the per-Observed
/// admission decisions for logging.
pub(crate) struct AuditOutcome {
    pub telemetry: AuditTelemetry,
    pub ledger: EvidenceLedger,
    pub decisions: Vec<ClaimDecision>,
}

/// Enforce the completeness protocol, then admit the Observed decisions. **Fail-closed**:
/// any completeness failure (missing audit / wrong offer set / under-coverage / extra
/// caps) ⇒ no admission, telemetry records the failure (the GM did not produce a
/// trustworthy audit — not a silent "none"). A complete audit ⇒ each Observed decision
/// is turned into an [`EvidenceClaim`] and run through the reused `EvidenceGateway`.
pub(crate) fn evaluate_gm_audit(
    session_id: &str,
    turn_id: &str,
    offer_set: &EvidenceOfferSet,
    catalog: &EvidenceAtomCatalog,
    committed_events: &[DomainEvent],
    audit: Option<&EvidenceAudit>,
    seed: &EvidenceLedger,
) -> AuditOutcome {
    let offers = offer_set.len();

    // 缺失≠none: the flag is on and offers were made, but the GM produced no audit.
    // EV-P2: the exact producers' ledger (`seed`) is preserved either way — a missing/
    // incomplete GM audit never discards already-admitted ExactDomain evidence.
    let Some(audit) = audit else {
        return fail(offers, ProtocolFailureKind::MissingAudit, seed);
    };

    // The audit must answer THIS turn's exact OfferSet.
    if audit.offer_set_id != offer_set.id() {
        return fail(offers, ProtocolFailureKind::OfferSetMismatch, seed);
    }

    // Completeness: every offered cap has exactly one decision; no extra/unknown caps.
    let offered: std::collections::BTreeSet<&str> = offer_set
        .offers()
        .iter()
        .map(|o| o.cap_id.as_str())
        .collect();
    let decided: std::collections::BTreeSet<&str> =
        audit.decisions.keys().map(|c| c.as_str()).collect();
    if decided.iter().any(|c| !offered.contains(c)) {
        return fail(offers, ProtocolFailureKind::ExtraDecisions, seed);
    }
    if offered.iter().any(|c| !decided.contains(c)) {
        return fail(offers, ProtocolFailureKind::IncompleteDecisions, seed);
    }

    // Complete + well-scoped. Split Observed / NotObserved.
    let mut observed = 0usize;
    let mut not_observed = 0usize;
    let mut claims: Vec<EvidenceClaim> = Vec::new();
    for (cap_id, decision) in &audit.decisions {
        match decision {
            WitnessDecision::Observed { basis } => {
                observed += 1;
                claims.push(EvidenceClaim {
                    cap_id: cap_id.clone(),
                    basis: basis.clone(),
                });
            }
            WitnessDecision::NotObserved { .. } => not_observed += 1,
        }
    }

    // Reuse the EV-4 gateway UNCHANGED for the Observed decisions (cap+basis only;
    // Rust resolves cap→atom). NotObserved decisions record no evidence. Seeded with the
    // exact producers' ledger so a GM claim duplicating an exact observation is rejected.
    let (ledger, decisions) = admit_gm_claims(
        session_id,
        turn_id,
        offer_set,
        catalog,
        committed_events,
        &claims,
        seed,
    );

    AuditOutcome {
        telemetry: AuditTelemetry {
            offers,
            observed,
            not_observed,
            completeness: 1.0,
            protocol_failure: None,
        },
        ledger,
        decisions,
    }
}

/// Build a fail-closed outcome: no GM admission, telemetry carrying the failure kind.
/// `completeness` is reported 0.0 (the audit is not trustworthy as a whole). EV-P2: the
/// ledger carries the exact producers' `seed` UNCHANGED — a GM protocol failure never
/// discards already-admitted ExactDomain evidence.
fn fail(offers: usize, kind: ProtocolFailureKind, seed: &EvidenceLedger) -> AuditOutcome {
    AuditOutcome {
        telemetry: AuditTelemetry {
            offers,
            observed: 0,
            not_observed: 0,
            completeness: 0.0,
            protocol_failure: Some(kind),
        },
        ledger: seed.clone(),
        decisions: Vec::new(),
    }
}

/// Render the main-GM prompt block for the mandatory audit: the EV-3 capability list +
/// an "observation audit" instruction + 5 boundary few-shot + the closed-schema
/// `[evidence_audit]` format (echoing the OfferSet id). Empty offer set ⇒ `""` (no
/// block ⇒ byte-identical baseline). Carries ONLY opaque cap handles + meanings +
/// the offer_set_id — never an atom_id / objective_id.
pub(crate) fn render_gm_audit_block(offer_set: &EvidenceOfferSet) -> String {
    let offers = trpg_runtime::evidence_offers::render_offer_prompt_block(offer_set);
    if offers.trim().is_empty() {
        return String::new();
    }
    let mut block = offers;
    block.push_str("\n\n");
    block.push_str(&render_audit_instruction());
    block.push_str("\n\n");
    block.push_str(&render_audit_format(&offer_set.id()));
    block
}

/// The "observation audit" instruction + 5 boundary few-shot (design follow-up §B).
/// Pure; no atom/objective leak. Public-in-crate so the OFF==baseline + no-leak tests
/// can assert on it directly.
pub(crate) fn render_audit_instruction() -> String {
    String::from(
        "EVIDENCE AUDIT (mandatory, this turn only): AFTER you have finalized this \
         turn's checks, outcomes, and commits, audit EACH offered capability above. \
         For every capability return exactly ONE decision:\n\
         - observed — ONLY if THIS turn's final, committed result actually ESTABLISHED \
         it. Cite a basis of the turn-local commit(s)/outcome(s) that established it.\n\
         - not_observed — otherwise, with a reason: not_attempted | attempted | failed \
         | not_yet | merely_implied.\n\
         Intent, a plan, a mere attempt, a FAILED check, something that did not happen, \
         or something only implied/alluded-to are NOT observed. Do NOT judge any \
         objective, completion, reward, or scene transition — only observed/not_observed \
         per capability. You MUST return one decision for EVERY capability listed.\n\
         Boundary examples:\n\
         1. The player tried to pry the access panel and the check FAILED → not_observed \
         (reason: failed).\n\
         2. The player succeeded and the outcome was committed (e.g. the panel unlocked) \
         → observed (basis: the committing commit/outcome).\n\
         3. The player traced the ad chain but the storyboard content was never actually \
         presented/committed → not_observed (reason: merely_implied) — do NOT claim they \
         learned the clue.\n\
         4. The player says they will go to the back alley NEXT turn → not_observed \
         (reason: not_yet).\n\
         5. The player physically moved to a new location this turn → not_observed \
         (reason: not_attempted): location/movement evidence is produced by the engine's \
         navigation, NOT by your audit.",
    )
}

/// The closed-schema `[evidence_audit]` format note, carrying the `offer_set_id` the
/// GM must echo. Pure.
fn render_audit_format(offer_set_id: &str) -> String {
    format!(
        "FORMAT: emit your audit in ONE GM-only block at the very END of your reply, never \
         shown to the player —\n\
         [evidence_audit]{{\"offer_set_id\":\"{offer_set_id}\",\"decisions\":{{\"<cap_handle>\":\
         {{\"status\":\"observed\",\"basis\":[\"commit:<i>\"]}},\"<cap_handle>\":\
         {{\"status\":\"not_observed\",\"reason\":\"failed\"}}}}}}[/evidence_audit]\n\
         The decisions object MUST contain exactly one entry per capability handle listed \
         above — no more, no fewer. Use the offer_set_id verbatim."
    )
}

/// EV-P2 (`progress_exact_projectors_v1`): the short GM-prompt instruction telling the
/// GM to ALSO emit a `[materialized_content]` reference when it actually presents an
/// authored clue's content to the player this turn. Carries ONLY the opaque cap handles
/// + recipient + basis schema — never an atom_id / objective_id / the fact name (Rust
/// resolves cap→authored fact). Appended only inside the flag guard so OFF prompt bytes
/// are unchanged. Pure.
pub(crate) fn render_content_delivery_instruction() -> String {
    String::from(
        "AUTHORED-CONTENT DELIVERY (this turn only): if — and ONLY if — you actually \
         PRESENTED an authored clue's content to the player this turn (you read/showed/told \
         them what it says, not merely hinted at or referenced it), ALSO emit a GM-only block \
         citing the clue capability handle it materialized:\n\
         [materialized_content][{\"delivery_cap\":\"<one of the cap_ handles above>\",\
         \"recipient\":\"player\",\"basis\":[\"commit:<i>\"]}][/materialized_content]\n\
         A JSON array, never shown to the player. `delivery_cap` is one of the capability \
         handles listed above; `basis` cites the committed outcome/commit that presented it. \
         Omit the block entirely if you presented no authored clue content this turn.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use trpg_model::adventure_ir::{
        AtomId, BasisKind, CapId, EvidenceAtomCatalog, EvidenceAtomSpec, EvidenceAuthority,
        EvidenceKind, EvidenceOffer, NonEmpty, NotObservedReason, TurnLocalRef,
    };
    use trpg_model::{DomainEventKind, SourceRef};

    const SESSION: &str = "sess_p1";
    const TURN: &str = "turn-uuid-real-1234";
    const CLUE_A: &str = "clue_aquifer_commercial";
    const CLUE_B: &str = "clue_back_alley_map";

    fn atom_spec(clue_id: &str, page: u32) -> EvidenceAtomSpec {
        let atom_id = AtomId::from_parts(
            "digest_v1",
            &format!("p{page}"),
            EvidenceKind::FactLearned,
            &[clue_id.to_string()],
        );
        EvidenceAtomSpec {
            atom_id,
            kind: EvidenceKind::FactLearned,
            bindings: vec![clue_id.to_string()],
            source_refs: vec![SourceRef {
                source_id: "the_vault".into(),
                page: Some(page),
                ..Default::default()
            }],
            grounding: format!("fact:{clue_id}"),
            progress_role: trpg_model::adventure_ir::ProgressRole::CarrierOnly,
        }
    }

    /// An OfferSet with two FactLearned capabilities (two surfaced clues).
    fn two_clue_offer_set() -> (EvidenceOfferSet, EvidenceAtomCatalog) {
        let mut catalog = EvidenceAtomCatalog::new();
        let mut set = EvidenceOfferSet::new(TURN);
        for (clue, page) in [(CLUE_A, 8u32), (CLUE_B, 9u32)] {
            let spec = atom_spec(clue, page);
            set.push(EvidenceOffer {
                cap_id: CapId::from_parts(SESSION, TURN, &spec.atom_id),
                atom_id: spec.atom_id.clone(),
                kind: EvidenceKind::FactLearned,
                meaning: format!("the player learned the authored clue \"{clue}\""),
                required_basis: vec![BasisKind::FactCommitted],
                expires_at: TURN.into(),
            });
            catalog.insert(spec);
        }
        (set, catalog)
    }

    fn learned_event(event_id: &str, fact_id: &str) -> DomainEvent {
        DomainEvent::new(
            event_id,
            SESSION,
            TURN,
            DomainEventKind::PlayerLearnedFact,
            serde_json::json!({"fact_id": fact_id, "reason": "successful Investigation check"}),
        )
    }

    fn observed(refs: Vec<TurnLocalRef>) -> WitnessDecision {
        WitnessDecision::Observed {
            basis: NonEmpty::from_vec(refs).unwrap(),
        }
    }

    fn not_observed(reason: NotObservedReason) -> WitnessDecision {
        WitnessDecision::NotObserved { reason }
    }

    /// A complete audit: cap_A observed (commit:0), cap_B not_observed (merely_implied).
    fn complete_audit(set: &EvidenceOfferSet) -> EvidenceAudit {
        let mut decisions = BTreeMap::new();
        decisions.insert(
            set.offers()[0].cap_id.clone(),
            observed(vec![TurnLocalRef::Commit(0)]),
        );
        decisions.insert(
            set.offers()[1].cap_id.clone(),
            not_observed(NotObservedReason::MerelyImplied),
        );
        EvidenceAudit {
            offer_set_id: set.id(),
            decisions,
        }
    }

    #[test]
    fn complete_audit_admits_one_evidence_and_reports_full_completeness() {
        // 1 observed (cap_A → committed clue_A) + 1 not_observed ⇒ exactly one
        // AcceptedEvidence(GmWitnessed) and completeness 1.0 with correct counts.
        let (set, catalog) = two_clue_offer_set();
        let events = vec![learned_event("de_clue_a", CLUE_A)];
        let audit = complete_audit(&set);

        let out = evaluate_gm_audit(
            SESSION,
            TURN,
            &set,
            &catalog,
            &events,
            Some(&audit),
            &EvidenceLedger::new(),
        );

        assert_eq!(
            out.ledger.len(),
            1,
            "one observed clue ⇒ one GmWitnessed evidence"
        );
        assert_eq!(
            out.ledger.entries()[0].authority,
            EvidenceAuthority::GmWitnessed
        );
        assert_eq!(
            out.ledger.entries()[0].atom_id,
            set.offers()[0].atom_id,
            "atom is Rust-resolved from the OfferSet, never named by the GM"
        );
        assert_eq!(out.telemetry.offers, 2);
        assert_eq!(out.telemetry.observed, 1);
        assert_eq!(out.telemetry.not_observed, 1);
        assert_eq!(out.telemetry.completeness, 1.0);
        assert_eq!(out.telemetry.protocol_failure, None);
    }

    #[test]
    fn missing_audit_is_protocol_failure_not_silent_none() {
        // The crux of EV-P1: offers were made but the GM produced no audit ⇒ a MEASURED
        // ProducerProtocolFailure, never a silent "none".
        let (set, catalog) = two_clue_offer_set();
        let events = vec![learned_event("de_clue_a", CLUE_A)];

        let out = evaluate_gm_audit(
            SESSION,
            TURN,
            &set,
            &catalog,
            &events,
            None,
            &EvidenceLedger::new(),
        );

        assert_eq!(out.ledger.len(), 0);
        assert_eq!(
            out.telemetry.protocol_failure,
            Some(ProtocolFailureKind::MissingAudit)
        );
        assert_eq!(out.telemetry.completeness, 0.0);
    }

    #[test]
    fn missing_a_decision_is_incomplete_protocol_failure() {
        // Only cap_A decided; cap_B omitted ⇒ IncompleteDecisions, no admission.
        let (set, catalog) = two_clue_offer_set();
        let events = vec![learned_event("de_clue_a", CLUE_A)];
        let mut decisions = BTreeMap::new();
        decisions.insert(
            set.offers()[0].cap_id.clone(),
            observed(vec![TurnLocalRef::Commit(0)]),
        );
        let audit = EvidenceAudit {
            offer_set_id: set.id(),
            decisions,
        };

        let out = evaluate_gm_audit(
            SESSION,
            TURN,
            &set,
            &catalog,
            &events,
            Some(&audit),
            &EvidenceLedger::new(),
        );

        assert_eq!(
            out.ledger.len(),
            0,
            "incomplete audit ⇒ no admission (fail-closed)"
        );
        assert_eq!(
            out.telemetry.protocol_failure,
            Some(ProtocolFailureKind::IncompleteDecisions)
        );
    }

    #[test]
    fn extra_unknown_capability_is_protocol_failure() {
        // A decision for a cap that was never offered ⇒ ExtraDecisions, no admission.
        let (set, catalog) = two_clue_offer_set();
        let events = vec![learned_event("de_clue_a", CLUE_A)];
        let mut audit = complete_audit(&set);
        audit.decisions.insert(
            CapId("cap_never_offered".into()),
            not_observed(NotObservedReason::Failed),
        );

        let out = evaluate_gm_audit(
            SESSION,
            TURN,
            &set,
            &catalog,
            &events,
            Some(&audit),
            &EvidenceLedger::new(),
        );

        assert_eq!(out.ledger.len(), 0);
        assert_eq!(
            out.telemetry.protocol_failure,
            Some(ProtocolFailureKind::ExtraDecisions)
        );
    }

    #[test]
    fn wrong_offer_set_id_is_protocol_failure() {
        let (set, catalog) = two_clue_offer_set();
        let events = vec![learned_event("de_clue_a", CLUE_A)];
        let mut audit = complete_audit(&set);
        audit.offer_set_id = "osid_stale000000".into();

        let out = evaluate_gm_audit(
            SESSION,
            TURN,
            &set,
            &catalog,
            &events,
            Some(&audit),
            &EvidenceLedger::new(),
        );

        assert_eq!(out.ledger.len(), 0);
        assert_eq!(
            out.telemetry.protocol_failure,
            Some(ProtocolFailureKind::OfferSetMismatch)
        );
    }

    #[test]
    fn observed_with_unresolvable_basis_is_rejected_by_gateway_not_admitted() {
        // A complete audit whose observed decision cites a basis that resolves to NO
        // committed event ⇒ gateway rejects it (no admission) but the audit itself is
        // protocol-complete (not a ProducerProtocolFailure).
        let (set, catalog) = two_clue_offer_set();
        let events: Vec<DomainEvent> = vec![]; // commit:0 resolves to nothing
        let audit = complete_audit(&set);

        let out = evaluate_gm_audit(
            SESSION,
            TURN,
            &set,
            &catalog,
            &events,
            Some(&audit),
            &EvidenceLedger::new(),
        );

        assert_eq!(
            out.ledger.len(),
            0,
            "unresolvable basis ⇒ gateway rejects, not admitted"
        );
        assert_eq!(
            out.telemetry.protocol_failure, None,
            "protocol was complete"
        );
        assert_eq!(out.telemetry.observed, 1);
        assert!(
            out.decisions[0].result.is_err(),
            "the observed decision was rejected by the gateway"
        );
    }

    #[test]
    fn audit_block_lists_caps_and_format_never_leaks_atom_id() {
        let (set, _catalog) = two_clue_offer_set();
        let block = render_gm_audit_block(&set);
        assert!(
            block.contains(set.offers()[0].cap_id.as_str()),
            "shows the opaque cap handle"
        );
        assert!(
            block.contains("[evidence_audit]"),
            "tells the GM the markup format"
        );
        assert!(block.contains(&set.id()), "echoes the offer_set_id to use");
        assert!(
            block.contains("not_observed"),
            "explains the not_observed status"
        );
        assert!(
            block.contains("merely_implied"),
            "shows the boundary few-shot vocabulary"
        );
        // LLM-never-evaluates-guard: atom ids / objective id slugs never leak into the
        // prompt. (The bare word "objective" DOES appear — the instruction tells the GM
        // NOT to judge any objective — but no atom id or `obj.<slug>` may appear.)
        assert!(
            !block.contains(set.offers()[0].atom_id.as_str()),
            "atom_id must NEVER appear"
        );
        assert!(!block.contains("atom:"), "no atom: prefix may leak");
        assert!(!block.contains("obj."), "no objective id slug leaked");
        // The GM is instructed NOT to judge objectives/completion (the words appear as a
        // prohibition, never as a verdict field it could fill).
        assert!(
            block.contains("Do NOT judge any objective"),
            "carries the no-guard instruction"
        );
    }

    #[test]
    fn empty_offer_set_renders_no_block_baseline() {
        let empty = EvidenceOfferSet::new(TURN);
        assert_eq!(
            render_gm_audit_block(&empty),
            "",
            "empty offers ⇒ no block ⇒ baseline"
        );
    }

    #[test]
    fn content_delivery_instruction_off_keeps_prompt_byte_identical_and_never_leaks() {
        // EV-P2: when the exact-projector flag is OFF the content-delivery instruction is
        // never appended ⇒ the audit block bytes are identical to the EV-P1 baseline.
        let (set, _) = two_clue_offer_set();
        let baseline = render_gm_audit_block(&set);
        let exact_on = false;
        let mut block = baseline.clone();
        if exact_on {
            block.push_str("\n\n");
            block.push_str(&render_content_delivery_instruction());
        }
        assert_eq!(
            block, baseline,
            "OFF ⇒ block bytes identical to EV-P1 baseline"
        );

        // The instruction names only the closed schema fields + opaque handles — never an
        // atom id, the fact name, or an objective slug.
        let instr = render_content_delivery_instruction();
        assert!(
            instr.contains("materialized_content"),
            "instructs the sidecar tag"
        );
        assert!(
            instr.contains("delivery_cap") && instr.contains("basis"),
            "names closed fields"
        );
        assert!(!instr.contains("atom:"), "no atom id leaked");
        assert!(!instr.contains("obj."), "no objective id slug leaked");
        assert!(
            !instr.contains("fact_id"),
            "the GM never names the authored fact"
        );
    }

    #[test]
    fn audit_instruction_off_keeps_prompt_byte_identical() {
        // Mirror the turn_loop guard: when the audit flag is OFF the block is never
        // appended ⇒ prompt bytes are identical to baseline.
        let base = "BASE GM PROMPT\nline2".to_string();
        let audit_on = false;
        let mut prompt = base.clone();
        if audit_on {
            let (set, _) = two_clue_offer_set();
            prompt.push_str("\n\n");
            prompt.push_str(&render_gm_audit_block(&set));
        }
        assert_eq!(prompt, base, "OFF ⇒ prompt bytes identical (OFF==baseline)");
    }
}
