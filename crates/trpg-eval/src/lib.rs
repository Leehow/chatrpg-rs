//! `trpg-eval` — the static-transcript arm of the chatrpg evaluator.
//!
//! Reads a frozen 战报 markdown and judges it against the milestone-1 root causes
//! (蓝图 `design/测试设计.md` §十). Complements the DB-grounded live judges in
//! `harness/eval/`: those need a live session; this FAILs a recorded transcript
//! with root-cause + turn evidence.

pub mod aggregate;
pub mod contract;
pub mod model;
pub mod parser;
pub mod player;
pub mod probes;
pub mod report;
pub mod score;

pub use aggregate::evaluate;
pub use contract::{contract_for, contracts, FieldRequest, IntentField, ResponseContract};
pub use model::{EvalFinding, RootCause, Severity, Transcript, Turn, Verdict};
pub use parser::parse_transcript;
pub use player::{
    deliberate, ActionCandidate, ActionKind, PersonaKind, PersonaWeights, PlayerDecision,
    PlayerPersona, SimulatedPlayerState,
};
pub use report::redboard;
pub use score::{score_card, Dimension, DimensionScore, ScoreCard};
