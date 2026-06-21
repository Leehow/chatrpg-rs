//! M2 — per-layer memory projection (memory-wiring).
//!
//! A single retrieval pass produces a set of [`ContextBlock`]s; different consumers need
//! different SLICES of that set (the §2 read matrix):
//!
//! - the **Adjudicator** reads MECHANICAL memory only and must NEVER receive Story/Director
//!   memory (decision #5 — kept honest by a fail-closed guard in M3, and by every story/director
//!   block classifying as [`MemoryLayer::Story`] here, never falling through to `Mechanical`);
//! - the **Narrator** reads PLAYER-PERCEIVED memory (what the player has actually seen);
//! - the **Director** reads STORY memory (threads/promises/beats/spotlight/clue board);
//! - **World** reasoning reads WORLD memory (time/events/state).
//!
//! This module REUSES the existing ContextBlock taxonomy ([`BlockKind`] + [`Visibility`]) — no new
//! columns, no parallel classification scheme. [`layer_of`] is a TOTAL function (every block maps
//! to exactly one layer) and [`project_blocks_by_layers`] is the pure filter the consumers apply.
//!
//! ## Byte-identical default
//! [`crate::MemoryQuery::layers`] is `#[serde(default)]` ⇒ an un-layered query (the pre-M2 shape,
//! and any deserialized old payload) yields an EMPTY layer set, and an empty set means
//! [`project_blocks_by_layers`] returns every block unchanged — exactly the monolithic behavior.
//! Per-layer projection only engages when a consumer explicitly asks for layers (M3).

use crate::{BlockKind, ContextBlock, Visibility};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The audience-partition of memory. Each [`ContextBlock`] belongs to exactly one layer
/// ([`layer_of`]); a consumer requests the layers it is allowed to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryLayer {
    /// GM mechanical substrate — rules, dice, checks, parameters, mechanical state. The only layer
    /// the Adjudicator reads.
    Mechanical,
    /// What the player has actually perceived — player-visible recall + the narrative surface.
    PlayerPerceived,
    /// Story/Director memory — threads, promises, beats, spotlight, clue board, consequences.
    Story,
    /// World memory — world time, scheduled/clock events, world state.
    World,
}

impl MemoryLayer {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryLayer::Mechanical => "mechanical",
            MemoryLayer::PlayerPerceived => "player_perceived",
            MemoryLayer::Story => "story",
            MemoryLayer::World => "world",
        }
    }
}

/// Classify one [`ContextBlock`] into its [`MemoryLayer`] — TOTAL (every block maps to one layer).
///
/// Story/Director and World kinds are matched EXPLICITLY so they can never fall through to the
/// `Mechanical` default (the Adjudicator guard depends on story/director blocks classifying as
/// [`MemoryLayer::Story`]). The narrative surface (transcript/current input) is player-perceived.
/// Everything else is the GM mechanical substrate, EXCEPT a player-visible/public block, which is
/// player-perceived recall regardless of its kind.
pub fn layer_of(block: &ContextBlock) -> MemoryLayer {
    use BlockKind::*;
    match block.kind {
        // ── Story / Director memory (decision #5: NEVER reaches the Adjudicator) ──────────────
        DirectorPolicy | ActionableSituationBrief | ClueBoard | ConsequenceContract
        | SpotlightState => MemoryLayer::Story,
        // ── World memory (time / events / state) ─────────────────────────────────────────────
        WorldState | WorldTime | WorldEvent | TimeAdvance | ScheduledEvent | TimeAnchor
        | ClockEvent => MemoryLayer::World,
        // ── Player-perceived narrative surface ───────────────────────────────────────────────
        RecentTranscript | CurrentInput => MemoryLayer::PlayerPerceived,
        // ── Default: GM mechanical substrate, unless the block is player-visible recall ───────
        _ => match block.visibility {
            Visibility::PlayerVisible | Visibility::Public => MemoryLayer::PlayerPerceived,
            _ => MemoryLayer::Mechanical,
        },
    }
}

