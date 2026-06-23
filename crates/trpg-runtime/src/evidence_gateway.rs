//! EV-4 `progress_claims_shadow_v1` — the **EvidenceGateway** + the GM-claim
//! parser (GPT Pro design `GPTpro-progression-evidence-layer.md` §3 GM
//! TurnDocument + §5 EvidenceGateway). This is the slice that closes the
//! GM↔Rust loop for the witnessed path, in **shadow**.
//!
//! The GM (shown EV-3's capability offers) returns a structured `progress_claims`
//! sidecar of [`EvidenceClaim`]s. Rust:
//! 1. [`parse_progress_claims`] — reads the closed-schema sidecar (fail-closed; a
//!    forged field / malformed ref is dropped, never guessed).
//! 2. [`EvidenceGateway::admit`] — admits each claim against THIS turn's
//!    [`EvidenceOfferSet`] + committed [`DomainEvent`]s + the
//!    [`EvidenceAtomCatalog`], with the **fixed rejection order** (design §5):
//!    `UnknownCapability → Expired → WrongTurnOrSession → UnresolvedBasis →
//!    UncommittedBasis → WrongBasisEventKind → BindingMismatch → CausationMismatch
//!    → SourceHashMismatch → DuplicateEvidence → Accepted`.
//!
//! Design law honored:
//! - **LLM never evaluates a guard**: the claim carries only `cap_id` + a basis of
//!   turn-local refs. Rust maps `cap → atom` via the OfferSet — **the LLM never
//!   names the atom**; the admitted [`AcceptedEvidence`]'s `atom_id` comes from the
//!   offer/catalog, not the claim.
//! - **fail-closed**: every admission check, on any doubt, rejects with the first
//!   applicable reason; a missing/forged/expired/duplicate/wrong-bound claim is
//!   never admitted.
//! - **shadow**: the live wiring (`scene_navigation::tiered`, flag-gated default
//!   OFF) only *logs* each admission decision; it does NOT call the
//!   ProgressionEngine and CANNOT complete an objective. The resulting
//!   [`AcceptedEvidence`] (authority [`EvidenceAuthority::GmWitnessed`]) coexists in
//!   the same ledger type as EV-2's `ExactDomain` evidence (engine consumes neither
//!   yet — that is EV-6).
//! - **OFF == byte-identical baseline**: [`progress_claims_shadow_enabled`] is
//!   default OFF; the live block parses nothing and mutates no prompt when off.

use serde_json::Value;
use trpg_model::adventure_ir::{
    AcceptedEvidence, AtomId, BasisKind, CapId, EvidenceAtomCatalog, EvidenceAuthority,
    EvidenceClaim, EvidenceLedger, EvidenceOffer, EvidenceOfferSet,
};
use trpg_model::{DomainEvent, DomainEventKind};

const PROGRESS_CLAIMS_SHADOW_V1_ENV: &str = "TRPG_PROGRESS_CLAIMS_SHADOW_V1";
const PROGRESS_CLAIMS_ON_GM_V1_ENV: &str = "TRPG_PROGRESS_CLAIMS_ON_GM_V1";
const PROGRESS_EVIDENCE_AUDIT_REQUIRED_V1_ENV: &str = "TRPG_PROGRESS_EVIDENCE_AUDIT_REQUIRED_V1";
const PROGRESS_EVIDENCE_V1_ENV: &str = "TRPG_PROGRESS_EVIDENCE_V1";

/// Pure flag parse (env-race-free; mirrors the EV-2/EV-3 flags).
fn flag_on(raw: &str) -> bool {
    let v = raw.trim().to_ascii_lowercase();
    v == "1" || v == "true" || v == "on"
}

/// Whether the EV-4 claim parsing + shadow admission is active. Default OFF ==
/// byte-identical baseline. ON when EITHER the master `progress_evidence_v1` OR the
/// `progress_claims_shadow_v1` slice flag is truthy.
pub fn progress_claims_shadow_enabled() -> bool {
    std::env::var(PROGRESS_CLAIMS_SHADOW_V1_ENV)
        .map(|v| flag_on(&v))
        .unwrap_or(false)
        || std::env::var(PROGRESS_EVIDENCE_V1_ENV)
            .map(|v| flag_on(&v))
            .unwrap_or(false)
}

