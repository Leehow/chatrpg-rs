//! EV-P4 `progress_capability_binding_v1` (Part A) — the **CapabilityBinding +
//! OutcomeProjector** (GPT Pro follow-up `design/GPTpro-producer-firing-followup.md`
//! §A "action atom 绑尝试非完成"). This closes the EV-P3 gap where the MAIN GM
//! *observed* an offered action (observed=2) but the gateway *rejected* every one
//! (admitted=0).
//!
//! **Why EV-P3 rejected the observed action claims** (the diagnosis this slice
//! fixes): an EV-P3 observable-action atom has `grounding = "action:<entity>"` and,
//! per `required_basis_for`, `required_basis = [OutcomeCommitted]` (→
//! `WorldFactChanged`). When the GM marked it *observed* and cited the turn's check
//! commit as basis, the reused [`EvidenceGateway`] rejected it at
//! `WrongBasisEventKind` (the cited `CheckResolved` is not a `WorldFactChanged`) or
//! `BindingMismatch` (no committed event carries `fact_id = "action:<entity>"` — an
//! action atom binds to an authored *entity*, not a committed *fact*). The free
//! GM-cited basis was unresolvable for an action atom.
//!
//! **The fix** — split the reasoning (design law: *LLM judges "what action", Rust
//! judges "did it succeed"*): when the GM declares a check it may tag
//! `evidence_attempts: [cap_id]` (the offered action the check attempts; closed
//! schema, NO objective/atom id). Rust then deterministically produces
//! `AcceptedEvidence(ActionResolved, ExactDomain)` for the bound atom **iff** this
//! turn committed a successful/partial check — basis = the committed `CheckResolved`
//! event(s). A failed-only check produces nothing (fail-closed). Still **shadow**
//! (the engine does not consume the ledger — that is EV-APPLY).

use serde::Deserialize;
use serde_json::Value;
use trpg_model::adventure_ir::{
    AcceptedEvidence, CapId, EvidenceAtomCatalog, EvidenceAuthority, EvidenceKind, EvidenceLedger,
    EvidenceOffer, EvidenceOfferSet,
};
use trpg_model::{DomainEvent, DomainEventKind};

const PROGRESS_CAPABILITY_BINDING_V1_ENV: &str = "TRPG_PROGRESS_CAPABILITY_BINDING_V1";
const PROGRESS_EVIDENCE_V1_ENV: &str = "TRPG_PROGRESS_EVIDENCE_V1";

fn flag_on(raw: &str) -> bool {
    let v = raw.trim().to_ascii_lowercase();
    v == "1" || v == "true" || v == "on"
}

/// Whether the EV-P4 capability binding is active. Default OFF == byte-identical
/// baseline (no parsing, no producer, no prompt mutation). ON when EITHER the master
/// `progress_evidence_v1` OR the `progress_capability_binding_v1` slice flag is truthy.
pub fn progress_capability_binding_enabled() -> bool {
    std::env::var(PROGRESS_CAPABILITY_BINDING_V1_ENV)
        .map(|v| flag_on(&v))
        .unwrap_or(false)
        || std::env::var(PROGRESS_EVIDENCE_V1_ENV)
            .map(|v| flag_on(&v))
            .unwrap_or(false)
}

/// One GM-declared attempt: "a check this turn attempted offered capability `cap_id`".
/// Closed schema (`deny_unknown_fields`) — it CANNOT carry an objective/atom id or a
/// completion/reward verdict, so the LLM literally cannot evaluate a guard (it only
/// names the opaque handle it attempted; Rust decides success + admission).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAttempt {
    cap_id: String,
}

/// Why a tagged attempt produced no bound evidence (fixed evaluation order). Distinct
/// from a gateway `RejectionReason` — the binding producer's checks are check-outcome
/// specific, not fact-basis specific.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingReject {
    /// The cap_id is not in this turn's OfferSet (never offered / fabricated).
    UnknownCapability,
    /// The offer's single-turn window is not the turn being bound.
    Expired,
    /// The opaque handle was not minted for this (session, turn, atom).
    WrongTurnOrSession,
    /// The cap is not an action/state capability (Fact* caps bind via the gateway /
    /// exact projectors, never a check outcome).
    NotAnActionCapability,
    /// No committed successful/partial `CheckResolved` this turn — fail-closed (a
    /// failed-only check, or no check at all, never produces ActionResolved evidence).
    NoCommittedSuccess,
    /// The offered atom has no verifiable provenance in the catalog (source-grounded).
    SourceHashMismatch,
    /// An identical AcceptedEvidence is already in the ledger.
    DuplicateEvidence,
}

