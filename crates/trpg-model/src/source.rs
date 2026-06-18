//! 来源文档基础类型（规则书/模组的文档、页、块、索引、引用、锚点）。
//!
//! 从 `lib.rs` facade 拆出的低耦合定义簇。通过 `pub use source::*` 在 crate 根
//! 重导出，serde 名称、derive、字段、默认值与公共 API 保持不变。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Rulebook,
    Module,
    Unknown,
}

impl SourceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceKind::Rulebook => "rulebook",
            SourceKind::Module => "module",
            SourceKind::Unknown => "unknown",
        }
    }
}

impl Default for SourceKind {
    fn default() -> Self {
        Self::Unknown
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SourceDocument {
    pub id: Uuid,
    pub source_id: String,
    pub source_kind: SourceKind,
    pub title: String,
    pub file_path: String,
    pub markdown_path: Option<String>,
    pub source_hash: String,
    pub parse_config_hash: String,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PageText {
    pub page: u32,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct DocumentBoundingBox {
    pub page: Option<u32>,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct DocumentChunk {
    pub chunk_id: String,
    pub text: String,
    #[serde(default)]
    pub full_text: String,
    #[serde(default)]
    pub page_numbers: Vec<u32>,
    #[serde(default)]
    pub element_types: Vec<String>,
    #[serde(default)]
    pub heading_context: Vec<String>,
    pub token_estimate: Option<u32>,
    #[serde(default)]
    pub is_oversized: bool,
    #[serde(default)]
    pub bounding_boxes: Vec<DocumentBoundingBox>,
    #[serde(default)]
    pub text_hash: Option<String>,
    #[serde(default)]
    pub clean_status: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PlainTextBook {
    pub source_id: String,
    pub title: String,
    pub source_hash: String,
    pub pages: Vec<PageText>,
    #[serde(default)]
    pub chunks: Vec<DocumentChunk>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct SourceIndex {
    pub sources: Vec<SourceDocumentRef>,
    pub anchors: Vec<SourceAnchor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SourceDocumentRef {
    pub source_id: String,
    pub title: String,
    pub source_kind: SourceKind,
    pub source_hash: String,
    pub file_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SourceAnchor {
    pub anchor_id: String,
    pub source_id: String,
    pub page: Option<u32>,
    pub section_path: Vec<String>,
    pub char_start: Option<usize>,
    pub char_end: Option<usize>,
    pub text_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
pub struct SourceRef {
    pub source_id: String,
    pub page: Option<u32>,
    pub anchor_id: Option<String>,
    pub section_path: Vec<String>,
    pub char_start: Option<usize>,
    pub char_end: Option<usize>,
    pub text_hash: Option<String>,
    pub note: Option<String>,
}
