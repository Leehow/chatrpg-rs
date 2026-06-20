//! NPC Profile v1 (TC-NPC-01): a static, source-grounded persona + speech style.
//!
//! Scope: a small data model an NPC can be loaded from independently of combat,
//! plus PURE projection helpers. GM-only secrets are modeled as an explicit,
//! visibility-tagged channel ([`NpcSecret`]) so the player/prompt-safe projection
//! ([`NpcProfile::safe_view`]) can drop them STRUCTURALLY — secret text is never
//! present in the safe view type, so it cannot leak through prompts, JSON, or
//! trace summaries. Speech style compiles into a small narration prompt block
//! ([`SpeechStyle::to_prompt_block`]) that a later GM narration path can consume.
//!
//! Storage is serde/JSON round-trip (no DB migration in v1). Source refs reuse
//! [`SourceRef`] and secret visibility reuses [`Visibility`] for consistency with
//! the rest of the model.
use crate::{SourceRef, Visibility};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A single GM-authored secret about an NPC. `content` is sensitive text; its
/// `visibility` decides whether it may ever surface to players. Default
/// visibility is [`Visibility::GmOnly`] (fail-closed: an untagged secret stays
/// GM-only). Only [`Visibility::Public`]/[`Visibility::PlayerVisible`] secrets are
/// considered player-safe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NpcSecret {
    pub secret_id: String,
    /// Sensitive secret text. NEVER copy into a player/prompt-safe surface;
    /// route through [`NpcProfile::safe_view`] / [`NpcSecret::is_player_safe`].
    pub content: String,
    #[serde(default)]
    pub visibility: Visibility,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_refs: Vec<SourceRef>,
}

impl NpcSecret {
    /// Whether this secret is safe to surface to players. Conservative: only
    /// explicitly public / player-visible secrets pass; gm_only / npc_private /
    /// system_only are withheld.
    pub fn is_player_safe(&self) -> bool {
        matches!(
            self.visibility,
            Visibility::Public | Visibility::PlayerVisible
        )
    }
}

/// Static speech-style descriptors. All fields optional/defaulted so a profile
/// can be partially specified; the compiled prompt block omits blank fields.
/// Free-form strings (not enums) keep this source-grounded — descriptors can be
/// lifted verbatim from a module without forcing a fixed vocabulary.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SpeechStyle {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formality: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sentence_length: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emotionality: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slang_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub humor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threat_style: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub taboo_topics: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub catchphrases: Vec<String>,
}

/// Append `- {label}: {value}` for a non-blank optional descriptor.
fn push_opt(lines: &mut Vec<String>, label: &str, value: &Option<String>) {
    if let Some(s) = value {
        let s = s.trim();
        if !s.is_empty() {
            lines.push(format!("- {label}: {s}"));
        }
    }
}

impl SpeechStyle {
    /// Whether any descriptor is set (used to skip an empty block).
    pub fn is_empty(&self) -> bool {
        self.formality.is_none()
            && self.directness.is_none()
            && self.sentence_length.is_none()
            && self.emotionality.is_none()
            && self.slang_level.is_none()
            && self.humor.is_none()
            && self.threat_style.is_none()
            && self.taboo_topics.is_empty()
            && self.catchphrases.is_empty()
    }

    /// Compile into a small, narration-ready prompt block. Contains ONLY style
    /// descriptors (never secret content). Blank fields are omitted; an entirely
    /// empty style yields an empty string.
    pub fn to_prompt_block(&self) -> String {
        let mut lines = Vec::new();
        push_opt(&mut lines, "Formality", &self.formality);
        push_opt(&mut lines, "Directness", &self.directness);
        push_opt(&mut lines, "Sentence length", &self.sentence_length);
        push_opt(&mut lines, "Emotionality", &self.emotionality);
        push_opt(&mut lines, "Slang", &self.slang_level);
        push_opt(&mut lines, "Humor", &self.humor);
        push_opt(&mut lines, "Threat style", &self.threat_style);
        if !self.taboo_topics.is_empty() {
            lines.push(format!("- Avoid topics: {}", self.taboo_topics.join(", ")));
        }
        if !self.catchphrases.is_empty() {
            lines.push(format!("- Catchphrases: {}", self.catchphrases.join("; ")));
        }
        if lines.is_empty() {
            return String::new();
        }
        format!("Speech style:\n{}", lines.join("\n"))
    }
}

/// Static NPC persona (v1). Identity is the stable `actor_id` (the same id used
/// by the actor-identity / knowledge-holder contract); everything else is
/// descriptive persona. `secrets` is the GM-only channel — see [`Self::safe_view`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NpcProfile {
    /// Stable actor id (NOT a display name). Anchors the profile to one NPC.
    pub actor_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Verbatim, source-grounded persona prose (the module entity's free `body`
    /// description), NOT a synthesized structured field. Carries no GM-only secret
    /// text: when materialized from a spoiler-flagged entity, secret_terms are
    /// redacted before this is set (MAT.M8). Flows through `safe_view`, so it can
    /// ground an NPC's voice without inventing traits. `None` = no source prose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persona_description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archetype: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub personality_traits: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drives: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fears: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub goals: Vec<String>,
    /// GM-only secrets channel (visibility-tagged). Dropped by `safe_view`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<NpcSecret>,
    #[serde(default)]
    pub speech_style: SpeechStyle,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub behavioral_boundaries: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_refs: Vec<SourceRef>,
}

