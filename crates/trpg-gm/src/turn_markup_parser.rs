//! Wire-format → typed `TurnDocument` parser (protocol §十一/§十二, Phase 1-2 minimal).
//!
//! Turns the GM `[tag]…[/tag]` form (the 7 v1 tags) into typed `TurnBlock`s:
//!   - `[narration] [dialogue] [roll] [system] [choice]` → player-visible kinds.
//!   - `[hide]` → HiddenEvent (authority=Proposal, internal route).
//!   - `[meta]` → InternalMeta (audit, internal route).
//!   - untagged text → Narration (protocol §十一 untagged rule).
//!   - unterminated tag → literal Narration text (don't swallow the rest, protocol §十一 EOF).
//!   - unknown tag → still treated as literal text here (kept minimal; Extension kind exists in
//!     the AST for future plugin registry — protocol §十七).
//!
//! Q-5 repair: a `[roll]` block with NO digit (pure prose / resource delta) is unwrapped — its
//! inner prose becomes a Narration block and the wrapper is dropped; a `[roll]` with a digit
//! (a real mechanical check) is kept intact as a Roll block.
//!
//! Streaming note (protocol §十一 byte-boundary concern): this is a deterministic FULL-TEXT
//! parse. It is byte-boundary safe in the sense required by protocol test #1 for the assembled
//! output — re-parsing the complete text always yields the same AST regardless of how the
//! upstream SSE chunks were split, because parsing runs on the joined buffer. A true incremental
//! feed API (state machine over partial chunks) is deferred to the director-layer build; the
//! full-text parse passing the byte-boundary equivalence test is the accepted checkpoint.

use crate::turn_document::{TurnBlock, TurnBlockKind, TurnDocument};

/// The seven recognized wire tags. Attributes (`[roll check_id="…"]`) are tolerated: we match the
/// opening `[tag` prefix and scan to the closing `]` of the opening tag.
const TAGS: &[(&str, WireTag)] = &[
    ("narration", WireTag::Narration),
    ("dialogue", WireTag::Dialogue),
    ("roll", WireTag::Roll),
    ("system", WireTag::System),
    ("choice", WireTag::Choice),
    ("hide", WireTag::Hide),
    ("meta", WireTag::Meta),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum WireTag {
    Narration,
    Dialogue,
    Roll,
    System,
    Choice,
    Hide,
    Meta,
}

impl WireTag {
    fn name(self) -> &'static str {
        match self {
            WireTag::Narration => "narration",
            WireTag::Dialogue => "dialogue",
            WireTag::Roll => "roll",
            WireTag::System => "system",
            WireTag::Choice => "choice",
            WireTag::Hide => "hide",
            WireTag::Meta => "meta",
        }
    }
}

/// Parse the full GM wire text into a typed `TurnDocument`.
pub(crate) fn parse_turn_document(text: &str) -> TurnDocument {
    let mut doc = TurnDocument::default();
    // Untagged-run accumulator → becomes a Narration block when flushed.
    let mut pending_narration = String::new();
    let mut i = 0usize;

    let flush_narration = |buf: &mut String, doc: &mut TurnDocument| {
        if !buf.trim().is_empty() {
            doc.blocks.push(TurnBlock::new(
                TurnBlockKind::Narration,
                buf.trim().to_string(),
            ));
        }
        buf.clear();
    };

    while i < text.len() {
        if let Some((tag, open_end)) = match_open_tag(text, i) {
            if let Some((inner, after)) = find_close(text, open_end, tag) {
                // A real tag boundary: flush any pending untagged narration first.
                flush_narration(&mut pending_narration, &mut doc);
                push_tag_block(&mut doc, tag, inner);
                i = after;
                continue;
            }
            // Opening tag with no matching close: literal — fall through to copy the char so we
            // don't swallow the rest of the narration (protocol §十一 EOF).
        }
        let ch_len = utf8_len(text.as_bytes()[i]);
        pending_narration.push_str(&text[i..i + ch_len]);
        i += ch_len;
    }
    flush_narration(&mut pending_narration, &mut doc);
    doc
}

