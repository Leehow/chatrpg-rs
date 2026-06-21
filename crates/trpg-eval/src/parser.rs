//! Parse a 战报 markdown into a [`Transcript`].
//!
//! Format (both `### 第 N 回合` and `## 第 N 回合` variants are accepted):
//! ```text
//! ## 第 N 回合
//! **玩家**：<action>
//! **GM 原文(raw)**： ```<gm raw>```
//! **分块 audience**：
//! - `[玩家可见]` 散文：<prose>
//! - `[玩家可见]` [roll]：<roll outcome>
//! - `[玩家可见]` [dialogue]：<npc line>
//! <sub>体检：…</sub>
//! ```

use crate::model::{Transcript, Turn};
use regex::Regex;

pub fn parse_transcript(md: &str) -> Transcript {
    let header = Regex::new(r"^#{2,4}\s*第\s*(\d+)\s*回合").unwrap();
    let roll_tag = Regex::new(r"\[roll\]([^\[]*)\[/roll\]").unwrap();
    let mut title = String::new();
    let mut turns: Vec<Turn> = Vec::new();
    let mut cur: Option<Turn> = None;
    let mut in_fence = false;

    for raw in md.lines() {
        let line = raw.trim_end();
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if let Some(c) = header.captures(line) {
            if let Some(t) = cur.take() {
                turns.push(finalize(t, &roll_tag));
            }
            let mut t = Turn::default();
            t.index = c[1].parse().unwrap_or(0);
            cur = Some(t);
            in_fence = false;
            continue;
        }
        let Some(t) = cur.as_mut() else {
            if title.is_empty() && line.starts_with("# ") {
                title = line.trim_start_matches('#').trim().to_string();
            }
            continue;
        };

        // Inside the GM raw fence: capture full narration verbatim.
        if in_fence {
            if line.contains("待结算") {
                t.debt_lines.push(line.to_string());
            }
            if !t.gm_raw.is_empty() {
                t.gm_raw.push('\n');
            }
            t.gm_raw.push_str(line);
            continue;
        }

        // 待结算 may also appear in the audience recap — any occurrence = debt.
        if line.contains("待结算") {
            t.debt_lines.push(line.to_string());
        }

        if let Some(rest) = strip_player(line) {
            if t.player_action.is_empty() {
                t.player_action = rest;
            }
            continue;
        }
        if line.contains("<sub>") {
            t.self_check = line.to_string();
            continue;
        }
        if let Some((kind, content)) = audience_block(line) {
            if content.is_empty() {
                continue;
            }
            if kind.contains("roll") {
                t.roll_lines.push(content.clone());
            }
            if kind.contains("散文") || kind.contains("dialogue") {
                if !t.visible_prose.is_empty() {
                    t.visible_prose.push('\n');
                }
                t.visible_prose.push_str(&content);
            }
        }
    }
    if let Some(t) = cur.take() {
        turns.push(finalize(t, &roll_tag));
    }
    Transcript { title, turns }
}

/// Derive roll lines + scene opening from the captured GM raw block.
fn finalize(mut t: Turn, roll_tag: &Regex) -> Turn {
    for cap in roll_tag.captures_iter(&t.gm_raw) {
        let line = cap[1].trim().to_string();
        if !line.is_empty() && !t.roll_lines.iter().any(|r| r.contains(&line)) {
            t.roll_lines.push(line);
        }
    }
    // Scene opening = first narrative paragraph of the GM raw (skip bracketed tags).
    if let Some(p) = t
        .gm_raw
        .split('\n')
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('['))
    {
        t.scene_opening = p.to_string();
    }
    t
}

/// `**玩家**：<action>` → the action with the prefix and leading punctuation removed.
fn strip_player(line: &str) -> Option<String> {
    let after = line.strip_prefix("**玩家**")?;
    let action = after
        .trim_start_matches(['：', ':', ' '])
        .trim()
        .to_string();
    Some(action)
}

/// `- `[玩家可见]` <kind>：<content>` → (kind, content). Hidden blocks are skipped.
fn audience_block(line: &str) -> Option<(String, String)> {
    if !line.contains("玩家可见") {
        return None;
    }
    let idx = line.find("玩家可见")?;
    // Skip past "玩家可见" and the closing backtick / bracket.
    let mut rest = &line[idx + "玩家可见".len()..];
    rest = rest.trim_start_matches(['`', ']', ' ']);
    let (kind, content) = split_once_cjk(rest)?;
    Some((kind.trim().to_string(), content.trim().to_string()))
}

/// Split on the first CJK or ASCII colon.
fn split_once_cjk(s: &str) -> Option<(&str, &str)> {
    let pos = s.find('：').or_else(|| s.find(':'))?;
    let after = &s[pos + s[pos..].chars().next()?.len_utf8()..];
    Some((&s[..pos], after))
}
