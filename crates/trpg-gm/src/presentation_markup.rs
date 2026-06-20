//! Player-visible markup pass (EXAM Q-5 / Q-7 + protocol appendix A.0 correction).
//!
//! This is now a thin adapter over the typed `TurnDocument` parser
//! (`crate::turn_markup_parser`). It parses the GM wire text into typed blocks and exposes the
//! dual-route result the caller (narrator phase) needs:
//!   - `player_text` = `TurnDocument::player_text()` — player-visible blocks only
//!     ({narration, dialogue, roll, system, choice}), deterministically rebuilt.
//!   - `meta_blocks` = `[meta]` content → engine/audit (Flight Recorder).
//!   - `hide_blocks` = `[hide]` content → campaign-canon **Proposal** (NOT player-knowledge,
//!     never auto-committed).
//!
//! ⚠️ A.0 correction (TRPG_TurnDocument_Protocol_v1 appendix A.0): the PREVIOUS implementation
//! WRONGLY stripped `[system]` from the player text. Per A.0, `[system]` is **player-visible**
//! in-flow process/prompt text; the internal out-of-game meta tag is `[meta]`. So the strip set
//! is {meta, hide}; the keep set is {narration, dialogue, roll, system, choice}.
//!
//! Q-5 empty-`[roll]` repair is preserved (a roll wrapping prose with no digit is unwrapped to
//! narration; a real roll with a digit stays intact) — implemented in the parser.
//!
//! Flag-gated by the caller (`TRPG_GM_CRAFT`); when OFF the caller skips this pass entirely, so
//! OFF==baseline byte-equal. No ruleset_id/module_id branching.

use crate::turn_markup_parser::parse_turn_document;

/// Result of the player-visible markup pass over one GM narration string.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StrippedNarration {
    /// Player-facing text: {narration, dialogue, roll, system, choice}, with `[meta]`/`[hide]`
    /// removed and empty `[roll]` wrappers unwrapped. `[system]` is KEPT (A.0).
    pub player_text: String,
    /// `[meta]` block contents — internal out-of-game note; route to engine/audit, never player.
    pub meta_blocks: Vec<String>,
    /// `[hide]` block contents — Proposal for campaign-canon, NOT player-knowledge.
    pub hide_blocks: Vec<String>,
    /// Count of empty `[roll]` wrappers that were unwrapped (inner prose preserved as narration).
    pub empty_rolls_unwrapped: usize,
    /// Count of MALFORMED `[roll]` wrappers unwrapped (R-1): a roll that fired but whose
    /// target/difficulty/result was unbound ("未定"/"未知"/"目标：?"/"DV?"). The narrator-phase
    /// debug trace + downstream 战报 assert this is ZERO (no undetermined [roll] reaches output).
    pub malformed_rolls_unwrapped: usize,
}

impl StrippedNarration {
    pub(crate) fn changed(&self, original: &str) -> bool {
        self.player_text != original
    }
}

