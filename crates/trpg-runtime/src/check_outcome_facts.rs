//! L-W — project a committed `CheckResolved(success)` event into a durable, source-backed
//! `world_fact`.
//!
//! 理念依据(设计4补充 §二):
//! - §二.1 Event Log 是唯一权威 —— kernel 判定的 `CheckResolved` 已写进事件日志;
//! - §二.3 Projection 是视图,不是第二真相 —— 这里把那条已落账事件**投射**成一条 durable
//!   world_fact,绝不发明事件本身;
//! - §二.8 叙述/持久化只表达**已决定**的东西,不发明机制 —— 只对 kernel 已判 `success`
//!   的检定派生,用检定**自身**的 `stakes.success_public`(模组/GM 撰写)作为后果文本。
//!
//! 这修复 J2 真根(smokeLV census):普通技能检定 SUCCESS 不进 `committed_patches`(只
//! effect-roll/attack/resource-track 检定才 populate),且无任何 proposer 把检定后果写
//! `world_facts` ⇒ 决定的后果蒸发进念白、不落 committed state ⇒ 潜行渗透打法 J2 结构性饿死。
//!
//! 纯派生 + 无 LLM + 确定性 + 幂等(`fact_id = wf_chk_<check_id>`)。flag
//! `TRPG_CHECK_OUTCOME_WORLD_FACT` 默认 ON;OFF ⇒ 无派生 = 基线字节等价(走原 review/commit
//! 管线,knowledge kernel 仍各自门控)。零 ruleset 分支(同 `meet_or_beat` 系门控之外的通用层)。

use trpg_model::{CheckContract, FactTruthStatus, MemoryExtractionProposal, WorldFactCandidate};

/// flag 门控。默认 ON;仅 `0`/`false`/`off`(大小写不敏感)关闭。OFF ⇒ 引擎跳过派生。
pub fn check_outcome_world_fact_enabled() -> bool {
    match std::env::var("TRPG_CHECK_OUTCOME_WORLD_FACT") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off"
        ),
        Err(_) => true,
    }
}

