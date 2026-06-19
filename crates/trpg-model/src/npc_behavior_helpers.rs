//! Private derivation helpers for [`NpcBehaviorPlan`] (split out for the
//! ≤400-line file budget; pure, zero-behavior move from `npc_behavior.rs`).

use super::{NpcBehaviorContext, NpcEmotionalState, REVEAL_SECRET_THRESHOLD};
use crate::npc_mind::{NpcMindView, RelationshipSummary};
use crate::npc_relationship::RelationshipStance;
use serde::Serialize;

/// Serialize a `serde`-tagged enum to its snake_case wire word for prompt text.
pub(super) fn enum_word<T: Serialize>(value: &T, fallback: &str) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| fallback.to_string())
}

/// Append a labeled bullet list, skipping an empty list (no empty headers).
pub(super) fn push_list(lines: &mut Vec<String>, label: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    lines.push(format!("{label}:"));
    for item in items {
        lines.push(format!("- {item}"));
    }
}

/// Pick the relationship summary the plan focuses on. With an explicit target, match it;
/// otherwise default to the player party. Returns `None` if no such relationship exists.
pub(super) fn focus_relationship<'a>(
    view: &'a NpcMindView,
    ctx: &NpcBehaviorContext,
) -> Option<&'a RelationshipSummary> {
    match &ctx.focus_target {
        Some(t) => view
            .relationships
            .iter()
            .find(|r| r.target_kind == t.target_kind && r.target_id == t.target_id),
        None => view
            .relationships
            .iter()
            .find(|r| r.target_kind == "player_party"),
    }
}

/// Dominant-channel emotional read. Severity-ordered so hostility wins over fear, etc.
pub(super) fn derive_emotion(trust: i64, fear: i64, hostility: i64) -> NpcEmotionalState {
    if hostility >= 50 {
        NpcEmotionalState::Hostile
    } else if fear >= 50 {
        NpcEmotionalState::Afraid
    } else if trust <= -20 || hostility >= 25 {
        NpcEmotionalState::Guarded
    } else if trust >= 40 {
        NpcEmotionalState::Warm
    } else {
        NpcEmotionalState::Neutral
    }
}

/// Deterministic preferred/forbidden action hints. Stance sets the baseline posture;
/// low help willingness and any withheld secret add explicit guardrails. Persona
/// behavioral boundaries and taboo topics are always folded into the forbidden list.
pub(super) fn derive_actions(
    view: &NpcMindView,
    stance: RelationshipStance,
    help: i16,
    reveal: i16,
    has_withheld: bool,
) -> (Vec<String>, Vec<String>) {
    let mut preferred = Vec::new();
    let mut forbidden = Vec::new();
    match stance {
        RelationshipStance::Hostile => {
            preferred.push("stand ground and warn the party off".into());
            preferred.push("demand they leave or state their business".into());
            forbidden.push("offer help freely".into());
        }
        RelationshipStance::Wary => {
            preferred.push("keep distance and give guarded answers".into());
            preferred.push("probe the party's intentions before committing".into());
        }
        RelationshipStance::Neutral => {
            preferred.push("respond plainly and stay noncommittal".into());
        }
        RelationshipStance::Cordial | RelationshipStance::Friendly | RelationshipStance::Allied => {
            preferred.push("engage warmly and offer reasonable help".into());
            preferred.push("share information that is safe to share".into());
        }
    }
    if help < 25 {
        forbidden.push("volunteer assistance the NPC would not give".into());
    }
    if has_withheld || reveal < REVEAL_SECRET_THRESHOLD {
        forbidden.push("disclose protected secrets".into());
    }
    for boundary in &view.persona.behavioral_boundaries {
        forbidden.push(boundary.clone());
    }
    for taboo in &view.persona.speech_style.taboo_topics {
        forbidden.push(format!("raise taboo topic: {taboo}"));
    }
    (preferred, forbidden)
}

/// One compact narration-steering line. Carries persona name + derived posture only.
pub(super) fn dialogue_guidance(
    view: &NpcMindView,
    stance: RelationshipStance,
    emotion: NpcEmotionalState,
) -> String {
    let stance_word = serde_json::to_value(stance)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "neutral".into());
    let emotion_word = serde_json::to_value(emotion)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "neutral".into());
    format!(
        "Voice {} as {stance_word} and {emotion_word}; stay in character and reveal only what this plan permits.",
        view.persona.name
    )
}