impl BindingReject {
    pub fn as_str(self) -> &'static str {
        match self {
            BindingReject::UnknownCapability => "unknown_capability",
            BindingReject::Expired => "expired",
            BindingReject::WrongTurnOrSession => "wrong_turn_or_session",
            BindingReject::NotAnActionCapability => "not_an_action_capability",
            BindingReject::NoCommittedSuccess => "no_committed_success",
            BindingReject::SourceHashMismatch => "source_hash_mismatch",
            BindingReject::DuplicateEvidence => "duplicate_evidence",
        }
    }
}

/// Per-attempt result (logged in the live wiring).
pub struct BindingDecision {
    pub cap_id: CapId,
    pub result: Result<String, BindingReject>,
}

/// Parse the GM output's `evidence_attempts` sidecar into the attempted [`CapId`]s.
/// **Fail-closed**: a missing field, a non-array, or a malformed/forged entry (extra
/// fields, missing/empty cap_id) is dropped — never guessed.
pub fn parse_evidence_attempts(value: &Value) -> Vec<CapId> {
    let Some(arr) = value.get("evidence_attempts").and_then(Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|entry| serde_json::from_value::<RawAttempt>(entry.clone()).ok())
        .filter(|a| !a.cap_id.trim().is_empty())
        .map(|a| CapId(a.cap_id))
        .collect()
}

/// Whether an offered atom kind is bindable by a check outcome (an action/state the
/// player *attempts* via a check). Fact* / Location / Choice kinds are NOT — those
/// have their own exact producers.
fn is_check_bindable_kind(kind: EvidenceKind) -> bool {
    matches!(
        kind,
        EvidenceKind::ActionResolved
            | EvidenceKind::StateEstablished
            | EvidenceKind::EntityEncountered
    )
}

