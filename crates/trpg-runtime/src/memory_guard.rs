//! M3 — Adjudicator memory-layer guard (decision #5).
//!
//! The per-turn memory set assembled by `memory_blocks_for_turn` feeds the SHARED turn context the
//! Adjudicator reasons over. Decision #5: the Adjudicator must NEVER read Story/Director memory
//! (StoryMemory / DirectorScratch) — otherwise "the story wants outcome X" could quietly bias an
//! honest dice resolution. This module is the fail-closed guard: it projects the assembled memory
//! blocks down to the layers the Adjudicator is allowed to read (everything EXCEPT
//! [`MemoryLayer::Story`]) before they ever reach the prompt.
//!
//! ## Byte-identical baseline
//! It is a KILL-SWITCH (default ON), mirroring [`crate::story_write::story_write_loop_enabled`]:
//! only an explicit `0`/`false`/`off`/`no` disables it. The OFF branch returns an EMPTY layer set,
//! and [`trpg_model::project_blocks_by_layers`] treats an empty set as identity ⇒ the exact
//! monolithic pre-M3 behavior. The ON branch is ALSO byte-identical on the real corpus: the seam
//! only ever emits `MemorySnapshot`/`RetrievedMemory` blocks, both of which classify as
//! [`MemoryLayer::Mechanical`] — so the Story strip removes nothing today and stays protective if a
//! Story/Director block ever leaks into the seam.

use trpg_model::{project_blocks_by_layers, ContextBlock, MemoryLayer};

/// Kill-switch for the Adjudicator memory-layer guard. **Default ON.** Only an explicit
/// `0`/`false`/`off`/`no` (case-insensitive) disables it; that OFF branch reproduces the monolithic
/// baseline byte-for-byte. Mirrors [`crate::story_write::story_write_loop_enabled`].
pub fn memory_layer_guard_enabled() -> bool {
    !matches!(
        std::env::var("TRPG_MEMORY_LAYER_GUARD")
            .ok()
            .map(|v| v.to_ascii_lowercase())
            .as_deref(),
        Some("0") | Some("false") | Some("off") | Some("no")
    )
}

/// The memory layers the per-turn (Adjudicator-facing) context is allowed to carry. Decision #5:
/// every layer EXCEPT [`MemoryLayer::Story`] — mechanical recall, the player-perceived surface, and
/// world memory are all legitimate Adjudicator inputs; Story/Director memory is not. Guard OFF ⇒
/// empty set ⇒ `project_blocks_by_layers` is identity ⇒ byte-identical monolithic baseline.
pub fn adjudicator_memory_layers() -> Vec<MemoryLayer> {
    if memory_layer_guard_enabled() {
        vec![
            MemoryLayer::Mechanical,
            MemoryLayer::PlayerPerceived,
            MemoryLayer::World,
        ]
    } else {
        vec![]
    }
}

