//! The GM run-kit — what an LLM-GM needs to RUN any game (NOT a combat-formula
//! pack). Filled by the reader agent, answering 6 questions each source-backed.

use serde::{Deserialize, Serialize};

/// Machine-readable resolution core — generated in the SAME reading pass (the
/// agent already read the rules pages). Feeds the engine's deterministic
/// resolvers (RuleKernel.dice_core/check_model/resource_tracks + the combat
/// formula compiler). `derived_formulas` use the engine's DerivedValue shape.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CoreRules {
    /// the core dice, e.g. "1d10", "1d100", "6d4", "2d6"
    #[serde(default)]
    pub dice: String,
    /// "roll_high" | "roll_under" | "pool_count"
    #[serde(default)]
    pub direction: String,
    /// what the roll is compared against, e.g. "DV (GM-set/by range table)", "skill%", "count 3s"
    #[serde(default)]
    pub compare_to: String,
    /// how success is read, e.g. "total >= DV; tie -> defender"
    #[serde(default)]
    pub success_rule: String,
    /// MACHINE-READABLE success operator (so the engine resolves without
    /// re-parsing prose). One of: "meet_or_beat" (total >= target),
    /// "roll_under" (total <= target), "count_faces" (count dice showing
    /// `target_face`, succeed when count >= `success_threshold`).
    #[serde(default)]
    pub compare: Option<String>,
    /// for pool_count/count_faces: which die face counts as a success (e.g. 3).
    #[serde(default)]
    pub target_face: Option<i32>,
    /// for pool_count/count_faces: how many counted dice are needed to succeed.
    #[serde(default)]
    pub success_threshold: Option<i32>,
    /// for meet_or_beat/roll_under with a fixed number: the static target value
    /// (omit when the target is per-skill/per-DV-table and resolved elsewhere).
    #[serde(default)]
    pub target_number: Option<i32>,
    /// graded success bands for games with success LEVELS (e.g. CoC
    /// critical/extreme/hard/regular/fumble; D&D nat20/nat1; PbtA 10+/7-9/6-).
    /// Machine-readable, evaluated against the resolved roll vs its target:
    ///   [{id, label, rank:int(higher=better), test:{kind, ...}}]
    /// where test.kind ∈ "roll_under_or_equal" | "roll_under_fraction"(denominator)
    ///   | "meet_or_beat_fraction"(numerator,denominator) | "exact"(value)
    ///   | "in_range"(min,max) | "otherwise", with an optional guard
    ///   when/unless {target_lt|target_gte}. Empty for pure pass/fail games.
    #[serde(default)]
    pub success_bands: Vec<serde_json::Value>,
    /// signature state tracks: [{name, kind, zero_means}]
    #[serde(default)]
    pub resource_tracks: Vec<serde_json::Value>,
    /// executable formulas, engine DerivedValue shape:
    /// [{field_id, formula, depends_on[], evaluator, notes}]
    #[serde(default)]
    pub derived_formulas: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GmRunKit {
    /// genre/tone/premise, what players do, the center of gravity
    #[serde(default)]
    pub game_identity: String,
    /// dice; roll-high or roll-under; vs what; modifiers; success rule
    #[serde(default)]
    pub core_resolution: String,
    /// what depletes/matters (hp/sanity/chaos/humanity/...) + what zero means
    #[serde(default)]
    pub state_tracks: String,
    /// what a PC is + the minimal path to have one (pregen/quickstart/build)
    #[serde(default)]
    pub character: String,
    /// the modes this game has + each one's centrality (none..central)
    #[serde(default)]
    pub subsystem_map: String,
    /// how the GM frames scenes / when to roll / minimal one-session prep
    #[serde(default)]
    pub gm_procedures: String,
    /// the pages actually read (provenance)
    #[serde(default)]
    pub source_pages: String,
    /// machine-readable resolution core (feeds the deterministic engine path)
    #[serde(default)]
    pub core: CoreRules,
    /// COMPLETE character sheet template JSON (fields, sections, derived_values
    /// formulas, creation_flow, validation_rules) from the deep character branch.
    /// Routed through coerce_character_template at parse time.
    #[serde(default)]
    pub character_template: serde_json::Value,
    /// Character option catalogs (Roles/skills/starter gear, starter-complete;
    /// long lists as locators). Routed into CharacterOnboardingPack.
    #[serde(default)]
    pub option_catalogs: serde_json::Value,
}