/// Whether a committed event is a `CheckResolved` carrying a success/partial outcome
/// (the "committed success policy"). Fail-closed: a missing/false/non-object outcome
/// is NOT a success.
fn is_committed_success_check(ev: &DomainEvent) -> bool {
    if ev.kind != DomainEventKind::CheckResolved {
        return false;
    }
    let outcome = ev.data.get("outcome");
    let success = outcome
        .and_then(|o| o.get("success"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let partial = outcome
        .and_then(|o| o.get("success_tier"))
        .and_then(Value::as_str)
        .map(|t| t.to_ascii_lowercase().contains("partial"))
        .unwrap_or(false);
    success || partial
}

/// Deterministic id for a capability-bound AcceptedEvidence — folds the (Rust-
/// resolved) atom + the cited basis event ids so the same atom+basis collapses to one
/// ledger entry.
fn bound_evidence_id(atom_id: &str, basis_event_ids: &[String]) -> String {
    format!("evb:{}:{}", atom_id, basis_event_ids.join(","))
}

/// The CapabilityBinding producer. For each GM-tagged attempt, deterministically
/// produce `AcceptedEvidence(ExactDomain)` for the bound action/state atom iff this
/// turn committed a successful/partial check. Reuses the EV-2/EV-3 atom/offer/ledger
/// types unchanged; NO new admission policy beyond the check-success gate. `seed` is
/// the turn's exact-producer ledger so a binding duplicating an exact observation is
/// rejected. Returns the augmented ledger + per-attempt decisions.
pub fn bind_capability_evidence(
    session_id: &str,
    turn_id: &str,
    offer_set: &EvidenceOfferSet,
    catalog: &EvidenceAtomCatalog,
    committed_events: &[DomainEvent],
    attempts: &[CapId],
    seed: &EvidenceLedger,
) -> (EvidenceLedger, Vec<BindingDecision>) {
    let mut ledger = seed.clone();
    let mut decisions = Vec::new();

    // The turn-local committed successful/partial checks = the shared basis for any
    // action attempt this turn (caused within this session/turn).
    let success_basis: Vec<String> = committed_events
        .iter()
        .filter(|e| {
            e.session_id == session_id
                && e.turn_id == turn_id
                && !e.event_id.trim().is_empty()
                && is_committed_success_check(e)
        })
        .map(|e| e.event_id.clone())
        .collect();

    for cap_id in attempts {
        let result = bind_one(
            cap_id,
            session_id,
            turn_id,
            offer_set,
            catalog,
            &success_basis,
            &mut ledger,
        );
        decisions.push(BindingDecision {
            cap_id: cap_id.clone(),
            result,
        });
    }
    (ledger, decisions)
}

fn bind_one(
    cap_id: &CapId,
    session_id: &str,
    turn_id: &str,
    offer_set: &EvidenceOfferSet,
    catalog: &EvidenceAtomCatalog,
    success_basis: &[String],
    ledger: &mut EvidenceLedger,
) -> Result<String, BindingReject> {
    let offer: &EvidenceOffer = offer_set
        .find(cap_id.as_str())
        .ok_or(BindingReject::UnknownCapability)?;
    if offer.expires_at != turn_id {
        return Err(BindingReject::Expired);
    }
    let expected = CapId::from_parts(session_id, turn_id, &offer.atom_id);
    if &expected != cap_id || offer_set.turn_id != turn_id {
        return Err(BindingReject::WrongTurnOrSession);
    }
    if !is_check_bindable_kind(offer.kind) {
        return Err(BindingReject::NotAnActionCapability);
    }
    if success_basis.is_empty() {
        return Err(BindingReject::NoCommittedSuccess); // fail-closed: no committed success
    }
    let atom = catalog
        .resolve_atom_id(&offer.atom_id)
        .ok_or(BindingReject::SourceHashMismatch)?;
    if atom.source_refs.is_empty() {
        return Err(BindingReject::SourceHashMismatch);
    }
    let evidence_id = bound_evidence_id(atom.atom_id.as_str(), success_basis);
    if ledger.entries().iter().any(|e| e.evidence_id == evidence_id) {
        return Err(BindingReject::DuplicateEvidence);
    }
    let atom_str = atom.atom_id.as_str().to_string();
    ledger.append(AcceptedEvidence {
        evidence_id,
        atom_id: atom.atom_id.clone(),
        evidence_kind: atom.kind,
        basis_event_ids: success_basis.to_vec(),
        source_refs: atom.source_refs.clone(),
        turn_id: turn_id.to_string(),
        authority: EvidenceAuthority::ExactDomain,
    });
    Ok(atom_str)
}

/// The short GM-prompt instruction telling the GM it MAY tag a check with the offered
/// action capability it is attempting. Additive; appended only inside the flag guard
/// so OFF prompt bytes are unchanged. Names ONLY the opaque handles — never an atom /
/// objective id (the GM tags the attempt; Rust decides success + admission).
pub fn render_attempt_instruction() -> String {
    String::from(
        "ACTION ATTEMPT BINDING (optional, this turn only): when a check the player \
         makes this turn is ATTEMPTING one of the action capabilities listed above, \
         you MAY tag it in a GM-only block — [evidence_attempts][{\"cap_id\":\"<one of \
         the cap_ handles above>\"}][/evidence_attempts] — a JSON array, never shown to \
         the player. Tag only the offered action the check genuinely attempts; whether \
         it SUCCEEDS is decided by the dice/Rust, not by you. Do not name objectives, \
         atoms, completion, or rewards — only the opaque handle.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::adventure_ir::{AtomId, BasisKind, EvidenceAtomSpec, ProgressRole};
    use trpg_model::SourceRef;

    const SESSION: &str = "sess_p4";
    const TURN: &str = "turn-uuid-7";

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

    /// An offer set + catalog containing one offered atom (cap minted for it).
    fn offer_for(atom: EvidenceAtomSpec) -> (EvidenceOfferSet, EvidenceAtomCatalog, CapId) {
        let mut catalog = EvidenceAtomCatalog::new();
        let cap = CapId::from_parts(SESSION, TURN, &atom.atom_id);
        let mut set = EvidenceOfferSet::new(TURN);
        set.push(EvidenceOffer {
            cap_id: cap.clone(),
            atom_id: atom.atom_id.clone(),
            kind: atom.kind,
            meaning: "the player resolved an authored action".into(),
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

    #[test]
    fn attempted_action_cap_with_committed_success_admits_one_action_evidence() {
        let (set, catalog, cap) = offer_for(action_atom("npc_fount_anomaly"));
        let events = vec![check_event("de_check_chk_1", true)];
        let (ledger, decisions) = bind_capability_evidence(
            SESSION, TURN, &set, &catalog, &events, &[cap], &EvidenceLedger::new(),
        );
        assert_eq!(ledger.len(), 1, "one bound action evidence");
        let ev = &ledger.entries()[0];
        assert_eq!(ev.evidence_kind, EvidenceKind::ActionResolved);
        assert_eq!(ev.authority, EvidenceAuthority::ExactDomain, "deterministic producer");
        assert_eq!(ev.basis_event_ids, vec!["de_check_chk_1".to_string()], "basis = the committed check");
        assert!(decisions[0].result.is_ok());
        assert!(!ev.source_refs.is_empty(), "source-grounded");
    }

    #[test]
    fn failed_check_only_produces_no_evidence() {
        let (set, catalog, cap) = offer_for(action_atom("npc_fount_anomaly"));
        let events = vec![check_event("de_check_chk_1", false)];
        let (ledger, decisions) = bind_capability_evidence(
            SESSION, TURN, &set, &catalog, &events, &[cap], &EvidenceLedger::new(),
        );
        assert_eq!(ledger.len(), 0, "fail-closed: a failed check produces no ActionResolved");
        assert_eq!(decisions[0].result, Err(BindingReject::NoCommittedSuccess));
    }

    #[test]
    fn fact_capability_attempt_is_not_bound_by_a_check() {
        // A FactLearned cap is NOT check-bindable (it has an exact producer).
        let (set, catalog, cap) = offer_for(fact_atom("clue_aquifer_commercial"));
        let events = vec![check_event("de_check_chk_1", true)];
        let (ledger, decisions) = bind_capability_evidence(
            SESSION, TURN, &set, &catalog, &events, &[cap], &EvidenceLedger::new(),
        );
        assert_eq!(ledger.len(), 0);
        assert_eq!(decisions[0].result, Err(BindingReject::NotAnActionCapability));
    }

    #[test]
    fn unknown_capability_attempt_rejected() {
        let (set, catalog, _cap) = offer_for(action_atom("npc_fount_anomaly"));
        let events = vec![check_event("de_check_chk_1", true)];
        let (ledger, decisions) = bind_capability_evidence(
            SESSION, TURN, &set, &catalog, &events, &[CapId("cap_fabricated".into())], &EvidenceLedger::new(),
        );
        assert_eq!(ledger.len(), 0);
        assert_eq!(decisions[0].result, Err(BindingReject::UnknownCapability));
    }

    #[test]
    fn expired_offer_attempt_rejected() {
        let atom = action_atom("npc_fount_anomaly");
        let cap = CapId::from_parts(SESSION, TURN, &atom.atom_id);
        let mut catalog = EvidenceAtomCatalog::new();
        let mut set = EvidenceOfferSet::new(TURN);
        set.push(EvidenceOffer {
            cap_id: cap.clone(),
            atom_id: atom.atom_id.clone(),
            kind: atom.kind,
            meaning: "x".into(),
            required_basis: vec![BasisKind::OutcomeCommitted],
            expires_at: "turn_past".into(), // ← stale window
        });
        catalog.insert(atom);
        let events = vec![check_event("de_check_chk_1", true)];
        let (ledger, decisions) = bind_capability_evidence(
            SESSION, TURN, &set, &catalog, &events, &[cap], &EvidenceLedger::new(),
        );
        assert_eq!(ledger.len(), 0);
        assert_eq!(decisions[0].result, Err(BindingReject::Expired));
    }

    #[test]
    fn duplicate_attempt_does_not_double_emit() {
        let (set, catalog, cap) = offer_for(action_atom("npc_fount_anomaly"));
        let events = vec![check_event("de_check_chk_1", true)];
        let (ledger, decisions) = bind_capability_evidence(
            SESSION, TURN, &set, &catalog, &events, &[cap.clone(), cap], &EvidenceLedger::new(),
        );
        assert_eq!(ledger.len(), 1, "the same bound atom+basis collapses to one entry");
        assert!(decisions[0].result.is_ok());
        assert_eq!(decisions[1].result, Err(BindingReject::DuplicateEvidence));
    }

    #[test]
    fn partial_success_tier_also_binds() {
        let (set, catalog, cap) = offer_for(action_atom("npc_fount_anomaly"));
        let events = vec![DomainEvent::new(
            "de_check_chk_2",
            SESSION,
            TURN,
            DomainEventKind::CheckResolved,
            serde_json::json!({"check_id": "chk_2", "outcome": {"success": false, "success_tier": "partial"}}),
        )];
        let (ledger, _d) = bind_capability_evidence(
            SESSION, TURN, &set, &catalog, &events, &[cap], &EvidenceLedger::new(),
        );
        assert_eq!(ledger.len(), 1, "a partial success also satisfies the binding");
    }

    #[test]
    fn prior_observed_but_rejected_action_cap_now_admits_via_binding() {
        // The EV-P3 gap reproduced: the MAIN GM marked an offered action observed and
        // cited the turn's committed check as basis. The reused EvidenceGateway REJECTS
        // it (an action atom's required_basis is OutcomeCommitted=WorldFactChanged, but
        // the cited basis is a CheckResolved) → admitted=0. EV-P4's binding producer
        // admits the SAME atom from the SAME committed check → admitted=1.
        use crate::evidence_gateway::{EvidenceGateway, GatewayInputs, RejectionReason};
        use trpg_model::adventure_ir::{EvidenceClaim, NonEmpty, TurnLocalRef};

        let (set, catalog, cap) = offer_for(action_atom("npc_fount_anomaly"));
        let events = vec![check_event("de_check_chk_1", true)];

        // (1) the EV-P3 path: GM-witnessed claim citing the check → gateway rejects.
        let claim = EvidenceClaim {
            cap_id: cap.clone(),
            basis: NonEmpty::from_vec(vec![TurnLocalRef::Commit(0)]).unwrap(),
        };
        let ledger = EvidenceLedger::new();
        let verdict = EvidenceGateway::admit(
            &claim,
            &GatewayInputs {
                session_id: SESSION,
                turn_id: TURN,
                offer_set: &set,
                committed_events: &events,
                catalog: &catalog,
                ledger: &ledger,
            },
        );
        assert_eq!(
            verdict,
            Err(RejectionReason::WrongBasisEventKind),
            "EV-P3 reproduction: the gateway can't admit an action atom from a check basis"
        );

        // (2) the EV-P4 binding producer: the SAME atom + SAME check now admits.
        let (bound, decisions) = bind_capability_evidence(
            SESSION, TURN, &set, &catalog, &events, &[cap], &EvidenceLedger::new(),
        );
        assert_eq!(bound.len(), 1, "EV-P4: capability binding admits the observed action");
        assert!(decisions[0].result.is_ok());
        assert_eq!(bound.entries()[0].evidence_kind, EvidenceKind::ActionResolved);
    }

    #[test]
    fn parse_evidence_attempts_is_fail_closed() {
        // well-formed parses
        let v = serde_json::json!({"evidence_attempts": [{"cap_id": "cap_abc"}, {"cap_id": "cap_def"}]});
        let caps = parse_evidence_attempts(&v);
        assert_eq!(caps.len(), 2);
        assert_eq!(caps[0].as_str(), "cap_abc");
        // no field ⇒ empty; non-array ⇒ empty
        assert!(parse_evidence_attempts(&serde_json::json!({"x": 1})).is_empty());
        assert!(parse_evidence_attempts(&serde_json::json!({"evidence_attempts": "nope"})).is_empty());
        // a forged entry (extra `completed` field) and an empty cap_id are dropped
        let forged = serde_json::json!({"evidence_attempts": [
            {"cap_id": "cap_ok"},
            {"cap_id": "cap_forge", "completed": true},
            {"cap_id": ""}
        ]});
        let caps = parse_evidence_attempts(&forged);
        assert_eq!(caps.len(), 1, "only the well-formed closed-schema attempt parses");
        assert_eq!(caps[0].as_str(), "cap_ok");
    }

    #[test]
    fn flag_defaults_off_and_instruction_never_leaks() {
        assert!(!flag_on("0") && !flag_on("false") && !flag_on("off") && !flag_on(""));
        assert!(flag_on("1") && flag_on("true") && flag_on("on"));
        let instr = render_attempt_instruction();
        assert!(instr.contains("evidence_attempts") && instr.contains("cap_id"));
        assert!(!instr.contains("atom:"), "no atom id leaked");
        assert!(!instr.contains("obj."), "no objective id slug leaked");
    }
}
