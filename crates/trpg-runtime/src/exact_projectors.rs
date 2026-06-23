//! EV-P2 `progress_exact_projectors_v1` — the **deterministic exact producers** of
//! the Progression Evidence Layer (GPT Pro follow-up
//! `design/GPTpro-producer-firing-followup.md` §A "谁生产" table + §三投影路径).
//!
//! The producer-firing reframe: **most evidence must NOT depend on the GM's audit
//! compliance** (EV-P1 measured it at ~50%). These three exact producers run pure
//! Rust over THIS turn's committed [`DomainEvent`]s and need no LLM witness (the
//! content-delivery one consumes only a closed GM *reference*, never a GM verdict):
//!
//! - [`project_location_entered`] (pure Rust): a real committed `SceneTransitioned`
//!   (NavigationResolver / true player-driven move — NOT "the player says they'll
//!   go") whose destination resolves to an authored `location:<id>` atom ⇒ one
//!   [`AcceptedEvidence`]`(LocationEntered, ExactDomain)`.
//! - [`project_state_established`] (pure Rust): a committed `WorldFactChanged` whose
//!   `fact_id` resolves to an authored `state:<id>` atom (door=unlocked, etc.) and
//!   is established-true ⇒ `(StateEstablished, ExactDomain)`. *Scope note (EV-P2):*
//!   authored-state atoms are derived only from what already exists in the graph
//!   (scene-mechanic `SetObjectState`/`CreateFact` authored leaves); the **broad**
//!   observable-state catalog lands in EV-P3 — today many modules yield an empty
//!   state catalog ⇒ no state evidence (fail-closed, honest, NOT fabricated).
//! - [`project_content_delivery`]: the GM emits a closed [`ContentDelivery`] ONLY
//!   when it actually presents an authored clue's content; **Rust** verifies it
//!   (reusing [`EvidenceGateway::admit`] unchanged, then re-stamping authority to
//!   ExactDomain) ⇒ `(FactLearned, ExactDomain)`. The GM never names the fact/atom —
//!   Rust resolves `cap → authored fact` from the OfferSet/catalog.
//!
//! Design law honored: **LLM-never-evaluates-guard** (Location/State are pure Rust;
//! ContentDelivery: the GM emits only an opaque `delivery_cap` + basis, Rust decides);
//! **source-grounded** (every atom + every AcceptedEvidence carries ≥1 `SourceRef`);
//! **NO ruleset/module name branching** (structure only — node ids, scene-mechanic
//! shapes); **fail-closed** (no authored atom / unresolved ref / non-player-driven
//! nav / non-true state ⇒ no evidence); **reuse** EV-2 catalog + AcceptedEvidence/
//! EvidenceLedger + the EV-4 gateway.
//!
//! **Shadow**: all three append to an in-memory [`EvidenceLedger`] (the live wiring
//! shares ONE turn-local ledger across the exact path and the GM audit, so the same
//! observation never double-records — see [`project_content_delivery`]'s reuse of the
//! gateway's `evidence_id`). The engine consumes none of it yet (EV-APPLY).
//!
//! **OFF == byte-identical baseline**: [`progress_exact_projectors_enabled`] is
//! default OFF; the live wiring runs none of this and writes no ledger/event when off.

use serde_json::Value;
use trpg_model::adventure_ir::{
    AcceptedEvidence, AtomId, ContentDelivery, EvidenceAtomCatalog, EvidenceAtomSpec,
    EvidenceAuthority, EvidenceClaim, EvidenceKind, EvidenceLedger, EvidenceOfferSet,
};
use trpg_model::{DomainEvent, DomainEventKind, EffectPatchIntent, ModuleGraph, ScenarioNode, SourceRef};

use crate::evidence_gateway::{EvidenceGateway, GatewayInputs};

const PROGRESS_EXACT_PROJECTORS_V1_ENV: &str = "TRPG_PROGRESS_EXACT_PROJECTORS_V1";
const PROGRESS_EVIDENCE_V1_ENV: &str = "TRPG_PROGRESS_EVIDENCE_V1";

/// Pure flag parse (env-race-free; mirrors the EV-2/EV-3/EV-4 flags).
fn flag_on(raw: &str) -> bool {
    let v = raw.trim().to_ascii_lowercase();
    v == "1" || v == "true" || v == "on"
}

