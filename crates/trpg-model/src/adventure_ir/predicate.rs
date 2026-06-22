//! Adventure IR — PredicateExpr (P1-1): a FINITE, DETERMINISTIC, unit-testable
//! guard AST. The LLM NEVER evaluates a guard; Rust evaluates this AST, the LLM
//! only ranks/focuses within the legal frontier (设计4补充 §二: Rules decide
//! mechanics, Policy invents no facts).
//!
//! Fail-closed lives at the EXECUTION layer, not storage: an authored-but-
//! unnormalizable condition is stored as [`PredicateExpr::OpaqueAuthoredText`]
//! and evaluates to [`PredicateValue::NonExecutable`] — Rust never auto-fires it.
//! Three-valued (Kleene-style) logic propagates NonExecutable so a guard that
//! *cannot be decided* never silently passes.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A finite, comparable value used by leaf predicates. Deliberately small and
/// `Eq` so guard evaluation is deterministic (no float/NaN ambiguity).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "t", content = "v")]
pub enum IrValue {
    Text(String),
    Int(i64),
    Bool(bool),
}

/// Who must hold a piece of knowledge for [`PredicateExpr::RevelationKnown`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeHolder {
    /// The player party.
    Party,
    /// Any holder at all (party or any NPC) satisfies.
    Anyone,
    /// A specific NPC by id.
    Npc(String),
}

/// A finite event shape, used both as the `ON` of a ProgressRule (P1-2) and as
/// the anchor of [`PredicateExpr::ElapsedSince`]. `String` payloads carry a unit
/// id (e.g. `Entered("unit.foxwell_services")`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EventPattern {
    TimeAdvanced,
    WorldFactChanged,
    ChoiceRecorded,
    Entered(String),
    ObjectiveResolved(String),
    /// Escape hatch for an authored trigger we recognize but have not typed.
    Custom(String),
}

/// Lifecycle status of an [`super::ObjectiveSpec`]. Lives here because the guard
/// AST refers to it; re-exported from the module root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObjectiveStatus {
    Inactive,
    Active,
    Completed,
    Failed,
}

/// The finite guard AST. Every variant except [`Self::OpaqueAuthoredText`] is
/// executable and deterministically evaluated by [`Self::eval`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PredicateExpr {
    All(Vec<PredicateExpr>),
    Any(Vec<PredicateExpr>),
    Not(Box<PredicateExpr>),
    FactEquals { fact: String, value: IrValue },
    ObjectiveStatus { id: String, status: ObjectiveStatus },
    RevelationKnown { id: String, holder: KnowledgeHolder },
    ClockAtLeast { id: String, value: i32 },
    ElapsedSince { event: EventPattern, minutes: i64 },
    LocationEntered(String),
    ChoiceMade { key: String, value: String },
    EntityState { entity: String, field: String, value: IrValue },
    ResourceAtLeast { resource: String, amount: i64 },
    /// Authored condition that could not be normalized. Stored for Director/GM
    /// to read; **never** auto-fires (`eval` → `NonExecutable`).
    OpaqueAuthoredText { raw_text: String },
}

/// Three-valued result. `NonExecutable` means "Rust cannot decide this guard"
/// (an opaque leaf, or a combinator whose outcome hinges on one) — treated as
/// fail-closed: it must not be read as a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredicateValue {
    True,
    False,
    NonExecutable,
}

/// Snapshot of the facts a guard can read. Pure data; the runtime
/// ProgressionEngine (P1-3) builds it from DomainEvents.
#[derive(Debug, Clone, Default)]
pub struct EvalContext {
    pub facts: BTreeMap<String, IrValue>,
    pub objectives: BTreeMap<String, ObjectiveStatus>,
    /// (revelation_id, holder) pairs that are known.
    pub known_revelations: Vec<(String, KnowledgeHolder)>,
    pub clocks: BTreeMap<String, i32>,
    /// elapsed world-minutes since a given event occurred.
    pub elapsed: Vec<(EventPattern, i64)>,
    pub entered_locations: Vec<String>,
    pub choices: BTreeMap<String, String>,
    /// ((entity, field) -> value)
    pub entity_states: BTreeMap<(String, String), IrValue>,
    pub resources: BTreeMap<String, i64>,
}

impl EvalContext {
    fn elapsed_for(&self, ev: &EventPattern) -> Option<i64> {
        self.elapsed
            .iter()
            .find(|(e, _)| e == ev)
            .map(|(_, m)| *m)
    }
    fn revelation_known(&self, id: &str, holder: &KnowledgeHolder) -> bool {
        self.known_revelations.iter().any(|(rid, h)| {
            rid == id
                && match holder {
                    // Asking "does anyone know?" — any recorded holder satisfies.
                    KnowledgeHolder::Anyone => true,
                    // A recorded `Anyone` holder satisfies any specific ask.
                    _ => h == holder || *h == KnowledgeHolder::Anyone,
                }
        })
    }
}

