//! 上下文块内容与块结构（`BlockContent`/`ContextBlock`）。
//!
//! 从 `lib.rs` facade 拆出的低耦合定义簇。通过 `pub use context_block::*` 在
//! crate 根重导出，serde 名称、derive、字段、构造与哈希/token 估算行为保持不变。
//! 引用的模型类型仍在 `lib.rs` 中定义，经 `crate::` 路径取用。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    stable_json_hash, BlockKind, CacheZone, DirectorPolicy, ProcedureDef, ScenarioNode, Scope,
    SourceRef, Stability, Visibility,
};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "format", content = "value", rename_all = "snake_case")]
pub enum BlockContent {
    Markdown(String),
    Text(String),
    Json(serde_json::Value),
    Procedure(ProcedureDef),
    Asset(serde_json::Value),
    ScenarioNode(ScenarioNode),
    DirectorPolicy(DirectorPolicy),
}

impl BlockContent {
    pub fn render_text(&self) -> String {
        match self {
            BlockContent::Markdown(s) | BlockContent::Text(s) => s.clone(),
            BlockContent::Json(v) | BlockContent::Asset(v) => serde_json::to_string_pretty(v).unwrap_or_default(),
            BlockContent::Procedure(p) => serde_json::to_string_pretty(p).unwrap_or_default(),
            BlockContent::ScenarioNode(n) => serde_json::to_string_pretty(n).unwrap_or_default(),
            BlockContent::DirectorPolicy(p) => serde_json::to_string_pretty(p).unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ContextBlock {
    pub block_id: String,
    pub kind: BlockKind,
    pub title: String,
    pub content: BlockContent,
    pub visibility: Visibility,
    pub stability: Stability,
    pub cache_zone: CacheZone,
    pub scope: Scope,
    pub priority: i32,
    pub version: u32,
    pub tags: Vec<String>,
    pub source_refs: Vec<SourceRef>,
    pub dependencies: Vec<String>,
    pub content_hash: String,
    pub token_estimate: Option<u32>,
    pub expires_at_turn: Option<String>,
    pub expires_at_scene: Option<String>,
    pub load_reason: Option<String>,
}

impl ContextBlock {
    pub fn new(
        block_id: impl Into<String>,
        kind: BlockKind,
        title: impl Into<String>,
        content: BlockContent,
        visibility: Visibility,
        stability: Stability,
        cache_zone: CacheZone,
        scope: Scope,
        priority: i32,
    ) -> Self {
        let block_id = block_id.into();
        let title = title.into();
        let content_hash = stable_json_hash(&content);
        let token_estimate = Some((content.render_text().chars().count() as u32 / 4).max(1));
        Self {
            block_id,
            kind,
            title,
            content,
            visibility,
            stability,
            cache_zone,
            scope,
            priority,
            version: 1,
            tags: vec![],
            source_refs: vec![],
            dependencies: vec![],
            content_hash,
            token_estimate,
            expires_at_turn: None,
            expires_at_scene: None,
            load_reason: None,
        }
    }
}