/// Whether the EV-P2 exact projectors are active. Default OFF == byte-identical
/// baseline (no projector runs, no ledger built, no GM schema/prompt change). ON when
/// EITHER the master `progress_evidence_v1` OR the `progress_exact_projectors_v1`
/// slice flag is truthy.
pub fn progress_exact_projectors_enabled() -> bool {
    std::env::var(PROGRESS_EXACT_PROJECTORS_V1_ENV)
        .map(|v| flag_on(&v))
        .unwrap_or(false)
        || std::env::var(PROGRESS_EVIDENCE_V1_ENV)
            .map(|v| flag_on(&v))
            .unwrap_or(false)
}

// ───────────────────────── Location (pure Rust) ─────────────────────────

/// A non-empty trimmed string field of a JSON object, or `None` (fail-closed).
fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Build one source-grounded `location:<id>` [`EvidenceAtomSpec`] for an authored
/// node id. `anchor_id` = the node id (always a real authored ref) + the page when
/// present, so the atom carries ≥1 SourceRef even when the page is unknown.
fn location_atom(module_id: &str, node_id: &str, page: Option<u32>) -> EvidenceAtomSpec {
    let bound = vec![node_id.to_string()];
    let atom_id = AtomId::from_parts(
        module_id,
        &page.map(|p| format!("p{p}")).unwrap_or_else(|| format!("loc:{node_id}")),
        EvidenceKind::LocationEntered,
        &bound,
    );
    EvidenceAtomSpec {
        atom_id,
        kind: EvidenceKind::LocationEntered,
        bindings: bound,
        source_refs: vec![SourceRef {
            source_id: module_id.to_string(),
            page,
            anchor_id: Some(node_id.to_string()),
            ..Default::default()
        }],
        grounding: format!("location:{node_id}"),
        progress_role: trpg_model::adventure_ir::ProgressRole::CarrierOnly,
    }
}

/// Build the [`EvidenceAtomCatalog`] of `location:<id>` atoms from the module graph's
/// **scene + location nodes** (topology authority — available now, no new catalog
/// dep). Structure-only (node id + optional page); ZERO ruleset/module name branching;
/// fail-closed (a node with no id is skipped — never a fabricated atom).
pub fn build_location_atom_catalog(graph: &ModuleGraph) -> EvidenceAtomCatalog {
    let mut catalog = EvidenceAtomCatalog::new();
    let module_id = graph.module_id.as_str();
    let scene_page = |n: &ScenarioNode| n.page_start;
    for node in &graph.scenes {
        let id = node.node_id.trim();
        if id.is_empty() {
            continue;
        }
        catalog.insert(location_atom(module_id, id, scene_page(node)));
    }
    for loc in &graph.locations {
        let Some(id) = str_field(loc, "id") else {
            continue;
        };
        let page = loc.get("page").and_then(Value::as_u64).map(|p| p as u32);
        catalog.insert(location_atom(module_id, &id, page));
    }
    catalog
}

/// Project committed `SceneTransitioned` events into `(LocationEntered, ExactDomain)`
/// evidence. Deterministic, no LLM, fail-closed: only a real committed
/// `SceneTransitioned` (player-driven nav) whose `to` destination resolves to an
/// authored `location:<id>` atom projects; no `to` / unresolved destination ⇒ none.
/// One evidence per (event, location); idempotent on replay.
pub fn project_location_entered(
    events: &[DomainEvent],
    location_catalog: &EvidenceAtomCatalog,
) -> EvidenceLedger {
    let mut ledger = EvidenceLedger::new();
    for ev in events {
        if ev.kind != DomainEventKind::SceneTransitioned {
            continue; // only player-driven nav carries LocationEntered
        }
        let Some(to) = str_field(&ev.data, "to") else {
            continue; // fail-closed: no destination
        };
        let Some(atom) = location_catalog.resolve_grounding(&format!("location:{to}")) else {
            continue; // fail-closed: destination is not an authored location node
        };
        ledger.append(AcceptedEvidence {
            evidence_id: format!("evd_loc_{}_{}", ev.event_id, to),
            atom_id: atom.atom_id.clone(),
            evidence_kind: EvidenceKind::LocationEntered,
            basis_event_ids: vec![ev.event_id.clone()],
            source_refs: atom.source_refs.clone(),
            turn_id: ev.turn_id.clone(),
            authority: EvidenceAuthority::ExactDomain,
        });
    }
    ledger
}

