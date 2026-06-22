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

mod content_unit;
mod effect;
mod objective;
mod predicate;
mod relation;

pub use content_unit::{
    ContentUnit, DeliveryPolicy, FacetKind, UnitKind, VisibilityPolicy,
};
pub use effect::{EffectExpr, ScoreEffect};
pub use objective::{
    NormalizationStatus, ObjectiveSpec, ProgressRule, TrackerKind, TrackerRef, TrackerSpec,
};
pub use predicate::{
    EvalContext, EventPattern, IrValue, KnowledgeHolder, ObjectiveStatus, PredicateExpr,
    PredicateValue,
};
pub use relation::{Authority, Enforcement, Relation, RelationKind};