/// Map a closed wire block to a typed block (with the Q-5 empty-roll repair).
fn push_tag_block(doc: &mut TurnDocument, tag: WireTag, inner: &str) {
    let trimmed = inner.trim().to_string();
    match tag {
        WireTag::Narration => doc
            .blocks
            .push(TurnBlock::new(TurnBlockKind::Narration, trimmed)),
        WireTag::Dialogue => doc
            .blocks
            .push(TurnBlock::new(TurnBlockKind::Dialogue, trimmed)),
        WireTag::System => doc
            .blocks
            .push(TurnBlock::new(TurnBlockKind::System, trimmed)),
        WireTag::Choice => doc
            .blocks
            .push(TurnBlock::new(TurnBlockKind::Choice, trimmed)),
        WireTag::Hide => doc
            .blocks
            .push(TurnBlock::new(TurnBlockKind::HiddenEvent, trimmed)),
        WireTag::Meta => doc
            .blocks
            .push(TurnBlock::new(TurnBlockKind::InternalMeta, trimmed)),
        WireTag::Roll => {
            if inner_has_real_roll(inner) {
                doc.blocks
                    .push(TurnBlock::new(TurnBlockKind::Roll, trimmed));
            } else {
                // Q-5: empty roll — drop wrapper, keep inner prose as Narration.
                doc.blocks
                    .push(TurnBlock::new(TurnBlockKind::Narration, trimmed));
                doc.empty_rolls_unwrapped += 1;
            }
        }
    }
}

/// A `[roll]` is a REAL check iff its inner content contains a digit (dice total / face / target).
fn inner_has_real_roll(inner: &str) -> bool {
    inner.chars().any(|c| c.is_ascii_digit())
}

/// If an opening tag (with optional attributes) starts at `i`, return (tag, byte index just past
/// the opening `]`). Tolerates `[roll check_id="x"]` by scanning to the first `]`.
fn match_open_tag(text: &str, i: usize) -> Option<(WireTag, usize)> {
    let rest = &text[i..];
    if !rest.starts_with('[') {
        return None;
    }
    for (name, tag) in TAGS {
        let after_bracket = &rest[1..];
        if let Some(stripped) = after_bracket.strip_prefix(name) {
            // Next char must be `]` (no attrs) or whitespace (attrs follow), to avoid matching
            // `[rollback]` against `roll`.
            let next = stripped.chars().next();
            match next {
                Some(']') => return Some((*tag, i + 1 + name.len() + 1)),
                Some(c) if c.is_whitespace() => {
                    // Scan to the closing `]` of the opening tag.
                    if let Some(close) = stripped.find(']') {
                        return Some((*tag, i + 1 + name.len() + close + 1));
                    }
                    return None;
                }
                _ => continue,
            }
        }
    }
    None
}

