//! Materialization affordance mode flag — controls which materialization paths are
//! active for the current process.  Reads from `TRPG_MATERIALIZATION_AFFORDANCE` env
//! var (OFF | enforce; default = OFF so the baseline is byte-equal to pre-M5).
//!
//! Use `MaterializationAffordanceMode::from_env()` at each call-site that needs the
//! gate; it is intentionally cheap (one `std::env::var` read).

/// Runtime mode that gates extended materialization affordances (M5 content tier).
///
/// `Off`     — baseline, no new paths active (byte-equal to pre-M5).
/// `Enforce` — player-visible affordances on; 4b provisional content tier enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterializationAffordanceMode {
    Off,
    Enforce,
}

impl MaterializationAffordanceMode {
    /// Read from `TRPG_MATERIALIZATION_AFFORDANCE` env var.
    /// Values `enforce` / `1` / `true` / `yes` → Enforce; anything else → Off.
    pub fn from_env() -> Self {
        Self::from_str(
            &std::env::var("TRPG_MATERIALIZATION_AFFORDANCE").unwrap_or_default()
        )
    }

    /// Parse from a string value (exposed for testing without env var races).
    pub fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "enforce" | "1" | "true" | "yes" => Self::Enforce,
            _ => Self::Off,
        }
    }

    /// Returns `true` when player-visible affordances are active.
    pub fn is_enforce(self) -> bool {
        matches!(self, Self::Enforce)
    }

    /// Returns `true` when the baseline (off) mode is active.
    pub fn is_off(self) -> bool {
        matches!(self, Self::Off)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_variant_is_off() {
        assert!(MaterializationAffordanceMode::Off.is_off());
        assert!(!MaterializationAffordanceMode::Off.is_enforce());
    }

    #[test]
    fn enforce_variant_is_enforce() {
        assert!(MaterializationAffordanceMode::Enforce.is_enforce());
        assert!(!MaterializationAffordanceMode::Enforce.is_off());
    }

    #[test]
    fn parse_enforce_strings() {
        // Use from_str to avoid env-var races in parallel test execution.
        for val in ["enforce", "1", "true", "yes"] {
            assert!(
                MaterializationAffordanceMode::from_str(val).is_enforce(),
                "expected Enforce for value={val}"
            );
        }
    }

    #[test]
    fn unrecognised_string_gives_off() {
        for val in ["off", "no", "false", "0", "", "maybe"] {
            assert!(
                MaterializationAffordanceMode::from_str(val).is_off(),
                "expected Off for value={val}"
            );
        }
    }
}
