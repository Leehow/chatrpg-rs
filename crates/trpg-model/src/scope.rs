//! 作用域基础类型（`ScopeType`/`Scope`）。
//!
//! 从 `lib.rs` facade 拆出的低耦合定义簇。通过 `pub use scope::*` 在 crate 根
//! 重导出，serde 名称、derive、字段、默认值与构造助手行为保持不变。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScopeType {
    Global,
    Ruleset,
    Module,
    Campaign,
    Chapter,
    Mission,
    Location,
    Npc,
    Session,
    Scene,
    Turn,
    Material,
    Character,
    Object,
}

impl ScopeType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ScopeType::Global => "global",
            ScopeType::Ruleset => "ruleset",
            ScopeType::Module => "module",
            ScopeType::Campaign => "campaign",
            ScopeType::Chapter => "chapter",
            ScopeType::Mission => "mission",
            ScopeType::Location => "location",
            ScopeType::Npc => "npc",
            ScopeType::Session => "session",
            ScopeType::Scene => "scene",
            ScopeType::Turn => "turn",
            ScopeType::Material => "material",
            ScopeType::Character => "character",
            ScopeType::Object => "object",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Scope {
    pub scope_type: ScopeType,
    pub scope_id: String,
}

impl Default for Scope {
    fn default() -> Self { Self { scope_type: ScopeType::Global, scope_id: "global".to_string() } }
}

impl Scope {
    pub fn global() -> Self {
        Self { scope_type: ScopeType::Global, scope_id: "global".to_string() }
    }
    pub fn ruleset(id: impl Into<String>) -> Self {
        Self { scope_type: ScopeType::Ruleset, scope_id: id.into() }
    }
    pub fn module(id: impl Into<String>) -> Self {
        Self { scope_type: ScopeType::Module, scope_id: id.into() }
    }
}
