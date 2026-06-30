//! Provider-free TRPG evaluation primitives.
//!
//! This crate is deliberately pure: it parses recorded player-visible fixtures,
//! runs deterministic auditors, and renders evidence-backed verdicts. It does
//! not drive the GM, call an LLM, or inspect hidden runtime state directly.

pub mod detectors;
pub mod model;
pub mod parser;
pub mod report;

pub use detectors::{evaluate_fixture, run_detectors};
pub use model::{
    EvalFinding, EvalFixture, EvalReport, EvalTurn, FindingCategory, FindingSeverity, GmResponse,
    PlayerDecision, ResponseContract, RollTrace, RubricDimension, RubricScore, TraceObservation,
    Verdict,
};
pub use parser::{parse_markdown_fixture, EvalParseError};
pub use report::render_markdown_report;
