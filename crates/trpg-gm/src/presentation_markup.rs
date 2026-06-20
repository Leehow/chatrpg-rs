//! Player-visible markup strip (EXAM_QUALITY_BAR Q-5 / Q-7, design-philosophy §2a.2/§2b.3).
//!
//! A pure post-processing pass over the GM's narration before it is persisted/delivered to
//! the player. It enforces the unified roll-render convention and the visibility markup:
//!   - **Q-7 `[system]…[/system]`** = out-of-game meta (dice math, GM reasoning). STRIPPED
//!     from the player text; returned in `system_blocks` for engine/audit routing.
//!   - **Q-7 `[hide]…[/hide]`** = in-fiction events this player did not perceive. STRIPPED
//!     from the player text; returned in `hide_blocks` for campaign-canon routing (NOT
//!     player-knowledge).
//!   - **Q-5 empty `[roll]…[/roll]`** = a roll tag wrapping no real dice/check (pure prose
//!     or a resource delta). The wrapper is REMOVED but its inner prose is KEPT inline (so a
//!     real-roll block is untouched, while "[roll]she retreats[/roll]" becomes "she retreats").
//!     A `[roll]` block containing a digit is treated as a real check and left intact.
//!
//! Flag-gated by the caller (`TRPG_GM_CRAFT`); when OFF the caller skips this pass entirely,
//! so OFF==baseline byte-equal. No ruleset_id/module_id branching.

/// Result of stripping player-visible markup from one narration string.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StrippedNarration {
    /// Player-facing text with `[system]`/`[hide]` removed and empty `[roll]` wrappers unwrapped.
    pub player_text: String,
    /// `[system]` block contents (meta) — route to engine/audit, never the player.
    pub system_blocks: Vec<String>,
    /// `[hide]` block contents — route to campaign-canon, NOT player-knowledge.
    pub hide_blocks: Vec<String>,
    /// Count of empty `[roll]` wrappers that were unwrapped (inner prose preserved).
    pub empty_rolls_unwrapped: usize,
}

impl StrippedNarration {
    pub(crate) fn changed(&self, original: &str) -> bool {
        self.player_text != original
    }
}

/// A `[roll]…[/roll]` block is a REAL check iff its inner content contains a digit (the dice
/// total / die face / threshold of a resolved mechanical check). Otherwise it is an empty roll.
fn inner_has_real_roll(inner: &str) -> bool {
    inner.chars().any(|c| c.is_ascii_digit())
}

/// Strip player-visible markup. Returns the cleaned player text plus the routed blocks.
pub(crate) fn strip_player_markup(text: &str) -> StrippedNarration {
    let mut out = StrippedNarration::default();
    let mut player = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < text.len() {
        if let Some((tag, open_end)) = match_open_tag(text, i) {
            if let Some(close_start) = find_close(text, open_end, tag) {
                let inner = &text[open_end..close_start];
                let after = close_start + close_tag_len(tag);
                match tag {
                    Tag::System => out.system_blocks.push(inner.trim().to_string()),
                    Tag::Hide => out.hide_blocks.push(inner.trim().to_string()),
                    Tag::Roll => {
                        if inner_has_real_roll(inner) {
                            // Real check: keep the whole block verbatim.
                            player.push_str(&text[i..after]);
                        } else {
                            // Q-5: empty roll — drop the wrapper, keep inner prose.
                            player.push_str(inner);
                            out.empty_rolls_unwrapped += 1;
                        }
                    }
                }
                i = after;
                continue;
            }
        }
        // Not a recognized tag boundary: copy one UTF-8 char.
        let ch_len = utf8_len(bytes[i]);
        player.push_str(&text[i..i + ch_len]);
        i += ch_len;
    }
    out.player_text = collapse_blank_runs(&player);
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tag {
    System,
    Hide,
    Roll,
}

fn open_lit(tag: Tag) -> &'static str {
    match tag {
        Tag::System => "[system]",
        Tag::Hide => "[hide]",
        Tag::Roll => "[roll]",
    }
}

fn close_lit(tag: Tag) -> &'static str {
    match tag {
        Tag::System => "[/system]",
        Tag::Hide => "[/hide]",
        Tag::Roll => "[/roll]",
    }
}

