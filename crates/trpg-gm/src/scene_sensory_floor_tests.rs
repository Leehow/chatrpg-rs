//! G1（§6 大考）：env-gated「场景感兜底」narrator starvation relief 的确定性回归测试。
//!
//! 全部零 DB / 零 LLM——只断言 build_narrator_messages(packet, sensory_floor) 的纯字符串装配：
//! OFF 必须与基线**字节等价**（无兜底子句）；ON 仅在「真饿」时（facts 与
//! player_perceivable_facts 皆空且 scene 非空）追加兜底子句，且只用 user 消息追加、
//! system prompt 在所有分支恒定不变。env 绝不进测试（避免并行污染），全部直接传 bool。

use super::*;
use crate::packet::NarrationPacket;

/// 兜底子句锚点（与 turn_loop.rs 中 verbatim 一致的可识别片段）。
const FLOOR_HEADER: &str = "场景感兜底（仅在本回合没有新增可公开结果时适用）：";
const FLOOR_HARD_LIMIT: &str = "硬限制：只可使用\"当前场景\"文本中明示出现的要素";

/// 一个「真饿」packet：facts 与 player_perceivable_facts 皆空，但 scene 非空。
fn starved_packet_with_scene() -> NarrationPacket {
    NarrationPacket {
        player_input: "我环顾四周".to_string(),
        what_happened: vec![],
        what_changed: vec![],
        player_perceivable_facts: vec![],
        scene_context: vec!["你站在潮湿的石廊里，火把在墙上投下摇曳的影子。".to_string()],
        ..Default::default()
    }
}

fn user_content(messages: &[serde_json::Value]) -> String {
    messages[1]["content"].as_str().unwrap().to_string()
}

fn system_content(messages: &[serde_json::Value]) -> String {
    messages[0]["content"].as_str().unwrap().to_string()
}

/// G1-1：OFF 字节等价——同一 packet，sensory_floor=false 时 user 内容**绝不**含兜底子句，
/// 且与不传 floor 的基线装配逐字节一致（无任何兜底痕迹）。
#[test]
fn g1_off_is_byte_equal_no_floor_clause() {
    let packet = starved_packet_with_scene();
    let off = build_narrator_messages(&packet, false);
    let user = user_content(&off);
    assert!(
        !user.contains(FLOOR_HEADER) && !user.contains(FLOOR_HARD_LIMIT),
        "OFF 分支不得出现任何兜底子句，实得 user：\n{user}"
    );
    // OFF user 必须仍以基线收尾行结束（确保未篡改既有格式）。
    assert!(
        user.trim_end().ends_with("请据此用第二人称写一段有场景感、逐条覆盖上述机械结果的连贯散文。"),
        "OFF user 必须保持基线收尾行字节不变，实得：\n{user}"
    );
}

/// G1-2：ON + 真饿 → user 必须**包含**「场景感兜底」标题与「硬限制」禁止行。
#[test]
fn g1_on_starved_appends_floor_clause() {
    let packet = starved_packet_with_scene();
    let on = build_narrator_messages(&packet, true);
    let user = user_content(&on);
    assert!(
        user.contains(FLOOR_HEADER),
        "ON 真饿必须追加兜底标题，实得 user：\n{user}"
    );
    assert!(
        user.contains(FLOOR_HARD_LIMIT),
        "ON 真饿必须追加硬限制禁止行，实得 user：\n{user}"
    );
}

/// G1-2b：ON 追加兜底后，基线收尾行仍在兜底子句之前（兜底是**追加**的尾段，而非替换）。
#[test]
fn g1_on_floor_is_appended_after_baseline_tail() {
    let packet = starved_packet_with_scene();
    let on = build_narrator_messages(&packet, true);
    let user = user_content(&on);
    let baseline_tail = "请据此用第二人称写一段有场景感、逐条覆盖上述机械结果的连贯散文。";
    let tail_pos = user.find(baseline_tail).expect("基线收尾行必须仍在");
    let floor_pos = user.find(FLOOR_HEADER).expect("兜底标题必须在");
    assert!(
        tail_pos < floor_pos,
        "兜底子句必须**追加在**基线收尾行之后"
    );
}

/// G1-3：ON 但有非空 facts（what_changed）→ **不**触发兜底（仅在真饿时适用）。
#[test]
fn g1_on_with_nonempty_facts_no_floor() {
    let mut packet = starved_packet_with_scene();
    packet.what_changed = vec!["你的 HP 从 10 降到 7".to_string()];
    let on = build_narrator_messages(&packet, true);
    let user = user_content(&on);
    assert!(
        !user.contains(FLOOR_HEADER) && !user.contains(FLOOR_HARD_LIMIT),
        "有机械事实时绝不触发兜底，实得 user：\n{user}"
    );
}

/// G1-3b：ON 但 facts 来自 what_happened 也算非空 → 不触发兜底。
#[test]
fn g1_on_with_nonempty_what_happened_no_floor() {
    let mut packet = starved_packet_with_scene();
    packet.what_happened = vec!["门被推开了".to_string()];
    let on = build_narrator_messages(&packet, true);
    let user = user_content(&on);
    assert!(
        !user.contains(FLOOR_HEADER),
        "what_happened 非空时不触发兜底，实得 user：\n{user}"
    );
}

/// G1-4：ON、facts 空但 player_perceivable_facts 非空 → **不**触发兜底（有可感知信息可叙）。
#[test]
fn g1_on_empty_facts_but_nonempty_perceivable_no_floor() {
    let mut packet = starved_packet_with_scene();
    packet.player_perceivable_facts = vec!["你听见远处水滴声".to_string()];
    let on = build_narrator_messages(&packet, true);
    let user = user_content(&on);
    assert!(
        !user.contains(FLOOR_HEADER) && !user.contains(FLOOR_HARD_LIMIT),
        "有玩家可感知信息时不触发兜底，实得 user：\n{user}"
    );
}

/// G1-5：ON、真饿但 scene 为空 → **不**触发兜底（无可重新落地的场景文本）。
#[test]
fn g1_on_empty_scene_no_floor() {
    let mut packet = starved_packet_with_scene();
    packet.scene_context = vec![];
    let on = build_narrator_messages(&packet, true);
    let user = user_content(&on);
    assert!(
        !user.contains(FLOOR_HEADER),
        "scene 为空时不触发兜底，实得 user：\n{user}"
    );
}

/// G1-5b：ON、scene 仅含空白字符（trim 后空）→ 不触发兜底。
#[test]
fn g1_on_whitespace_only_scene_no_floor() {
    let mut packet = starved_packet_with_scene();
    packet.scene_context = vec!["   \n  ".to_string()];
    let on = build_narrator_messages(&packet, true);
    let user = user_content(&on);
    assert!(
        !user.contains(FLOOR_HEADER),
        "scene 仅空白时不触发兜底，实得 user：\n{user}"
    );
}

/// G1-6：system prompt 在 OFF / ON 两个分支必须**逐字节相同**（兜底只动 user）。
#[test]
fn g1_system_prompt_unchanged_across_floor_flag() {
    let packet = starved_packet_with_scene();
    let off = build_narrator_messages(&packet, false);
    let on = build_narrator_messages(&packet, true);
    assert_eq!(
        system_content(&off),
        system_content(&on),
        "兜底标志不得改动 system prompt"
    );
    // 触发分支的 user 必须不同于 OFF（确认兜底确实改变了 user）。
    assert_ne!(
        user_content(&off),
        user_content(&on),
        "ON 真饿的 user 必须与 OFF 不同"
    );
}
