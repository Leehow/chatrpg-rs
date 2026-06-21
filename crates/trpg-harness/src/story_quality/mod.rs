//! L2.x / L9.x — Story-quality checkpoints (pure, provider-free; no LLM, no DB).
//!
//! A family of typed spec + observed evidence + fail-closed terminal state + pure classifier,
//! all unit-testable without spawning the CLI, an LLM, or a database — mirroring the
//! mechanics-checkpoint families in `lib.rs`. Each checkpoint is an INDEPENDENT oracle: it
//! re-derives the invariant it guards and checks the emitted artifact against it, so it can
//! never be tautological with the production code it verifies.
//!
//! - [`beat`]      — checkpoint #1 (L2.1): the post-adjudication Beat reflects the committed result.
//! - [`railroad`]  — checkpoint #2 (L2.2): a rejected thread is never re-pushed (anti-railroad).
//! - [`reveal`]    — checkpoint #3 (L2.3): premature-reveal fail-closed (`gm_truth ∖ player_known`).
//!
//! Each module is kept under the 400-line discipline cap; this `mod.rs` re-exports the public
//! surface so consumers keep using `trpg_harness::story_quality::<Item>` unchanged.

pub mod beat;
pub mod railroad;

pub use beat::*;
pub use railroad::*;
