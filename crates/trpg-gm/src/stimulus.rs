//! 刺激驱动检定预 pass（J2 SAN 评测根因修复）：回合头部把目录条目的
//! `when_to_use`（语义路由的唯一依据）与本回合虚构内容——玩家输入 + 上回合
//! 叙事尾段——做语义匹配；命中 → `MechanicDue` 候选，经 watcher `admit_dues`
//! 抑制+落库后进 `ObligationLedger` 阻塞闭环（roll_check {mechanic_id} 处理
//! 或 waive_obligation 带理由豁免）。恐怖刺激→SAN 检定一类"被动触发、GM 该
//! 主动开"的检定由此获得强制通路。
//!
//! fail-closed：门关闭 / LLM 失败 / 解析失败 / 命中 id 不在目录 → 空，GM
//! 裁量不变、绝不编造。零 per-ruleset 硬编码——CoC 的 SAN、CPR 的 Humanity、
//! Triangle 的 Chaos 走同一套逻辑，监听什么完全由目录数据决定。

use serde_json::{json, Value};
use std::sync::Arc;
use trpg_llm::LlmClient;
use trpg_model::{ChatMessage, DueSource, DueStatus, MechanicDue, MechanicEntry};
use uuid::Uuid;

/// 单回合命中上限（防债务刷屏；目录条目仍全量参与判定）。
const MAX_HITS_PER_TURN: usize = 2;
/// 上回合叙事尾段喂给判定的最大字符数（刺激通常出现在叙事收尾处）。
const NARRATION_TAIL_CHARS: usize = 1200;

