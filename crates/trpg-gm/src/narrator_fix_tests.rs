//! §6 大考缺陷闭环 A1/A2/A3 的确定性回归测试（纯函数 / 投影 / prompt 装配 / 修复谓词）。
//!
//! 全部零 DB / 零 LLM——只断言：A1 第二人称 persona、A2 player-safe scene_context 投影
//! 与 build_narrator_messages 感官化 + 禁复述、A3 OmittedVisibleResult 触发 ON-only 修复
//! 谓词且确定性兜底收敛。OFF 字节等价由 packet.rs 既有测试 + 本文件谓词 OFF 分支覆盖。

use super::*;
use crate::packet::{AdjudicationPacket, NarrationPacket};
use trpg_agent::{
    NarrationVerifierResult, TurnLedgerSnapshot, VerifierFinding, VerifierFindingKind,
    VerifierSeverity,
};

// ───────────────────────── A1: 第二人称 persona ─────────────────────────

/// A1：空 style 的默认 persona 串必须是**第二人称「你」**指令，绝不再写「第三人称」。
#[test]
fn a1_empty_style_default_is_second_person_not_third() {
    let adj = AdjudicationPacket::project("看四周", &TurnLedgerSnapshot::default(), &[], "", None);
    let narration = NarrationPacket::project(&adj, "", &[]);
    assert!(
        !narration.style_profile.contains("第三人称"),
        "默认 persona 不得含「第三人称」字面，实得：{}",
        narration.style_profile
    );
    assert!(
        narration.style_profile.contains("第二人称") || narration.style_profile.contains("你"),
        "默认 persona 必须是第二人称「你」指令，实得：{}",
        narration.style_profile
    );
}

/// A1：build_narrator_messages 的 system prompt 必须含第二人称规则行。
#[test]
fn a1_narrator_system_prompt_enforces_second_person() {
    let packet = NarrationPacket {
        player_input: "我环顾房间".to_string(),
        what_happened: vec![],
        what_changed: vec![],
        player_perceivable_facts: vec![],
        ..Default::default()
    };
    let messages = build_narrator_messages(&packet);
    let system = messages[0]["content"].as_str().unwrap();
    assert!(
        system.contains("第二人称") && system.contains("你"),
        "Narrator system prompt 必须强制第二人称，实得：{system}"
    );
}

// ───────────────────── A2: player-safe scene_context ─────────────────────

/// A2：scene_context 字段经 player-safe 投影注入，且**绝不**携带 GM-only / secret。
/// 这里用 with_scene_context（player-safe 来源 = 上一回合已对玩家可见的散文）。
#[test]
fn a2_scene_context_is_player_safe_and_wired() {
    let adj = AdjudicationPacket::project(
        "我推开门",
        &TurnLedgerSnapshot::default(),
        &[],
        "GM 内部：门后藏着伏兵，SECRET 暗格密码 1234",
        None,
    );
    // player-safe 来源：上一回合玩家已看见的散文（按定义已脱敏）。
    let prior_player_visible = "走廊尽头一扇橡木门，门缝里渗出冷气。";
    let narration = NarrationPacket::project(&adj, "", &[])
        .with_scene_context(&[prior_player_visible.to_string()]);
    // scene_context 落位
    assert_eq!(narration.scene_context, vec![prior_player_visible.to_string()]);
    // 绝不泄漏 adjudicator_prose / secret（投影从不读 prose；调用方只喂 player-safe 源）
    let serialized = serde_json::to_string(&narration).unwrap();
    assert!(!serialized.contains("SECRET"), "scene_context 泄漏了 secret");
    assert!(!serialized.contains("伏兵"), "scene_context 泄漏了 GM-only");
    assert!(!serialized.contains("1234"), "scene_context 泄漏了密码");
}

/// A2：build_narrator_messages 必须把 scene_context 注入、要求感官场景、且禁止逐字复述玩家输入。
#[test]
fn a2_narrator_messages_inject_scene_and_forbid_parroting() {
    let packet = NarrationPacket {
        player_input: "我走进酒馆".to_string(),
        what_happened: vec![],
        what_changed: vec![],
        player_perceivable_facts: vec![],
        ..Default::default()
    }
    .with_scene_context(&["昏黄油灯下，木桌油腻，角落坐着独眼客。".to_string()]);
    let messages = build_narrator_messages(&packet);
    let system = messages[0]["content"].as_str().unwrap();
    let user = messages[1]["content"].as_str().unwrap();
    // 感官 / 禁复述 指令在 system
    assert!(system.contains("感官"), "system 必须要求感官场景，实得：{system}");
    assert!(
        system.contains("复述") || system.contains("逐字") || system.contains("照搬"),
        "system 必须禁止复述玩家输入，实得：{system}"
    );
    // scene_context 文本进入 user 消息
    assert!(
        user.contains("独眼客"),
        "scene_context 必须注入 narrator user 消息，实得：{user}"
    );
}