/// Run the player-visible markup pass. Parses the wire text into a typed `TurnDocument` and
/// projects the dual-route result. `[system]` is preserved in `player_text` (A.0).
pub(crate) fn strip_player_markup(text: &str) -> StrippedNarration {
    let doc = parse_turn_document(text);
    StrippedNarration {
        player_text: doc.player_text(),
        meta_blocks: doc
            .meta_blocks()
            .iter()
            .map(|b| b.content.clone())
            .collect(),
        hide_blocks: doc
            .hide_blocks()
            .iter()
            .map(|b| b.content.clone())
            .collect(),
        empty_rolls_unwrapped: doc.empty_rolls_unwrapped,
        malformed_rolls_unwrapped: doc.malformed_rolls_unwrapped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_unchanged() {
        let r = strip_player_markup("你推开门，走廊一片寂静。");
        assert_eq!(r.player_text, "你推开门，走廊一片寂静。");
        assert!(!r.changed("你推开门，走廊一片寂静。"));
        assert!(r.meta_blocks.is_empty() && r.hide_blocks.is_empty());
        assert_eq!(r.empty_rolls_unwrapped, 0);
    }

    #[test]
    fn keeps_system_block_in_player_text_a0() {
        // A.0 regression guard: the OLD impl stripped [system]; the NEW MUST keep it.
        let r = strip_player_markup(
            "[narration]你感到一阵寒意。[/narration][system]请决定继续追问或离开。[/system]",
        );
        assert!(
            r.player_text.contains("请决定继续追问或离开"),
            "{}",
            r.player_text
        );
        assert!(r.player_text.contains("你感到一阵寒意"));
        // [system] is NOT routed to meta/hide.
        assert!(r.meta_blocks.is_empty() && r.hide_blocks.is_empty());
    }

    #[test]
    fn strips_meta_block_from_player() {
        let r = strip_player_markup(
            "[narration]脚步在身后停下。[/narration][meta]侦查 65 掷 63 通常成功[/meta]",
        );
        assert!(!r.player_text.contains("65"));
        assert!(!r.player_text.contains("侦查"));
        assert_eq!(r.meta_blocks, vec!["侦查 65 掷 63 通常成功".to_string()]);
        assert!(r.player_text.contains("脚步在身后停下"));
    }

    #[test]
    fn strips_hide_block_to_canon() {
        let r = strip_player_markup(
            "[narration]门厅空无一人。[/narration][hide]楼上的守卫悄悄换了班[/hide]",
        );
        assert!(!r.player_text.contains("守卫"));
        assert_eq!(r.hide_blocks, vec!["楼上的守卫悄悄换了班".to_string()]);
        assert_eq!(r.player_text, "门厅空无一人。");
    }

    #[test]
    fn empty_roll_unwrapped_inner_kept() {
        let r = strip_player_markup("[roll]你转身退回阴影里[/roll]");
        assert_eq!(r.player_text, "你转身退回阴影里");
        assert_eq!(r.empty_rolls_unwrapped, 1);
    }

    #[test]
    fn real_roll_block_left_intact() {
        let r = strip_player_markup("[roll]侦查 1d100=63 ≤ 65 成功[/roll]");
        assert!(r.player_text.contains("63"));
        assert!(r.player_text.contains("成功"));
        assert_eq!(r.empty_rolls_unwrapped, 0);
        assert_eq!(r.malformed_rolls_unwrapped, 0);
    }

    #[test]
    fn malformed_unbound_roll_unwrapped_and_counted_r1() {
        // R-1: 未定 / unbound-target roll → unwrapped to narration, counted separately.
        let r = strip_player_markup("[roll]结果：未定[/roll]");
        assert_eq!(r.player_text, "结果：未定");
        assert_eq!(r.malformed_rolls_unwrapped, 1);
        assert_eq!(r.empty_rolls_unwrapped, 0);
    }

    #[test]
    fn bound_roll_kept_malformed_count_zero_r1() {
        // R-1 no over-fire: a real bound check stays a [roll] and malformed count is zero.
        let r = strip_player_markup("[roll]入侵终端 1d10+6=14 ≥ DV13 成功[/roll]");
        assert!(r.player_text.contains("14"));
        assert!(r.player_text.contains("成功"));
        assert_eq!(r.malformed_rolls_unwrapped, 0);
        assert_eq!(r.empty_rolls_unwrapped, 0);
    }

    #[test]
    fn unterminated_tag_is_literal() {
        let r = strip_player_markup("她低声说 [system] 然后停住");
        assert_eq!(r.player_text, "她低声说 [system] 然后停住");
    }

    #[test]
    fn multiple_blocks_mixed_keeps_system_strips_meta_hide() {
        let r = strip_player_markup(concat!(
            "[narration]你靠近。[/narration]",
            "[system]你可以继续追问或离开。[/system]",
            "[meta]meta-note[/meta]",
            "[hide]密信已被取走[/hide]",
        ));
        assert!(r.player_text.contains("你靠近"));
        assert!(r.player_text.contains("继续追问")); // [system] kept (A.0)
        assert!(!r.player_text.contains("meta-note"));
        assert!(!r.player_text.contains("密信"));
        assert_eq!(r.meta_blocks.len(), 1);
        assert_eq!(r.hide_blocks.len(), 1);
    }
}