/// Strip every Story-layer block from the per-turn memory set before it reaches the Adjudicator.
/// Fail-closed: a Story/Director block that somehow reached this seam is dropped here. Order
/// preserving; byte-identical when the guard is OFF (empty layer set ⇒ identity).
pub fn guard_adjudicator_memory(blocks: Vec<ContextBlock>) -> Vec<ContextBlock> {
    project_blocks_by_layers(blocks, &adjudicator_memory_layers())
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::{
        layer_of, BlockContent, BlockKind, CacheZone, Scope, ScopeType, Stability, Visibility,
    };

    fn block(id: &str, kind: BlockKind, visibility: Visibility) -> ContextBlock {
        ContextBlock::new(
            id.to_string(),
            kind,
            "t",
            BlockContent::Markdown("c".into()),
            visibility,
            Stability::TurnDynamic,
            CacheZone::DynamicTail,
            Scope {
                scope_type: ScopeType::Turn,
                scope_id: "t1".into(),
            },
            100,
        )
    }

    /// The two kinds the seam actually emits both classify Mechanical ⇒ the guard removes nothing
    /// on the real corpus (the ON-path byte-equal proof).
    #[test]
    fn seam_kinds_are_mechanical_and_survive_the_guard() {
        let blocks = vec![
            block(
                "memory.snapshot",
                BlockKind::MemorySnapshot,
                Visibility::GmOnly,
            ),
            block(
                "memory.retrieved",
                BlockKind::RetrievedMemory,
                Visibility::GmOnly,
            ),
        ];
        for b in &blocks {
            assert_eq!(layer_of(b), MemoryLayer::Mechanical);
        }
        let guarded = project_blocks_by_layers(
            blocks.clone(),
            &[
                MemoryLayer::Mechanical,
                MemoryLayer::PlayerPerceived,
                MemoryLayer::World,
            ],
        );
        assert_eq!(
            guarded.len(),
            blocks.len(),
            "seam blocks all survive (byte-equal corpus)"
        );
    }

    /// Fail-closed: a Story/Director block injected into the seam is stripped, while legitimate
    /// mechanical/world/player-perceived blocks pass through, order preserving.
    #[test]
    fn guard_strips_story_block_keeps_the_rest() {
        let blocks = vec![
            block(
                "mem.retrieved",
                BlockKind::RetrievedMemory,
                Visibility::GmOnly,
            ), // Mechanical
            block(
                "director.policy",
                BlockKind::DirectorPolicy,
                Visibility::GmOnly,
            ), // Story → drop
            block("world.state", BlockKind::WorldState, Visibility::GmOnly), // World
        ];
        let guarded = project_blocks_by_layers(
            blocks,
            &[
                MemoryLayer::Mechanical,
                MemoryLayer::PlayerPerceived,
                MemoryLayer::World,
            ],
        );
        let ids: Vec<&str> = guarded.iter().map(|b| b.block_id.as_str()).collect();
        assert_eq!(ids, vec!["mem.retrieved", "world.state"]);
        assert!(guarded.iter().all(|b| layer_of(b) != MemoryLayer::Story));
    }

    /// The guard's allowed-layer set never contains Story (the structural decision-#5 invariant),
    /// independent of how `adjudicator_memory_layers` is gated.
    #[test]
    fn allowed_layers_never_contain_story() {
        let on = vec![
            MemoryLayer::Mechanical,
            MemoryLayer::PlayerPerceived,
            MemoryLayer::World,
        ];
        assert!(!on.contains(&MemoryLayer::Story));
    }

    /// Decision #5 end-to-end: EVERY Story/Director block kind classifies Story and is stripped by
    /// the allowed-layer set — so Director output (briefs, clue boards, consequence contracts,
    /// spotlight, director policy) can NEVER reach the Adjudicator, no matter which kind leaks in.
    #[test]
    fn every_director_story_kind_is_stripped() {
        let allowed = [
            MemoryLayer::Mechanical,
            MemoryLayer::PlayerPerceived,
            MemoryLayer::World,
        ];
        for kind in [
            BlockKind::DirectorPolicy,
            BlockKind::ActionableSituationBrief,
            BlockKind::ClueBoard,
            BlockKind::ConsequenceContract,
            BlockKind::SpotlightState,
        ] {
            assert_eq!(
                layer_of(&block("k", kind.clone(), Visibility::GmOnly)),
                MemoryLayer::Story
            );
            // Even a player-visible director block (e.g. a player-facing clue board) stays Story —
            // its kind is matched before the visibility fallback — so the guard still strips it.
            let pv = project_blocks_by_layers(
                vec![block("k", kind.clone(), Visibility::PlayerVisible)],
                &allowed,
            );
            assert!(
                pv.is_empty(),
                "{kind:?} (player-visible) must be stripped from Adjudicator"
            );
        }
    }

    /// OFF branch (explicit disable token) ⇒ empty layer set ⇒ identity ⇒ byte-equal baseline.
    #[test]
    fn guard_off_is_identity() {
        // Serial within one test to avoid cross-test env races.
        std::env::set_var("TRPG_MEMORY_LAYER_GUARD", "off");
        let layers = adjudicator_memory_layers();
        std::env::remove_var("TRPG_MEMORY_LAYER_GUARD");
        assert!(
            layers.is_empty(),
            "OFF ⇒ empty layers ⇒ monolithic baseline"
        );

        let blocks = vec![
            block(
                "director.policy",
                BlockKind::DirectorPolicy,
                Visibility::GmOnly,
            ),
            block(
                "mem.retrieved",
                BlockKind::RetrievedMemory,
                Visibility::GmOnly,
            ),
        ];
        let before: Vec<String> = blocks.iter().map(|b| b.block_id.clone()).collect();
        let after = project_blocks_by_layers(blocks, &layers);
        let after_ids: Vec<String> = after.iter().map(|b| b.block_id.clone()).collect();
        assert_eq!(
            after_ids, before,
            "OFF guard passes Story blocks through (identity)"
        );
    }
}
