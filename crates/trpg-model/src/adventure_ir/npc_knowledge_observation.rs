//! F1 `progress_scene_npc_knowledge_v1` — **scene-salient NPC-knowledge atoms**
//! (broadens scene-advance coverage to talk/combat / non-investigative scenes).
//!
//! Wall B residual (the J3 freeze on talk scenes): EV-P3
//! [`super::compile_authored_observations`] only mints scene-salient atoms from
//! affordance items, scene-mechanic intents, and gm-notes affordances — all
//! **investigative** content. A talk/interrogation scene (homecoming
//! `scene_02_questions_for_athena`: `scene_mechanics=0`, `gm_notes=null`, no clue/
//! location/encounter refs) has none of those, so it compiles **0** scene-tagged
//! atoms ⇒ [`super::scene_advance_objective`] is `None` ⇒ the scene can never earn
//! its `obj.scene_advance.*` ⇒ it freezes (max_frozen_run=15, T7–T21).
//!
//! Yet such a scene DOES carry authored salient-advance content: the **NPC knowledge
//! a scene authorizes the player to elicit**, sitting in the scene's typed
//! `referenced_npc_ids` + the referenced NPC's authored prose/secrets (e.g.
//! `npc_athena`'s 825-char `body` of dialogue beats + `spoiler.secret_terms`). This
//! module re-expresses that as one **EntityEncountered** atom per (scene, salient
//! referenced NPC), tagged `scene:<id>`, so it flows through the EXACT existing
//! pipeline (catalog → offer → witness admit → [`super::scene_advance_objective`]'s
//! `EvidenceAnyOf` → D2 consume → E1 transition). NO new matcher, NO parallel path.
//!
//! Design law honored (codex-validated):
//! - **source-grounded** — an atom is emitted ONLY for an NPC the scene's typed
//!   `referenced_npc_ids` names AND that exists in `graph.npcs` AND carries authored
//!   content; its `SourceRef` cites the scene + the authored NPC span. No content ⇒
//!   no atom (fail-closed). Whether the player *actually* engaged the NPC is the
//!   untrusted LLM witness's opaque call; Rust only does `atom_id` set membership
//!   (`LLM ∩ ProgressSignal = ∅`). CarrierOnly never completes an objective alone.
//! - **NO keyword/regex/alias map, NO scene-type classifier, NO ruleset/module name
//!   branch** — salience is purely structural: `referenced_npc_ids` ∩ NPC
//!   authored-content presence. Talk/combat coverage is an EFFECT of the uniform
//!   rule, never a classification of scene kind.
//! - **read_aloud is NEVER touched** — we read the typed `referenced_npc_ids` and the
//!   NPC's authored `body`/`spoiler` (authored GM knowledge), never the scene's
//!   flavour `read_aloud` box (EV-P3 discipline, authored_observation.rs:20).
//! - **additive · flag-gated** — [`scene_npc_knowledge_enabled`] default OFF; the
//!   single gated `extend` in [`super::compile_authored_observations`] means OFF ⇒
//!   byte-identical to the investigative-only baseline.

use crate::adventure_ir::authored_observation::tag_scene;
use crate::adventure_ir::{AtomId, EvidenceAtomSpec, EvidenceKind, ProgressRole};
use crate::entity_prose::entity_body_prose;
use crate::{ModuleGraph, SourceRef};
use serde_json::Value;

/// Env flag enabling the scene-salient NPC-knowledge atom source. Default OFF ⇒
/// [`super::compile_authored_observations`] is byte-identical to the investigative-
/// only baseline. Strict whitelist (`1`/`true`/`on`/`yes`, case-insensitive) so no
/// stray truthy-looking value silently arms new behavior.
pub const SCENE_NPC_KNOWLEDGE_ENV: &str = "TRPG_PROGRESS_SCENE_NPC_KNOWLEDGE_V1";

/// Whether F1 NPC-knowledge atoms are compiled this run (default OFF).
pub fn scene_npc_knowledge_enabled() -> bool {
    std::env::var(SCENE_NPC_KNOWLEDGE_ENV)
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "on" | "yes"
            )
        })
        .unwrap_or(false)
}

