//! Shared tri-state flag for the materialization / affordance contract.
//!
//! `TRPG_MATERIALIZATION_AFFORDANCE` gates the additive layer that promotes
//! *source-present* module content (referenced NPCs, clue bodies, persona prose)
//! into discoverable runtime objects (see `MATERIALIZATION_DESIGN.md`, M1–M6).
//!
//! Mirrors `KnowledgeKernelMode` (memory_proposal.rs): a pure-parsed tri-state
//! that is **`Off` by default** so OFF == byte-equal baseline. It lives in
//! `trpg-model` because both the runtime (activation / clue affordance) and the
//! `trpg-material` crate (content policy / strict gate) must read the same flag.
//!
//! Constitution note (Rule 11 / §二-⑪): this flag is GENERIC — it never branches
//! on `ruleset_id` / `module_id`. It only switches the additive behavior on/off.

/// Environment variable name for the materialization/affordance flag.
pub const MATERIALIZATION_AFFORDANCE_ENV: &str = "TRPG_MATERIALIZATION_AFFORDANCE";

/// Tri-state activation mode for the materialization/affordance layer.
///
/// - `Off`     — default. Strict no-op; OFF == baseline byte-equal. No activation
///               derivation, no clue affordance, no content materialization demand.
/// - `Shadow`  — compute + audit-log the affordance decisions (e.g. to
///               `material_hydration_events`) but produce **no player-visible
///               effect** — used to observe what WOULD materialize.
/// - `Enforce` — fully active: derive activation, materialize source-present
///               content, drive clue reveal nominations through the commit boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterializationAffordanceMode {
    Off,
    Shadow,
    Enforce,
}

impl MaterializationAffordanceMode {
    /// Read the mode from the process environment.
    pub fn from_env() -> Self {
        Self::parse(&std::env::var(MATERIALIZATION_AFFORDANCE_ENV).unwrap_or_default())
    }

    /// Pure parse of the flag value (kept separate from `from_env` so it is
    /// testable without mutating the process-global environment — env-race-free
    /// per the flake discipline).
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "on" | "shadow" => MaterializationAffordanceMode::Shadow,
            "enforce" => MaterializationAffordanceMode::Enforce,
            // unset / "0" / "off" / "false" / anything unrecognized → baseline.
            _ => MaterializationAffordanceMode::Off,
        }
    }

    /// Any non-Off mode: the additive affordance layer participates (compute /
    /// audit at minimum). Callers must still gate *player-visible* effects on
    /// [`Self::is_enforce`].
    pub fn is_on(self) -> bool {
        !matches!(self, MaterializationAffordanceMode::Off)
    }

    /// Enforce mode: the affordance produces player-visible effects (activation
    /// derivation, content materialization, committed clue reveals).
    pub fn is_enforce(self) -> bool {
        matches!(self, MaterializationAffordanceMode::Enforce)
    }

    /// Off mode (baseline). Convenience inverse of [`Self::is_on`].
    pub fn is_off(self) -> bool {
        matches!(self, MaterializationAffordanceMode::Off)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_and_off_aliases_parse_off() {
        for raw in ["", "0", "off", "OFF", "false", "no", "enabled", "garbage", "  "] {
            assert_eq!(
                MaterializationAffordanceMode::parse(raw),
                MaterializationAffordanceMode::Off,
                "{raw:?} must parse Off (baseline fail-safe)"
            );
        }
    }

    #[test]
    fn shadow_aliases_parse_shadow() {
        for raw in ["1", "true", "on", "shadow", "ON", "Shadow", " true "] {
            assert_eq!(
                MaterializationAffordanceMode::parse(raw),
                MaterializationAffordanceMode::Shadow,
                "{raw:?} must parse Shadow"
            );
        }
    }

    #[test]
    fn enforce_parses_enforce() {
        for raw in ["enforce", "ENFORCE", " Enforce "] {
            assert_eq!(
                MaterializationAffordanceMode::parse(raw),
                MaterializationAffordanceMode::Enforce,
                "{raw:?} must parse Enforce"
            );
        }
    }

    #[test]
    fn is_on_and_is_enforce_semantics() {
        assert!(!MaterializationAffordanceMode::Off.is_on());
        assert!(!MaterializationAffordanceMode::Off.is_enforce());
        assert!(MaterializationAffordanceMode::Shadow.is_on());
        assert!(!MaterializationAffordanceMode::Shadow.is_enforce());
        assert!(MaterializationAffordanceMode::Enforce.is_on());
        assert!(MaterializationAffordanceMode::Enforce.is_enforce());
    }
}