/// 门 `TRPG_STIMULUS_CHECK_TRIGGER`，默认开。
pub fn stimulus_pass_enabled() -> bool {
    std::env::var("TRPG_STIMULUS_CHECK_TRIGGER")
        .map(|v| !matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no"))
        .unwrap_or(true)
}

/// 候选行 `id | name | when_to_use`（与 BP1 目录索引同款物料）；when_to_use
/// 空的条目无路由语义，不参与判定。
fn candidate_lines(catalog: &[MechanicEntry]) -> Vec<String> {
    catalog
        .iter()
        .filter(|e| !e.when_to_use.trim().is_empty())
        .map(|e| format!("{} | {} | {}", e.id, e.name, e.when_to_use.replace('\n', " ")))
        .collect()
}

/// LLM 裁决原文 → 合法命中 (mechanic_id, reason)：id 必须是参与判定的目录
/// 条目（防幻觉）、去重、超 `MAX_HITS_PER_TURN` 截断；形状不对 → 空。
fn parse_hits(raw: &Value, catalog: &[MechanicEntry]) -> Vec<(String, String)> {
    let Some(arr) = raw.get("hits").and_then(|h| h.as_array()) else { return Vec::new() };
    let mut out: Vec<(String, String)> = Vec::new();
    for hit in arr {
        let Some(id) = hit.get("mechanic_id").and_then(|v| v.as_str()) else { continue };
        let known = catalog.iter().any(|e| e.id == id && !e.when_to_use.trim().is_empty());
        if !known || out.iter().any(|(seen, _)| seen == id) { continue; }
        let reason = hit.get("reason").and_then(|v| v.as_str()).unwrap_or("").to_string();
        out.push((id.to_string(), reason));
        if out.len() >= MAX_HITS_PER_TURN { break; }
    }
    out
}

/// 取字符串末尾至多 `n` 个字符（按 char 不按字节，CJK 安全）。
fn tail_chars(s: &str, n: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    chars[chars.len().saturating_sub(n)..].iter().collect()
}

fn due_from_hit(
    entry: &MechanicEntry,
    reason: &str,
    session_id: &str,
    turn_id: &str,
    player_input: &str,
) -> MechanicDue {
    MechanicDue {
        due_id: format!("due_{}", Uuid::new_v4().simple()),
        session_id: session_id.to_string(),
        turn_id: turn_id.to_string(),
        source: DueSource::SemanticTrigger,
        source_track: None,
        hook_event: Some("stimulus_match".to_string()),
        mechanic_id: Some(entry.id.clone()),
        threshold_desc: format!(
            "scene stimulus semantically triggers '{}' — resolve it now via roll_check {{mechanic_id:\"{}\"}} (or waive_obligation with a real reason): {}",
            entry.name, entry.id, reason
        ),
        followup_procedure_id: None,
        owner_kind: "actor".to_string(),
        owner_id: "pc.current".to_string(),
        evidence: json!({
            "reason": reason,
            "player_input": tail_chars(player_input, 200),
            "when_to_use": entry.when_to_use,
        }),
        status: DueStatus::Open,
        created_at: chrono::Utc::now(),
    }
}

/// 主入口：语义判定 → due 候选（未抑制未落库——两者由调用方经 watcher
/// `admit_dues` 完成，与 hook 通路同一套防刷屏规则）。
pub async fn stimulus_due_candidates(
    llm: &Arc<dyn LlmClient>,
    catalog: &[MechanicEntry],
    session_id: &str,
    turn_id: &str,
    player_input: &str,
    recent_narration: Option<&str>,
) -> Vec<MechanicDue> {
    let lines = candidate_lines(catalog);
    if lines.is_empty() { return Vec::new(); }
    let narration_tail = tail_chars(recent_narration.unwrap_or(""), NARRATION_TAIL_CHARS);
    let system = "You are the rules watcher of a tabletop RPG engine. Given the mechanics catalog index (one entry per line: `id | name | when_to_use`), decide which entries' when_to_use semantics are CLEARLY triggered by what is happening in the fiction RIGHT NOW and demand a mechanical resolution this turn. Judge by meaning, never by keywords. Be strict: flag an entry only when the fictional trigger is explicit (e.g. the character is directly exposed to something the entry's when_to_use describes) and the mechanic has not obviously been resolved already; when in doubt return no hits. Respond with JSON only: {\"hits\":[{\"mechanic_id\":\"<id from the index>\",\"reason\":\"<one short sentence citing the fictional trigger>\"}]} (at most 2 hits) or {\"hits\":[]}.";
    let user = format!(
        "[catalog index]\n{}\n[/catalog index]\n\n[last GM narration tail]\n{}\n[/last GM narration tail]\n\n[player input this turn]\n{}\n[/player input]",
        lines.join("\n"),
        narration_tail,
        player_input,
    );
    let messages = vec![
        ChatMessage { role: "system".to_string(), content: system.to_string() },
        ChatMessage { role: "user".to_string(), content: user },
    ];
    let raw = match llm.complete_json(messages, 0.0).await {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(error = %err, "stimulus pass LLM call failed (fail-closed: no dues)");
            return Vec::new();
        }
    };
    parse_hits(&raw, catalog)
        .into_iter()
        .filter_map(|(id, reason)| {
            catalog.iter().find(|e| e.id == id).map(|entry| due_from_hit(entry, &reason, session_id, turn_id, player_input))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, when: &str) -> MechanicEntry {
        MechanicEntry { id: id.to_string(), name: id.to_string(), when_to_use: when.to_string(), ..Default::default() }
    }

    #[test]
    fn candidate_lines_skip_entries_without_when_to_use() {
        let catalog = vec![entry("rs.sanity", "exposure to horror"), entry("rs.silent", "  ")];
        let lines = candidate_lines(&catalog);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("rs.sanity | "));
    }

    #[test]
    fn parse_hits_drops_hallucinated_ids_dedups_and_caps() {
        let catalog = vec![entry("rs.a", "w"), entry("rs.b", "w"), entry("rs.c", "w")];
        let raw = json!({"hits": [
            {"mechanic_id": "rs.invented", "reason": "幻觉条目必须被丢弃"},
            {"mechanic_id": "rs.a", "reason": "r1"},
            {"mechanic_id": "rs.a", "reason": "重复 id 去重"},
            {"mechanic_id": "rs.b", "reason": "r2"},
            {"mechanic_id": "rs.c", "reason": "超上限截断"},
        ]});
        let hits = parse_hits(&raw, &catalog);
        assert_eq!(hits.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), vec!["rs.a", "rs.b"]);
        // 形状不对 → 空（fail-closed）。
        assert!(parse_hits(&json!({"nope": 1}), &catalog).is_empty());
        assert!(parse_hits(&json!("plain string"), &catalog).is_empty());
    }

    #[test]
    fn due_from_hit_binds_mechanic_id_and_marks_semantic_source() {
        let e = entry("coc.sanity_roll_and_loss", "sanity-blasting encounter");
        let due = due_from_hit(&e, "saw an impossible creature", "s1", "t1", "我盯着那条狗");
        assert_eq!(due.source, DueSource::SemanticTrigger);
        assert_eq!(due.mechanic_id.as_deref(), Some("coc.sanity_roll_and_loss"));
        assert_eq!(due.hook_event.as_deref(), Some("stimulus_match"));
        assert!(due.threshold_desc.contains("roll_check"), "GM 修复指引必须在 desc 里: {}", due.threshold_desc);
        assert_eq!(due.status, DueStatus::Open);
    }

    #[test]
    fn tail_chars_is_char_safe_for_cjk() {
        assert_eq!(tail_chars("恐怖的狗", 2), "的狗");
        assert_eq!(tail_chars("ab", 10), "ab");
        assert_eq!(tail_chars("", 5), "");
    }
}