fn close_tag_len(tag: Tag) -> usize {
    close_lit(tag).len()
}

/// If an opening tag starts at `i`, return (tag, byte index just past the opening tag).
fn match_open_tag(text: &str, i: usize) -> Option<(Tag, usize)> {
    for tag in [Tag::System, Tag::Hide, Tag::Roll] {
        let lit = open_lit(tag);
        if text[i..].starts_with(lit) {
            return Some((tag, i + lit.len()));
        }
    }
    None
}

/// Find the byte index where the matching close tag begins, searching from `from`.
fn find_close(text: &str, from: usize, tag: Tag) -> Option<usize> {
    text[from..].find(close_lit(tag)).map(|p| from + p)
}

fn utf8_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else if b >> 3 == 0b11110 {
        4
    } else {
        1
    }
}

/// Collapse 3+ consecutive newlines (left by removed blocks) into a single blank line,
/// and trim trailing/leading whitespace — keeps prose tidy after stripping.
fn collapse_blank_runs(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut newline_run = 0usize;
    for ch in s.chars() {
        if ch == '\n' {
            newline_run += 1;
            if newline_run <= 2 {
                result.push('\n');
            }
        } else {
            newline_run = 0;
            result.push(ch);
        }
    }
    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_unchanged() {
        let r = strip_player_markup("你推开门，走廊一片寂静。");
        assert_eq!(r.player_text, "你推开门，走廊一片寂静。");
        assert!(!r.changed("你推开门，走廊一片寂静。"));
        assert!(r.system_blocks.is_empty() && r.hide_blocks.is_empty());
        assert_eq!(r.empty_rolls_unwrapped, 0);
    }

    #[test]
    fn strips_system_block_from_player() {
        let r = strip_player_markup("你感到一阵寒意。[system]侦查 65 掷 63 通常成功[/system]脚步在身后停下。");
        assert!(!r.player_text.contains("[system]"));
        assert!(!r.player_text.contains("65"));
        assert_eq!(r.system_blocks, vec!["侦查 65 掷 63 通常成功".to_string()]);
        assert!(r.player_text.contains("你感到一阵寒意"));
        assert!(r.player_text.contains("脚步在身后停下"));
    }

    #[test]
    fn strips_hide_block_to_canon() {
        let r = strip_player_markup("门厅空无一人。[hide]楼上的守卫悄悄换了班[/hide]");
        assert!(!r.player_text.contains("[hide]"));
        assert!(!r.player_text.contains("守卫"));
        assert_eq!(r.hide_blocks, vec!["楼上的守卫悄悄换了班".to_string()]);
        assert_eq!(r.player_text, "门厅空无一人。");
    }

    #[test]
    fn empty_roll_unwrapped_inner_kept() {
        // Q-5 real bug: narration / resource delta wrapped in a roll tag with no dice.
        let r = strip_player_markup("[roll]你转身退回阴影里[/roll]");
        assert_eq!(r.player_text, "你转身退回阴影里");
        assert_eq!(r.empty_rolls_unwrapped, 1);
    }

    #[test]
    fn real_roll_block_left_intact() {
        let r = strip_player_markup("[roll]侦查 1d100=63 ≤ 65 成功[/roll]");
        assert_eq!(r.player_text, "[roll]侦查 1d100=63 ≤ 65 成功[/roll]");
        assert_eq!(r.empty_rolls_unwrapped, 0);
    }

    #[test]
    fn unterminated_tag_is_literal() {
        // No close tag: leave as-is (don't swallow the rest of the narration).
        let r = strip_player_markup("她低声说 [system] 然后停住");
        assert_eq!(r.player_text, "她低声说 [system] 然后停住");
    }

    #[test]
    fn multiple_blocks_mixed() {
        let r = strip_player_markup(
            "你靠近。[system]meta[/system]她的眼神闪过一丝慌乱。[hide]密信已被取走[/hide]",
        );
        assert!(!r.player_text.contains("meta"));
        assert!(!r.player_text.contains("密信"));
        assert!(r.player_text.contains("你靠近"));
        assert!(r.player_text.contains("一丝慌乱"));
        assert_eq!(r.system_blocks.len(), 1);
        assert_eq!(r.hide_blocks.len(), 1);
    }
}