/// EV-4R: whether the MAIN GM claim loop (`progress_claims_on_gm_v1`) is active —
/// turn-start OfferSet derivation + capability/claim injection into the main GM prompt +
/// `[progress_claims]` parsing + reused gateway admission (still shadow). Default OFF ==
/// byte-identical baseline (the main GM prompt + output schema + RNG are unchanged). ON when
/// EITHER the master `progress_evidence_v1` OR the `progress_claims_on_gm_v1` slice flag is
/// truthy. Distinct from `progress_claims_shadow_enabled` (the retired scene-nav wiring): the
/// claim loop now lives in ONE place (the main GM adjudication), never the navigation LLM.
pub fn progress_claims_on_gm_enabled() -> bool {
    std::env::var(PROGRESS_CLAIMS_ON_GM_V1_ENV)
        .map(|v| flag_on(&v))
        .unwrap_or(false)
        || std::env::var(PROGRESS_EVIDENCE_V1_ENV)
            .map(|v| flag_on(&v))
            .unwrap_or(false)
}

/// EV-P1: whether the MAIN GM **mandatory EvidenceAudit** protocol
/// (`progress_evidence_audit_required_v1`) is active — the GM must return exactly one
/// observed/not_observed decision for EVERY offered capability (a missing/extra/
/// malformed decision is a `ProducerProtocolFailure`, never a silent "none"). When
/// this is ON it SUPERSEDES the EV-4R optional `[progress_claims]` path (the claim
/// loop lives in ONE place — the audit). Default OFF == byte-identical baseline (the
/// main GM prompt + output schema + RNG are unchanged). ON when EITHER the master
/// `progress_evidence_v1` OR the `progress_evidence_audit_required_v1` slice flag is
/// truthy.
pub fn progress_evidence_audit_required_enabled() -> bool {
    std::env::var(PROGRESS_EVIDENCE_AUDIT_REQUIRED_V1_ENV)
        .map(|v| flag_on(&v))
        .unwrap_or(false)
        || std::env::var(PROGRESS_EVIDENCE_V1_ENV)
            .map(|v| flag_on(&v))
            .unwrap_or(false)
}

/// The fixed-order admission verdict for a rejected claim (design §5). The ordering
/// of the variants is the *evaluation order* — a claim that trips several checks is
/// rejected with the FIRST (lowest) applicable reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectionReason {
    /// The cap_id is not in this turn's OfferSet (never offered / fabricated).
    UnknownCapability,
    /// The offer's single-turn window is not the turn being admitted.
    Expired,
    /// The opaque handle was not minted for this (session, turn, atom).
    WrongTurnOrSession,
    /// A basis ref points past the turn's committed events (no such slot).
    UnresolvedBasis,
    /// A basis ref resolves to a slot that is not a committed event.
    UncommittedBasis,
    /// No cited basis event is of a kind the offer's `required_basis` accepts.
    WrongBasisEventKind,
    /// The cited basis event does not carry the offered atom's bound authored ref.
    BindingMismatch,
    /// The basis event was not caused this turn/session (causation chain broken).
    CausationMismatch,
    /// The offered atom has no verifiable provenance in the catalog.
    SourceHashMismatch,
    /// An identical AcceptedEvidence is already in the ledger.
    DuplicateEvidence,
}

impl RejectionReason {
    pub fn as_str(self) -> &'static str {
        match self {
            RejectionReason::UnknownCapability => "unknown_capability",
            RejectionReason::Expired => "expired",
            RejectionReason::WrongTurnOrSession => "wrong_turn_or_session",
            RejectionReason::UnresolvedBasis => "unresolved_basis",
            RejectionReason::UncommittedBasis => "uncommitted_basis",
            RejectionReason::WrongBasisEventKind => "wrong_basis_event_kind",
            RejectionReason::BindingMismatch => "binding_mismatch",
            RejectionReason::CausationMismatch => "causation_mismatch",
            RejectionReason::SourceHashMismatch => "source_hash_mismatch",
            RejectionReason::DuplicateEvidence => "duplicate_evidence",
        }
    }
}

