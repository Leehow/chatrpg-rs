//! World layer module boundary (P4.1, codex F9).
//!
//! This module establishes — WITHIN the existing `trpg-runtime` crate — the logical
//! boundary of the "World simulation" layer (设计4 §二: World proposes reasonable NPC
//! reactions / world pressure; it commits nothing). It is a **boundary marker + façade**,
//! not a new crate and not a code move:
//!
//! - **Re-exports only.** The npc_*/relationship_extraction items the World layer
//!   logically owns are surfaced here via `pub use` of their ORIGINAL definitions. The
//!   source modules are NOT moved or renamed, so there is ZERO behavior change and no
//!   risk to the existing green tests. Callers may migrate to `world::` paths over time.
//! - **New World-native submodules.** [`reaction`] (P4.4 reaction-set assembly) and
//!   [`clock`] (P4.5 clock delegate) are the genuinely new, additive World surfaces.
//!
//! None of this is wired into the turn loop yet (that is P4.6).

pub mod reaction;
pub use reaction::{
    assemble_world_reaction_set, derive_attack_intents, load_world_reaction_plans,
    load_world_reaction_set, render_world_reaction_block,
};

pub mod clock;
pub use clock::derive_clock_proposals;

// ── World-owned re-exports (no source move; original modules remain canonical) ────────
//
// NPC profile/persona, relationships, mind projection, behavior planning, and
// relationship extraction are the World layer's existing primitives. Re-exporting them
// here gives the layer a single import surface without duplicating or relocating code.
pub use crate::npc_behavior::{
    derive_npc_behavior_plan, load_active_npc_guidance, load_npc_behavior_plan,
    viewer_behavior_context,
};
pub use crate::npc_mind::{assemble_npc_mind_view, load_npc_mind_view};
pub use crate::npc_profile::persona_from_profile;
pub use crate::npc_relationship::{apply_npc_relationship_delta, next_relationship};
pub use crate::relationship_extraction::{
    build_relationship_messages, parse_relationship_triples, relationship_facts_from_inputs,
    relationship_gate_should_run, resolve_entity_refs, text_has_social_signal, EntityRef,
};

#[cfg(test)]
mod tests {
    /// Re-export visibility: the World-owned items resolve through the `world::` path and
    /// are the SAME items as their canonical definitions (re-export, not redefinition).
    #[test]
    fn world_reexports_are_visible() {
        // `fn_addr_eq` is the idiomatic identity check: `world::foo` and
        // `crate::module::foo` must point at one and the same function item. If a
        // re-export were accidentally shadowed/redefined, this would fail.
        assert!(
            std::ptr::fn_addr_eq(
                super::derive_npc_behavior_plan
                    as fn(
                        &trpg_model::NpcMindView,
                        &trpg_model::NpcBehaviorContext,
                    ) -> trpg_model::NpcBehaviorPlan,
                crate::npc_behavior::derive_npc_behavior_plan
                    as fn(
                        &trpg_model::NpcMindView,
                        &trpg_model::NpcBehaviorContext,
                    ) -> trpg_model::NpcBehaviorPlan,
            ),
            "world::derive_npc_behavior_plan must re-export the canonical fn"
        );
        assert!(
            std::ptr::fn_addr_eq(
                super::text_has_social_signal as fn(&str) -> bool,
                crate::relationship_extraction::text_has_social_signal as fn(&str) -> bool,
            ),
            "world::text_has_social_signal must re-export the canonical fn"
        );
    }

    /// The new World-native submodules are reachable through the boundary façade.
    #[test]
    fn world_native_submodules_are_reachable() {
        // clock delegate is fail-closed on None.
        assert!(super::derive_clock_proposals(None).is_empty());
        // assemble of no plans is the empty (commit-nothing) set.
        assert!(super::assemble_world_reaction_set(&[]).is_empty());
    }
}
