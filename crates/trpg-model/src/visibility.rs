//! 缓存分区与可见性/稳定性基础枚举（`CacheZone`/`Visibility`/`Stability`）。
//!
//! 从 `lib.rs` facade 拆出的低耦合定义簇。通过 `pub use visibility::*` 在 crate 根
//! 重导出，serde 名称、derive、字段、默认值与公共 API 保持不变。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum CacheZone {
    Prefix,
    PinnedMiddle,
    DynamicTail,
    NeverPrompt,
}

impl CacheZone {
    pub fn as_str(&self) -> &'static str {
        match self {
            CacheZone::Prefix => "prefix",
            CacheZone::PinnedMiddle => "pinned_middle",
            CacheZone::DynamicTail => "dynamic_tail",
            CacheZone::NeverPrompt => "never_prompt",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    Public,
    PlayerVisible,
    GmOnly,
    NpcPrivate,
    SystemOnly,
}

impl Visibility {
    pub fn as_str(&self) -> &'static str {
        match self {
            Visibility::Public => "public",
            Visibility::PlayerVisible => "player_visible",
            Visibility::GmOnly => "gm_only",
            Visibility::NpcPrivate => "npc_private",
            Visibility::SystemOnly => "system_only",
        }
    }
}

impl Default for Visibility {
    fn default() -> Self {
        Self::GmOnly
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Stability {
    Immutable,
    RarelyChanged,
    SceneStable,
    TurnDynamic,
    Ephemeral,
}

impl Default for Stability {
    fn default() -> Self {
        Self::RarelyChanged
    }
}

impl Stability {
    pub fn as_str(&self) -> &'static str {
        match self {
            Stability::Immutable => "immutable",
            Stability::RarelyChanged => "rarely_changed",
            Stability::SceneStable => "scene_stable",
            Stability::TurnDynamic => "turn_dynamic",
            Stability::Ephemeral => "ephemeral",
        }
    }
}