fn b3(x: bool) -> PredicateValue {
    if x {
        PredicateValue::True
    } else {
        PredicateValue::False
    }
}

impl PredicateExpr {
    /// `true` iff this AST contains NO opaque leaf — i.e. Rust can fully decide
    /// it. Producers use this to classify NormalizationStatus.
    pub fn is_executable(&self) -> bool {
        match self {
            PredicateExpr::OpaqueAuthoredText { .. } => false,
            PredicateExpr::All(v) | PredicateExpr::Any(v) => v.iter().all(|c| c.is_executable()),
            PredicateExpr::Not(b) => b.is_executable(),
            _ => true,
        }
    }

    /// Deterministically evaluate this guard against `ctx` with three-valued
    /// fail-closed logic. Pure: no env reads, no IO, no clock.
    pub fn eval(&self, ctx: &EvalContext) -> PredicateValue {
        use PredicateValue::*;
        match self {
            PredicateExpr::All(v) => {
                let mut acc = True;
                for c in v {
                    match c.eval(ctx) {
                        False => return False,
                        NonExecutable => acc = NonExecutable,
                        True => {}
                    }
                }
                acc
            }
            PredicateExpr::Any(v) => {
                let mut acc = False;
                for c in v {
                    match c.eval(ctx) {
                        True => return True,
                        NonExecutable => acc = NonExecutable,
                        False => {}
                    }
                }
                acc
            }
            PredicateExpr::Not(b) => match b.eval(ctx) {
                True => False,
                False => True,
                NonExecutable => NonExecutable,
            },
            PredicateExpr::FactEquals { fact, value } => {
                b3(ctx.facts.get(fact) == Some(value))
            }
            PredicateExpr::ObjectiveStatus { id, status } => {
                b3(ctx.objectives.get(id) == Some(status))
            }
            PredicateExpr::RevelationKnown { id, holder } => b3(ctx.revelation_known(id, holder)),
            PredicateExpr::ClockAtLeast { id, value } => {
                b3(ctx.clocks.get(id).copied().unwrap_or(0) >= *value)
            }
            PredicateExpr::ElapsedSince { event, minutes } => {
                // Event has not occurred yet → not elapsed (deterministic False).
                b3(ctx.elapsed_for(event).map(|m| m >= *minutes).unwrap_or(false))
            }
            PredicateExpr::LocationEntered(loc) => {
                b3(ctx.entered_locations.iter().any(|l| l == loc))
            }
            PredicateExpr::ChoiceMade { key, value } => {
                b3(ctx.choices.get(key).map(|v| v == value).unwrap_or(false))
            }
            PredicateExpr::EntityState {
                entity,
                field,
                value,
            } => b3(ctx
                .entity_states
                .get(&(entity.clone(), field.clone()))
                == Some(value)),
            PredicateExpr::ResourceAtLeast { resource, amount } => {
                b3(ctx.resources.get(resource).copied().unwrap_or(0) >= *amount)
            }
            // KEY fail-closed rule: Rust never auto-fires authored opaque text.
            PredicateExpr::OpaqueAuthoredText { .. } => NonExecutable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PredicateValue::*;
    use super::*;

    fn ctx() -> EvalContext {
        let mut c = EvalContext::default();
        c.facts
            .insert("athena.status".into(), IrValue::Text("disabled".into()));
        c.objectives
            .insert("obj.neutralize_athena".into(), ObjectiveStatus::Completed);
        c.clocks.insert("clock.escape".into(), 3);
        c.elapsed
            .push((EventPattern::Entered("unit.foxwell".into()), 16));
        c.entered_locations.push("loc.condo_1".into());
        c.choices.insert("quil_vs_hisako".into(), "side_with_quil".into());
        c.resources.insert("eb".into(), 500);
        c.known_revelations
            .push(("rev.coords".into(), KnowledgeHolder::Party));
        c.entity_states.insert(
            ("npc.athena".into(), "hostile".into()),
            IrValue::Bool(false),
        );
        c
    }

    #[test]
    fn fact_equals_true_and_false() {
        let c = ctx();
        assert_eq!(
            PredicateExpr::FactEquals {
                fact: "athena.status".into(),
                value: IrValue::Text("disabled".into())
            }
            .eval(&c),
            True
        );
        assert_eq!(
            PredicateExpr::FactEquals {
                fact: "athena.status".into(),
                value: IrValue::Text("online".into())
            }
            .eval(&c),
            False
        );
        // unknown fact → False (deterministic), not NonExecutable.
        assert_eq!(
            PredicateExpr::FactEquals {
                fact: "missing".into(),
                value: IrValue::Int(1)
            }
            .eval(&c),
            False
        );
    }

    #[test]
    fn objective_status_and_clock_and_resource() {
        let c = ctx();
        assert_eq!(
            PredicateExpr::ObjectiveStatus {
                id: "obj.neutralize_athena".into(),
                status: ObjectiveStatus::Completed
            }
            .eval(&c),
            True
        );
        assert_eq!(
            PredicateExpr::ClockAtLeast {
                id: "clock.escape".into(),
                value: 3
            }
            .eval(&c),
            True
        );
        // absent clock counts as 0 → fail-closed below positive threshold.
        assert_eq!(
            PredicateExpr::ClockAtLeast {
                id: "clock.none".into(),
                value: 1
            }
            .eval(&c),
            False
        );
        assert_eq!(
            PredicateExpr::ResourceAtLeast {
                resource: "eb".into(),
                amount: 500
            }
            .eval(&c),
            True
        );
    }

    #[test]
    fn elapsed_since_requires_event_occurred() {
        let c = ctx();
        // golden: Foxwell entered, 16 >= 15 → Scavvs may fire.
        assert_eq!(
            PredicateExpr::ElapsedSince {
                event: EventPattern::Entered("unit.foxwell".into()),
                minutes: 15
            }
            .eval(&c),
            True
        );
        // never entered → not elapsed (deterministic False, not NonExecutable).
        assert_eq!(
            PredicateExpr::ElapsedSince {
                event: EventPattern::Entered("unit.nowhere".into()),
                minutes: 1
            }
            .eval(&c),
            False
        );
    }

    #[test]
    fn location_choice_revelation_entitystate() {
        let c = ctx();
        assert_eq!(
            PredicateExpr::LocationEntered("loc.condo_1".into()).eval(&c),
            True
        );
        assert_eq!(
            PredicateExpr::ChoiceMade {
                key: "quil_vs_hisako".into(),
                value: "side_with_quil".into()
            }
            .eval(&c),
            True
        );
        assert_eq!(
            PredicateExpr::RevelationKnown {
                id: "rev.coords".into(),
                holder: KnowledgeHolder::Anyone
            }
            .eval(&c),
            True
        );
        assert_eq!(
            PredicateExpr::EntityState {
                entity: "npc.athena".into(),
                field: "hostile".into(),
                value: IrValue::Bool(false)
            }
            .eval(&c),
            True
        );
    }

    #[test]
    fn opaque_never_fires() {
        let c = ctx();
        let p = PredicateExpr::OpaqueAuthoredText {
            raw_text: "若玩家以某种巧妙方式说服守卫".into(),
        };
        assert_eq!(p.eval(&c), NonExecutable);
        assert!(!p.is_executable());
    }

    #[test]
    fn three_valued_combinators() {
        let c = ctx();
        let t = PredicateExpr::LocationEntered("loc.condo_1".into());
        let f = PredicateExpr::LocationEntered("loc.nope".into());
        let o = PredicateExpr::OpaqueAuthoredText {
            raw_text: "x".into(),
        };
        // All: a False short-circuits even past opaque.
        assert_eq!(
            PredicateExpr::All(vec![f.clone(), o.clone()]).eval(&c),
            False
        );
        // All: True + opaque → cannot confirm all → NonExecutable (fail-closed).
        assert_eq!(
            PredicateExpr::All(vec![t.clone(), o.clone()]).eval(&c),
            NonExecutable
        );
        // Any: a True satisfies even alongside opaque.
        assert_eq!(PredicateExpr::Any(vec![t.clone(), o.clone()]).eval(&c), True);
        // Any: False + opaque → cannot confirm any → NonExecutable.
        assert_eq!(
            PredicateExpr::Any(vec![f.clone(), o.clone()]).eval(&c),
            NonExecutable
        );
        // Not(opaque) stays undecidable.
        assert_eq!(PredicateExpr::Not(Box::new(o)).eval(&c), NonExecutable);
        assert_eq!(PredicateExpr::Not(Box::new(t)).eval(&c), False);
    }

    #[test]
    fn is_executable_detects_nested_opaque() {
        let exec = PredicateExpr::All(vec![
            PredicateExpr::LocationEntered("a".into()),
            PredicateExpr::Not(Box::new(PredicateExpr::ClockAtLeast {
                id: "c".into(),
                value: 1,
            })),
        ]);
        assert!(exec.is_executable());
        let nonexec = PredicateExpr::Any(vec![
            PredicateExpr::LocationEntered("a".into()),
            PredicateExpr::OpaqueAuthoredText {
                raw_text: "y".into(),
            },
        ]);
        assert!(!nonexec.is_executable());
    }

    #[test]
    fn golden_neutralize_athena_completes() {
        // ON WorldFactChanged WHEN athena.status in {disabled,...} THEN Complete.
        // Modeled: Any(FactEquals disabled/controlled/destroyed).
        let c = ctx();
        let guard = PredicateExpr::Any(vec![
            PredicateExpr::FactEquals {
                fact: "athena.status".into(),
                value: IrValue::Text("disabled".into()),
            },
            PredicateExpr::FactEquals {
                fact: "athena.status".into(),
                value: IrValue::Text("controlled".into()),
            },
            PredicateExpr::FactEquals {
                fact: "athena.status".into(),
                value: IrValue::Text("destroyed".into()),
            },
        ]);
        assert_eq!(guard.eval(&c), True);
    }
}
