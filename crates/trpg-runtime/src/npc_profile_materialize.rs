//! Axis-1 source-backed NPC profile materialization (MAT.M2).
//!
//! When an NPC is active (per [`crate::npc_activation`], MAT.M1) but has no durable
//! `npc_profiles` row, `build_npc_behavior_guidance` (turn_loop) skips it → the NPC
//! is inert even though present in the scene (the Cyberpunk Athena softlock:
//! `active_npc_ids` non-empty but 0 profiles). M2 lazily upserts a **thin
//! source-backed** [`NpcProfile`] from the module's `graph.npcs[id]` entry so the
//! profile load succeeds and the NPC becomes a live responder.
//!
//! # Source-backed only (no fabrication)
//!
//! The builder copies ONLY fields that are present in the module NPC JSON
//! (`id`/`actor_id`, `name`, optional `role`, and — MAT.M8 — the verbatim free
//! `body` prose). It NEVER invents persona structure, traits, or
//! `facts_can_reveal` (those, if present in source, would flow through their own
//! typed channels; a thin profile carries none, so World reveals nothing on its own
//! — the constitutionally safe default).
//!
//! ## MAT.M8: source body prose → `persona_description`
//!
//! The body prose is VerifiedExact verbatim source content (e.g. CoC
//! `npc_nate_patterson.body` = "白天通常待在埃索加油站的三人之一…"). Pre-M8 it was
//! dropped, so the materialized profile's persona-safe view was contentless and the
//! no-invention guard kept the active NPC SILENT. M8 folds it into
//! [`NpcProfile::persona_description`], the field the persona-safe view + behavior
//! plan render, so the NPC has real, source-grounded substance to voice.
//!
//! Knowledge/secret gate stays intact: the body is read through the same
//! [`entity_prose`] reader the scene path uses, then secret_terms for a
//! spoiler-flagged entity are REDACTED (the same [`SpoilerMeta`] redaction the
//! scene/GM-context path applies) BEFORE it is stored — a spoiler entity's hidden
//! terms never enter the player/prompt-safe profile. No fact is folded into
//! `knowledge_basis` (that remains `facts_can_reveal`-only, M4).
//!
//! Gating is the caller's responsibility (Enforce only; OFF == baseline no-op).

use trpg_model::{entity_prose, NpcProfile, SpoilerMeta};

/// Read the entity's verbatim source body prose, spoiler-redacted, as the
/// player/prompt-safe persona description. Returns `None` when no source body
/// (or only whitespace) is present, or when redaction empties it — never invents.
///
/// Generic: only JSON keys decide the body field (via [`entity_prose`]) and the
/// spoiler metadata; zero ruleset/module name-branch.
fn source_persona_description(entry: &serde_json::Value) -> Option<String> {
    let raw = entity_prose::entity_body_prose(entry)?.trim();
    if raw.is_empty() {
        return None;
    }
    // Defense in depth: redact this entity's own declared secret_terms before the
    // prose can enter a player/prompt-safe profile. A non-spoiler entity has no
    // terms → byte-identical passthrough ("别太严"). fail-closed: a redacted-to-blank
    // result yields None rather than an empty persona line.
    let redacted = SpoilerMeta::from_value(entry).redact(raw);
    let redacted = redacted.trim();
    if redacted.is_empty() {
        return None;
    }
    Some(redacted.to_string())
}