/// Project a block set to the requested memory `layers`. **EMPTY `layers` ⇒ identity** (every block
/// passes, returned unchanged) — this is the serde-default of [`crate::MemoryQuery::layers`], so an
/// un-layered query keeps the exact pre-M2 monolithic behavior (byte-identical). A non-empty set
/// keeps only blocks whose [`layer_of`] is in the set, ORDER-PRESERVING.
pub fn project_blocks_by_layers(
    blocks: Vec<ContextBlock>,
    layers: &[MemoryLayer],
) -> Vec<ContextBlock> {
    if layers.is_empty() {
        return blocks;
    }
    blocks
        .into_iter()
        .filter(|b| layers.contains(&layer_of(b)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlockContent, CacheZone, Scope, ScopeType, Stability};

    fn block(kind: BlockKind, visibility: Visibility) -> ContextBlock {
        ContextBlock::new(
            format!("blk.{}", kind_token(&kind)),
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

    fn kind_token(kind: &BlockKind) -> String {
        serde_json::to_value(kind)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| "k".into())
    }

    #[test]
    fn layer_of_classifies_each_family() {
        // Story/Director kinds → Story (must never be Mechanical — Adjudicator guard depends on it)
        for k in [
            BlockKind::DirectorPolicy,
            BlockKind::ActionableSituationBrief,
            BlockKind::ClueBoard,
            BlockKind::ConsequenceContract,
            BlockKind::SpotlightState,
        ] {
            assert_eq!(layer_of(&block(k, Visibility::GmOnly)), MemoryLayer::Story);
        }
        // World kinds → World
        for k in [
            BlockKind::WorldState,
            BlockKind::WorldTime,
            BlockKind::WorldEvent,
            BlockKind::ClockEvent,
        ] {
            assert_eq!(layer_of(&block(k, Visibility::GmOnly)), MemoryLayer::World);
        }
        // Narrative surface → PlayerPerceived
        assert_eq!(
            layer_of(&block(BlockKind::RecentTranscript, Visibility::PlayerVisible)),
            MemoryLayer::PlayerPerceived
        );
        // GM-only recall → Mechanical
        assert_eq!(
            layer_of(&block(BlockKind::RetrievedMemory, Visibility::GmOnly)),
            MemoryLayer::Mechanical
        );
        assert_eq!(
            layer_of(&block(BlockKind::MemorySnapshot, Visibility::GmOnly)),
            MemoryLayer::Mechanical
        );
        // A player-visible block of an otherwise-mechanical kind is player-perceived recall.
        assert_eq!(
            layer_of(&block(BlockKind::MemoryFact, Visibility::PlayerVisible)),
            MemoryLayer::PlayerPerceived
        );
    }

    #[test]
    fn empty_layers_is_identity_byte_equal() {
        let blocks = vec![
            block(BlockKind::DirectorPolicy, Visibility::GmOnly),
            block(BlockKind::RetrievedMemory, Visibility::GmOnly),
            block(BlockKind::WorldState, Visibility::GmOnly),
        ];
        let before_ids: Vec<String> = blocks.iter().map(|b| b.block_id.clone()).collect();
        let after = project_blocks_by_layers(blocks, &[]);
        let after_ids: Vec<String> = after.iter().map(|b| b.block_id.clone()).collect();
        assert_eq!(
            after_ids, before_ids,
            "empty layers ⇒ identity (monolithic baseline)"
        );
    }

    #[test]
    fn single_layer_keeps_only_that_layer_order_preserving() {
        let blocks = vec![
            block(BlockKind::DirectorPolicy, Visibility::GmOnly), // Story
            block(BlockKind::RetrievedMemory, Visibility::GmOnly), // Mechanical
            block(BlockKind::ClueBoard, Visibility::PlayerVisible), // Story
            block(BlockKind::WorldState, Visibility::GmOnly),     // World
        ];
        let story = project_blocks_by_layers(blocks.clone(), &[MemoryLayer::Story]);
        assert_eq!(story.len(), 2);
        assert_eq!(story[0].kind, BlockKind::DirectorPolicy);
        assert_eq!(story[1].kind, BlockKind::ClueBoard);
        // Adjudicator slice excludes every Story block.
        let mech = project_blocks_by_layers(blocks, &[MemoryLayer::Mechanical]);
        assert_eq!(mech.len(), 1);
        assert_eq!(mech[0].kind, BlockKind::RetrievedMemory);
        assert!(mech.iter().all(|b| layer_of(b) == MemoryLayer::Mechanical));
    }

    #[test]
    fn memory_query_layers_serde_default_absent_is_empty() {
        // A pre-M2 payload (no `layers` field) must deserialize to an empty set ⇒ monolithic
        // behavior. Build it by serializing a real query and dropping the `layers` key, so the test
        // stays robust to the rest of MemoryQuery's serde shape.
        let q = crate::MemoryQuery {
            session_id: "s1".into(),
            text: String::new(),
            ruleset_id: None,
            module_id: None,
            scene_id: None,
            location_id: None,
            actor_ids: vec![],
            tags: vec![],
            limit: 8,
            viewer: crate::VisibilityProfile::gm(),
            layers: vec![MemoryLayer::Story],
        };
        let mut v = serde_json::to_value(&q).unwrap();
        v.as_object_mut().unwrap().remove("layers");
        let back: crate::MemoryQuery = serde_json::from_value(v).expect("deserialize without layers");
        assert!(
            back.layers.is_empty(),
            "absent layers ⇒ empty ⇒ byte-equal baseline"
        );
    }
}
