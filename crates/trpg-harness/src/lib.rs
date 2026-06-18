//! Pure, provider-free journey-harness foundation.
//!
//! These primitives let the `journey-prelude-character-session` journey reach a
//! terminal classification without spawning a process, touching the network, or
//! requiring an LLM provider. Three concerns, one per module:
//!
//! - [`prelude`]: character-creation readiness + persisted/session-binding
//!   verification verdicts (TC-JRNY-00).
//! - [`checkpoint`]: check/dice evidence classification and the human-input /
//!   roll-request guards, fail-closed against false-`PASS` (TC-JRNY-01).
//! - [`cassette`]: one scenario identity + evidence schema driving deterministic,
//!   live, and replay executors, plus the accepted-live-run cassette writer
//!   (TC-JRNY-02).
//!
//! The `trpg-harness` binary keeps its own black-box playtest runner; this
//! library is the provider-free decision core that the journey scenarios and
//! integration tests exercise directly.

pub mod cassette;
pub mod checkpoint;
pub mod prelude;

pub use cassette::*;
pub use checkpoint::*;
pub use prelude::*;