/// Build a thin source-backed [`NpcProfile`] from a module `graph.npcs[id]` JSON
/// entry, for the given active `npc_id`.
///
/// Returns `None` (fail-closed) when the entry has no usable name. The `actor_id`
/// is the stable graph `npc_id` (the same id used by activation + the
/// actor-identity/knowledge-holder contract), NOT a display name.
///
/// MAT.M8: also folds the verbatim, spoiler-redacted source `body` prose into
/// [`NpcProfile::persona_description`] (the field the persona-safe view renders).
pub fn thin_profile_from_module_npc(npc_id: &str, entry: &serde_json::Value) -> Option<NpcProfile> {
    let name = entry
        .get("name")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if name.is_empty() {
        // No source-present display name → nothing faithful to materialize.
        return None;
    }
    // Only structured, source-present `role` is copied (optional). Everything else
    // (except the verbatim body) is left at Default — no invented persona/traits/secrets.
    let role = entry
        .get("role")
        .and_then(|x| x.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    Some(NpcProfile {
        actor_id: npc_id.to_string(),
        name,
        role,
        persona_description: source_persona_description(entry),
        ..Default::default()
    })
}

/// Locate the module NPC JSON entry for `npc_id` within a `graph.npcs` list
/// (entries keyed by `id` or `actor_id`). fail-closed → `None`.
pub fn find_module_npc<'a>(
    npcs: &'a [serde_json::Value],
    npc_id: &str,
) -> Option<&'a serde_json::Value> {
    npcs.iter().find(|v| {
        v.get("id").and_then(|x| x.as_str()) == Some(npc_id)
            || v.get("actor_id").and_then(|x| x.as_str()) == Some(npc_id)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builds_thin_profile_from_id_and_name() {
        // `正文`-keyed prose with body-key normalize OFF (default) is not read → no
        // persona description (byte-stable historic read path).
        let entry = json!({"id": "npc_athena", "name": "雅典娜", "正文": "free prose"});
        let p = thin_profile_from_module_npc("npc_athena", &entry).expect("profile");
        assert_eq!(p.actor_id, "npc_athena");
        assert_eq!(p.name, "雅典娜");
        assert!(p.role.is_none());
        assert!(p.persona_description.is_none(), "OFF: 正文 key not read");
        // No invented structure.
        assert!(p.personality_traits.is_empty());
        assert!(p.secrets.is_empty());
    }

    #[test]
    fn folds_source_body_into_persona_description() {
        // MAT.M8 TDD #1: a `body`-carrying NPC materializes a profile whose
        // persona-feeding field holds the verbatim source body prose.
        let entry = json!({
            "id": "npc_nate",
            "name": "内特",
            "body": "白天通常待在埃索加油站的三人之一。"
        });
        let p = thin_profile_from_module_npc("npc_nate", &entry).expect("profile");
        assert_eq!(
            p.persona_description.as_deref(),
            Some("白天通常待在埃索加油站的三人之一。")
        );
    }

    #[test]
    fn no_body_still_materializes_safe_name_only_profile() {
        // MAT.M8 TDD #2: an NPC with no source body still materializes a safe
        // name-only profile (no synthesis, persona_description = None).
        let entry = json!({"id": "npc_x", "name": "X"});
        let p = thin_profile_from_module_npc("npc_x", &entry).expect("profile");
        assert_eq!(p.name, "X");
        assert!(
            p.persona_description.is_none(),
            "no body → no persona prose"
        );
    }

    #[test]
    fn spoiler_secret_terms_are_redacted_before_folding() {
        // MAT.M8 TDD #4: a spoiler-flagged entity's secret_terms are redacted out of
        // the persona prose before it enters the player/prompt-safe profile.
        let entry = json!({
            "id": "npc_butler",
            "name": "管家詹姆斯",
            "body": "他其实是连环杀手，真名莫里亚蒂教授。",
            "spoiler": {
                "secret_terms": ["连环杀手", "莫里亚蒂教授"],
                "public_aliases": ["管家詹姆斯"],
                "reveal_conditions": ["在地窖发现尸体后"]
            }
        });
        let p = thin_profile_from_module_npc("npc_butler", &entry).expect("profile");
        let desc = p.persona_description.expect("redacted prose remains");
        assert!(!desc.contains("连环杀手"), "secret_term redacted: {desc}");
        assert!(
            !desc.contains("莫里亚蒂教授"),
            "secret_term redacted: {desc}"
        );
    }

    #[test]
    fn non_spoiler_body_is_passed_through_verbatim() {
        // 别太严: a non-spoiler entity's body is folded byte-for-byte (no over-redaction).
        let entry = json!({"id": "npc_russ", "name": "拉斯", "body": "他热情地招呼你。"});
        let p = thin_profile_from_module_npc("npc_russ", &entry).expect("profile");
        assert_eq!(p.persona_description.as_deref(), Some("他热情地招呼你。"));
    }

    #[test]
    fn copies_source_present_role_only() {
        let entry = json!({"id": "n1", "name": "Russ", "role": "bartender"});
        let p = thin_profile_from_module_npc("n1", &entry).expect("profile");
        assert_eq!(p.role.as_deref(), Some("bartender"));
    }

    #[test]
    fn fail_closed_when_no_name() {
        let entry = json!({"id": "n1", "正文": "prose but no name"});
        assert!(thin_profile_from_module_npc("n1", &entry).is_none());
        let blank = json!({"id": "n1", "name": "   "});
        assert!(thin_profile_from_module_npc("n1", &blank).is_none());
    }

    #[test]
    fn actor_id_is_graph_id_not_name() {
        // Even if the entry's own id differs, the caller-supplied npc_id anchors identity.
        let entry = json!({"id": "ignored", "name": "Nate"});
        let p = thin_profile_from_module_npc("npc_nate", &entry).expect("profile");
        assert_eq!(p.actor_id, "npc_nate");
    }

    #[test]
    fn find_module_npc_matches_id_or_actor_id() {
        let npcs = vec![
            json!({"id": "a", "name": "A"}),
            json!({"actor_id": "b", "name": "B"}),
        ];
        assert!(find_module_npc(&npcs, "a").is_some());
        assert!(find_module_npc(&npcs, "b").is_some());
        assert!(find_module_npc(&npcs, "c").is_none());
    }
}
