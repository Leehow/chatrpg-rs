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
//! Q-5 / R-1 repair: a `[roll]` with NO digit OR an unbound/"未定" marker (target/result not bound)
//! is MALFORMED → unwrapped to Narration (counted in `empty_rolls_unwrapped` / `malformed_rolls_
//! unwrapped`); only a `[roll]` with a digit AND no unbound marker is kept as a real Roll block.
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
// Longest-name-first within shared prefixes is not required (we confirm the next char is `]`
// or whitespace), but `progress_claims` must precede nothing it prefixes — it stands alone.
const TAGS: &[(&str, WireTag)] = &[
    ("narration", WireTag::Narration),
    ("dialogue", WireTag::Dialogue),
    ("roll", WireTag::Roll),
    ("system", WireTag::System),
    ("choice", WireTag::Choice),
    ("hide", WireTag::Hide),
    ("meta", WireTag::Meta),
    ("progress_claims", WireTag::ProgressClaims),
    ("evidence_audit", WireTag::EvidenceAudit),
    ("evidence_attempts", WireTag::EvidenceAttempts),
    ("materialized_content", WireTag::MaterializedContent),
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
    /// EV-4R `[progress_claims]` — GM-only sidecar; inner is a JSON array of closed-schema
    /// EvidenceClaim. Parsed into `TurnDocument.progress_claims`; NO player-visible block.
    ProgressClaims,
    /// EV-P1 `[evidence_audit]` — GM-only sidecar; inner is a single closed-schema JSON
    /// object (EvidenceAudit). Parsed into `TurnDocument.evidence_audit`; NO player block.
    EvidenceAudit,
    /// EV-P4 `[evidence_attempts]` — GM-only sidecar; inner is a JSON array of closed
    /// `{cap_id}` objects (the offered actions a check attempts this turn). Parsed into
    /// `TurnDocument.evidence_attempts`; NO player-visible block.
    EvidenceAttempts,
    /// EV-P2 `[materialized_content]` — GM-only sidecar; inner is a JSON array of
    /// closed-schema ContentDelivery. Parsed into `TurnDocument.materialized_content`;
    /// NO player-visible block.
    MaterializedContent,
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
            WireTag::ProgressClaims => "progress_claims",
            WireTag::EvidenceAudit => "evidence_audit",
            WireTag::EvidenceAttempts => "evidence_attempts",
            WireTag::MaterializedContent => "materialized_content",
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
        WireTag::ProgressClaims => {
            // EV-4R: the inner is a JSON array of closed-schema EvidenceClaim. Reuse the EV-4
            // fail-closed per-entry parser (`evidence_gateway::parse_progress_claims`, which
            // filters forged/empty-basis entries via the `deny_unknown_fields` schema) by
            // wrapping the array under the `progress_claims` key it expects. Non-array /
            // malformed inner ⇒ no claims (fail-closed). NO player-visible block is pushed ⇒
            // the tag is consumed but never reaches `player_text` (GM-only, like `[meta]`).
            if let Ok(arr) = serde_json::from_str::<serde_json::Value>(trimmed.trim()) {
                if arr.is_array() {
                    let wrapped = serde_json::json!({ "progress_claims": arr });
                    let claims = trpg_runtime::evidence_gateway::parse_progress_claims(&wrapped);
                    doc.progress_claims.extend(claims);
                }
            }
        }
        WireTag::EvidenceAudit => {
            // EV-P1: the inner is a single closed-schema EvidenceAudit JSON object.
            // Fail-closed: a non-object / forged / malformed audit ⇒ `None` (the live
            // completeness check then logs a ProducerProtocolFailure — never guessed).
            // NO player-visible block ⇒ GM-only, like `[meta]`. Last well-formed tag wins.
            if let Ok(audit) =
                serde_json::from_str::<trpg_model::adventure_ir::EvidenceAudit>(trimmed.trim())
            {
                doc.evidence_audit = Some(audit);
            }
        }
        WireTag::EvidenceAttempts => {
            // EV-P4: the inner is a JSON array of closed `{cap_id}` objects. Reuse the
            // fail-closed parser (`evidence_binding::parse_evidence_attempts`, which
            // drops forged/extra-field/empty entries via `deny_unknown_fields`) by
            // wrapping the array under the `evidence_attempts` key it expects. Non-array
            // / malformed inner ⇒ no attempts. NO player-visible block ⇒ GM-only.
            if let Ok(arr) = serde_json::from_str::<serde_json::Value>(trimmed.trim()) {
                if arr.is_array() {
                    let wrapped = serde_json::json!({ "evidence_attempts": arr });
                    let caps = trpg_runtime::evidence_binding::parse_evidence_attempts(&wrapped);
                    doc.evidence_attempts.extend(caps);
                }
            }
        }
        WireTag::MaterializedContent => {
            // EV-P2: the inner is a JSON array of closed-schema ContentDelivery. Parse
            // each entry through the `deny_unknown_fields` schema; a forged entry
            // (extra field), bad recipient, or empty basis is dropped per-entry
            // (fail-closed). Non-array / malformed inner ⇒ no deliveries. NO
            // player-visible block ⇒ GM-only, like `[meta]`.
            if let Ok(serde_json::Value::Array(arr)) =
                serde_json::from_str::<serde_json::Value>(trimmed.trim())
            {
                for entry in arr {
                    if let Ok(cd) =
                        serde_json::from_value::<trpg_model::adventure_ir::ContentDelivery>(entry)
                    {
                        doc.materialized_content.push(cd);
                    }
                }
            }
        }
        WireTag::Roll => {
            if inner_is_malformed_roll(inner) {
                // R-1 (Q-5 guard extension): a roll that "fired" but whose target/result is
                // unbound ("未定"/"未知"/"目标：?"/"DV?") is NOT a real check — drop the
                // wrapper, keep inner prose as Narration, and count it. Mirrors the empty path.
                doc.blocks
                    .push(TurnBlock::new(TurnBlockKind::Narration, trimmed));
                doc.malformed_rolls_unwrapped += 1;
            } else if inner_has_real_roll(inner) {
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

/// A `[roll]` is a REAL check iff it contains a digit AND is not malformed (the malformed check
/// runs first in `push_tag_block`), so here we only confirm a digit is present.
fn inner_has_real_roll(inner: &str) -> bool {
    inner.chars().any(|c| c.is_ascii_digit())
}

/// Markers signalling a `[roll]` fired but its target/difficulty/result is unbound. Such a block is
/// MALFORMED → unwrapped to Narration (R-1), even if it carries an unrelated digit. Zero survive.
const MALFORMED_ROLL_MARKERS: &[&str] = &[
    "未定",
    "未知",
    "未绑定",
    "待定",
    "结果：未",
    "结果:未",
    "目标：?",
    "目标:?",
    "目标？",
    "目标?",
    "DV?",
    "DV：?",
    "DV:?",
    "DC?",
    "DC：?",
    "DC:?",
];

/// True iff the inner `[roll]` content signals an unbound / undetermined check (R-1).
fn inner_is_malformed_roll(inner: &str) -> bool {
    MALFORMED_ROLL_MARKERS.iter().any(|m| inner.contains(m))
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
    fn malformed_unbound_roll_unwrapped_to_narration() {
        // R-1: a roll that "fired" but whose result is 未定 (unbound) → NOT a real check.
        let doc = parse_turn_document("[roll]结果：未定[/roll]");
        assert_eq!(doc.player_text(), "结果：未定");
        assert_eq!(doc.malformed_rolls_unwrapped, 1);
        assert_eq!(doc.empty_rolls_unwrapped, 0);
        assert_eq!(doc.blocks[0].kind, TurnBlockKind::Narration);
    }

    #[test]
    fn malformed_unbound_roll_unwrapped_even_with_digit() {
        // R-1: even WITH a stray digit, an unbound-target/DV marker means malformed → unwrap.
        for s in [
            "[roll]侦查 1d100 目标：? 结果未定[/roll]",
            "[roll]入侵终端 1d10 DV? 未知[/roll]",
        ] {
            let doc = parse_turn_document(s);
            assert_eq!(doc.malformed_rolls_unwrapped, 1, "{s}");
            assert_eq!(doc.empty_rolls_unwrapped, 0, "{s}");
            assert_eq!(doc.blocks[0].kind, TurnBlockKind::Narration, "{s}");
        }
    }

    #[test]
    fn bound_roll_kept_despite_no_malformed_marker() {
        // R-1 guard must NOT over-fire: a real bound roll (digit + no 未定 marker) stays a Roll.
        let doc = parse_turn_document("[roll]入侵终端 1d10+6=14 ≥ DV13 成功[/roll]");
        assert_eq!(doc.malformed_rolls_unwrapped, 0);
        assert_eq!(doc.empty_rolls_unwrapped, 0);
        assert_eq!(doc.blocks[0].kind, TurnBlockKind::Roll);
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

    // ── EV-4R progress_claims sidecar (progress_claims_on_gm_v1) ─────────────────────────
    #[test]
    fn progress_claims_tag_parsed_and_never_player_visible() {
        // A main-GM `[progress_claims]` tag whose inner is a JSON array of closed-schema
        // EvidenceClaim → fills doc.progress_claims; its content NEVER reaches player_text.
        let doc = parse_turn_document(concat!(
            "[narration]你撬开终端，数据流泻出。[/narration]",
            "[progress_claims][{\"cap_id\":\"cap_2a0d946812d6\",\"basis\":[\"commit:0\"]}][/progress_claims]",
        ));
        assert_eq!(doc.progress_claims.len(), 1, "one claim parsed");
        assert_eq!(doc.progress_claims[0].cap_id.as_str(), "cap_2a0d946812d6");
        assert_eq!(doc.progress_claims[0].basis.len(), 1);
        // Player text is the narration only — the cap handle / JSON never leaks.
        let pt = doc.player_text();
        assert_eq!(pt, "你撬开终端，数据流泻出。");
        assert!(!pt.contains("cap_2a0d946812d6"), "cap handle leaked: {pt}");
        assert!(!pt.contains("progress_claims"), "tag leaked: {pt}");
    }

    #[test]
    fn progress_claims_forged_conclusion_fields_dropped_fail_closed() {
        // The closed EvidenceClaim schema (deny_unknown_fields) means a forged entry carrying
        // a progress CONCLUSION (completed / atom_id / objective_id / reward) fails to parse and
        // is dropped per-entry — never guessed (LLM output ∩ ProgressSignal = ∅).
        let doc = parse_turn_document(concat!(
            "[progress_claims][",
            "{\"cap_id\":\"cap_good00000001\",\"basis\":[\"commit:0\"]},",
            "{\"cap_id\":\"cap_forged0002\",\"basis\":[\"commit:1\"],\"completed\":true},",
            "{\"cap_id\":\"cap_forged0003\",\"basis\":[\"commit:2\"],\"atom_id\":\"atom:xyz\"},",
            "{\"cap_id\":\"cap_forged0004\",\"basis\":[]}",
            "][/progress_claims]",
        ));
        // Only the well-formed entry survives; the three forged ones are dropped.
        assert_eq!(
            doc.progress_claims.len(),
            1,
            "forged entries must be dropped"
        );
        assert_eq!(doc.progress_claims[0].cap_id.as_str(), "cap_good00000001");
    }

    #[test]
    fn progress_claims_malformed_json_inner_is_empty_no_panic() {
        // Non-array / malformed inner → fail-closed empty, no panic, tag still consumed
        // (not leaked into narration).
        for inner in ["not json at all", "{\"progress_claims\":1}", ""] {
            let doc = parse_turn_document(&format!("[progress_claims]{inner}[/progress_claims]"));
            assert!(
                doc.progress_claims.is_empty(),
                "inner {inner:?} must yield no claims"
            );
            // The tag is consumed (no player block) ⇒ neither the tag nor a non-empty inner leaks.
            let pt = doc.player_text();
            assert!(
                !pt.contains("progress_claims"),
                "tag leaked for inner {inner:?}: {pt}"
            );
            if !inner.is_empty() {
                assert!(
                    !pt.contains(inner),
                    "inner {inner:?} leaked to player: {pt}"
                );
            }
        }
    }

    #[test]
    fn no_progress_claims_tag_keeps_field_empty_baseline() {
        // The default GM output (no tag, flag OFF / not instructed) leaves progress_claims empty.
        let doc = parse_turn_document("[narration]平平无奇的一回合。[/narration]");
        assert!(doc.progress_claims.is_empty());
    }

    // ── EV-P1 evidence_audit sidecar (progress_evidence_audit_required_v1) ────────────────
    #[test]
    fn evidence_audit_tag_parsed_and_never_player_visible() {
        // A main-GM `[evidence_audit]` tag (single closed-schema JSON object) → fills
        // doc.evidence_audit; its content NEVER reaches player_text.
        let doc = parse_turn_document(concat!(
            "[narration]终端亮起，储运图谱铺开。[/narration]",
            "[evidence_audit]{\"offer_set_id\":\"osid_abc123def456\",\"decisions\":",
            "{\"cap_2a0d946812d6\":{\"status\":\"observed\",\"basis\":[\"commit:0\"]},",
            "\"cap_bb11cc22dd33\":{\"status\":\"not_observed\",\"reason\":\"merely_implied\"}}}",
            "[/evidence_audit]",
        ));
        let audit = doc.evidence_audit.as_ref().expect("audit parsed");
        assert_eq!(audit.offer_set_id, "osid_abc123def456");
        assert_eq!(audit.decisions.len(), 2);
        let pt = doc.player_text();
        assert_eq!(pt, "终端亮起，储运图谱铺开。");
        assert!(!pt.contains("cap_2a0d946812d6"), "cap handle leaked: {pt}");
        assert!(!pt.contains("evidence_audit"), "tag leaked: {pt}");
    }

    #[test]
    fn evidence_audit_forged_or_malformed_inner_is_none_fail_closed() {
        // A forged conclusion field (closed schema), an Observed without basis, and
        // non-JSON inner all fail to parse ⇒ None (never guessed), tag still consumed.
        for inner in [
            // forged top-level field (deny_unknown_fields)
            "{\"offer_set_id\":\"osid_x\",\"decisions\":{},\"objective_completed\":\"obj.win\"}",
            // observed without a basis
            "{\"offer_set_id\":\"osid_x\",\"decisions\":{\"cap_a\":{\"status\":\"observed\"}}}",
            // not JSON at all
            "garbage not json",
            "",
        ] {
            let doc = parse_turn_document(&format!("[evidence_audit]{inner}[/evidence_audit]"));
            assert!(
                doc.evidence_audit.is_none(),
                "inner {inner:?} must yield no audit"
            );
            let pt = doc.player_text();
            assert!(
                !pt.contains("evidence_audit"),
                "tag leaked for inner {inner:?}: {pt}"
            );
        }
    }

    #[test]
    fn no_evidence_audit_tag_keeps_field_none_baseline() {
        // Default GM output (flag OFF / not instructed) leaves evidence_audit None.
        let doc = parse_turn_document("[narration]平平无奇的一回合。[/narration]");
        assert!(doc.evidence_audit.is_none());
    }

    // ── EV-P4 evidence_attempts sidecar (progress_capability_binding_v1) ──────────────────
    #[test]
    fn evidence_attempts_tag_parsed_fail_closed_and_never_player_visible() {
        // A main-GM `[evidence_attempts]` array (closed `{cap_id}`) fills
        // doc.evidence_attempts; a forged extra field is dropped; never player-visible.
        let doc = parse_turn_document(concat!(
            "[narration]特工撬开异常体的取样面板。[/narration]",
            "[evidence_attempts][{\"cap_id\":\"cap_2a0d946812d6\"},",
            "{\"cap_id\":\"cap_forge\",\"completed\":true}][/evidence_attempts]",
        ));
        assert_eq!(
            doc.evidence_attempts.len(),
            1,
            "only the closed-schema attempt parses"
        );
        assert_eq!(doc.evidence_attempts[0].as_str(), "cap_2a0d946812d6");
        let pt = doc.player_text();
        assert_eq!(pt, "特工撬开异常体的取样面板。");
        assert!(!pt.contains("cap_2a0d946812d6"), "cap handle leaked: {pt}");
        assert!(!pt.contains("evidence_attempts"), "tag leaked: {pt}");
    }

    #[test]
    fn no_evidence_attempts_tag_keeps_field_empty_baseline() {
        // Default GM output (flag OFF / not instructed) leaves evidence_attempts empty,
        // and the `evidence_attempts` tag does not disturb the sibling `evidence_audit` tag.
        let doc = parse_turn_document("[narration]平平无奇的一回合。[/narration]");
        assert!(doc.evidence_attempts.is_empty());
    }

    // ── EV-P2 materialized_content sidecar (progress_exact_projectors_v1) ─────────────────
    #[test]
    fn materialized_content_tag_parsed_and_never_player_visible() {
        // A main-GM `[materialized_content]` tag (JSON array of closed-schema ContentDelivery)
        // → fills doc.materialized_content; its content NEVER reaches player_text.
        let doc = parse_turn_document(concat!(
            "[narration]你把广告分镜递到她面前，逐格讲解。[/narration]",
            "[materialized_content][{\"delivery_cap\":\"cap_2a0d946812d6\",",
            "\"recipient\":\"player\",\"basis\":[\"commit:0\"]}][/materialized_content]",
        ));
        assert_eq!(doc.materialized_content.len(), 1, "one delivery parsed");
        assert_eq!(
            doc.materialized_content[0].delivery_cap.as_str(),
            "cap_2a0d946812d6"
        );
        assert_eq!(doc.materialized_content[0].basis.len(), 1);
        let pt = doc.player_text();
        assert_eq!(pt, "你把广告分镜递到她面前，逐格讲解。");
        assert!(!pt.contains("cap_2a0d946812d6"), "cap handle leaked: {pt}");
        assert!(!pt.contains("materialized_content"), "tag leaked: {pt}");
    }

    #[test]
    fn materialized_content_forged_entries_dropped_fail_closed() {
        // The closed ContentDelivery schema (deny_unknown_fields + closed recipient + non-empty
        // basis) drops a forged fact_id/objective entry, a bad recipient, and an empty basis;
        // only the well-formed entry survives.
        let doc = parse_turn_document(concat!(
            "[materialized_content][",
            "{\"delivery_cap\":\"cap_good00000001\",\"recipient\":\"player\",\"basis\":[\"commit:0\"]},",
            "{\"delivery_cap\":\"cap_forged0002\",\"recipient\":\"player\",\"basis\":[\"commit:1\"],\"fact_id\":\"clue_x\"},",
            "{\"delivery_cap\":\"cap_forged0003\",\"recipient\":\"npc\",\"basis\":[\"commit:2\"]},",
            "{\"delivery_cap\":\"cap_forged0004\",\"recipient\":\"player\",\"basis\":[]}",
            "][/materialized_content]",
        ));
        assert_eq!(
            doc.materialized_content.len(),
            1,
            "forged/bad entries must be dropped"
        );
        assert_eq!(
            doc.materialized_content[0].delivery_cap.as_str(),
            "cap_good00000001"
        );
    }

    #[test]
    fn materialized_content_malformed_inner_is_empty_no_panic() {
        for inner in ["not json at all", "{\"materialized_content\":1}", ""] {
            let doc = parse_turn_document(&format!(
                "[materialized_content]{inner}[/materialized_content]"
            ));
            assert!(
                doc.materialized_content.is_empty(),
                "inner {inner:?} must yield no deliveries"
            );
            let pt = doc.player_text();
            assert!(
                !pt.contains("materialized_content"),
                "tag leaked for inner {inner:?}: {pt}"
            );
        }
    }

    #[test]
    fn no_materialized_content_tag_keeps_field_empty_baseline() {
        let doc = parse_turn_document("[narration]平平无奇的一回合。[/narration]");
        assert!(doc.materialized_content.is_empty());
    }
}
