//! Player Lab (蓝图 §二/§三, §九 player/): persistent personas + the per-turn
//! decision loop. Deterministic and GM-text-driven so the §六 counterfactual can
//! prove the simulated player actually reads the GM.

mod counterfactual;
mod deliberate;
mod persona;
mod state;

pub use counterfactual::{
    audit_player_reads_gm, invariance, memory_alert, no_response_escalates, sensitivity,
    spoiler_leak, NoResponseResult, PlayerReadsGmReport, SensitivityResult,
};
pub use deliberate::{deliberate, perceive, Perception};
pub use persona::{PersonaKind, PersonaWeights, PlayerPersona};
pub use state::{ActionCandidate, ActionKind, PlayerDecision, SimulatedPlayerState};
