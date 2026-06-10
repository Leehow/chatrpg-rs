//! Semantic-unit substrate for the rulebook-reader agent. The parser emits
//! semantic_units as raw json (chatrpg.semantic_source_unit.v1); we deserialize
//! the fields the reader navigates by. In-engine the same Vec<Unit> is built
//! from the parse output instead of a file.

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Unit {
    #[serde(default)]
    pub unit_id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub content_text: String,
    #[serde(default)]
    pub heading_context: Vec<String>,
    #[serde(default)]
    pub page_numbers: Vec<u32>,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub signal_class: String,
    #[serde(default)]
    pub metadata: Value,
}

impl Unit {
    pub fn is_noise(&self) -> bool {
        self.signal_class == "noise"
    }
    pub fn first_page(&self) -> Option<u32> {
        self.page_numbers.first().copied()
    }
    pub fn head_path(&self) -> String {
        let h: Vec<&str> = self.heading_context.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect();
        if h.is_empty() {
            if self.title.is_empty() { "?".into() } else { self.title.clone() }
        } else {
            h.join(" > ")
        }
    }
    pub fn index_weight(&self) -> f64 {
        self.metadata.get("index_weight").and_then(Value::as_f64).unwrap_or(0.5)
    }
    pub fn mech_tags(&self) -> Vec<String> {
        self.metadata
            .get("mechanics_tags")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default()
    }
    /// Lowercased haystack of heading + title + body + mechanics_tags for search.
    pub fn haystack(&self) -> String {
        format!("{} {} {} {}", self.head_path(), self.title, self.content_text, self.mech_tags().join(" ")).to_lowercase()
    }
}

pub fn load_units(path: &Path) -> Result<Vec<Unit>> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut out = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(u) = serde_json::from_str::<Unit>(line) {
            out.push(u);
        }
    }
    Ok(out)
}
