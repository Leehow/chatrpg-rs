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
    let messages = build_narrator_messages(&packet, false, false);
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
    let messages = build_narrator_messages(&packet, false, false);
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
    let messages = build_narrator_messages(&packet, false, false);
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
    let messages = build_narrator_messages(&packet, false, false);
    let system = messages[0]["content"].as_str().unwrap();
    assert!(
        system.contains("逐条") || system.contains("每一条") || system.contains("每条"),
        "system 必须命令逐条复述机械结果，实得：{system}"
    );
}

// ───────── A3-HARDEN: 机器上下文回显 / 原始 JSON dump ON-only 守卫 ─────────

/// t02 形态原始 dump：header「根据本回合已确认的结果」+ 原始 `check_<id>{...JSON...}` 记录。
/// 复刻 coc_campaign_final.md:23 的 ~2057 char dump 形状。
fn t02_machine_echo_dump() -> String {
    let mut s = String::from("根据本回合已确认的结果：· check check_7c3");
    for _ in 0..5 {
        s.push_str(
            "{\"check_id\":\"check_7c3a1f\",\"skill\":\"Spot Hidden\",\"roll\":22,\
             \"target\":60,\"outcome\":\"strong_success\",\"margin\":38}",
        );
    }
    s.push_str("· check check_f41");
    for _ in 0..5 {
        s.push_str(
            "{\"check_id\":\"check_f41b2\",\"skill\":\"Psychology\",\"roll\":46,\
             \"target\":50,\"outcome\":\"success\"}",
        );
    }
    s
}

/// A3-HARDEN (a)：机器上下文回显 / 原始 JSON dump ⇒ 守卫 **FIRES**。
#[test]
fn a3harden_predicate_fires_on_machine_context_echo_dump() {
    let dump = t02_machine_echo_dump();
    assert!(
        narration_is_machine_context_echo(&dump),
        "t02 形态原始 JSON dump 必须触发机器回显守卫，长度={}",
        dump.len()
    );
}

/// A3-HARDEN (a')：header + 原始字段键共现(支线 B)即触发，即便花括号不足 4。
#[test]
fn a3harden_predicate_fires_on_header_plus_field_key() {
    // 一段含 header 与单个原始 `"check_id"` 字段键的回显(brace 仅 2，仍触发)。
    let echo = format!(
        "根据本回合已确认的结果：{}· check check_7c3{{\"check_id\":\"check_7c3a1f\",\"outcome\":\"strong_success\"}}",
        "玩家可见的机械结果账本被原样回吐，没有任何散文叙述，"
    );
    assert!(
        narration_is_machine_context_echo(&echo),
        "header + 原始字段键共现必须触发(支线 B)，实得 len={}",
        echo.len()
    );
}

/// A3-HARDEN (b)：正常含 roll 值 + `[roll]` 标签的散文 ⇒ 守卫 **不** FIRES(零误杀)。
/// 复刻 P1/P2/P3 探针念白(coc_a3_turn2_investigation.md)。
#[test]
fn a3harden_predicate_ignores_legit_prose_with_roll_tags() {
    let prose = "你绕着加油站走了一圈，记者的直觉让你在油泵旁停下。\
        [roll]侦查成功（44/60）。[/roll]那辆灰色轿车的后视镜微微调整了角度——有人在车里盯着你。\
        你压低帽檐，心跳加快，[roll]心理学检定成功（46/50）。[/roll]对方在伪装平静，但指节发白。\
        你假装系鞋带，用余光记下车牌的前两位，然后若无其事地走回自己的车边。";
    assert!(
        !narration_is_machine_context_echo(prose),
        "含 [roll] 标签与括号 roll 值的正常散文绝不得触发(无 {{}}、无原始字段键)，len={}",
        prose.len()
    );
}

