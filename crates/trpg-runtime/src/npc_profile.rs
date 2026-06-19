//! Adapter: static [`NpcProfile`](trpg_model::NpcProfile) -> [`NpcPersona`] for
//! GM-side parameter synthesis (`npc_synth`). PURE; no combat/behavior logic.
//!
//! The synthesis grounding prose is persona-only (role / archetype / personality
//! / drives / values). GM-only secrets are intentionally EXCLUDED so synthesis
//! prompts and their provenance never carry secret text — secret-bearing paths
//! must go through [`NpcProfile::safe_view`](trpg_model::NpcProfile::safe_view)
//! or GM-only tooling, not this adapter.
use crate::npc_synth::NpcPersona;
use trpg_model::NpcProfile;

/// Build a parameter-synthesis persona from a static profile. Prose is assembled
/// from non-secret persona fields and the compiled speech-style block.
pub fn persona_from_profile(profile: &NpcProfile) -> NpcPersona {
    let mut parts: Vec<String> = Vec::new();
    if let Some(role) = profile.role.as_deref().filter(|s| !s.trim().is_empty()) {
        parts.push(format!("Role: {role}"));
    }
    if let Some(arch) = profile
        .archetype
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        parts.push(format!("Archetype: {arch}"));
    }
    if !profile.personality_traits.is_empty() {
        parts.push(format!(
            "Personality: {}",
            profile.personality_traits.join(", ")
        ));
    }
    if !profile.values.is_empty() {
        parts.push(format!("Values: {}", profile.values.join(", ")));
    }
    if !profile.drives.is_empty() {
        parts.push(format!("Drives: {}", profile.drives.join(", ")));
    }
    let speech = profile.speech_prompt_block();
    if !speech.is_empty() {
        parts.push(speech);
    }
    NpcPersona {
        actor_id: profile.actor_id.clone(),
        name: profile.name.clone(),
        prose: parts.join("\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::{NpcProfile, NpcSecret, SpeechStyle, Visibility};

    fn profile_with_secret() -> NpcProfile {
        NpcProfile {
            actor_id: "npc_lars".into(),
            name: "拉斯".into(),
            role: Some("加油站老板".into()),
            archetype: Some("taciturn_veteran".into()),
            personality_traits: vec!["寡言".into()],
            drives: vec!["守护小镇".into()],
            secrets: vec![NpcSecret {
                secret_id: "sec".into(),
                content: "GMONLY_SENTINEL_secret".into(),
                visibility: Visibility::GmOnly,
                source_refs: vec![],
            }],
            speech_style: SpeechStyle {
                humor: Some("dry".into()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn persona_carries_identity_and_persona_prose() {
        let persona = persona_from_profile(&profile_with_secret());
        assert_eq!(persona.actor_id, "npc_lars");
        assert_eq!(persona.name, "拉斯");
        assert!(persona.prose.contains("加油站老板"));
        assert!(persona.prose.contains("taciturn_veteran"));
        assert!(persona.prose.contains("守护小镇"));
        assert!(persona.prose.contains("dry"));
    }

    #[test]
    fn persona_prose_never_carries_gm_only_secret() {
        let persona = persona_from_profile(&profile_with_secret());
        assert!(
            !persona.prose.contains("GMONLY_SENTINEL_secret"),
            "GM-only secret leaked into synthesis persona prose"
        );
    }
}