/// Everything the gateway needs to admit a claim, in one borrow bundle. `session_id`
/// + `turn_id` are the authoritative scope the admission is evaluated against (the
/// opaque handle is recomputed against them; basis events must be caused within
/// them).
pub struct GatewayInputs<'a> {
    pub session_id: &'a str,
    pub turn_id: &'a str,
    pub offer_set: &'a EvidenceOfferSet,
    pub committed_events: &'a [DomainEvent],
    pub catalog: &'a EvidenceAtomCatalog,
    pub ledger: &'a EvidenceLedger,
}

/// Whether a committed event kind satisfies any of an offer's `required_basis`
/// categories. Closed mapping (no ruleset/module name branching). `ChoiceCommitted`
/// has no committed DomainEvent kind yet (the live pipeline emits none — P1-2
/// finding) → fail-closed (nothing satisfies it).
fn event_kind_satisfies(required: &[BasisKind], k: DomainEventKind) -> bool {
    required.iter().any(|b| match b {
        BasisKind::CheckResolved => matches!(k, DomainEventKind::CheckResolved),
        BasisKind::FactCommitted => matches!(
            k,
            DomainEventKind::PlayerLearnedFact | DomainEventKind::FactRevealed
        ),
        BasisKind::OutcomeCommitted => matches!(k, DomainEventKind::WorldFactChanged),
        BasisKind::LocationEntered => matches!(k, DomainEventKind::SceneTransitioned),
        BasisKind::ChoiceCommitted => false,
    })
}

/// The authored fact id an offered atom is bound to (the `fact:<id>` grounding's id),
/// recovered from the catalog atom. Used for the BindingMismatch check.
fn atom_bound_fact_id<'a>(catalog: &'a EvidenceAtomCatalog, atom_id: &AtomId) -> Option<&'a str> {
    catalog
        .resolve_atom_id(atom_id)
        .map(|a| a.grounding.strip_prefix("fact:").unwrap_or(&a.grounding))
}

/// Deterministic id for a GM-witnessed AcceptedEvidence — folds in the (Rust-
/// resolved) atom + the cited basis event ids so the same atom+basis collapses to one
/// ledger entry (duplicate claims never double-complete).
fn gm_witnessed_evidence_id(atom_id: &AtomId, basis_event_ids: &[String]) -> String {
    format!("evg:{}:{}", atom_id.as_str(), basis_event_ids.join(","))
}

/// The closed admission gate. Maps a GM [`EvidenceClaim`] to an [`AcceptedEvidence`]
/// (authority [`EvidenceAuthority::GmWitnessed`]) iff every check in the fixed order
/// passes; otherwise the first applicable [`RejectionReason`]. **Rust resolves
/// cap→atom from the OfferSet — the LLM never names the atom.**
pub struct EvidenceGateway;

impl EvidenceGateway {
    pub fn admit(
        claim: &EvidenceClaim,
        inp: &GatewayInputs,
    ) -> Result<AcceptedEvidence, RejectionReason> {
        admit_impl(claim, inp)
    }
}

