//! L2.x / L9.x — Story-quality checkpoints (pure, provider-free; no LLM, no DB).
//!
//! A family of typed spec + observed evidence + fail-closed terminal state + pure classifier,
//! all unit-testable without spawning the CLI, an LLM, or a database — mirroring the
//! mechanics-checkpoint families in `lib.rs`. Each checkpoint is an INDEPENDENT oracle: it
//! re-derives the invariant it guards and checks the emitted artifact against it, so it can
//! never be tautological with the production code it verifies.
//!
//! - [`beat`]          — checkpoint #1 (L2.1): the post-adjudication Beat reflects the committed result.
//! - [`railroad`]      — checkpoint #2 (L2.2): a rejected thread is never re-pushed (anti-railroad).
//! - [`reveal`]        — checkpoint #3 (L2.3): premature-reveal fail-closed (`gm_truth ∖ player_known`).
//! - [`unpaid_setup`]  — checkpoint #4 (L9.1): a ripe, overdue promise is never left unpaid.
//! - [`beat_repeat`]   — checkpoint #5 (L9.1): a beat kind is not repeated past a run cap (monotony).
//! - [`scene_restate`] — checkpoint #6 (L9.1): a scene is not restated with no progress (treading water).
//! - [`consequence`]   — checkpoint #7 (L9.1): every committed choice gets a downstream consequence.
//! - [`spotlight`]     — checkpoint #8 (L9.1): a high spotlight-debt PC is not perpetually sidelined.
//! - [`narrator_mechanics`] — L6.2: the split Narrator authors no mechanics unbacked by the ledger.
//!
//! Each module is kept under the 400-line discipline cap; this `mod.rs` re-exports the public
//! surface so consumers keep using `trpg_harness::story_quality::<Item>` unchanged.

pub mod beat;
pub mod beat_repeat;
pub mod consequence;
pub mod narrator_mechanics;
pub mod railroad;
pub mod reveal;
pub mod scene_restate;
pub mod spotlight;
pub mod unpaid_setup;

pub use beat::*;
pub use beat_repeat::*;
pub use consequence::*;
pub use narrator_mechanics::*;
pub use railroad::*;
pub use reveal::*;
pub use scene_restate::*;
pub use spotlight::*;
pub use unpaid_setup::*;
