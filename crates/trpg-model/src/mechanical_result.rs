//! Mechanical result view (L0.3): the read-only, committed-result projection the design
//! says is missing (`director_plan.rs` self-doc: "no MechanicalResultView exists"). It is a
//! PURE projection of an already-committed [`crate::CheckResultRecord`] — success/degree +
//! a compact, generic summary of the committed `StatePatch`es — that the post-adjudication
//! Director (the spine, L1.x) reads to plan a beat reflecting the REAL outcome.
//!
//! Discipline:
//! - **Read-only, no handle, no commit.** Built by projection only; carries no DB handle.
//! - **Generic — no `ruleset_id`/`module_id` name-branching** (constitution rule 11). The
//!   effect `kind` is the patch's structural variant tag, never a game-specific label.
//! - **Fail-closed.** A check whose `outcome` has no `success` bool and no `degree`
//!   projects to [`CheckOutcomeView::Unresolved`] — never silently "passed".
//!
//! Crate-cycle note: `TurnLedger` lives in `trpg-gm`, so this `trpg-model` type CANNOT impl
//! `From<&TurnLedger>` (would create model→gm). The trpg-gm-side projector that reads
//! `ctx.ledger.snapshot()` and builds these views lives in `trpg-gm` (L1.1).

use crate::{CheckResultRecord, StatePatch};
use serde::{Deserialize, Serialize};

/// Pass/fail disposition of a committed check, fail-closed to `Unresolved`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckOutcomeView {
    Passed,
    Failed,
    /// No `success` bool and no `degree` ⇒ not a bound/resolved check. Fail-closed default.
    #[default]
    Unresolved,
}

impl CheckOutcomeView {
    /// True only for an explicitly passed check (never for `Unresolved`).
    pub fn is_success(self) -> bool {
        matches!(self, CheckOutcomeView::Passed)
    }
}

/// A compact, generic summary of one committed [`StatePatch`]. `kind` is the structural
/// variant tag; `target` is the affected track/actor/object id; `detail` is a short,
/// game-agnostic description (never raw secret prose).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EffectSummary {
    pub kind: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub detail: String,
}

impl EffectSummary {
    /// Project one committed patch into a generic summary. Pure; no name-branching.
    pub fn from_patch(patch: &StatePatch) -> Self {
        match patch {
            StatePatch::SetTrack { target, value, .. } => Self {
                kind: "set_track".into(),
                target: target.clone(),
                detail: format!("= {value}"),
            },
            StatePatch::ModifyTrack { target, amount, .. } => Self {
                kind: "modify_track".into(),
                target: target.clone(),
                detail: format!("{amount:+}"),
            },
            StatePatch::ActorHpDelta {
                actor_id,
                from,
                delta,
                to,
                ..
            } => Self {
                kind: "actor_hp_delta".into(),
                target: actor_id.clone(),
                detail: format!(
                    "hp {delta:+} ({}->{})",
                    from.map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
                    to.map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
                ),
            },
            StatePatch::CreateAspect { target, .. } => mk("create_aspect", target),
            StatePatch::RemoveAspect { target, .. } => mk("remove_aspect", target),
            StatePatch::ApplyStatus { target, .. } => mk("apply_status", target),
            StatePatch::RemoveStatus { target, .. } => mk("remove_status", target),
            StatePatch::CreateFact { target, .. } => mk("create_fact", target),
            StatePatch::SetTrait {
                actor_id, trait_id, ..
            } => Self {
                kind: "set_trait".into(),
                target: actor_id.clone(),
                detail: trait_id.clone(),
            },
            StatePatch::GrantObject {
                actor_id,
                object_id,
                ..
            } => Self {
                kind: "grant_object".into(),
                target: actor_id.clone(),
                detail: object_id.clone(),
            },
            StatePatch::ObjectPatch { object_id, .. } => Self {
                kind: "object_patch".into(),
                target: object_id.clone().unwrap_or_default(),
                detail: String::new(),
            },
            StatePatch::LoadMaterial { material_id, .. } => Self {
                kind: "load_material".into(),
                target: material_id.clone(),
                detail: String::new(),
            },
        }
    }
}

fn mk(kind: &str, target: &str) -> EffectSummary {
    EffectSummary {
        kind: kind.into(),
        target: target.into(),
        detail: String::new(),
    }
}

/// Read-only projection of one committed check: its id, pass/fail disposition, the degree
/// band (if any), the roll expression, and a generic summary of committed effects.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MechanicalResultView {
    #[serde(default)]
    pub check_id: String,
    #[serde(default)]
    pub outcome: CheckOutcomeView,
    /// The degree/band token from `outcome.degree` (e.g. `extreme_success`), if present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degree: Option<String>,
    /// The dice expression of the bound roll, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roll_expression: Option<String>,
    /// Generic summaries of the committed `StatePatch`es.
    #[serde(default)]
    pub effects: Vec<EffectSummary>,
}

