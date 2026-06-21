//! Story structure types (P5.1): commit-nothing typed state the narrative/Director
//! layer reasons over — story threads, planted promises, character arcs, recent beats,
//! pacing, and player-interest signals. Mirrors the proposal-type discipline of
//! [`crate::world_reaction`]:
//!
//! - **State view, not a write path.** Nothing here carries a DB handle, appends an
//!   event, or mutates anything. These are pure data the runtime loads / persists; the
//!   System Kernel owns the actual commit (设计4补充 §二-④).
//! - **Round-trips from partial JSON.** Containers carry `#[serde(default)]` so a partial
//!   payload deserializes to empty/zero defaults (fail-closed: an absent field never
//!   invents pressure or a phantom thread).
//! - **`validated()` fail-closed.** Out-of-range / NaN scores clamp to a sane range and
//!   id-less threads/promises drop, so a corrupt blob can never inject an un-addressable
//!   thread or a NaN that poisons later scoring.
//!
//! Blueprint: 设计4补充 §11.2-§11.4. The `PlayerInterestSignal.rejected` field is an
//! additive completion required for §二十四-#13 (player-rejected threads stay un-pushed).
use serde::{Deserialize, Serialize};

/// Whole-story state the Director reasons over. Every field `#[serde(default)]` so an
/// empty/partial blob round-trips to an empty (fail-closed) story that proposes nothing.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StoryState {
    #[serde(default)]
    pub active_threads: Vec<StoryThread>,
    #[serde(default)]
    pub promises: Vec<StoryPromise>,
    #[serde(default)]
    pub character_arcs: Vec<CharacterArcState>,
    #[serde(default)]
    pub recent_beats: Vec<BeatRecord>,
    #[serde(default)]
    pub pacing: PacingState,
    #[serde(default)]
    pub player_interests: Vec<PlayerInterestSignal>,
}

/// One live dramatic thread. Scores are normalized `0.0..=1.0` (clamped by `validated()`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StoryThread {
    pub thread_id: String,
    #[serde(default)]
    pub premise: String,
    #[serde(default)]
    pub dramatic_question: String,
    #[serde(default)]
    pub stakes: Vec<String>,
    #[serde(default)]
    pub participant_ids: Vec<String>,
    #[serde(default)]
    pub related_fact_ids: Vec<String>,
    #[serde(default)]
    pub status: StoryThreadStatus,
    #[serde(default)]
    pub urgency: f32,
    #[serde(default)]
    pub momentum: f32,
    #[serde(default)]
    pub player_interest: f32,
    #[serde(default)]
    pub unresolved_questions: Vec<String>,
    #[serde(default)]
    pub unresolved_consequences: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_touched_turn: Option<String>,
    // ── L3.3 §五 field parity (additive, serde(default); old snapshots deserialize cleanly) ──
    /// Short human-facing label for the thread (free-form). Empty when absent.
    #[serde(default)]
    pub title: String,
    /// Where this thread came from (provenance). Fail-closed default `Unspecified`.
    #[serde(default)]
    pub origin: StoryThreadOrigin,
    /// The module [`NarrativeAnchor`](crate) id that seeded this thread, if any (L8.x). `None`
    /// for emergent/player-driven threads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_anchor: Option<String>,
    /// Candidate payoff descriptors/ids this thread could pay off into (director MATERIAL, not a
    /// script). Empty when none proposed.
    #[serde(default)]
    pub payoff_candidates: Vec<String>,
}

/// Where a [`StoryThread`] originated. `Unspecified` is the fail-closed default — an absent
/// origin never asserts a concrete provenance. Generic (NOT branched on ruleset/module, §二-⑪).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoryThreadOrigin {
    #[default]
    Unspecified,
    /// Seeded from a parsed module narrative anchor (L8.x).
    ModuleAnchor,
    /// Emerged from a player action / expressed interest.
    PlayerDriven,
    /// Surfaced by world dynamics / the Director (not authored, not player-initiated).
    Emergent,
}

/// Lifecycle of a [`StoryThread`]. `Dormant` is the fail-closed default (an unknown /
/// absent status never reads as live pressure).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoryThreadStatus {
    #[default]
    Dormant,
    Introduced,
    Active,
    Escalating,
    ReadyForPayoff,
    Resolved,
    Abandoned,
    Transformed,
}

