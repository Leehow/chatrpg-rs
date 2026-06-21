//! Pure Director story-core (P5.3/P5.4): the commit-nothing decision layer that turns
//! a [`StoryState`] + a pool of [`WorldReactionCandidate`]s into a typed
//! [`trpg_model::DirectorPlan`]. Every function here is PURE — no DB, no async, no env
//! reads, no LLM, deterministic — and produces a plan only: it commits nothing, resolves
//! nothing, rolls no dice, and reveals nothing directly (设计4补充 §11.8). The Kernel owns
//! all follow-through.
//!
//! ## Layout
//! - [`select`] — [`build_director_brief_packet`]: the 12-term linear scorer + four pure
//!   invariants (pool-filter / reveal-gating fail-closed / pool-empty→WorldQuery /
//!   anti-railroad). §二十四 rows #3, #10/#8, #13.
//! - [`fallback`] — [`fallback_beat_plan`] + [`pick_spotlight_target`]: a playable plan
//!   when the [`StoryState`] is empty, and min-spotlight target selection (§二十四-#2).
//!
//! ## Zero hardcoding (FRAMEWORK §1)
//! Scoring weights are a single UNIVERSAL `GENERIC` default const vector — never branched
//! on `ruleset_id` / `module_id` (§二-⑪ forbidden) and never read from
//! `DirectorModuleConfig` (it carries no weight fields, so reading it would be scope
//! creep). See [`select`] for the documented weight table and the future data-driven
//! override TODO.
//!
//! ## Packet content safety (codex fold #2/#3)
//! The emitted plan carries only ids / enums / short generic structural strings — NEVER
//! secret prose or fact bodies. Reveals are by `fact_id`, not fact text.

pub mod fallback;
pub mod render;
pub mod outcome;
pub mod scene_plan;
pub mod select;

pub use fallback::{fallback_beat_plan, pick_spotlight_target};
pub use scene_plan::{derive_scene_plan, scene_plan_enabled, ScenePlan};
pub use outcome::{
    apply_committed_outcome, DESIRED_CHANGE_CAPITALIZE_SUCCESS, DESIRED_CHANGE_FAIL_FORWARD,
};
pub use render::render_director_packet_block;
pub use select::build_director_brief_packet;