/// 截断到 `max` 字符(按 char 边界,UTF-8 安全),避免超长念白塞进 fact object。
fn truncate_chars(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

/// 决定性后果度数:只有 kernel 已判 success 家族的检定才**确立**世界变更。
/// failure/fumble/pending 不确立任何 committed 世界事实(理念:不发明未发生的东西)。
fn degree_is_established(degree: &str) -> bool {
    matches!(
        degree.trim().to_ascii_lowercase().as_str(),
        "success" | "critical_success" | "critical" | "strong_success"
    )
}

/// 把一条已 commit 的成功检定投射成 world_fact 提案。
///
/// - `contract`:该检定的合约(发起者/标签/动作摘要/stakes)。
/// - `degree`:resolution 的后果度数 token(`check_results.outcome.degree`)。
/// - `turn_id`:本回合 id(写入 world_facts.turn_id,供 J2 归因该回合 consequential)。
///
/// 返回 `None` 当:非成功度数 / 无发起者 actor / 无可用后果文本。否则返回一条
/// `WorldFactCandidate`(`truth_status=True` ⇒ commit 时写 world_fact 行 + gm `knows_true` 边)。
pub fn world_fact_from_check(
    contract: &CheckContract,
    degree: &str,
    turn_id: &str,
) -> Option<MemoryExtractionProposal> {
    if !degree_is_established(degree) {
        return None;
    }
    let actor = contract.initiator.actor_id.trim();
    if actor.is_empty() {
        return None;
    }
    // 后果文本优先用检定自身的 success_public(模组/GM 已撰写的"已决定"后果);
    // 退回 action_summary,再退回 check_label。三者皆空 ⇒ 不派生(无可表达的后果)。
    let success_public = contract.stakes.success_public.trim();
    let action_summary = contract.action_summary.trim();
    let label = contract.check_label.trim();
    let object_src = if !success_public.is_empty() {
        success_public
    } else if !action_summary.is_empty() {
        action_summary
    } else {
        label
    };
    if object_src.is_empty() {
        return None;
    }
    let object = truncate_chars(object_src, 240);
    Some(MemoryExtractionProposal::WorldFact(WorldFactCandidate {
        // 幂等:同一 check 重复投射稳定命中同一行(on-conflict 改写,不增行)。
        fact_id: format!("wf_chk_{}", contract.check_id.trim()),
        subject: actor.to_string(),
        predicate: "established".to_string(),
        object: object.clone(),
        summary: object,
        confidence: Some(1.0),
        // 真事件:insert_check_result 落账时 append 的 CheckResolved 幂等键 = de_check_<check_id>。
        source_event_ids: vec![format!("de_check_{}", contract.check_id.trim())],
        turn_id: Some(turn_id.to_string()),
        truth_status: Some(FactTruthStatus::True),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::{ActorKind, ActorRef, CheckStakes, CheckTargetModel, OppositionModel};

    fn dv14_target() -> CheckTargetModel {
        CheckTargetModel::StaticNumber {
            value: 14,
            label: "DV14".to_string(),
        }
    }

    fn contract(check_id: &str, actor: &str, success_public: &str, action: &str) -> CheckContract {
        CheckContract {
            check_id: check_id.to_string(),
            session_id: "s".to_string(),
            turn_id: "t".to_string(),
            ruleset_id: "cyberpunk_red".to_string(),
            module_id: None,
            initiator: ActorRef {
                actor_id: actor.to_string(),
                actor_kind: ActorKind::PlayerCharacter,
                display_name: None,
            },
            target_actor: None,
            opposition: OppositionModel::StaticDc {
                dc: 14,
                label: "DV14".to_string(),
            },
            action_summary: action.to_string(),
            intent_kind: "tech".to_string(),
            check_label: "TECH + Basic Tech".to_string(),
            dice_expression: "1d10".to_string(),
            modifiers: vec![],
            target: dv14_target(),
            tested_parameter: None,
            opponent_tested_parameter: None,
            actor_snapshot_ids: vec![],
            source_refs: vec![],
            learned_packet_ids: vec![],
            roll_visibility: Default::default(),
            roll_authority: Default::default(),
            disclosure: Default::default(),
            stakes: CheckStakes {
                success_public: success_public.to_string(),
                ..Default::default()
            },
            confidence: Default::default(),
            ruling_status: Default::default(),
            advice_refs: vec![],
            expires_at_turn: None,
        }
    }

    fn unwrap_wf(p: MemoryExtractionProposal) -> WorldFactCandidate {
        match p {
            MemoryExtractionProposal::WorldFact(c) => c,
            other => panic!("expected WorldFact, got {other:?}"),
        }
    }

    #[test]
    fn success_check_yields_world_fact_keyed_on_check_id() {
        let c = contract(
            "chk1",
            "pc.current",
            "供电已被切断,粗缆嗡鸣停止",
            "短断关键线",
        );
        let p = world_fact_from_check(&c, "success", "turn9").expect("success ⇒ Some");
        let wf = unwrap_wf(p);
        assert_eq!(wf.fact_id, "wf_chk_chk1");
        assert_eq!(wf.subject, "pc.current");
        assert_eq!(wf.predicate, "established");
        assert_eq!(wf.object, "供电已被切断,粗缆嗡鸣停止");
        assert_eq!(wf.turn_id.as_deref(), Some("turn9"));
        assert_eq!(wf.truth_status, Some(FactTruthStatus::True));
        assert_eq!(wf.source_event_ids, vec!["de_check_chk1".to_string()]);
    }

    #[test]
    fn failure_and_pending_degrees_yield_nothing() {
        let c = contract("chk2", "pc.current", "x", "y");
        assert!(world_fact_from_check(&c, "failure", "t").is_none());
        assert!(world_fact_from_check(&c, "fumble", "t").is_none());
        assert!(world_fact_from_check(&c, "pending", "t").is_none());
    }

    #[test]
    fn critical_success_family_is_established() {
        let c = contract("chk3", "pc.current", "outcome", "act");
        assert!(world_fact_from_check(&c, "critical_success", "t").is_some());
        assert!(world_fact_from_check(&c, "SUCCESS", "t").is_some()); // case-insensitive
    }

    #[test]
    fn empty_actor_yields_nothing() {
        let c = contract("chk4", "   ", "outcome", "act");
        assert!(world_fact_from_check(&c, "success", "t").is_none());
    }

    #[test]
    fn falls_back_to_action_summary_then_label_when_no_success_public() {
        let mut c = contract("chk5", "pc.current", "", "撬开缆线固定扣");
        let wf = unwrap_wf(world_fact_from_check(&c, "success", "t").unwrap());
        assert_eq!(wf.object, "撬开缆线固定扣"); // action_summary fallback
        c.action_summary = "".to_string();
        let wf2 = unwrap_wf(world_fact_from_check(&c, "success", "t").unwrap());
        assert_eq!(wf2.object, "TECH + Basic Tech"); // check_label fallback
    }

    #[test]
    fn all_text_empty_yields_nothing() {
        let mut c = contract("chk6", "pc.current", "", "");
        c.check_label = "".to_string();
        assert!(world_fact_from_check(&c, "success", "t").is_none());
    }

    #[test]
    fn flag_defaults_on() {
        // 默认(未设环境变量)应为 ON;不在测试里 set/unset env(进程级竞态),仅断言默认分支。
        // 由 from_env 的 Err(_) => true 保证;此处文档化意图。
        assert!(
            check_outcome_world_fact_enabled()
                || std::env::var("TRPG_CHECK_OUTCOME_WORLD_FACT").is_ok()
        );
    }

    #[test]
    fn long_object_truncated_to_240_chars() {
        let long = "字".repeat(500);
        let c = contract("chk7", "pc.current", &long, "act");
        let wf = unwrap_wf(world_fact_from_check(&c, "success", "t").unwrap());
        assert_eq!(wf.object.chars().count(), 240);
    }
}
