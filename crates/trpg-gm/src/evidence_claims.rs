//! EV-4R (`progress_claims_on_gm_v1`) main-GM claim loop helpers.
//!
//! The claim↔admission loop was relocated from the scene-nav LLM (EV-4, retired) onto the MAIN
//! GM adjudication — the call that knows which checks succeeded this turn (GPT Pro §3 "首版由同
//! 一个 GM 调用输出 sidecar"). This module holds the two pure pieces the pipeline wires in:
//!
//!  1. [`render_gm_offer_and_claim_block`] — the GM-prompt block (EV-3 capability list + the
//!     reused EV-4 claim instruction + a markup-format note for the `[progress_claims]` tag).
//!  2. [`admit_gm_claims`] — reuse `EvidenceGateway::admit` over the GM's parsed claims, with
//!     `turn_id = request.turn_id` (the alignment fix: cap_id minting + admission MUST use the
//!     same turn_id stamped on the committed DomainEvents, else CausationMismatch rejects every
//!     real claim). Still SHADOW — the engine is never called; admission only logs.
//!
//! Both reuse the EV-2/EV-3/EV-4 types + gateway unchanged; nothing here re-implements admission.

use trpg_model::adventure_ir::{
    EvidenceAtomCatalog, EvidenceClaim, EvidenceLedger, EvidenceOfferSet,
};
use trpg_model::DomainEvent;
use trpg_runtime::evidence_gateway::{EvidenceGateway, GatewayInputs, RejectionReason};

/// One shadow admission decision (for `info!` logging — never feeds the engine).
pub(crate) struct ClaimDecision {
    pub cap_id: String,
    /// `Ok(atom_id)` admitted (Rust-resolved atom — the LLM never named it) | `Err(reason)`.
    pub result: Result<String, RejectionReason>,
}

/// Render the main-GM prompt block: EV-3 capability list + the reused EV-4 claim instruction +
/// a markup-format note telling the GM to emit a single GM-only `[progress_claims]` tag. Empty
/// offer set ⇒ `""` (no block ⇒ byte-identical baseline). The block carries ONLY opaque cap_id
/// handles + human meanings + required-basis tokens — never atom_id / objective_id.
pub(crate) fn render_gm_offer_and_claim_block(offer_set: &EvidenceOfferSet) -> String {
    let offers = trpg_runtime::evidence_offers::render_offer_prompt_block(offer_set);
    if offers.trim().is_empty() {
        return String::new();
    }
    let mut block = offers;
    block.push_str("\n\n");
    block.push_str(&trpg_runtime::evidence_gateway::render_claim_instruction());
    block.push_str(
        "\nFORMAT: emit any claims in ONE GM-only block at the very END of your reply — \
         [progress_claims][{\"cap_id\":\"<handle above>\",\"basis\":[\"commit:<i>\"]}]\
         [/progress_claims] — a JSON array, never shown to the player. Omit the block entirely \
         if no capability genuinely occurred.",
    );
    block
}

