//! Shared job-status types for staged (progressive) ruleset parsing.
//!
//! Kept separate from `staged.rs` so the parser, api, and cli crates can all
//! share the serde shape. `begin`/`finish`/`fail` stamp `started_at`/`finished_at`
//! with `chrono::Utc::now()` (rfc3339) so per-stage durations are reported; the
//! deterministic transition test asserts only status/progress, not timestamps.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Ordered stages and their cumulative progress percentage on completion.
pub const STAGES: &[(&str, u8)] = &[("identity", 5), ("character", 35), ("deep", 100)];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageState {
    pub name: String,
    pub status: String,
    pub detail: String,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub finished_at: Option<String>,
}

impl StageState {
    /// Wall-clock seconds this stage took, if it has both timestamps.
    pub fn duration_secs(&self) -> Option<f64> {
        let s = self.started_at.as_deref()?;
        let f = self.finished_at.as_deref()?;
        let s = DateTime::parse_from_rfc3339(s).ok()?;
        let f = DateTime::parse_from_rfc3339(f).ok()?;
        Some((f - s).num_milliseconds() as f64 / 1000.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobStatus {
    pub ruleset_id: String,
    pub stage: String,
    pub stages: Vec<StageState>,
    pub progress_pct: u8,
    pub current_detail: String,
    #[serde(default)]
    pub error: Option<String>,
}

impl JobStatus {
    pub fn new(ruleset_id: &str) -> Self {
        let stages = STAGES
            .iter()
            .map(|(n, _)| StageState {
                name: n.to_string(),
                status: "pending".into(),
                detail: String::new(),
                started_at: None,
                finished_at: None,
            })
            .collect();
        Self {
            ruleset_id: ruleset_id.into(),
            stage: "identity".into(),
            stages,
            progress_pct: 0,
            current_detail: String::new(),
            error: None,
        }
    }

    fn at(&mut self, name: &str) -> Option<&mut StageState> {
        self.stages.iter_mut().find(|s| s.name == name)
    }

    pub fn begin(&mut self, name: &str) {
        self.stage = name.into();
        self.current_detail = format!("{name} started");
        let now = Utc::now().to_rfc3339();
        if let Some(s) = self.at(name) {
            s.status = "running".into();
            s.started_at = Some(now);
        }
    }

    pub fn finish(&mut self, name: &str, detail: &str) {
        let now = Utc::now().to_rfc3339();
        if let Some(s) = self.at(name) {
            s.status = "done".into();
            s.detail = detail.into();
            s.finished_at = Some(now);
        }
        self.progress_pct = STAGES
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, p)| *p)
            .unwrap_or(self.progress_pct);
        self.current_detail = detail.into();
    }

    pub fn fail(&mut self, name: &str, err: &str) {
        let now = Utc::now().to_rfc3339();
        if let Some(s) = self.at(name) {
            s.status = "failed".into();
            s.detail = err.into();
            s.finished_at = Some(now);
        }
        self.error = Some(err.into());
    }

    pub fn note(&mut self, detail: &str) {
        self.current_detail = detail.into();
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn progress_and_transitions() {
        let mut s = JobStatus::new("call_of_cthulhu_7e");
        assert_eq!(s.progress_pct, 0);
        s.begin("identity");
        s.finish("identity", "premise ready");
        s.begin("character");
        assert_eq!(s.stage, "character");
        assert!(s
            .stages
            .iter()
            .any(|x| x.name == "identity" && x.status == "done"));
        assert!(s.progress_pct > 0 && s.progress_pct < 100);
        s.fail("character", "boom");
        assert_eq!(s.error.as_deref(), Some("boom"));
        assert!(s
            .stages
            .iter()
            .any(|x| x.name == "character" && x.status == "failed"));
    }
}