/// A3-HARDEN (c)：确定性兜底模板(收敛终点) ⇒ 守卫 **不** FIRES ⇒ 阶梯确定性收敛、不死循环。
/// 兜底也以「根据本回合已确认的结果」开头；codex ④ 审指出真实 `result.outcome` JSON **含**
/// `"check_id"` 等原始记录字段键(经 compact_value 原样进 what_happened)，故兜底必须先经
/// `strip_machine_json_objects` 抹掉 JSON 正文，否则兜底自身满足 header+字段键 ⇒ 反被守卫判机器回显。
#[test]
fn a3harden_predicate_ignores_deterministic_fallback_converges() {
    // **真实形状**：what_happened 里嵌真实 outcome JSON(含 "check_id"/"outcome" 原始字段键，
    // 复刻 trpg-contest::resolve_outcome 的 json! 输出)。验证兜底经 strip 后既无 {} 也无字段键，
    // ⇒ 守卫放过 ⇒ 收敛终点 honest(玩家最终所见绝不含原始机器 JSON)。
    let packet = NarrationPacket {
        player_input: "我开火".to_string(),
        what_happened: vec![
            "check c_fire: {\"check_id\":\"c_fire\",\"outcome\":\"success\",\"total\":44,\"target\":60}"
                .to_string(),
            "check c_spot: {\"check_id\":\"c_spot\",\"outcome\":\"strong_success\",\"total\":22}"
                .to_string(),
        ],
        what_changed: vec![
            "effect e_dmg (damage)".to_string(),
            "pc.ammo subtract 1".to_string(),
        ],
        ..Default::default()
    };
    let fallback = deterministic_committed_facts_narration(&packet);
    assert!(fallback.contains("根据本回合已确认的结果"), "兜底含 header");
    // 兜底经 strip 后绝无原始 JSON 正文/字段键(否则收敛终点自相矛盾)。
    assert!(!fallback.contains('{'), "兜底必须已抹掉 JSON 正文，实得：{fallback}");
    assert!(!fallback.contains("\"check_id\""), "兜底不得含原始字段键");
    // ledger token(check id / effect id / 资源名)仍被保留 ⇒ 仍是可对账的人话摘要。
    assert!(fallback.contains("c_fire") && fallback.contains("c_spot"), "保留 check ledger token");
    assert!(fallback.contains("e_dmg") && fallback.contains("ammo"), "保留 effect/资源 token");
    assert!(
        !narration_is_machine_context_echo(&fallback),
        "确定性兜底(经 strip)绝不得触发守卫，否则不收敛；实得：{fallback}"
    );
}

/// A3-HARDEN (c')：strip_machine_json_objects 抹掉 `{...}` 正文但保留前缀 ledger token，
/// 且抹后串绝不含 `{`/原始字段键(收敛终点 honest 的直接单测)。
#[test]
fn a3harden_strip_machine_json_keeps_token_drops_json() {
    let line = "check c_fire: {\"check_id\":\"c_fire\",\"outcome\":\"success\"}";
    let stripped = strip_machine_json_objects(line);
    assert!(stripped.starts_with("check c_fire:"), "保留 ledger 前缀，实得：{stripped}");
    assert!(!stripped.contains('{'), "抹掉花括号");
    assert!(!stripped.contains("\"check_id\""), "抹掉原始字段键");
    // 无花括号的行原样返回。
    assert_eq!(
        strip_machine_json_objects("effect e_dmg (damage)"),
        "effect e_dmg (damage)"
    );
}

