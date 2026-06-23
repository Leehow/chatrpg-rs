//! Adventure IR — the unified intermediate representation that makes "progress" a
//! first-class runtime dimension (设计评审 `GPTpro-设计评审-AdventureIR.md` +
//! `AdventureIR-怎么做-主管评估.md`).
//!
//! Design law honored here:
//! - **Unified base + orthogonal facets + ECA rules** — one identity/source/
//!   predicate/effect/state vocabulary; movement/causality/clue/score/time/agenda
//!   are NOT force-unified into one edge type.
//! - **fail-closed at the EXECUTION layer**: authored-but-unnormalized conditions
//!   are stored as [`PredicateExpr::OpaqueAuthoredText`] (never auto-fired by
//!   Rust), not dropped — so recall is preserved.
//! - **Rust evaluates guards, LLM only ranks/focuses** within the legal frontier.
//! - **Structure branches (fingerprint/facets), never ruleset/module name
//!   branches.**
//!
//! All types are additive and unreferenced by the live pipeline until the
//! flag-gated producer/consumer lanes (P0-1.. P1-3) wire them in; defining them
//! here keeps `trpg-model` the leaf that both producer and runtime depend on.

mod authored_observation;
mod content_delivery;
mod content_unit;
mod effect;
mod evidence;
mod evidence_audit;
mod evidence_claim;
mod evidence_offer;
mod hierarchy;
mod mission_scoring;
mod mission_template;
mod mission_text;
mod objective;
mod predicate;
mod prep_packet_objectives;
mod progress_signal;
mod relation;

pub use authored_observation::{
    classify_verb, compile_authored_observations, compile_objective_leaves, is_tautology,
    parse_action_phrase, GraphRefIndex, ProgressRole,
};
pub use content_delivery::{ContentDelivery, DeliveryRecipient};
pub use content_unit::{
    ContentUnit, DeliveryPolicy, FacetKind, UnitKind, VisibilityPolicy,
};
pub use effect::{EffectExpr, ScoreEffect};
pub use evidence::{
    AcceptedEvidence, AtomId, EvidenceAtomCatalog, EvidenceAtomSpec, EvidenceAuthority,
    EvidenceKind, EvidenceLedger,
};
pub use evidence_audit::{EvidenceAudit, NotObservedReason, WitnessDecision};
pub use evidence_claim::{EvidenceClaim, NonEmpty, TurnLocalRef};
pub use evidence_offer::{BasisKind, CapId, EvidenceOffer, EvidenceOfferSet};
pub use hierarchy::{derive_content_units, project_chapters, unit_kind_for};
pub use mission_scoring::{
    parse_aftermath_outcomes, parse_chaos_tracker, parse_optional_objectives,
};
pub use mission_template::{
    campaign_root, induct_missions, mission_content_units, project_missions, MissionPage,
    MissionSpec,
};
pub use mission_text::pages_from_markdown;
pub use objective::{
    NormalizationStatus, ObjectiveSpec, ProgressRule, TrackerKind, TrackerRef, TrackerRung,
    TrackerSpec,
};
pub use predicate::{
    EvalContext, EventPattern, IrValue, KnowledgeHolder, ObjectiveStatus, PredicateExpr,
    PredicateValue,
};
pub use prep_packet_objectives::{
    compile_prep_packet_guard_leaves, objectives_from_prep_packet, prep_packet_binding_hints,
};
pub use progress_signal::{J3Axis, ProgressSignal, ProgressSignalKind};
pub use relation::{Authority, Enforcement, Relation, RelationKind};