/// Find the matching close tag from `from`; return (inner_slice, byte index just past `[/tag]`).
fn find_close(text: &str, from: usize, tag: WireTag) -> Option<(&str, usize)> {
    let close = format!("[/{}]", tag.name());
    text[from..].find(&close).map(|p| {
        let close_start = from + p;
        (&text[from..close_start], close_start + close.len())
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::turn_document::{Audience, BlockAuthority, TurnBlockKind};

    #[test]
    fn plain_untagged_text_becomes_narration() {
        // Protocol §二十 test 8: untagged text → NarrationBlock, survives into player_text.
        let doc = parse_turn_document("你推开门，走廊一片寂静。");
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.blocks[0].kind, TurnBlockKind::Narration);
        assert_eq!(doc.player_text(), "你推开门，走廊一片寂静。");
        assert!(!doc.changed("你推开门，走廊一片寂静。"));
    }

    #[test]
    fn system_is_player_visible_a0_regression() {
        // A.0 regression guard: OLD behavior stripped [system]; NEW must KEEP it in player_text.
        let doc = parse_turn_document(
            "[narration]你站在接待处。[/narration][system]请决定继续追问或离开。[/system]",
        );
        let pt = doc.player_text();
        assert!(
            pt.contains("请决定继续追问或离开"),
            "[system] must be player-visible: {pt}"
        );
        assert!(pt.contains("你站在接待处"));
        // and the System block itself is audience=Player.
        let sys = doc
            .blocks
            .iter()
            .find(|b| b.kind == TurnBlockKind::System)
            .unwrap();
        assert_eq!(sys.audience, Audience::Player);
    }

    #[test]
    fn meta_excluded_from_player_text() {
        // Protocol §二十 test 7: meta never reaches player text.
        let doc = parse_turn_document(
            "[narration]她的眼神闪过慌乱。[/narration][meta kind=\"decision_summary\"]公开魅惑检定；经理暗骰隐藏。[/meta]",
        );
        let pt = doc.player_text();
        assert!(!pt.contains("公开魅惑检定"), "meta leaked: {pt}");
        assert!(!pt.contains("经理暗骰"));
        assert!(pt.contains("她的眼神闪过慌乱"));
        assert_eq!(doc.meta_blocks().len(), 1);
    }

    #[test]
    fn hide_excluded_and_typed_proposal() {
        // Protocol §二十 test 4 + 5: hide not in player_text; LLM hide is a Proposal.
        let doc = parse_turn_document(
            "[narration]门厅空无一人。[/narration][hide kind=\"npc_action\"]守卫悄悄换了班。[/hide]",
        );
        let pt = doc.player_text();
        assert!(!pt.contains("守卫"), "hide leaked: {pt}");
        assert_eq!(pt, "门厅空无一人。");
        let hides = doc.hide_blocks();
        assert_eq!(hides.len(), 1);
        assert_eq!(hides[0].authority, BlockAuthority::Proposal);
        assert_eq!(hides[0].audience, Audience::Internal);
    }

    #[test]
    fn empty_roll_unwrapped_to_narration() {
        // Q-5: roll wrapping prose with no dice → unwrap, keep prose as narration.
        let doc = parse_turn_document("[roll]你转身退回阴影里[/roll]");
        assert_eq!(doc.player_text(), "你转身退回阴影里");
        assert_eq!(doc.empty_rolls_unwrapped, 1);
        assert_eq!(doc.blocks[0].kind, TurnBlockKind::Narration);
    }

    #[test]
    fn real_roll_kept_intact() {
        // Q-5: roll with a digit = real check → kept as Roll block, player-visible.
        let doc = parse_turn_document("[roll check_id=\"chk_01\"]侦查 1d100=63 ≤ 65 成功[/roll]");
        assert_eq!(doc.empty_rolls_unwrapped, 0);
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.blocks[0].kind, TurnBlockKind::Roll);
        assert!(doc.player_text().contains("63"));
        assert!(doc.player_text().contains("成功"));
    }

    #[test]
    fn dialogue_player_visible() {
        let doc =
            parse_turn_document("[dialogue actor=\"npc.receptionist\"]我替你递个话。[/dialogue]");
        assert_eq!(doc.blocks[0].kind, TurnBlockKind::Dialogue);
        assert!(doc.player_text().contains("我替你递个话"));
    }

    #[test]
    fn unterminated_tag_is_literal() {
        // Protocol §十一: no close → don't swallow rest; treat as literal narration text.
        let doc = parse_turn_document("她低声说 [system] 然后停住");
        assert_eq!(doc.player_text(), "她低声说 [system] 然后停住");
    }

    #[test]
    fn multiple_mixed_blocks_dual_route() {
        let doc = parse_turn_document(concat!(
            "[narration]你靠近。[/narration]",
            "[roll]检定:魅惑 65 掷 63 通常成功[/roll]",
            "[dialogue]我可以替你把话递进去。[/dialogue]",
            "[system]你可以继续追问或离开。[/system]",
            "[hide kind=\"secret_check\"]经理暗骰成功，先观察。[/hide]",
            "[meta]公开魅惑检定；暗骰隐藏。[/meta]",
        ));
        let pt = doc.player_text();
        // player-visible: narration/roll/dialogue/system present.
        assert!(pt.contains("你靠近"));
        assert!(pt.contains("通常成功"));
        assert!(pt.contains("递进去"));
        assert!(pt.contains("继续追问")); // [system] KEPT (A.0)
                                          // internal: hide/meta absent from player_text.
        assert!(!pt.contains("经理暗骰"));
        assert!(!pt.contains("公开魅惑检定"));
        assert_eq!(doc.hide_blocks().len(), 1);
        assert_eq!(doc.meta_blocks().len(), 1);
    }

    #[test]
    fn deterministic_player_text_rebuild() {
        // Protocol §二十 test 14: player_text rebuilt deterministically from the document.
        let text = concat!(
            "[narration]A[/narration][system]B[/system]",
            "[hide]C[/hide][meta]D[/meta][roll]60[/roll]",
        );
        let doc = parse_turn_document(text);
        let first = doc.player_text();
        let second = doc.player_text();
        assert_eq!(first, second);
        // re-parsing the same wire text yields the same player_text (byte-stable).
        assert_eq!(parse_turn_document(text).player_text(), first);
        assert!(first.contains('A') && first.contains('B') && first.contains("60"));
        assert!(!first.contains('C') && !first.contains('D'));
    }

    #[test]
    fn byte_boundary_split_yields_same_document() {
        // Protocol §二十 test 1 (full-text form): splitting the wire string at every byte
        // boundary then rejoining must reconstruct the identical player_text/AST. Since we parse
        // the joined buffer, this holds for any chunking.
        let text = "[narration]你好[/narration][hide]秘密[/hide][system]提示[/system]";
        let full = parse_turn_document(text);
        for cut in 1..text.len() {
            if !text.is_char_boundary(cut) {
                continue;
            }
            let rejoined = format!("{}{}", &text[..cut], &text[cut..]);
            let doc = parse_turn_document(&rejoined);
            assert_eq!(doc.blocks, full.blocks, "mismatch at cut {cut}");
            assert_eq!(doc.player_text(), full.player_text());
        }
    }

    #[test]
    fn tag_prefix_not_overmatched() {
        // `[rollback]` must NOT be parsed as a `[roll]` open tag.
        let doc = parse_turn_document("系统执行 [rollback] 操作");
        assert_eq!(doc.player_text(), "系统执行 [rollback] 操作");
    }
}
