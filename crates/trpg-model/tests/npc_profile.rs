//! TC-NPC-01 acceptance tests: NPC Profile v1 — static persona, prompt-safe
//! projection, and speech-style compilation. These are the named acceptance
//! tests from the task card; fine-grained helper coverage lives inline in
//! `trpg_model::npc_profile`.
use trpg_model::{NpcProfile, NpcSecret, SourceRef, SpeechStyle, Visibility};

/// Build a representative profile with one GM-only secret and a populated
/// speech style. The secret content is a recognizable sentinel so leak checks
/// can assert its absence without ever printing it on failure.
fn sample_profile() -> NpcProfile {
    NpcProfile {
        actor_id: "npc_lars".into(),
        name: "拉斯".into(),
        role: Some("加油站老板".into()),
        persona_description: Some("加油站的老板，白天守在棚下。".into()),
        archetype: Some("taciturn_veteran".into()),
        personality_traits: vec!["寡言".into(), "警惕".into()],
        values: vec!["忠诚".into()],
        drives: vec!["守护小镇".into()],
        fears: vec!["旧战友被牵连".into()],
        goals: vec!["把加油站撑下去".into()],
        secrets: vec![NpcSecret {
            secret_id: "sec_deserter".into(),
            content: "GMONLY_SENTINEL_he_is_a_wartime_deserter".into(),
            visibility: Visibility::GmOnly,
            source_refs: vec![SourceRef {
                source_id: "module_book".into(),
                page: Some(42),
                ..Default::default()
            }],
        }],
        speech_style: SpeechStyle {
            formality: Some("blunt".into()),
            directness: Some("very direct".into()),
            sentence_length: Some("short clipped sentences".into()),
            emotionality: Some("flat".into()),
            slang_level: Some("military slang".into()),
            humor: Some("dry".into()),
            threat_style: Some("quiet menace".into()),
            taboo_topics: vec!["the war".into()],
            catchphrases: vec!["现金还是赊账？".into()],
        },
        behavioral_boundaries: vec!["不会主动开第一枪".into()],
        source_refs: vec![SourceRef {
            source_id: "module_book".into(),
            page: Some(40),
            ..Default::default()
        }],
    }
}

const SECRET_SENTINEL: &str = "GMONLY_SENTINEL_he_is_a_wartime_deserter";

#[test]
fn npc_profile_roundtrip() {
    let profile = sample_profile();
    let json = serde_json::to_string(&profile).expect("serialize NpcProfile");
    let back: NpcProfile = serde_json::from_str(&json).expect("deserialize NpcProfile");
    assert_eq!(profile, back, "NpcProfile must round-trip through serde");
    // Source refs survive the round-trip.
    assert_eq!(back.source_refs.len(), 1);
    assert_eq!(back.source_refs[0].page, Some(40));
    assert_eq!(back.secrets[0].source_refs[0].page, Some(42));
}

#[test]
fn npc_profile_safe_view_hides_secret() {
    let profile = sample_profile();
    let safe = profile.safe_view();
    let safe_json = serde_json::to_string(&safe).expect("serialize safe view");
    // The GM-only secret must not leak through the safe view or its serialization.
    assert!(
        !safe_json.contains(SECRET_SENTINEL),
        "GM-only secret leaked into prompt-safe view"
    );
    assert!(
        safe.known_secrets.is_empty(),
        "no player-facing secret was declared, so none should surface"
    );
    // But persona that is safe to surface is preserved.
    assert_eq!(safe.name, "拉斯");
    assert_eq!(safe.role.as_deref(), Some("加油站老板"));
    // The original profile still carries the GM-only secret for GM-side use.
    assert_eq!(profile.gm_only_secrets().len(), 1);
}

#[test]
fn npc_speech_style_projection_contains_expected_traits() {
    let profile = sample_profile();
    let block = profile.speech_prompt_block();
    for expected in [
        "blunt",
        "very direct",
        "short clipped sentences",
        "dry",
        "quiet menace",
        "the war",
        "现金还是赊账？",
    ] {
        assert!(
            block.contains(expected),
            "speech-style prompt block missing expected trait: {expected}"
        );
    }
    // The speech block is persona-style only and must never carry secret text.
    assert!(
        !block.contains(SECRET_SENTINEL),
        "secret text leaked into speech-style prompt block"
    );
}