/// A planted promise/setup awaiting payoff. Solves the LLM failure modes of dangling
/// foreshadowing, un-setup reveals, and forgotten old events (设计4补充 §11.4).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StoryPromise {
    pub promise_id: String,
    #[serde(default)]
    pub setup_event_ids: Vec<String>,
    #[serde(default)]
    pub expected_payoff_kind: PayoffKind,
    #[serde(default)]
    pub status: PromiseStatus,
    /// How ripe the promise is for payoff, normalized `0.0..=1.0`.
    #[serde(default)]
    pub maturity: f32,
    #[serde(default)]
    pub payoff_candidate_fact_ids: Vec<String>,
    // ── L3.3 §五 field parity (additive, serde(default); old snapshots deserialize cleanly) ──
    /// The owning [`StoryThread`] id, if this promise belongs to a thread. Empty when free-floating.
    #[serde(default)]
    pub thread_id: String,
    /// Free-form description of the setup that planted this promise. Empty when absent.
    #[serde(default)]
    pub setup: String,
    /// The earliest turn id at which this promise should be eligible to pay off (a soft floor),
    /// if known. `None` ⇒ no floor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub earliest_payoff_turn: Option<String>,
    /// What happens to the promise once it is overdue. Fail-closed default `Never` — a setup is
    /// never silently dropped unless an explicit policy says so.
    #[serde(default)]
    pub expiry_policy: ExpiryPolicy,
    /// The domain-event id that actually paid this promise off, once it has. `None` while unpaid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payoff_event_id: Option<String>,
}

/// What happens to a [`StoryPromise`] once it passes its `earliest_payoff_turn` without being
/// paid off. `Never` is the fail-closed default — we never silently drop a planted setup unless
/// an explicit policy says so. Generic (NOT branched on ruleset/module, §二-⑪).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpiryPolicy {
    #[default]
    Never,
    /// Past due it loses maturity/pressure but persists (a soft fade, still payable).
    Soft,
    /// Past due it breaks (transitions toward `PromiseStatus::Broken`).
    Hard,
}

/// The shape a promise's payoff is expected to take. `Unspecified` is the fail-closed
/// default — an absent kind never asserts a concrete payoff direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PayoffKind {
    #[default]
    Unspecified,
    /// A hidden truth comes to light.
    Revelation,
    /// A confrontation / clash the setup pointed toward.
    Confrontation,
    /// A reunion or relationship resolution.
    Reunion,
    /// A loss / cost the setup foreshadowed.
    Loss,
    /// A reward / triumph the setup earned.
    Triumph,
    /// A reversal / twist on the expectation.
    Twist,
}

/// Lifecycle of a [`StoryPromise`]. `Planted` is the fail-closed default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromiseStatus {
    #[default]
    Planted,
    Developing,
    Ripe,
    PaidOff,
    Broken,
}

/// One character's arc progress. Scores normalized `0.0..=1.0` (clamped by `validated()`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CharacterArcState {
    pub character_id: String,
    #[serde(default)]
    pub arc_premise: String,
    #[serde(default)]
    pub current_stage: String,
    /// Progress along the arc, normalized `0.0..=1.0`.
    #[serde(default)]
    pub progress: f32,
    #[serde(default)]
    pub want: String,
    #[serde(default)]
    pub need: String,
    #[serde(default)]
    pub related_thread_ids: Vec<String>,
    // ── L3.4 §五 CharacterArcState rebuild (additive, serde(default); old snapshots deserialize) ──
    /// How overdue this character is for spotlight (a debt magnitude, normalized `0.0..=1.0` by
    /// `validated()`). High ⇒ the Director should weight this character up (L5.2).
    #[serde(default)]
    pub spotlight_debt: f32,
    /// Desires the character has expressed in play (free-form). Material for beat selection.
    #[serde(default)]
    pub expressed_desires: Vec<String>,
    /// Personal hooks raised but not yet resolved (free-form).
    #[serde(default)]
    pub unresolved_personal_hooks: Vec<String>,
    /// Relationship ids the Director should treat as load-bearing for this character.
    #[serde(default)]
    pub important_relationship_ids: Vec<String>,
    /// Recent meaningful choices this character made (free-form, newest-last by convention).
    #[serde(default)]
    pub recent_choices: Vec<String>,
    /// Conflicts that keep recurring for this character (free-form).
    #[serde(default)]
    pub recurring_conflicts: Vec<String>,
    /// Coarse emotional-direction token (e.g. `hardening`); free-form, snake_case at the producer.
    #[serde(default)]
    pub emotional_direction: String,
}