fn admit_impl(
    claim: &EvidenceClaim,
    inp: &GatewayInputs,
) -> Result<AcceptedEvidence, RejectionReason> {
    // 1. UnknownCapability — the cap must be in THIS turn's OfferSet.
    let offer: &EvidenceOffer = inp
        .offer_set
        .find(claim.cap_id.as_str())
        .ok_or(RejectionReason::UnknownCapability)?;

    // 2. Expired — the offer's single-turn window must be the turn we admit in.
    if offer.expires_at != inp.turn_id {
        return Err(RejectionReason::Expired);
    }

    // 3. WrongTurnOrSession — the opaque handle must recompute to the one minted for
    //    THIS (session, turn, atom). A handle from another session/turn won't match.
    let expected_cap = CapId::from_parts(inp.session_id, inp.turn_id, &offer.atom_id);
    if expected_cap != claim.cap_id || inp.offer_set.turn_id != inp.turn_id {
        return Err(RejectionReason::WrongTurnOrSession);
    }

    // 4. UnresolvedBasis — every basis ref must point at an existing committed slot.
    let mut basis_events: Vec<&DomainEvent> = Vec::new();
    for r in claim.basis.iter() {
        let ev = inp
            .committed_events
            .get(r.index())
            .ok_or(RejectionReason::UnresolvedBasis)?;
        basis_events.push(ev);
    }
    // 5. UncommittedBasis — a resolved slot must be a real committed event (non-empty
    //    event_id; an adjudicated-but-uncommitted outcome carries no canonical id).
    if basis_events.iter().any(|e| e.event_id.trim().is_empty()) {
        return Err(RejectionReason::UncommittedBasis);
    }

    // 6. WrongBasisEventKind — at least one cited event must be of a kind the offer's
    //    required_basis accepts.
    let kind_matches: Vec<&DomainEvent> = basis_events
        .iter()
        .copied()
        .filter(|e| event_kind_satisfies(&offer.required_basis, e.kind))
        .collect();
    if kind_matches.is_empty() {
        return Err(RejectionReason::WrongBasisEventKind);
    }

    // 7. BindingMismatch — the cited (kind-matching) event must carry the offered
    //    atom's bound authored ref. If the atom's provenance is unknown here, skip
    //    (the SourceHashMismatch check at #9 catches a missing atom).
    let bound_fact = atom_bound_fact_id(inp.catalog, &offer.atom_id);
    if let Some(want_fact) = bound_fact {
        let bound: Vec<&DomainEvent> = kind_matches
            .iter()
            .copied()
            .filter(|e| e.data.get("fact_id").and_then(Value::as_str) == Some(want_fact))
            .collect();
        if bound.is_empty() {
            return Err(RejectionReason::BindingMismatch);
        }
    }

    // 8. CausationMismatch — every cited basis event must have been caused this
    //    turn/session (not carried over from another turn/session).
    if basis_events
        .iter()
        .any(|e| e.session_id != inp.session_id || e.turn_id != inp.turn_id)
    {
        return Err(RejectionReason::CausationMismatch);
    }

    // 9. SourceHashMismatch — the offered atom must have verifiable provenance: it
    //    must exist in the catalog with ≥1 source ref (source-grounded).
    let atom = inp
        .catalog
        .resolve_atom_id(&offer.atom_id)
        .ok_or(RejectionReason::SourceHashMismatch)?;
    if atom.source_refs.is_empty() {
        return Err(RejectionReason::SourceHashMismatch);
    }

    // The basis the admitted evidence records = the kind-matching, binding-bound
    // cited events (≥1 guaranteed by #6/#7).
    let basis_event_ids: Vec<String> = kind_matches
        .iter()
        .filter(|e| {
            bound_fact
                .map(|f| e.data.get("fact_id").and_then(Value::as_str) == Some(f))
                .unwrap_or(true)
        })
        .map(|e| e.event_id.clone())
        .collect();

    let evidence_id = gm_witnessed_evidence_id(&atom.atom_id, &basis_event_ids);

    // 10. DuplicateEvidence — an identical AcceptedEvidence already recorded.
    if inp
        .ledger
        .entries()
        .iter()
        .any(|e| e.evidence_id == evidence_id)
    {
        return Err(RejectionReason::DuplicateEvidence);
    }

    // 11. Accepted — atom_id + source_refs come from the catalog (Rust-resolved),
    //     NEVER from the claim. Authority = GmWitnessed.
    Ok(AcceptedEvidence {
        evidence_id,
        atom_id: atom.atom_id.clone(),
        evidence_kind: atom.kind,
        basis_event_ids,
        source_refs: atom.source_refs.clone(),
        turn_id: inp.turn_id.to_string(),
        authority: EvidenceAuthority::GmWitnessed,
    })
}

