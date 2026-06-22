//! Adventure IR — EffectExpr (P1-1): the typed `THEN` of a ProgressRule. Like
//! [`super::PredicateExpr`] it is finite data; the ProgressionEngine (P1-3)
//! applies it. An [`EffectExpr::OpaqueAuthoredText`] is stored for the GM but is
//! NOT auto-applied (fail-closed at the execution layer).
use super::IrValue;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A finite, deterministic effect to apply when a rule fires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EffectExpr {
    /// Activate a unit/beat/encounter by id.
    Activate(String),
    /// Mark an objective completed.
    Complete(String),
    /// Mark an objective failed.
    Fail(String),
    /// Disable a unit/outcome by id (e.g. the foreclosed branch).
    Disable(String),
    /// Reveal a revelation by id.
    Reveal(String),
    /// Advance a clock/tracker by `by` ticks.
    AdvanceClock { id: String, by: i32 },
    /// Set a world fact.
    SetFact { fact: String, value: IrValue },
    /// Authored effect we recognize but could not normalize — never auto-applied.
    OpaqueAuthoredText { raw_text: String },
}

impl EffectExpr {
    /// `false` for an opaque effect — the engine must skip it (fail-closed).
    pub fn is_executable(&self) -> bool {
        !matches!(self, EffectExpr::OpaqueAuthoredText { .. })
    }
}

/// A scoring side-effect (Vault Commendation/Demerit, optional objectives).
/// Carried by [`super::ObjectiveSpec`]; finite so it stays deterministic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScoreEffect {
    pub label: String,
    pub delta: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_effect_is_not_executable() {
        assert!(!EffectExpr::OpaqueAuthoredText {
            raw_text: "GM 自行裁定奖励".into()
        }
        .is_executable());
        assert!(EffectExpr::Complete("obj.x".into()).is_executable());
        assert!(EffectExpr::AdvanceClock {
            id: "c".into(),
            by: 1
        }
        .is_executable());
    }

    #[test]
    fn effect_roundtrips_json() {
        let e = EffectExpr::Disable("outcome.accept_hisako".into());
        let s = serde_json::to_string(&e).unwrap();
        let back: EffectExpr = serde_json::from_str(&s).unwrap();
        assert_eq!(e, back);
    }
}