impl From<&CheckResultRecord> for MechanicalResultView {
    /// Project a committed check record. `outcome.success` bool ⇒ Passed/Failed; absent ⇒
    /// Unresolved (fail-closed). `outcome.degree` (str) becomes `degree`. Effects summarize
    /// `committed_patches` in order.
    fn from(rec: &CheckResultRecord) -> Self {
        let oc = &rec.outcome;
        let outcome = match oc.get("success").and_then(|v| v.as_bool()) {
            Some(true) => CheckOutcomeView::Passed,
            Some(false) => CheckOutcomeView::Failed,
            None => CheckOutcomeView::Unresolved,
        };
        let degree = oc
            .get("degree")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let expr = rec.roll.expression.trim();
        let roll_expression = (!expr.is_empty()).then(|| expr.to_string());
        let effects = rec
            .committed_patches
            .iter()
            .map(EffectSummary::from_patch)
            .collect();
        Self {
            check_id: rec.check_id.clone(),
            outcome,
            degree,
            roll_expression,
            effects,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ActorKind, CheckResultRecord, DiceRollRecord, RollVisibility, StatePatch};
    use chrono::Utc;
    use serde_json::json;

    fn roll(expr: &str) -> DiceRollRecord {
        DiceRollRecord {
            roll_id: "r1".into(),
            session_id: "s1".into(),
            turn_id: "t1".into(),
            check_id: Some("c1".into()),
            roller_kind: ActorKind::PlayerCharacter,
            roller_id: Some("pc.current".into()),
            visibility: RollVisibility::PublicGmRoll,
            expression: expr.into(),
            result: json!({"total": 42}),
            seed_commitment: "seed".into(),
            revealed_at: None,
            created_at: Utc::now(),
        }
    }

    fn rec(
        check_id: &str,
        outcome: serde_json::Value,
        patches: Vec<StatePatch>,
    ) -> CheckResultRecord {
        CheckResultRecord {
            check_id: check_id.into(),
            roll: roll("1d100"),
            outcome,
            committed_patches: patches,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn projects_a_passed_check_with_effects() {
        let r = rec(
            "spot_hidden",
            json!({"success": true, "degree": "extreme_success", "target": 70}),
            vec![StatePatch::ActorHpDelta {
                actor_id: "npc.cultist".into(),
                from: Some(10),
                delta: -4,
                to: Some(6),
                reason: "knife".into(),
            }],
        );
        let view = MechanicalResultView::from(&r);
        assert_eq!(view.check_id, "spot_hidden");
        assert_eq!(view.outcome, CheckOutcomeView::Passed);
        assert!(view.outcome.is_success());
        assert_eq!(view.degree.as_deref(), Some("extreme_success"));
        assert_eq!(view.roll_expression.as_deref(), Some("1d100"));
        assert_eq!(view.effects.len(), 1);
        assert_eq!(view.effects[0].kind, "actor_hp_delta");
        assert_eq!(view.effects[0].target, "npc.cultist");
        assert_eq!(view.effects[0].detail, "hp -4 (10->6)");
    }

    #[test]
    fn projects_a_failed_check() {
        let r = rec(
            "dodge",
            json!({"success": false, "degree": "failure"}),
            vec![],
        );
        let view = MechanicalResultView::from(&r);
        assert_eq!(view.outcome, CheckOutcomeView::Failed);
        assert!(!view.outcome.is_success());
        assert_eq!(view.degree.as_deref(), Some("failure"));
        assert!(view.effects.is_empty());
    }

    #[test]
    fn unresolved_outcome_is_fail_closed() {
        // No success bool, no degree ⇒ Unresolved (never silently passed).
        let r = rec("provisional", json!({"awaiting_binding": true}), vec![]);
        let view = MechanicalResultView::from(&r);
        assert_eq!(view.outcome, CheckOutcomeView::Unresolved);
        assert!(!view.outcome.is_success());
        assert_eq!(view.degree, None);
    }

    #[test]
    fn round_trips_via_serde() {
        let r = rec(
            "persuade",
            json!({"success": true, "degree": "success"}),
            vec![StatePatch::ModifyTrack {
                target: "tracks.tension".into(),
                amount: 2,
                reason: "raised stakes".into(),
            }],
        );
        let view = MechanicalResultView::from(&r);
        let s = serde_json::to_string(&view).unwrap();
        let back: MechanicalResultView = serde_json::from_str(&s).unwrap();
        assert_eq!(view, back);
        assert_eq!(back.effects[0].kind, "modify_track");
        assert_eq!(back.effects[0].detail, "+2");
    }
}