/// Structural, fail-closed test that a referenced NPC carries **authored knowledge
/// substance** the scene can authorize eliciting — i.e. it is more than a bare
/// id+name mention. True iff the NPC has non-empty authored body prose (via the
/// canonical [`entity_body_prose`] accessor) OR a non-empty `spoiler.secret_terms`
/// array (authored secrets to elicit). This is data-access on typed NPC fields, NOT
/// a semantic keyword classification.
pub fn npc_has_authored_knowledge(npc: &Value) -> bool {
    let has_body = entity_body_prose(npc)
        .map(|b| !b.trim().is_empty())
        .unwrap_or(false);
    let has_secrets = npc
        .get("spoiler")
        .and_then(|s| s.get("secret_terms"))
        .and_then(Value::as_array)
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    has_body || has_secrets
}

/// The NPC JSON in `graph.npcs` whose `id` exactly equals `npc_id`, or `None`
/// (fail-closed: a dangling reference yields no atom).
fn find_npc<'a>(graph: &'a ModuleGraph, npc_id: &str) -> Option<&'a Value> {
    graph.npcs.iter().find(|n| {
        n.get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .map(|id| id == npc_id)
            .unwrap_or(false)
    })
}

/// A short, source-grounded display excerpt of the NPC's authored body (≤`max`
/// chars), used as the atom's `SourceRef.note` (the GM-facing offer meaning). `None`
/// if the NPC has no body prose (caller falls back to the name).
fn body_excerpt(npc: &Value, max: usize) -> Option<String> {
    let body = entity_body_prose(npc)?.trim();
    if body.is_empty() {
        return None;
    }
    Some(body.chars().take(max).collect())
}

/// Compile **EntityEncountered** CarrierOnly atoms from each scene's typed
/// `referenced_npc_ids` whose NPC carries authored knowledge substance. One atom per
/// (scene, salient NPC), tagged `scene:<scene_id>`. Source-grounded; fail-closed (no
/// module id / dangling ref / contentless NPC ⇒ no atom). Mirrors the
/// [`super::compile_authored_observations`] CarrierOnly idiom (the scene-advance
/// objective re-roles GuardLeaf via [`super::scene_advance_guard_leaves`]).
pub fn from_referenced_npc_knowledge(graph: &ModuleGraph) -> Vec<EvidenceAtomSpec> {
    let module_id = graph.module_id.trim();
    if module_id.is_empty() {
        return Vec::new(); // source-grounded: no real source id ⇒ no atom
    }
    let mut out = Vec::new();
    for scene in &graph.scenes {
        let scene_id = scene.node_id.trim();
        if scene_id.is_empty() {
            continue;
        }
        for npc_ref in &scene.referenced_npc_ids {
            let npc_id = npc_ref.trim();
            if npc_id.is_empty() {
                continue;
            }
            let Some(npc) = find_npc(graph, npc_id) else {
                continue; // fail-closed: dangling NPC reference
            };
            if !npc_has_authored_knowledge(npc) {
                continue; // fail-closed: bare mention, no salient knowledge to elicit
            }
            // Canonical bound ref = the NPC's own id (sorted into the atom id by
            // AtomId::from_parts). source_span encodes the scene so the SAME NPC
            // referenced by two scenes yields two DISTINCT atom_ids.
            let source_span = format!("scene_npc:{scene_id}");
            let atom_id = AtomId::from_parts(
                module_id,
                &source_span,
                EvidenceKind::EntityEncountered,
                std::slice::from_ref(&npc_id.to_string()),
            );
            let name = npc
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(npc_id);
            let note = match body_excerpt(npc, 120) {
                Some(ex) => format!("{name}: {ex}"),
                None => name.to_string(),
            };
            let source = SourceRef {
                source_id: module_id.to_string(),
                anchor_id: Some(source_span),
                page: scene.page_start,
                note: Some(note),
                ..Default::default()
            };
            // grounding is SCENE-SCOPED so the compiler's grounding-dedup
            // (authored_observation.rs) keeps one atom PER scene, not one per NPC
            // (codex GAP#5: a non-scoped `entity:<npc>` would collapse the same NPC
            // across scenes and drop the later scene's atom). Distinct shape from the
            // existing `entity:<ref>` / `action:<ref>` groundings ⇒ no collision.
            let grounding = format!("entity:{scene_id}:{npc_id}");
            let atom = EvidenceAtomSpec {
                atom_id,
                kind: EvidenceKind::EntityEncountered,
                bindings: vec![npc_id.to_string()],
                source_refs: vec![source],
                grounding,
                progress_role: ProgressRole::CarrierOnly,
            };
            out.push(tag_scene(atom, scene_id));
        }
    }
    out
}

#[cfg(test)]
#[path = "npc_knowledge_observation_tests.rs"]
mod tests;
