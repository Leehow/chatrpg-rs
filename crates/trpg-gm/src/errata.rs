use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use trpg_agent::{VerifierFinding, VerifierFindingKind};
use trpg_model::{MemoryEvent, MemoryKind, Visibility};
use uuid::Uuid;

/// 一条勘误：流后 NarrationVerifier finding 落成的记忆条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrataEntry {
    pub kind: VerifierFindingKind,
    pub detail: String,
    pub turn_id: String,
    pub created_at: DateTime<Utc>,
}

/// 会话内勘误记忆：findings 不阻塞流式交付，注入下一轮上下文由 GM 自洽勘误；
/// 同类 finding ≥ threshold 升级持续提醒块（防同错高频复发）。
#[derive(Debug)]
pub struct ErrataMemory {
    entries: Vec<ErrataEntry>,
    kind_counts: BTreeMap<String, u32>,
    threshold: u8,
}

impl ErrataMemory {
    pub fn new(repeat_finding_threshold: u8) -> Self {
        Self {
            entries: Vec::new(),
            kind_counts: BTreeMap::new(),
            threshold: repeat_finding_threshold,
        }
    }

    /// 流后调用：记录本回合 findings，返回本次新增条目（供持久化）。
    pub fn record(&mut self, turn_id: &str, findings: &[VerifierFinding]) -> Vec<ErrataEntry> {
        let mut out = Vec::new();
        for f in findings {
            let key = kind_key(f.kind);
            *self.kind_counts.entry(key).or_default() += 1;
            let entry = ErrataEntry {
                kind: f.kind,
                detail: f.detail.clone(),
                turn_id: turn_id.to_string(),
                created_at: Utc::now(),
            };
            self.entries.push(entry.clone());
            out.push(entry);
        }
        out
    }

    /// 下一轮 dynamic tail 的勘误块：最近 ≤3 条勘误的提示文本；无勘误 → None。
    pub fn errata_block(&self) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        let lines = self
            .entries
            .iter()
            .rev()
            .take(3)
            .rev()
            .map(|e| format!("- {}: {}", kind_key(e.kind), e.detail))
            .collect::<Vec<_>>()
            .join("\n");
        Some(format!("[gm_errata]\nPrevious narration audit findings to repair in future fiction without retconning streamed text:\n{}\n[/gm_errata]", lines))
    }

    /// 任一 kind 累计 ≥ threshold ⇒ 持续提醒块（之后每轮注入直到会话结束）。
    pub fn standing_reminder_block(&self) -> Option<String> {
        let repeated = self
            .kind_counts
            .iter()
            .filter(|(_, v)| **v >= self.threshold as u32)
            .map(|(k, v)| format!("- {k}: {v} times"))
            .collect::<Vec<_>>();
        if repeated.is_empty() {
            None
        } else {
            Some(format!("[gm_errata_standing]\nThese finding kinds are recurring. Avoid them this turn:\n{}\n[/gm_errata_standing]", repeated.join("\n")))
        }
    }

    /// 按 kind 聚合统计（serde snake_case kind 字符串为键；供 e2e 报告与准则迭代）。
    pub fn kind_counts(&self) -> &BTreeMap<String, u32> {
        &self.kind_counts
    }

    /// 持久化载荷：本回合新增勘误折成一条 MemoryEvent；调用方负责 db.save_memory_event。
    pub fn to_memory_event(
        &self,
        request: &trpg_model::ContextRequest,
        new_entries: &[ErrataEntry],
    ) -> MemoryEvent {
        let mut tags = vec!["gm_errata".to_string()];
        tags.extend(
            new_entries
                .iter()
                .map(|e| kind_key(e.kind))
                .collect::<Vec<_>>(),
        );
        // MemoryEvent 的字段名是 source（serde_json::Value），不是 source_json。
        MemoryEvent {
            event_id: format!("mem_errata_{}", Uuid::new_v4().simple()),
            session_id: request.session_id.clone(),
            turn_id: Some(request.turn_id.clone()),
            ruleset_id: request.ruleset_id.clone(),
            module_id: request.module_id.clone(),
            scene_id: None,
            location_id: None,
            actor_ids: vec![],
            visibility: Visibility::GmOnly,
            event_kind: MemoryKind::Event,
            summary: new_entries
                .iter()
                .map(|e| format!("{}: {}", kind_key(e.kind), e.detail))
                .collect::<Vec<_>>()
                .join("; "),
            transcript_excerpt: None,
            source: json!({"source":"narration_verifier"}),
            tags,
            importance: 2,
            occurred_at: Utc::now(),
        }
    }
}

fn kind_key(kind: VerifierFindingKind) -> String {
    match kind {
        VerifierFindingKind::MissingCheck => "missing_check",
        VerifierFindingKind::MissingRollExecution => "missing_roll_execution",
        VerifierFindingKind::MissingEffect => "missing_effect",
        VerifierFindingKind::InventedEffect => "invented_effect",
        VerifierFindingKind::OmittedVisibleResult => "omitted_visible_result",
        VerifierFindingKind::ManualRollRequest => "manual_roll_request",
        VerifierFindingKind::PlayerAgencyViolation => "player_agency_violation",
        VerifierFindingKind::SecretLeak => "secret_leak",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_agent::{VerifierFinding, VerifierFindingKind};

    #[test]
    fn errata_records_recent_entries_and_counts() {
        let mut e = ErrataMemory::new(3);
        let entries = e.record(
            "turn_1",
            &[VerifierFinding::blocker(
                VerifierFindingKind::OmittedVisibleResult,
                "visible result missing",
            )],
        );
        assert_eq!(entries.len(), 1);
        assert!(e.errata_block().unwrap().contains("visible result missing"));
        assert_eq!(e.kind_counts().get("omitted_visible_result"), Some(&1));
    }

    #[test]
    fn repeated_kind_becomes_standing_reminder() {
        let mut e = ErrataMemory::new(2);
        e.record(
            "t1",
            &[VerifierFinding::blocker(
                VerifierFindingKind::SecretLeak,
                "secret one",
            )],
        );
        assert!(e.standing_reminder_block().is_none());
        e.record(
            "t2",
            &[VerifierFinding::blocker(
                VerifierFindingKind::SecretLeak,
                "secret two",
            )],
        );
        assert!(e.standing_reminder_block().unwrap().contains("secret_leak"));
    }
}
