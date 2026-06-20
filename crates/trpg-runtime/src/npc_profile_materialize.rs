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
//! (`id`/`actor_id`, `name`, optional `role`). It NEVER invents persona structure,
//! traits, or `facts_can_reveal` (those, if present in source, would flow through
//! their own typed channels; a thin profile carries none, so World reveals nothing
//! on its own — the constitutionally safe default). Free body prose is intentionally
//! NOT mapped onto structured persona fields (that would be invention); it flows via
//! `scene_npc_personas` separately.
//!
//! Gating is the caller's responsibility (Enforce only; OFF == baseline no-op).

use trpg_model::NpcProfile;

/// Build a thin source-backed [`NpcProfile`] from a module `graph.npcs[id]` JSON
/// entry, for the given active `npc_id`.
///
/// Returns `None` (fail-closed) when the entry has neither a usable name. The
/// `actor_id` is the stable graph `npc_id` (the same id used by activation +
/// the actor-identity/knowledge-holder contract), NOT a display name.
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
    // is left at Default — no invented persona/traits/secrets/facts.
    let role = entry
        .get("role")
        .and_then(|x| x.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    Some(NpcProfile {
        actor_id: npc_id.to_string(),
        name,
        role,
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
        let entry = json!({"id": "npc_athena", "name": "雅典娜", "正文": "free prose"});
        let p = thin_profile_from_module_npc("npc_athena", &entry).expect("profile");
        assert_eq!(p.actor_id, "npc_athena");
        assert_eq!(p.name, "雅典娜");
        assert!(p.role.is_none());
        // No invented structure.
        assert!(p.personality_traits.is_empty());
        assert!(p.secrets.is_empty());
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