/// A record of one narrative beat that recently played. Provenance only — no resolution.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct BeatRecord {
    #[serde(default)]
    pub beat_kind: BeatKind,
    #[serde(default)]
    pub turn_id: String,
    #[serde(default)]
    pub thread_id: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub source_event_ids: Vec<String>,
}

/// Kinds of narrative beat a turn may play. `Respond` is the fail-closed default (a plain
/// reaction to the player — never invents escalation or a payoff on a partial blob).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BeatKind {
    #[default]
    Respond,
    Reveal,
    Complicate,
    Consequence,
    Choice,
    Reaction,
    Callback,
    Foreshadow,
    Escalate,
    Relief,
    Payoff,
    Transition,
}

/// Pacing read used to modulate beat selection. Scores normalized `0.0..=1.0`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PacingState {
    /// Current dramatic tension, normalized `0.0..=1.0`.
    #[serde(default)]
    pub tension: f32,
    /// How long since the last release/relief, normalized `0.0..=1.0`.
    #[serde(default)]
    pub time_since_relief: f32,
    /// Turns since the last payoff/escalation beat (>=0).
    #[serde(default)]
    pub beats_since_escalation: u32,
    /// Coarse pacing phase token (e.g. `rising`); free-form, snake_case at the producer.
    #[serde(default)]
    pub phase: String,
}

/// A signal of how much the player engaged with a thread. `rejected` records an explicit
/// player rebuff so the Director never re-pushes a refused thread (§二十四-#13).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PlayerInterestSignal {
    pub thread_id: String,
    #[serde(default)]
    pub signal: InterestSignal,
    /// The player explicitly refused/disengaged from this thread. Required for §24-#13.
    #[serde(default)]
    pub rejected: bool,
    /// Strength of the signal, normalized `0.0..=1.0`.
    #[serde(default)]
    pub strength: f32,
    #[serde(default)]
    pub source_event_ids: Vec<String>,
}

/// Direction of a player-interest read. `Neutral` is the fail-closed default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterestSignal {
    #[default]
    Neutral,
    Engaged,
    Curious,
    Invested,
    Disengaged,
    Avoidant,
}

/// Clamp an `f32` score to `0.0..=1.0`, mapping NaN to `0.0` (fail-closed: a corrupt score
/// never reads as max pressure).
fn clamp_unit(v: f32) -> f32 {
    if v.is_nan() {
        0.0
    } else {
        v.clamp(0.0, 1.0)
    }
}

impl StoryState {
    /// Fail-closed normalization: clamp every normalized score to `0.0..=1.0` (NaN→0.0)
    /// and drop threads/promises/arcs/signals whose primary id is empty — an
    /// un-addressable thread can never be selected, so carrying it only risks phantom
    /// pressure. Pure: consumes and returns a cleaned copy, mutates no external state.
    pub fn validated(mut self) -> Self {
        self.active_threads.retain(|t| !t.thread_id.is_empty());
        for t in &mut self.active_threads {
            t.urgency = clamp_unit(t.urgency);
            t.momentum = clamp_unit(t.momentum);
            t.player_interest = clamp_unit(t.player_interest);
        }
        self.promises.retain(|p| !p.promise_id.is_empty());
        for p in &mut self.promises {
            p.maturity = clamp_unit(p.maturity);
        }
        self.character_arcs.retain(|a| !a.character_id.is_empty());
        for a in &mut self.character_arcs {
            a.progress = clamp_unit(a.progress);
            a.spotlight_debt = clamp_unit(a.spotlight_debt); // L3.4: new field, default 0.0
        }
        self.player_interests.retain(|s| !s.thread_id.is_empty());
        for s in &mut self.player_interests {
            s.strength = clamp_unit(s.strength);
        }
        self.pacing.tension = clamp_unit(self.pacing.tension);
        self.pacing.time_since_relief = clamp_unit(self.pacing.time_since_relief);
        self
    }

    /// True when the story carries no live content at all (every channel empty).
    pub fn is_empty(&self) -> bool {
        self.active_threads.is_empty()
            && self.promises.is_empty()
            && self.character_arcs.is_empty()
            && self.recent_beats.is_empty()
            && self.player_interests.is_empty()
    }
}