// ───────────────────────── State (pure Rust) ─────────────────────────

/// Build one source-grounded `state:<ref>` [`EvidenceAtomSpec`] for an authored
/// object-state / created-fact ref, anchored to the scene-mechanic's `source_anchor`.
fn state_atom(module_id: &str, state_ref: &str, anchor: &str) -> EvidenceAtomSpec {
    let bound = vec![state_ref.to_string()];
    let atom_id = AtomId::from_parts(
        module_id,
        &format!("state:{state_ref}@{anchor}"),
        EvidenceKind::StateEstablished,
        &bound,
    );
    EvidenceAtomSpec {
        atom_id,
        kind: EvidenceKind::StateEstablished,
        bindings: bound,
        source_refs: vec![SourceRef {
            source_id: module_id.to_string(),
            note: Some(format!("authored scene-mechanic effect: {anchor}")),
            ..Default::default()
        }],
        grounding: format!("state:{state_ref}"),
        progress_role: trpg_model::adventure_ir::ProgressRole::CarrierOnly,
    }
}

/// Build the [`EvidenceAtomCatalog`] of `state:<id>` atoms from authored scene-mechanic
/// effect leaves (`SetObjectState{object_id}` / `CreateFact{target}`) that carry a
/// non-empty `source_anchor`. Structure-only; fail-closed (no anchor / no ref ⇒ no
/// atom — never fabricated). **Scope note (EV-P2):** this covers ONLY the authored
/// object-state leaves already present in the graph; the broad observable-state
/// catalog (from objective leaves + explicit affordances + topology) lands in EV-P3.
pub fn build_state_atom_catalog(graph: &ModuleGraph) -> EvidenceAtomCatalog {
    let mut catalog = EvidenceAtomCatalog::new();
    let module_id = graph.module_id.as_str();
    for scene in &graph.scenes {
        for intent in &scene.scene_mechanics {
            let anchor = intent.source_anchor.trim();
            if anchor.is_empty() {
                continue; // fail-closed: no authored source span → no state atom
            }
            let patches = intent
                .effect_policy
                .on_success
                .iter()
                .chain(intent.effect_policy.on_failure.iter());
            for patch in patches {
                let state_ref = match patch {
                    EffectPatchIntent::SetObjectState { object_id, .. } => object_id.trim(),
                    EffectPatchIntent::CreateFact { target, .. } => target.trim(),
                    _ => continue, // only object-state / created-fact leaves are state atoms
                };
                if state_ref.is_empty() {
                    continue;
                }
                catalog.insert(state_atom(module_id, state_ref, anchor));
            }
        }
    }
    catalog
}

/// Whether a committed `WorldFactChanged` payload represents an **established-true**
/// authored state. `truth_status` (EV-1 carrier) takes priority; else `knowledge_state`
/// (knows_true/revealed); a bare authored-state commit (neither field) counts as
/// established. An explicit non-true marker ⇒ false (fail-closed).
fn state_is_established(data: &Value) -> bool {
    if let Some(t) = data.get("truth_status").and_then(Value::as_str) {
        return matches!(t.trim(), "true");
    }
    match data.get("knowledge_state").and_then(Value::as_str) {
        None => true,
        Some(s) => matches!(s.trim(), "knows_true" | "revealed"),
    }
}

/// Project committed `WorldFactChanged` events into `(StateEstablished, ExactDomain)`
/// evidence. Deterministic, no LLM, fail-closed: only a fact whose `fact_id` resolves
/// to an authored `state:<id>` atom AND is established-true projects. Synthetic
/// `wf_chk_*` ids never resolve (no authored-state atom) — fail-closed by construction.
pub fn project_state_established(
    events: &[DomainEvent],
    state_catalog: &EvidenceAtomCatalog,
) -> EvidenceLedger {
    let mut ledger = EvidenceLedger::new();
    for ev in events {
        if ev.kind != DomainEventKind::WorldFactChanged {
            continue;
        }
        let Some(fact_id) = str_field(&ev.data, "fact_id") else {
            continue;
        };
        if !state_is_established(&ev.data) {
            continue; // fail-closed: a non-true authored state is not established
        }
        let Some(atom) = state_catalog.resolve_grounding(&format!("state:{fact_id}")) else {
            continue; // fail-closed: synthetic / non-authored ref never resolves
        };
        ledger.append(AcceptedEvidence {
            evidence_id: format!("evd_st_{}_{}", ev.event_id, fact_id),
            atom_id: atom.atom_id.clone(),
            evidence_kind: EvidenceKind::StateEstablished,
            basis_event_ids: vec![ev.event_id.clone()],
            source_refs: atom.source_refs.clone(),
            turn_id: ev.turn_id.clone(),
            authority: EvidenceAuthority::ExactDomain,
        });
    }
    ledger
}

