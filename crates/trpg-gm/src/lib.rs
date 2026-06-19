pub mod errata;
pub mod execute;
pub mod gate;
pub mod ledger;
pub mod mode;
pub mod mode_catalog;
pub mod obligations;
pub mod opposed_prepass;
pub mod packet;
pub mod plugin;
pub mod plugins;
pub mod prompts;
pub mod scene_policy;
pub mod stimulus;
pub mod stream;
pub mod tools;
pub mod turn_event;
pub mod turn_loop;
pub mod turn_plan;
pub(crate) mod turn_trace;

#[cfg(test)]
#[path = "plugin_mechanism_tests.rs"]
mod plugin_mechanism_tests;

// 层化迁移架构门（P0 护栏，零运行时行为变更）——设计4 §19 Rust 侧基线门 + P6 占位锚。
#[cfg(test)]
#[path = "arch_gates_tests.rs"]
mod arch_gates_tests;

pub use errata::{ErrataEntry, ErrataMemory};
pub use execute::{execute_turn, OwnedTurnRequest};
pub use gate::GateResolverFn;
pub use ledger::TurnLedger;
pub use mode::{
    active_mode_manifest, current_mode, load_mode_manifest, CatalogFilter, EnterModeTool,
    ExitModeTool, ModeManifest, TempoOverrides,
};
pub use mode_catalog::{mode_catalog_section, MODE_CATALOG_HEADER};
pub use obligations::{ModeExitObligation, ObligationLedger, WaiveScope};
pub use opposed_prepass::{detect_attack_target, opposed_prepass_enabled, OpposedBinding};
pub use packet::{AdjudicationPacket, MechanicalFact, MechanicalFactKind, NarrationPacket};
pub use plugin::*;
pub use plugins::{load_gm_skill_with_plugins, load_plugins};
pub use prompts::{
    load_gm_skill, load_gm_skill_with_mode, validate_compiled_budget, DynamicTailInput,
    TurnMessages,
};
pub use stream::RedactingBuffer;
pub use tools::{
    AwaitingPlayerRoll, GmTool, SceneDeepExtractFn, ToolCtx, ToolDispatchOutcome, ToolError,
    ToolOutput, ToolRegistry, ToolSpec,
};
pub use turn_event::TurnEvent;
pub use turn_loop::{CtxProviderFn, GmLoop, GmTurnInput, LoopConfig, TurnOutcome};
pub use turn_plan::{PhaseId, PhaseKind, TurnPhasePlan, CANONICAL_TURN_PLAN};
