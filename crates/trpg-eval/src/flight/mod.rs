//! Flight-Recorder arm (蓝图 §九/§十 第三阶段 = 验收3): read the runtime's own
//! per-turn contribution trail and attribute each defect to the producing layer.
//! Complements the static-transcript arm (final text only) with structured,
//! per-stage attribution to the eight §十 layers.

pub mod attribution;
pub mod record;

pub use attribution::{
    attribute, layer_of, redboard as flight_redboard, AttributionKind, LayerFinding, LayerVerdict,
};
pub use record::{Contribution, FlightRecording, FlightTurn};