// ───────────────────────── Content delivery (GM ref, Rust verdict) ─────────────────────────

/// Project verified GM [`ContentDelivery`] references into `(FactLearned, ExactDomain)`
/// evidence. The GM emits only `{delivery_cap, recipient, basis}` (a closed schema —
/// it cannot name the fact/atom); **Rust** verifies it by reusing
/// [`EvidenceGateway::admit`] UNCHANGED (cap is in this turn's OfferSet, not expired,
/// right session/turn, basis resolves to a committed fact carrier of the bound
/// authored clue, source-grounded, not duplicate), then **re-stamps authority to
/// ExactDomain** (an explicit materialization is an exact producer, not a GM guess).
///
/// `seed` is the shared turn-local ledger (exact producers run first): the admitted
/// evidence reuses the gateway's `evidence_id`, so a later GM-audit Observed of the
/// SAME clue+basis collapses to the same entry (no double-emit; ExactDomain wins).
/// fail-closed: a delivery whose offered cap is not a fact kind (a clue), or that the
/// gateway rejects for any reason, projects nothing.
pub fn project_content_delivery(
    deliveries: &[ContentDelivery],
    session_id: &str,
    turn_id: &str,
    offer_set: &EvidenceOfferSet,
    catalog: &EvidenceAtomCatalog,
    committed_events: &[DomainEvent],
    seed: &EvidenceLedger,
) -> EvidenceLedger {
    let mut ledger = seed.clone();
    for d in deliveries {
        // recipient is closed to Player at the type level (DeliveryRecipient) — a
        // delivery to anyone else is unrepresentable, so no runtime check is needed.

        // "fact presentable": the offered capability must be an authored *clue* (a
        // fact kind). A location/state/other cap can never be a content delivery.
        let Some(offer) = offer_set.find(d.delivery_cap.as_str()) else {
            continue; // fail-closed: unknown cap (the gateway would also reject it)
        };
        if !matches!(offer.kind, EvidenceKind::FactLearned | EvidenceKind::FactRevealed) {
            continue; // fail-closed: not a presentable authored clue
        }

        // Reuse the EV-4 gateway UNCHANGED to verify cap/expiry/turn/basis/binding/
        // source/duplicate (cap+basis only — Rust resolves cap→atom).
        let claim = EvidenceClaim {
            cap_id: d.delivery_cap.clone(),
            basis: d.basis.clone(),
        };
        let result = {
            let inp = GatewayInputs {
                session_id,
                turn_id,
                offer_set,
                committed_events,
                catalog,
                ledger: &ledger,
            };
            EvidenceGateway::admit(&claim, &inp)
        };
        if let Ok(ev) = result {
            // An explicit authored-content materialization is an EXACT producer, not a
            // GM guess: re-stamp authority. The gateway's evidence_id is reused so the
            // GM-witnessed audit path dedups against it (no double-emit).
            ledger.append(AcceptedEvidence {
                authority: EvidenceAuthority::ExactDomain,
                ..ev
            });
        }
    }
    ledger
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use trpg_model::adventure_ir::{
        BasisKind, CapId, EvidenceOffer, NonEmpty, TurnLocalRef,
    };
    use trpg_model::mechanics::{EffectPolicy, SceneMechanicIntent};
    use trpg_model::ScenarioNode;

    const SESSION: &str = "sess_p2";
    const TURN: &str = "turn-uuid-real-1234";

    // ── flag ──────────────────────────────────────────────────────────────────
    #[test]
    fn flag_defaults_off() {
        assert!(!flag_on("0") && !flag_on("false") && !flag_on("off") && !flag_on(""));
        assert!(flag_on("1") && flag_on("true") && flag_on("on"));
    }

    // ── LocationEntered ─────────────────────────────────────────────────────────
    fn nav_graph() -> ModuleGraph {
        let scene = |id: &str, p: u32| ScenarioNode {
            node_id: id.into(),
            node_type: "scene".into(),
            page_start: Some(p),
            ..Default::default()
        };
        ModuleGraph {
            module_id: "homecoming".into(),
            scenes: vec![scene("scene_porch", 3), scene("scene_kitchen", 4)],
            locations: vec![json!({"id": "loc_attic", "page": 7})],
            ..Default::default()
        }
    }

    fn transitioned(event_id: &str, to: &str) -> DomainEvent {
        DomainEvent::new(
            event_id,
            SESSION,
            TURN,
            DomainEventKind::SceneTransitioned,
            json!({"to": to, "from": "scene_porch"}),
        )
    }

    #[test]
    fn location_catalog_covers_scene_and_location_nodes() {
        let cat = build_location_atom_catalog(&nav_graph());
        assert_eq!(cat.len(), 3, "two scenes + one location node");
        let a = cat.resolve_grounding("location:scene_kitchen").expect("scene atom");
        assert_eq!(a.kind, EvidenceKind::LocationEntered);
        assert_eq!(a.source_refs[0].page, Some(4), "scene page_start is the source span");
        let l = cat.resolve_grounding("location:loc_attic").expect("location atom");
        assert_eq!(l.source_refs[0].page, Some(7));
    }

    #[test]
    fn committed_nav_projects_one_location_entered() {
        let cat = build_location_atom_catalog(&nav_graph());
        let events = vec![transitioned("de_nav", "scene_kitchen")];
        let led = project_location_entered(&events, &cat);
        assert_eq!(led.len(), 1, "exactly one LocationEntered");
        let ev = &led.entries()[0];
        assert_eq!(ev.authority, EvidenceAuthority::ExactDomain);
        assert_eq!(ev.evidence_kind, EvidenceKind::LocationEntered);
        assert_eq!(ev.atom_id, cat.resolve_grounding("location:scene_kitchen").unwrap().atom_id);
        assert_eq!(ev.basis_event_ids, vec!["de_nav".to_string()]);
        assert_eq!(ev.turn_id, TURN);
        assert!(!ev.source_refs.is_empty(), "source-grounded");
    }

    #[test]
    fn nav_to_unauthored_destination_does_not_project() {
        let cat = build_location_atom_catalog(&nav_graph());
        let events = vec![transitioned("de_nav", "scene_not_in_graph")];
        assert!(
            project_location_entered(&events, &cat).is_empty(),
            "fail-closed: destination is not an authored location node"
        );
    }

    #[test]
    fn non_scene_transitioned_event_does_not_project_location() {
        // A player merely *saying* they'll go is not a committed SceneTransitioned, so
        // the carrier here is some other event kind ⇒ no location evidence.
        let cat = build_location_atom_catalog(&nav_graph());
        let events = vec![DomainEvent::new(
            "de_x",
            SESSION,
            TURN,
            DomainEventKind::WorldFactChanged,
            json!({"to": "scene_kitchen"}), // right field, WRONG kind
        )];
        assert!(
            project_location_entered(&events, &cat).is_empty(),
            "fail-closed: only a committed SceneTransitioned (real move) projects"
        );
    }

    #[test]
    fn replayed_nav_does_not_duplicate_location() {
        let cat = build_location_atom_catalog(&nav_graph());
        let e = transitioned("de_nav", "scene_kitchen");
        let led = project_location_entered(&[e.clone(), e], &cat);
        assert_eq!(led.len(), 1, "same (event, location) ⇒ no duplicate");
    }

    // ── StateEstablished ─────────────────────────────────────────────────────────
    fn state_graph() -> ModuleGraph {
        let intent = SceneMechanicIntent {
            intent_id: "pry_panel".into(),
            description: "pry the loading-dock access panel".into(),
            tested_parameter: "force".into(),
            difficulty: None,
            effect_policy: EffectPolicy {
                on_success: vec![EffectPatchIntent::SetObjectState {
                    object_id: "door_loading_dock".into(),
                    patch: json!({"state": "unlocked"}),
                }],
                on_failure: vec![],
            },
            source_anchor: "p12 §loading dock".into(),
        };
        let scene = ScenarioNode {
            node_id: "scene_dock".into(),
            node_type: "scene".into(),
            scene_mechanics: vec![intent],
            ..Default::default()
        };
        ModuleGraph {
            module_id: "the_vault".into(),
            scenes: vec![scene],
            ..Default::default()
        }
    }

    fn world_fact(event_id: &str, fact_id: &str, extra: Value) -> DomainEvent {
        let mut data = json!({"fact_id": fact_id});
        if let Value::Object(m) = extra {
            for (k, v) in m {
                data[k] = v;
            }
        }
        DomainEvent::new(event_id, SESSION, TURN, DomainEventKind::WorldFactChanged, data)
    }

    #[test]
    fn state_catalog_covers_authored_object_state_leaves() {
        let cat = build_state_atom_catalog(&state_graph());
        assert_eq!(cat.len(), 1, "one SetObjectState leaf ⇒ one state atom");
        let a = cat.resolve_grounding("state:door_loading_dock").expect("state atom");
        assert_eq!(a.kind, EvidenceKind::StateEstablished);
        assert!(!a.source_refs.is_empty(), "source-grounded by the authored anchor");
    }

    #[test]
    fn state_catalog_skips_mechanic_without_source_anchor() {
        let mut g = state_graph();
        g.scenes[0].scene_mechanics[0].source_anchor = "  ".into();
        assert!(
            build_state_atom_catalog(&g).is_empty(),
            "fail-closed: no authored source span ⇒ no fabricated state atom"
        );
    }

    #[test]
    fn committed_authored_state_projects_one_state_established() {
        let cat = build_state_atom_catalog(&state_graph());
        let events = vec![world_fact("de_st", "door_loading_dock", json!({"truth_status": "true"}))];
        let led = project_state_established(&events, &cat);
        assert_eq!(led.len(), 1);
        let ev = &led.entries()[0];
        assert_eq!(ev.authority, EvidenceAuthority::ExactDomain);
        assert_eq!(ev.evidence_kind, EvidenceKind::StateEstablished);
        assert_eq!(ev.atom_id, cat.resolve_grounding("state:door_loading_dock").unwrap().atom_id);
    }

    #[test]
    fn synthetic_wf_chk_world_fact_does_not_project_state() {
        let cat = build_state_atom_catalog(&state_graph());
        let events = vec![world_fact("de_chk", "wf_chk_check_2c160e4", json!({"truth_status": "true"}))];
        assert!(
            project_state_established(&events, &cat).is_empty(),
            "fail-closed: a synthetic non-authored fact has no state atom"
        );
    }

    #[test]
    fn non_true_authored_state_does_not_project() {
        let cat = build_state_atom_catalog(&state_graph());
        let events = vec![world_fact(
            "de_false",
            "door_loading_dock",
            json!({"truth_status": "false"}),
        )];
        assert!(
            project_state_established(&events, &cat).is_empty(),
            "fail-closed: an explicit non-true authored state is not established"
        );
    }

    // ── ContentDelivery ─────────────────────────────────────────────────────────
    const CLUE: &str = "clue_aquifer_commercial";

    fn clue_atom(clue_id: &str, page: u32) -> EvidenceAtomSpec {
        let atom_id = AtomId::from_parts(
            "the_vault",
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

    fn clue_offer_set(clue_id: &str, page: u32) -> (EvidenceOfferSet, EvidenceAtomCatalog) {
        let atom = clue_atom(clue_id, page);
        let mut set = EvidenceOfferSet::new(TURN);
        set.push(EvidenceOffer {
            cap_id: CapId::from_parts(SESSION, TURN, &atom.atom_id),
            atom_id: atom.atom_id.clone(),
            kind: EvidenceKind::FactLearned,
            meaning: format!("the player learned the authored clue \"{clue_id}\""),
            required_basis: vec![BasisKind::FactCommitted],
            expires_at: TURN.into(),
        });
        let mut cat = EvidenceAtomCatalog::new();
        cat.insert(atom);
        (set, cat)
    }

    fn learned_event(event_id: &str, fact_id: &str) -> DomainEvent {
        DomainEvent::new(
            event_id,
            SESSION,
            TURN,
            DomainEventKind::PlayerLearnedFact,
            json!({"fact_id": fact_id, "reason": "presented the storyboard"}),
        )
    }

    fn delivery(cap: &CapId, refs: Vec<TurnLocalRef>) -> ContentDelivery {
        ContentDelivery {
            delivery_cap: cap.clone(),
            recipient: trpg_model::adventure_ir::DeliveryRecipient::Player,
            basis: NonEmpty::from_vec(refs).unwrap(),
        }
    }

    #[test]
    fn verified_content_delivery_projects_one_fact_learned_exact() {
        let (set, cat) = clue_offer_set(CLUE, 8);
        let events = vec![learned_event("de_reveal", CLUE)];
        let d = delivery(&set.offers()[0].cap_id, vec![TurnLocalRef::Commit(0)]);
        let led = project_content_delivery(
            &[d],
            SESSION,
            TURN,
            &set,
            &cat,
            &events,
            &EvidenceLedger::new(),
        );
        assert_eq!(led.len(), 1);
        let ev = &led.entries()[0];
        assert_eq!(ev.authority, EvidenceAuthority::ExactDomain, "materialization is an exact producer");
        assert_eq!(ev.evidence_kind, EvidenceKind::FactLearned);
        assert_eq!(ev.atom_id, set.offers()[0].atom_id, "atom is Rust-resolved, never named by the GM");
        assert_eq!(ev.basis_event_ids, vec!["de_reveal".to_string()]);
        assert!(!ev.source_refs.is_empty(), "source-grounded");
    }

    #[test]
    fn content_delivery_with_unknown_cap_projects_nothing() {
        let (set, cat) = clue_offer_set(CLUE, 8);
        let events = vec![learned_event("de_reveal", CLUE)];
        let d = delivery(&CapId("cap_fabricated".into()), vec![TurnLocalRef::Commit(0)]);
        assert!(
            project_content_delivery(&[d], SESSION, TURN, &set, &cat, &events, &EvidenceLedger::new())
                .is_empty(),
            "fail-closed: a cap not in this turn's OfferSet projects nothing"
        );
    }

    #[test]
    fn content_delivery_with_uncommitted_basis_projects_nothing() {
        let (set, cat) = clue_offer_set(CLUE, 8);
        let events: Vec<DomainEvent> = vec![]; // commit:0 resolves to nothing
        let d = delivery(&set.offers()[0].cap_id, vec![TurnLocalRef::Commit(0)]);
        assert!(
            project_content_delivery(&[d], SESSION, TURN, &set, &cat, &events, &EvidenceLedger::new())
                .is_empty(),
            "fail-closed: the gateway rejects an unresolved basis"
        );
    }

    #[test]
    fn content_delivery_does_not_double_emit_against_gm_audit_path() {
        // The exact content-delivery path appends evidence_id X first; a later GM-audit
        // Observed of the SAME clue+basis runs the SAME gateway with the shared ledger,
        // which then rejects it as DuplicateEvidence ⇒ the ledger keeps exactly ONE
        // entry (the ExactDomain one wins; GmWitnessed never double-records).
        let (set, cat) = clue_offer_set(CLUE, 8);
        let events = vec![learned_event("de_reveal", CLUE)];
        let d = delivery(&set.offers()[0].cap_id, vec![TurnLocalRef::Commit(0)]);
        let exact_led = project_content_delivery(
            &[d],
            SESSION,
            TURN,
            &set,
            &cat,
            &events,
            &EvidenceLedger::new(),
        );
        assert_eq!(exact_led.len(), 1);

        // Now replay the SAME observation through the gateway seeded with the exact
        // ledger (this is what the live GM-audit wiring does with the shared ledger).
        let claim = EvidenceClaim {
            cap_id: set.offers()[0].cap_id.clone(),
            basis: NonEmpty::from_vec(vec![TurnLocalRef::Commit(0)]).unwrap(),
        };
        let inp = GatewayInputs {
            session_id: SESSION,
            turn_id: TURN,
            offer_set: &set,
            committed_events: &events,
            catalog: &cat,
            ledger: &exact_led,
        };
        assert_eq!(
            EvidenceGateway::admit(&claim, &inp),
            Err(crate::evidence_gateway::RejectionReason::DuplicateEvidence),
            "the GM-audit path dedups against the exact entry (no double-emit)"
        );
    }
}