/// Player/prompt-safe projection of an [`NpcProfile`]. GM-only secrets are
/// STRUCTURALLY absent: `known_secrets` holds only the content of player-safe
/// secrets, so no gm_only text can travel through this type, its serialization,
/// or any trace built from it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NpcProfileSafeView {
    pub actor_id: String,
    pub name: String,
    pub role: Option<String>,
    /// Verbatim source persona prose (see [`NpcProfile::persona_description`]).
    /// Player/prompt-safe: secret_terms were redacted at materialization time, and
    /// this carries no GM-only secret channel.
    pub persona_description: Option<String>,
    pub archetype: Option<String>,
    pub personality_traits: Vec<String>,
    pub values: Vec<String>,
    pub drives: Vec<String>,
    pub fears: Vec<String>,
    pub goals: Vec<String>,
    /// Only player-facing secret content (public / player_visible). Empty when
    /// every secret is GM-only.
    pub known_secrets: Vec<String>,
    pub speech_style: SpeechStyle,
    pub behavioral_boundaries: Vec<String>,
}

impl NpcProfile {
    /// Project to a player/prompt-safe view. Drops every GM-only secret; keeps
    /// the descriptive persona and any explicitly player-safe secret content.
    pub fn safe_view(&self) -> NpcProfileSafeView {
        NpcProfileSafeView {
            actor_id: self.actor_id.clone(),
            name: self.name.clone(),
            role: self.role.clone(),
            persona_description: self.persona_description.clone(),
            archetype: self.archetype.clone(),
            personality_traits: self.personality_traits.clone(),
            values: self.values.clone(),
            drives: self.drives.clone(),
            fears: self.fears.clone(),
            goals: self.goals.clone(),
            known_secrets: self
                .secrets
                .iter()
                .filter(|s| s.is_player_safe())
                .map(|s| s.content.clone())
                .collect(),
            speech_style: self.speech_style.clone(),
            behavioral_boundaries: self.behavioral_boundaries.clone(),
        }
    }

    /// GM-only secrets, for GM-side tooling. Never feed into player/prompt-safe
    /// paths; use [`Self::safe_view`] for anything player-facing.
    pub fn gm_only_secrets(&self) -> Vec<&NpcSecret> {
        self.secrets
            .iter()
            .filter(|s| !s.is_player_safe())
            .collect()
    }

    /// Speech-style prompt block (delegates to [`SpeechStyle::to_prompt_block`];
    /// carries no secret content).
    pub fn speech_prompt_block(&self) -> String {
        self.speech_style.to_prompt_block()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untagged_secret_defaults_to_gm_only_and_is_withheld() {
        let json = r#"{"secret_id":"s1","content":"hidden"}"#;
        let s: NpcSecret = serde_json::from_str(json).unwrap();
        assert_eq!(s.visibility, Visibility::GmOnly);
        assert!(!s.is_player_safe(), "untagged secret must be withheld");
    }

    #[test]
    fn player_visible_secret_surfaces_in_safe_view() {
        let profile = NpcProfile {
            actor_id: "npc_a".into(),
            name: "A".into(),
            secrets: vec![
                NpcSecret {
                    secret_id: "open".into(),
                    content: "公开传闻".into(),
                    visibility: Visibility::PlayerVisible,
                    source_refs: vec![],
                },
                NpcSecret {
                    secret_id: "hidden".into(),
                    content: "GM_ONLY_TEXT".into(),
                    visibility: Visibility::GmOnly,
                    source_refs: vec![],
                },
            ],
            ..Default::default()
        };
        let safe = profile.safe_view();
        assert_eq!(safe.known_secrets, vec!["公开传闻".to_string()]);
        assert_eq!(profile.gm_only_secrets().len(), 1);
    }

    #[test]
    fn empty_speech_style_yields_empty_block() {
        assert!(SpeechStyle::default().is_empty());
        assert_eq!(SpeechStyle::default().to_prompt_block(), "");
    }

    #[test]
    fn blank_speech_fields_are_omitted_from_block() {
        let style = SpeechStyle {
            formality: Some("   ".into()),
            humor: Some("wry".into()),
            ..Default::default()
        };
        let block = style.to_prompt_block();
        assert!(block.contains("wry"));
        assert!(!block.contains("Formality"), "blank field must be omitted");
    }

    #[test]
    fn persona_description_flows_into_safe_view() {
        // MAT.M8: the verbatim source persona prose reaches the player/prompt-safe view
        // so an active NPC's voice can be grounded in source content.
        let profile = NpcProfile {
            actor_id: "npc_russ".into(),
            name: "拉斯".into(),
            persona_description: Some("白天通常待在埃索加油站的三人之一。".into()),
            ..Default::default()
        };
        let safe = profile.safe_view();
        assert_eq!(
            safe.persona_description.as_deref(),
            Some("白天通常待在埃索加油站的三人之一。")
        );
    }

    #[test]
    fn absent_persona_description_round_trips_and_is_omitted() {
        // OFF/baseline shape: a name-only profile carries no persona prose and serializes
        // without the field (byte-stable with pre-M8 thin profiles).
        let profile = NpcProfile {
            actor_id: "npc_min".into(),
            name: "Min".into(),
            ..Default::default()
        };
        assert!(profile.persona_description.is_none());
        let json = serde_json::to_string(&profile).unwrap();
        assert!(
            !json.contains("persona_description"),
            "absent field must be omitted: {json}"
        );
        let back: NpcProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(profile, back);
    }

    #[test]
    fn profile_with_no_optionals_round_trips() {
        let profile = NpcProfile {
            actor_id: "npc_min".into(),
            name: "Min".into(),
            ..Default::default()
        };
        let json = serde_json::to_string(&profile).unwrap();
        let back: NpcProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(profile, back);
    }
}