/// The short GM-prompt instruction (EV-4) telling the GM it MAY return a
/// `progress_claims` sidecar for a capability whose condition genuinely occurred this
/// turn — and must never claim what did not happen. Additive; appended only inside
/// the flag guard so OFF prompt bytes are unchanged.
pub fn render_claim_instruction() -> String {
    String::from(
        "PROGRESSION CLAIMS (optional, this turn only): if — and ONLY if — a capability's \
         condition genuinely occurred this turn, you MAY add a JSON field \
         \"progress_claims\": [{\"cap_id\": \"<one of the handles above>\", \"basis\": \
         [\"commit:<i>\"]}], where each basis entry cites the turn-local outcome/commit that \
         caused it. NEVER claim a capability whose condition did not happen. Omit the field \
         entirely if nothing applies. Do not name objectives, atoms, completion, or rewards — \
         only the opaque handle and its basis.",
    )
}

/// Parse the GM output's `progress_claims` sidecar (a structured JSON field on the
/// turn/nav decision) into typed [`EvidenceClaim`]s. **Fail-closed**: a missing
/// field, a non-array, or a malformed/forged entry (extra fields, empty basis, bad
/// ref) is dropped — never guessed. (Reuses the closed `EvidenceClaim` serde schema;
/// additive to the existing TurnDocument parser.)
pub fn parse_progress_claims(value: &Value) -> Vec<EvidenceClaim> {
    let Some(arr) = value.get("progress_claims").and_then(Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|entry| serde_json::from_value::<EvidenceClaim>(entry.clone()).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::adventure_ir::{
        EvidenceAtomSpec, EvidenceKind, EvidenceOfferSet, NonEmpty, TurnLocalRef,
    };
    use trpg_model::SourceRef;

    const SESSION: &str = "sess_a";
    const TURN: &str = "turn_5";
    const CLUE: &str = "clue_aquifer_commercial";

    fn src(page: u32) -> SourceRef {
        SourceRef {
            source_id: "the_vault".into(),
            page: Some(page),
            ..Default::default()
        }
    }

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
            source_refs: vec![src(page)],
            grounding: format!("fact:{clue_id}"),
        }
    }

    fn catalog_with(clue_id: &str, page: u32) -> EvidenceAtomCatalog {
        let mut c = EvidenceAtomCatalog::new();
        c.insert(atom_spec(clue_id, page));
        c
    }

    /// One FactLearned offer for `clue_id`, cap minted for (SESSION, TURN, atom).
    fn offer_set_with(clue_id: &str, page: u32) -> EvidenceOfferSet {
        let atom = atom_spec(clue_id, page);
        let mut set = EvidenceOfferSet::new(TURN);
        set.push(EvidenceOffer {
            cap_id: CapId::from_parts(SESSION, TURN, &atom.atom_id),
            atom_id: atom.atom_id,
            kind: EvidenceKind::FactLearned,
            meaning: format!("the player learned the authored clue \"{clue_id}\""),
            required_basis: vec![BasisKind::FactCommitted],
            expires_at: TURN.into(),
        });
        set
    }

    fn learned_event(event_id: &str, turn: &str, fact_id: &str) -> DomainEvent {
        DomainEvent::new(
            event_id,
            SESSION,
            turn,
            DomainEventKind::PlayerLearnedFact,
            serde_json::json!({"fact_id": fact_id, "reason": "successful Investigation check"}),
        )
    }

    fn claim_for(set: &EvidenceOfferSet, refs: Vec<TurnLocalRef>) -> EvidenceClaim {
        EvidenceClaim {
            cap_id: set.offers()[0].cap_id.clone(),
            basis: NonEmpty::from_vec(refs).unwrap(),
        }
    }

    fn inputs<'a>(
        set: &'a EvidenceOfferSet,
        events: &'a [DomainEvent],
        catalog: &'a EvidenceAtomCatalog,
        ledger: &'a EvidenceLedger,
    ) -> GatewayInputs<'a> {
        GatewayInputs {
            session_id: SESSION,
            turn_id: TURN,
            offer_set: set,
            committed_events: events,
            catalog,
            ledger,
        }
    }

    #[test]
    fn valid_claim_admits_one_gm_witnessed_evidence_with_rust_resolved_atom() {
        let set = offer_set_with(CLUE, 8);
        let events = vec![learned_event("de_1", TURN, CLUE)];
        let catalog = catalog_with(CLUE, 8);
        let ledger = EvidenceLedger::new();
        let claim = claim_for(&set, vec![TurnLocalRef::Commit(0)]);

        let ev = EvidenceGateway::admit(&claim, &inputs(&set, &events, &catalog, &ledger))
            .expect("valid claim admits");
        assert_eq!(ev.authority, EvidenceAuthority::GmWitnessed);
        // The atom is Rust-resolved from the OfferSet/catalog — the claim never named it.
        assert_eq!(ev.atom_id, set.offers()[0].atom_id, "atom_id is Rust-resolved, not from the LLM");
        assert_eq!(ev.evidence_kind, EvidenceKind::FactLearned);
        assert_eq!(ev.basis_event_ids, vec!["de_1".to_string()]);
        assert_eq!(ev.turn_id, TURN);
        assert!(!ev.source_refs.is_empty(), "source-grounded (carries the atom's source ref)");
        assert_eq!(ev.source_refs[0].page, Some(8));
    }

    #[test]
    fn unknown_capability_rejected() {
        let set = offer_set_with(CLUE, 8);
        let events = vec![learned_event("de_1", TURN, CLUE)];
        let catalog = catalog_with(CLUE, 8);
        let ledger = EvidenceLedger::new();
        let claim = EvidenceClaim {
            cap_id: CapId("cap_fabricated".into()),
            basis: NonEmpty::from_vec(vec![TurnLocalRef::Commit(0)]).unwrap(),
        };
        assert_eq!(
            EvidenceGateway::admit(&claim, &inputs(&set, &events, &catalog, &ledger)),
            Err(RejectionReason::UnknownCapability)
        );
    }

    #[test]
    fn expired_offer_rejected() {
        // Offer minted for an earlier turn; admitting in TURN.
        let atom = atom_spec(CLUE, 8);
        let mut set = EvidenceOfferSet::new(TURN);
        set.push(EvidenceOffer {
            cap_id: CapId::from_parts(SESSION, TURN, &atom.atom_id),
            atom_id: atom.atom_id.clone(),
            kind: EvidenceKind::FactLearned,
            meaning: "x".into(),
            required_basis: vec![BasisKind::FactCommitted],
            expires_at: "turn_3".into(), // ← past window
        });
        let events = vec![learned_event("de_1", TURN, CLUE)];
        let catalog = catalog_with(CLUE, 8);
        let ledger = EvidenceLedger::new();
        let claim = claim_for(&set, vec![TurnLocalRef::Commit(0)]);
        assert_eq!(
            EvidenceGateway::admit(&claim, &inputs(&set, &events, &catalog, &ledger)),
            Err(RejectionReason::Expired)
        );
    }

    #[test]
    fn wrong_turn_or_session_rejected() {
        // The cap was minted for SESSION; admit under a different session scope.
        let set = offer_set_with(CLUE, 8);
        let events = vec![learned_event("de_1", TURN, CLUE)];
        let catalog = catalog_with(CLUE, 8);
        let ledger = EvidenceLedger::new();
        let claim = claim_for(&set, vec![TurnLocalRef::Commit(0)]);
        let mut inp = inputs(&set, &events, &catalog, &ledger);
        inp.session_id = "sess_OTHER";
        assert_eq!(
            EvidenceGateway::admit(&claim, &inp),
            Err(RejectionReason::WrongTurnOrSession)
        );
    }

    #[test]
    fn unresolved_basis_rejected() {
        let set = offer_set_with(CLUE, 8);
        let events = vec![learned_event("de_1", TURN, CLUE)];
        let catalog = catalog_with(CLUE, 8);
        let ledger = EvidenceLedger::new();
        // commit:5 — out of range (only one committed event).
        let claim = claim_for(&set, vec![TurnLocalRef::Commit(5)]);
        assert_eq!(
            EvidenceGateway::admit(&claim, &inputs(&set, &events, &catalog, &ledger)),
            Err(RejectionReason::UnresolvedBasis)
        );
    }

    #[test]
    fn uncommitted_basis_rejected() {
        let set = offer_set_with(CLUE, 8);
        // An adjudicated-but-uncommitted slot: empty event_id.
        let events = vec![learned_event("", TURN, CLUE)];
        let catalog = catalog_with(CLUE, 8);
        let ledger = EvidenceLedger::new();
        let claim = claim_for(&set, vec![TurnLocalRef::Commit(0)]);
        assert_eq!(
            EvidenceGateway::admit(&claim, &inputs(&set, &events, &catalog, &ledger)),
            Err(RejectionReason::UncommittedBasis)
        );
    }

    #[test]
    fn wrong_basis_event_kind_rejected() {
        let set = offer_set_with(CLUE, 8);
        // The cited event is a CheckResolved, but the clue offer requires FactCommitted.
        let events = vec![DomainEvent::new(
            "de_chk",
            SESSION,
            TURN,
            DomainEventKind::CheckResolved,
            serde_json::json!({"fact_id": CLUE}),
        )];
        let catalog = catalog_with(CLUE, 8);
        let ledger = EvidenceLedger::new();
        let claim = claim_for(&set, vec![TurnLocalRef::Commit(0)]);
        assert_eq!(
            EvidenceGateway::admit(&claim, &inputs(&set, &events, &catalog, &ledger)),
            Err(RejectionReason::WrongBasisEventKind)
        );
    }

    #[test]
    fn binding_mismatch_rejected() {
        let set = offer_set_with(CLUE, 8);
        // Right kind, right turn, but the event carries a DIFFERENT fact than the
        // offered atom is bound to.
        let events = vec![learned_event("de_1", TURN, "clue_some_other_thing")];
        let catalog = catalog_with(CLUE, 8);
        let ledger = EvidenceLedger::new();
        let claim = claim_for(&set, vec![TurnLocalRef::Commit(0)]);
        assert_eq!(
            EvidenceGateway::admit(&claim, &inputs(&set, &events, &catalog, &ledger)),
            Err(RejectionReason::BindingMismatch)
        );
    }

    #[test]
    fn causation_mismatch_rejected() {
        let set = offer_set_with(CLUE, 8);
        // Right kind + binding, but the event was caused in a DIFFERENT turn.
        let events = vec![learned_event("de_1", "turn_1", CLUE)];
        let catalog = catalog_with(CLUE, 8);
        let ledger = EvidenceLedger::new();
        let claim = claim_for(&set, vec![TurnLocalRef::Commit(0)]);
        assert_eq!(
            EvidenceGateway::admit(&claim, &inputs(&set, &events, &catalog, &ledger)),
            Err(RejectionReason::CausationMismatch)
        );
    }

    #[test]
    fn source_hash_mismatch_rejected() {
        let set = offer_set_with(CLUE, 8);
        let events = vec![learned_event("de_1", TURN, CLUE)];
        // Catalog does NOT contain the offered atom → no verifiable provenance.
        let catalog = EvidenceAtomCatalog::new();
        let ledger = EvidenceLedger::new();
        let claim = claim_for(&set, vec![TurnLocalRef::Commit(0)]);
        assert_eq!(
            EvidenceGateway::admit(&claim, &inputs(&set, &events, &catalog, &ledger)),
            Err(RejectionReason::SourceHashMismatch)
        );
    }

    #[test]
    fn duplicate_evidence_rejected() {
        let set = offer_set_with(CLUE, 8);
        let events = vec![learned_event("de_1", TURN, CLUE)];
        let catalog = catalog_with(CLUE, 8);
        let claim = claim_for(&set, vec![TurnLocalRef::Commit(0)]);
        // First admission populates a ledger; a second identical claim duplicates.
        let mut ledger = EvidenceLedger::new();
        let first = EvidenceGateway::admit(&claim, &inputs(&set, &events, &catalog, &ledger))
            .expect("first admits");
        ledger.append(first);
        assert_eq!(
            EvidenceGateway::admit(&claim, &inputs(&set, &events, &catalog, &ledger)),
            Err(RejectionReason::DuplicateEvidence)
        );
    }

    #[test]
    fn rejection_order_is_fixed_unknown_before_everything() {
        // A claim that is BOTH unknown-cap AND has an unresolved basis ⇒ the earlier
        // reason (UnknownCapability) wins.
        let set = offer_set_with(CLUE, 8);
        let events: Vec<DomainEvent> = vec![];
        let catalog = catalog_with(CLUE, 8);
        let ledger = EvidenceLedger::new();
        let claim = EvidenceClaim {
            cap_id: CapId("cap_nope".into()),
            basis: NonEmpty::from_vec(vec![TurnLocalRef::Commit(9)]).unwrap(),
        };
        assert_eq!(
            EvidenceGateway::admit(&claim, &inputs(&set, &events, &catalog, &ledger)),
            Err(RejectionReason::UnknownCapability),
            "unknown capability short-circuits before basis resolution"
        );
    }

    #[test]
    fn parse_progress_claims_reads_well_formed_sidecar() {
        let v = serde_json::json!({
            "reason": "player hacked the server",
            "progress_claims": [
                {"cap_id": "cap_abc", "basis": ["commit:0"]},
                {"cap_id": "cap_def", "basis": ["outcome:1", "commit:2"]}
            ]
        });
        let claims = parse_progress_claims(&v);
        assert_eq!(claims.len(), 2);
        assert_eq!(claims[0].cap_id.as_str(), "cap_abc");
        assert_eq!(claims[1].basis.len(), 2);
    }

    #[test]
    fn parse_progress_claims_is_fail_closed() {
        // No field ⇒ empty.
        assert!(parse_progress_claims(&serde_json::json!({"reason": "x"})).is_empty());
        // Non-array ⇒ empty.
        assert!(parse_progress_claims(&serde_json::json!({"progress_claims": "nope"})).is_empty());
        // A forged entry (extra `completed` field) and an empty-basis entry are dropped;
        // the one valid entry survives.
        let v = serde_json::json!({"progress_claims": [
            {"cap_id": "cap_ok", "basis": ["commit:0"]},
            {"cap_id": "cap_forge", "basis": ["commit:0"], "completed": true},
            {"cap_id": "cap_empty", "basis": []}
        ]});
        let claims = parse_progress_claims(&v);
        assert_eq!(claims.len(), 1, "only the well-formed, closed-schema claim parses");
        assert_eq!(claims[0].cap_id.as_str(), "cap_ok");
    }

    #[test]
    fn flag_defaults_off() {
        assert!(!flag_on("0") && !flag_on("false") && !flag_on("off") && !flag_on(""));
        assert!(flag_on("1") && flag_on("true") && flag_on("on"));
    }

    #[test]
    fn claim_instruction_off_keeps_prompt_byte_identical_and_never_leaks() {
        // Mirror the tiered.rs prep-half guard: when shadow is OFF the instruction is
        // never appended ⇒ prompt bytes are identical to baseline.
        let base = "BASE NAV PROMPT\nline2".to_string();
        let shadow_on = false;
        let mut prompt = base.clone();
        if shadow_on {
            prompt.push_str("\n\n");
            prompt.push_str(&render_claim_instruction());
        }
        assert_eq!(prompt, base, "OFF ⇒ prompt bytes identical (OFF==baseline)");

        // The instruction itself must never leak an atom id or name an objective —
        // the GM only ever sees opaque handles (design: LLM never evaluates a guard).
        let instr = render_claim_instruction();
        assert!(instr.contains("progress_claims"), "instructs the sidecar field");
        assert!(instr.contains("cap_id") && instr.contains("basis"), "names only the closed fields");
        assert!(!instr.contains("atom:"), "no atom id leaked into the GM prompt");
        assert!(
            !instr.contains("objective_id") && !instr.contains("completed"),
            "the GM is told NOT to name objectives/completion"
        );
    }
}