/// Reuse `EvidenceGateway::admit` over the main GM's parsed claims (shadow). Pure: loops the
/// gateway with `turn_id` (= request.turn_id) over THIS turn's committed events (the caller
/// passes turn-local events so each `commit:<i>` basis resolves within the turn). Returns the
/// shadow ledger of admitted `GmWitnessed` evidence + per-claim decisions for logging. The
/// engine is NEVER called and no objective is completed (J3 unchanged).
pub(crate) fn admit_gm_claims(
    session_id: &str,
    turn_id: &str,
    offer_set: &EvidenceOfferSet,
    catalog: &EvidenceAtomCatalog,
    committed_events: &[DomainEvent],
    claims: &[EvidenceClaim],
    seed: &EvidenceLedger,
) -> (EvidenceLedger, Vec<ClaimDecision>) {
    // EV-P2: start from the shared turn-local ledger (the exact producers ran first),
    // so a GM-witnessed claim for an observation an exact producer already admitted is
    // rejected as DuplicateEvidence (no double-emit; ExactDomain wins).
    let mut ledger = seed.clone();
    let mut decisions = Vec::with_capacity(claims.len());
    for claim in claims {
        // Build inputs in an inner scope so the immutable `&ledger` borrow ends before the
        // mutable `ledger.append` below (the gateway returns an OWNED Result, not borrowing inp).
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
        let cap_id = claim.cap_id.as_str().to_string();
        match result {
            Ok(ev) => {
                let atom = ev.atom_id.as_str().to_string();
                ledger.append(ev);
                decisions.push(ClaimDecision {
                    cap_id,
                    result: Ok(atom),
                });
            }
            Err(reason) => decisions.push(ClaimDecision {
                cap_id,
                result: Err(reason),
            }),
        }
    }
    (ledger, decisions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::adventure_ir::{
        AtomId, BasisKind, CapId, EvidenceAtomSpec, EvidenceAuthority, EvidenceKind, EvidenceOffer,
        NonEmpty, TurnLocalRef,
    };
    use trpg_model::{DomainEventKind, SourceRef};

    const SESSION: &str = "sess_evr";
    // The aligned turn_id — in live play this is request.turn_id, the SAME value stamped on the
    // committed DomainEvents. The old EV-4 nav bug used `turn_{count}` which need not match.
    const TURN: &str = "turn-uuid-real-1234";
    const CLUE: &str = "clue_aquifer_commercial";

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

    fn catalog_with(clue_id: &str, page: u32) -> EvidenceAtomCatalog {
        let mut c = EvidenceAtomCatalog::new();
        c.insert(atom_spec(clue_id, page));
        c
    }

    /// One FactLearned offer, cap minted for (SESSION, turn, atom) — exactly as the turn-start
    /// derivation does with `turn_id = request.turn_id`.
    fn offer_set_for(clue_id: &str, page: u32, turn: &str) -> EvidenceOfferSet {
        let atom = atom_spec(clue_id, page);
        let mut set = EvidenceOfferSet::new(turn);
        set.push(EvidenceOffer {
            cap_id: CapId::from_parts(SESSION, turn, &atom.atom_id),
            atom_id: atom.atom_id,
            kind: EvidenceKind::FactLearned,
            meaning: format!("the player learned the authored clue \"{clue_id}\""),
            required_basis: vec![BasisKind::FactCommitted],
            expires_at: turn.into(),
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

    fn claim_citing(set: &EvidenceOfferSet, refs: Vec<TurnLocalRef>) -> EvidenceClaim {
        EvidenceClaim {
            cap_id: set.offers()[0].cap_id.clone(),
            basis: NonEmpty::from_vec(refs).unwrap(),
        }
    }

    #[test]
    fn valid_main_gm_claim_admits_one_gm_witnessed_with_aligned_turn_id() {
        // A valid main-GM claim (cap in this turn's OfferSet + basis commit:0 → a real committed
        // PlayerLearnedFact carrying turn_id == TURN) ⇒ one AcceptedEvidence(GmWitnessed) with the
        // Rust-resolved atom. The turn_id used for minting + admission == the committed event's
        // turn_id ⇒ NO spurious CausationMismatch (the EV-4R alignment fix).
        let set = offer_set_for(CLUE, 8, TURN);
        let catalog = catalog_with(CLUE, 8);
        let events = vec![learned_event("de_clue_learn", TURN, CLUE)];
        let claims = vec![claim_citing(&set, vec![TurnLocalRef::Commit(0)])];

        let (ledger, decisions) =
            admit_gm_claims(SESSION, TURN, &set, &catalog, &events, &claims, &EvidenceLedger::new());

        assert_eq!(ledger.len(), 1, "one GmWitnessed evidence admitted");
        assert_eq!(ledger.entries()[0].authority, EvidenceAuthority::GmWitnessed);
        assert_eq!(
            ledger.entries()[0].atom_id,
            set.offers()[0].atom_id,
            "atom is Rust-resolved from the OfferSet, never named by the GM"
        );
        assert_eq!(ledger.entries()[0].turn_id, TURN);
        assert_eq!(decisions.len(), 1);
        assert!(decisions[0].result.is_ok(), "decision logged as admitted");
        assert_eq!(decisions[0].cap_id, set.offers()[0].cap_id.as_str());
    }

    #[test]
    fn misaligned_turn_id_yields_causation_mismatch_not_admit() {
        // THE alignment regression guard — reproduces the EXACT EV-4 bug the worker flagged:
        // the OfferSet + admission both used a derived `turn_{count}` that is INTERNALLY
        // consistent (so Expired + WrongTurnOrSession pass), but the committed DomainEvent
        // carries the REAL request.turn_id. The basis event's turn then ≠ the cap's turn scope
        // ⇒ CausationMismatch rejects every real claim. EV-4R fixes this by minting + admitting
        // with request.turn_id itself (the `valid_…aligned…` test above).
        let derived = "turn_5"; // the buggy turn_{count} (offer + admission share it)
        let real = "turn-uuid-real-1234"; // request.turn_id, stamped on the committed event
        let set = offer_set_for(CLUE, 8, derived);
        let catalog = catalog_with(CLUE, 8);
        let events = vec![learned_event("de_clue_learn", real, CLUE)];
        let claims = vec![claim_citing(&set, vec![TurnLocalRef::Commit(0)])];

        let (ledger, decisions) =
            admit_gm_claims(SESSION, derived, &set, &catalog, &events, &claims, &EvidenceLedger::new());

        assert_eq!(ledger.len(), 0, "no admission when the cap's turn ≠ the event's turn");
        assert_eq!(
            decisions[0].result,
            Err(RejectionReason::CausationMismatch),
            "cap turn ≠ committed-event turn ⇒ CausationMismatch (the bug EV-4R fixes)"
        );
    }

    #[test]
    fn forged_capability_rejected_and_not_admitted() {
        // A cap the GM invented (not in this turn's OfferSet) ⇒ UnknownCapability, never admitted.
        let set = offer_set_for(CLUE, 8, TURN);
        let catalog = catalog_with(CLUE, 8);
        let events = vec![learned_event("de_clue_learn", TURN, CLUE)];
        let forged = EvidenceClaim {
            cap_id: CapId("cap_fabricated_by_llm".into()),
            basis: NonEmpty::from_vec(vec![TurnLocalRef::Commit(0)]).unwrap(),
        };

        let (ledger, decisions) =
            admit_gm_claims(SESSION, TURN, &set, &catalog, &events, &[forged], &EvidenceLedger::new());

        assert_eq!(ledger.len(), 0);
        assert_eq!(decisions[0].result, Err(RejectionReason::UnknownCapability));
    }

    #[test]
    fn empty_offer_set_renders_no_block_baseline() {
        // No surfaced capabilities ⇒ empty render ⇒ no prompt block ⇒ byte-identical baseline.
        let empty = EvidenceOfferSet::new(TURN);
        assert_eq!(render_gm_offer_and_claim_block(&empty), "");
    }

    #[test]
    fn non_empty_offer_block_carries_cap_and_basis_never_atom_id() {
        let set = offer_set_for(CLUE, 8, TURN);
        let block = render_gm_offer_and_claim_block(&set);
        assert!(block.contains(set.offers()[0].cap_id.as_str()), "shows the opaque cap handle");
        assert!(block.contains("[progress_claims]"), "tells the GM the markup format");
        assert!(block.contains("basis"), "explains the basis requirement");
        // LLM-never-evaluates-guard: the atom id never leaks into the prompt.
        assert!(
            !block.contains(set.offers()[0].atom_id.as_str()),
            "atom_id must NEVER appear in the GM prompt"
        );
        assert!(!block.contains("atom:"), "no atom: prefix may leak");
    }
}