/// A2：scene_context 为空时（无前序场景）prompt 装配不崩、不引入空占位泄漏。
#[test]
fn a2_empty_scene_context_is_graceful() {
    let packet = NarrationPacket {
        player_input: "我四处张望".to_string(),
        ..Default::default()
    };
    let messages = build_narrator_messages(&packet);
    assert_eq!(messages.len(), 2);
    // 不 panic、user 仍含玩家输入
    let user = messages[1]["content"].as_str().unwrap();
    assert!(user.contains("我四处张望"));
}

// ───────────────── A3: OmittedVisibleResult ON-only 修复谓词 ─────────────────

fn result_with_kinds(kinds: &[VerifierFindingKind]) -> NarrationVerifierResult {
    let findings: Vec<VerifierFinding> = kinds
        .iter()
        .map(|k| VerifierFinding {
            kind: *k,
            severity: VerifierSeverity::Blocker,
            detail: format!("{k:?} detail"),
        })
        .collect();
    NarrationVerifierResult {
        accepted: findings.is_empty(),
        findings,
        next_required_action: None,
    }
}

/// A3：OmittedVisibleResult Blocker 必须被 ON-only 修复谓词识别为「须修复」。
#[test]
fn a3_predicate_flags_omitted_visible_result() {
    let r = result_with_kinds(&[VerifierFindingKind::OmittedVisibleResult]);
    assert!(
        omitted_visible_result_needs_repair(&r),
        "OmittedVisibleResult Blocker 必须触发修复谓词"
    );
}

/// A3：谓词只认 Blocker 级 OmittedVisibleResult；Warning 级或其他 kind 不触发。
#[test]
fn a3_predicate_ignores_non_blocker_and_other_kinds() {
    // 其他 kind 不触发
    let r1 = result_with_kinds(&[VerifierFindingKind::MissingCheck]);
    assert!(!omitted_visible_result_needs_repair(&r1));
    // Warning 级 OmittedVisibleResult 不触发（只 Blocker 才空心上线）
    let r2 = NarrationVerifierResult {
        accepted: true,
        findings: vec![VerifierFinding {
            kind: VerifierFindingKind::OmittedVisibleResult,
            severity: VerifierSeverity::Warning,
            detail: "warn".into(),
        }],
        next_required_action: None,
    };
    assert!(!omitted_visible_result_needs_repair(&r2));
    // 空 findings 不触发
    let r3 = result_with_kinds(&[]);
    assert!(!omitted_visible_result_needs_repair(&r3));
}

/// A3：locked 白名单**没有**被 OmittedVisibleResult 触动——gate 对它仍判 Allow（charter 锁死）。
/// 修复路径是**谓词旁路**，绝不经白名单。
#[test]
fn a3_locked_whitelist_still_allows_omitted_visible_result() {
    let r = result_with_kinds(&[VerifierFindingKind::OmittedVisibleResult]);
    assert_eq!(
        crate::presentation_gate::presentation_gate_decision(&r),
        crate::presentation_gate::PresentationGate::Allow,
        "白名单不得因本修复而 widen——OmittedVisibleResult 仍 gate=Allow"
    );
}

/// A3：确定性兜底叙事必须复述每条 what_changed 的可见证据 token（保证收敛、不再空心）。
#[test]
fn a3_deterministic_narration_restates_visible_evidence_converges() {
    let packet = NarrationPacket {
        player_input: "我开火".to_string(),
        what_happened: vec!["check c_fire: 命中".to_string()],
        what_changed: vec![
            "effect e_dmg (damage)".to_string(),
            "pc.ammo subtract 1".to_string(),
        ],
        ..Default::default()
    };
    let narrated = deterministic_committed_facts_narration(&packet);
    // 每条 what_happened/what_changed 都被复述（含 ledger token c_fire / e_dmg / ammo）
    assert!(narrated.contains("c_fire"), "兜底须复述 check 结果");
    assert!(narrated.contains("e_dmg"), "兜底须复述 effect");
    assert!(narrated.contains("ammo"), "兜底须复述资源变化");
    assert!(!narrated.trim().is_empty(), "兜底绝不空白");
}

/// A3：build_narrator_messages 必须指令 Narrator 逐条复述 what_changed（预防空心念白）。
#[test]
fn a3_narrator_messages_force_restate_what_changed() {
    let packet = NarrationPacket {
        player_input: "我开火".to_string(),
        what_happened: vec!["check c_fire: 命中".to_string()],
        what_changed: vec!["effect e_dmg (damage)".to_string()],
        ..Default::default()
    };
    let messages = build_narrator_messages(&packet);
    let system = messages[0]["content"].as_str().unwrap();
    assert!(
        system.contains("逐条") || system.contains("每一条") || system.contains("每条"),
        "system 必须命令逐条复述机械结果，实得：{system}"
    );
}