/// A3-HARDEN (c''')：JSON 字符串字面量内部的 `}`(及 `\"` 转义)不得提前关深度而泄漏后续 object 正文
/// (codex ④ P1)。strip 后必无 `{`/`}`/原始字段键残留。
#[test]
fn a3harden_strip_handles_brace_inside_json_string() {
    // 字符串值里含 `}`：朴素括号计数会在此提前归零、泄漏 `,"outcome":"success"}`。
    let tricky = "check c: {\"check_id\":\"c\",\"note\":\"a}b\",\"outcome\":\"success\"}";
    let stripped = strip_machine_json_objects(tricky);
    assert!(stripped.starts_with("check c:"), "保留前缀，实得：{stripped}");
    assert!(!stripped.contains('{'), "无残留 {{，实得：{stripped}");
    assert!(!stripped.contains('}'), "无残留 }}，实得：{stripped}");
    assert!(!stripped.contains("\"outcome\""), "字符串内 }} 不得泄漏后续字段，实得：{stripped}");
    // 含转义引号 `\"` 的字符串值同样不破坏扫描。
    let esc = "check c: {\"label\":\"he said \\\"hi}\\\"\",\"check_id\":\"c\"}";
    let s2 = strip_machine_json_objects(esc);
    assert!(!s2.contains('{') && !s2.contains("\"check_id\""), "转义引号场景无泄漏，实得：{s2}");
}

/// A3-HARDEN (d)：**ON-only gating 组合**断言——`echo_repair` 的真值 = `narrator_split_enabled()
/// && narration_is_machine_context_echo(text)`(phase_verify_after_stream 的精确表达式)。验证：
/// split OFF ⇒ echo_repair 恒 false(谓词被短路、OFF 字节等价)；split ON + dump ⇒ echo_repair true
/// (ON 路径真正消费守卫)。用进程级 env，故 serial 串行 + 即时还原，避免与并发测试竞争。
#[test]
fn a3harden_on_only_gating_composition() {
    use std::sync::{Mutex, OnceLock};
    static ENV_GUARD: OnceLock<Mutex<()>> = OnceLock::new();
    let _g = ENV_GUARD.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|p| p.into_inner());

    let dump = t02_machine_echo_dump();
    let prior = std::env::var("TRPG_NARRATOR_SPLIT").ok();

    // split OFF ⇒ 短路：echo_repair 恒 false(即便文本是 dump)。
    std::env::remove_var("TRPG_NARRATOR_SPLIT");
    let echo_repair_off = narrator_split_enabled() && narration_is_machine_context_echo(&dump);
    assert!(!echo_repair_off, "split OFF ⇒ echo_repair 必 false(OFF 字节等价、谓词不消费)");

    // split ON + dump ⇒ echo_repair true(ON 路径真正吃守卫)。
    std::env::set_var("TRPG_NARRATOR_SPLIT", "1");
    let echo_repair_on = narrator_split_enabled() && narration_is_machine_context_echo(&dump);
    // split ON + 正常散文 ⇒ 仍 false(不误杀)。
    let prose = "你环顾四周，记者的直觉让你警觉起来，但什么也没发现异常。".repeat(3);
    let echo_repair_on_prose = narrator_split_enabled() && narration_is_machine_context_echo(&prose);

    // 还原 env。
    match prior {
        Some(v) => std::env::set_var("TRPG_NARRATOR_SPLIT", v),
        None => std::env::remove_var("TRPG_NARRATOR_SPLIT"),
    }

    assert!(echo_repair_on, "split ON + dump ⇒ echo_repair 必 true(ON 路径触发守卫)");
    assert!(!echo_repair_on_prose, "split ON + 正常散文 ⇒ echo_repair 必 false(不误杀)");
}

/// A3-HARDEN (b')：短文本 / 普通念白绝不触发(保守：原始 dump ~2057 chars，<64 直接放过)。
#[test]
fn a3harden_predicate_ignores_short_and_plain_text() {
    assert!(!narration_is_machine_context_echo(""), "空文本不触发");
    assert!(
        !narration_is_machine_context_echo("你成功了。"),
        "短念白不触发"
    );
    // 长但纯散文、无字段键：不触发。
    let long_prose = "你".repeat(200);
    assert!(
        !narration_is_machine_context_echo(&long_prose),
        "长但无原始字段键的散文不触发"
    );
}
