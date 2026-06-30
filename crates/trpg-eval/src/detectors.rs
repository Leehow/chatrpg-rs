use std::collections::HashMap;

use crate::model::{
    EvalFinding, EvalFixture, EvalReport, EvalTurn, FindingCategory, FindingSeverity, RollTrace,
    RubricDimension, RubricScore, Verdict,
};

pub fn evaluate_fixture(fixture: &EvalFixture) -> EvalReport {
    let findings = run_detectors(fixture);
    let rubric_scores = score_breakdown(&findings);
    let score = rubric_scores.iter().map(|s| s.score).sum();
    let verdict = if findings.iter().any(|f| f.severity == FindingSeverity::S1) {
        Verdict::Fail
    } else if findings.is_empty() {
        Verdict::Pass
    } else {
        Verdict::Warn
    };
    EvalReport {
        fixture_id: fixture.fixture_id.clone(),
        title: fixture.title.clone(),
        verdict,
        score,
        rubric_scores,
        findings,
    }
}

pub fn run_detectors(fixture: &EvalFixture) -> Vec<EvalFinding> {
    let mut findings = Vec::new();
    for turn in &fixture.turns {
        detect_player_decision_protocol(turn, &mut findings);
        detect_response_intent(turn, &mut findings);
        detect_player_agency_violation(turn, &mut findings);
        detect_success_without_information(turn, &mut findings);
        detect_mechanical_debt(turn, &mut findings);
        detect_semantic_noop(turn, &mut findings);
        detect_continuity(turn, &mut findings);
        detect_uncommitted_narrated_state(turn, &mut findings);
        detect_rules_trace(turn, &mut findings);
    }
    detect_player_script_loop(fixture, &mut findings);
    for (idx, finding) in findings.iter_mut().enumerate() {
        finding.finding_id = format!("F-{:05}", idx + 1);
    }
    findings
}

fn score_breakdown(findings: &[EvalFinding]) -> Vec<RubricScore> {
    RubricDimension::ALL
        .iter()
        .map(|dimension| {
            let mut penalty = 0;
            let mut evidence = Vec::new();
            for finding in findings
                .iter()
                .filter(|finding| finding_dimension(finding.category) == *dimension)
            {
                penalty += rubric_penalty(*dimension, finding.severity);
                evidence.push(format!(
                    "{} {}",
                    finding.severity.as_str(),
                    finding.category.as_str()
                ));
            }
            let weight = dimension.weight();
            RubricScore {
                dimension: *dimension,
                label: dimension.label().to_string(),
                weight,
                score: (weight - penalty.min(weight)).max(0),
                evidence,
            }
        })
        .collect()
}

fn finding_dimension(category: FindingCategory) -> RubricDimension {
    match category {
        FindingCategory::SuccessWithoutInformation
        | FindingCategory::PendingMechanicalDebt
        | FindingCategory::RulesTraceIncomplete => RubricDimension::RulesResolution,
        FindingCategory::StateContinuityFail => RubricDimension::Continuity,
        FindingCategory::ResponseIntentMismatch | FindingCategory::PlayerAgencyViolation => {
            RubricDimension::Responsiveness
        }
        FindingCategory::SemanticNoop => RubricDimension::NarrativeAgency,
        FindingCategory::PlayerScriptLoop | FindingCategory::PlayerPolicyViolation => {
            RubricDimension::PlayerSimulation
        }
    }
}

fn rubric_penalty(dimension: RubricDimension, severity: FindingSeverity) -> i32 {
    match severity {
        FindingSeverity::S1 => dimension.weight(),
        FindingSeverity::S2 => (dimension.weight() + 1) / 2,
        FindingSeverity::S3 => 3.min(dimension.weight()),
    }
}

fn detect_player_decision_protocol(turn: &EvalTurn, findings: &mut Vec<EvalFinding>) {
    let decision = &turn.player_decision;
    let mut missing = Vec::new();

    if decision.gm_visible_reply.trim().is_empty() {
        missing.push("gm_visible_reply");
    }
    if decision.perceived_facts.is_empty() {
        missing.push("perceived_facts");
    }
    if decision.active_goal.trim().is_empty() {
        missing.push("active_goal");
    }
    if decision.hypotheses.is_empty() {
        missing.push("hypotheses");
    }
    if decision.last_action_result.trim().is_empty() {
        missing.push("last_action_result");
    }
    if decision.risk_assessment.is_empty() {
        missing.push("risk_assessment");
    }
    if decision.resource_assessment.is_empty() {
        missing.push("resource_assessment");
    }
    if decision.open_questions.is_empty() {
        missing.push("open_questions");
    }
    if decision.candidate_actions.len() < 2 {
        missing.push("candidate_actions>=2");
    }
    if decision.selection_rationale.trim().is_empty() {
        missing.push("selection_rationale");
    }
    if decision.declared_action.trim().is_empty() {
        missing.push("declared_action");
    }
    if decision.sent_to_gm.trim().is_empty() {
        missing.push("sent_to_gm");
    }
    if decision.response_contract.intent.trim().is_empty() {
        missing.push("response_contract.intent");
    }
    if decision.response_contract.acceptable_resolutions.is_empty() {
        missing.push("response_contract.acceptable_resolutions");
    }

    let mut violations = Vec::new();
    if !decision.sent_to_gm.trim().is_empty()
        && normalize_for_compare(&decision.sent_to_gm)
            != normalize_for_compare(&decision.declared_action)
    {
        violations.push("sent_to_gm_must_equal_declared_action");
    }

    if missing.is_empty() && violations.is_empty() {
        return;
    }

    findings.push(finding(
        FindingCategory::PlayerPolicyViolation,
        FindingSeverity::S1,
        vec![turn.turn],
        "PlayerSimulator / Runner",
        "The player simulator must read the player-visible GM reply, extract perceived facts, update goal/hypotheses, judge the previous result, assess risks/resources/open questions, generate multiple candidate actions, choose by persona, record a response contract, and send only the final declared action to the GM.",
        "The recorded PlayerDecision does not prove the required player-simulator loop.",
        vec![
            format!("missing_fields: {:?}", missing),
            format!("violations: {:?}", violations),
            format!("declared_action: {}", decision.declared_action),
            format!("sent_to_gm: {}", decision.sent_to_gm),
        ],
        0.9,
    ));
}

fn normalize_for_compare(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn detect_response_intent(turn: &EvalTurn, findings: &mut Vec<EvalFinding>) {
    let contract = &turn.player_decision.response_contract;
    if contract.requested_information.is_empty() {
        return;
    }
    if response_satisfies_contract(turn) {
        return;
    }
    findings.push(finding(
        FindingCategory::ResponseIntentMismatch,
        FindingSeverity::S1,
        vec![turn.turn],
        "Narrator / Director",
        "GM should answer the player's declared intent with concrete information, a refusal with reason/cost, a check, a consequence, or a tracked pending item.",
        "GM output did not satisfy the response contract.",
        vec![
            format!("player_action: {}", turn.player_decision.declared_action),
            format!("intent: {}", contract.intent),
            format!("requested_information: {:?}", contract.requested_information),
            format!("gm_response: {}", turn.gm_response.text),
        ],
        0.91,
    ));
}

fn response_satisfies_contract(turn: &EvalTurn) -> bool {
    let gm = &turn.gm_response;
    let contract = &turn.player_decision.response_contract;
    if !gm.pending.is_empty() || turn.trace.pending_id.is_some() || !gm.choices_opened.is_empty() {
        return true;
    }
    if contract
        .acceptable_resolutions
        .iter()
        .any(|r| text_contains_semantic_hint(&gm.text, r))
    {
        return true;
    }
    if !gm.facts_exposed_to_player.is_empty() && !generic_confirmation_only(turn) {
        return true;
    }
    false
}

fn generic_confirmation_only(turn: &EvalTurn) -> bool {
    let intent = turn
        .player_decision
        .response_contract
        .intent
        .to_ascii_lowercase();
    let text = turn.gm_response.text.to_ascii_lowercase();
    let facts = &turn.gm_response.facts_exposed_to_player;
    (intent.contains("hidden") || intent.contains("interrogate"))
        && (text.contains("隐瞒什么")
            || facts
                .iter()
                .all(|f| f == "npc_is_hiding_something" || f.contains("hiding")))
}

fn text_contains_semantic_hint(text: &str, hint: &str) -> bool {
    let text = text.to_ascii_lowercase();
    let hint = hint.to_ascii_lowercase();
    let mut hits = 0;
    for token in hint.split(|c: char| !c.is_alphanumeric()) {
        if token.len() >= 4 && text.contains(token) {
            hits += 1;
        }
    }
    hits >= 2
}

fn detect_player_agency_violation(turn: &EvalTurn, findings: &mut Vec<EvalFinding>) {
    let text = turn.gm_response.text.trim();
    if text.is_empty() || !text_has_explicit_option_menu(text) {
        return;
    }

    findings.push(finding(
        FindingCategory::PlayerAgencyViolation,
        FindingSeverity::S1,
        vec![turn.turn],
        "Presentation / PlayerAgency",
        "GM should describe the situation, constraints, and visible affordances without turning the player's action space into an explicit menu.",
        "GM presented an explicit option menu for the player to choose from.",
        vec![
            format!("player_action: {}", turn.player_decision.declared_action),
            format!("gm_response: {}", turn.gm_response.text),
        ],
        0.95,
    ));
}

fn text_has_explicit_option_menu(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    if lowered.contains("your next move could be") {
        return true;
    }
    let menu_cues = [
        "选择其一",
        "你现在可以",
        "下一步可以",
        "你接下来可以",
        "可以立刻",
        "可以选择",
        "选一个",
    ];
    let has_menu_cue = menu_cues.iter().any(|cue| lowered.contains(cue));
    if text_has_inline_followup_action_menu(&lowered) {
        return true;
    }
    let option_lines = text
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("- ")
                || trimmed.starts_with("* ")
                || trimmed.starts_with("• ")
                || trimmed.chars().next().is_some_and(|ch| ch.is_ascii_digit())
                    && (trimmed.contains(". ") || trimmed.contains("、") || trimmed.contains(") "))
        })
        .count();

    has_menu_cue && (option_lines >= 2 || lowered.contains("选择其一"))
}

fn text_has_inline_followup_action_menu(text: &str) -> bool {
    if text_has_prepared_action_menu(text) {
        return true;
    }
    if text_has_prose_branch_action_menu(text) {
        return true;
    }
    if text_has_either_or_action_menu(text) {
        return true;
    }
    let menu_frame = [
        "后续机会",
        "实际的后续机会",
        "实际机会",
        "你现在有几个",
        "你眼前有几个",
        "你面前有几个",
        "几个很近",
        "你把这些观察串在一起",
        "观察串在一起",
    ];
    if !menu_frame.iter().any(|cue| text.contains(cue))
        || !text.contains('：') && !text.contains(':')
    {
        return false;
    }
    let branch_count =
        text.matches('；').count() + text.matches("或者").count() + text.matches("或是").count();
    if branch_count < 2 {
        return false;
    }
    inline_player_action_branch_count(text) >= 2
}

fn text_has_prepared_action_menu(text: &str) -> bool {
    let tail = ["接下来你是准备", "接下来你准备"]
        .iter()
        .find_map(|cue| text.split_once(cue).map(|(_, rest)| rest));
    let Some(tail) = tail else {
        return false;
    };
    if !["还是", "或者", "或是"]
        .iter()
        .any(|connector| tail.contains(connector))
    {
        return false;
    }
    let branch_count = tail.matches("还是").count()
        + tail.matches("或者").count()
        + tail.matches("或是").count()
        + tail.matches('；').count()
        + tail.matches(';').count();
    if branch_count < 2 {
        return false;
    }
    tail.split('；')
        .flat_map(|part| part.split(';'))
        .flat_map(|part| part.split("还是"))
        .flat_map(|part| part.split("或者"))
        .flat_map(|part| part.split("或是"))
        .filter(|part| branch_looks_like_player_action(part))
        .count()
        >= 2
}

fn text_has_either_or_action_menu(text: &str) -> bool {
    let has_frame = text.matches("要么").count() >= 2
        || text.contains("要么") && (text.contains("或者") || text.contains("或是"));
    if !has_frame {
        return false;
    }
    text.split("要么")
        .flat_map(|part| part.split("或者"))
        .flat_map(|part| part.split("或是"))
        .filter(|part| branch_looks_like_player_action(part))
        .count()
        >= 2
}

fn inline_player_action_branch_count(text: &str) -> usize {
    let tail = text
        .split_once('：')
        .map(|(_, rest)| rest)
        .or_else(|| text.split_once(':').map(|(_, rest)| rest))
        .unwrap_or(text);
    tail.split(|c| c == '；')
        .flat_map(|part| part.split("或者"))
        .flat_map(|part| part.split("或是"))
        .filter(|part| branch_looks_like_player_action(part))
        .count()
}

fn branch_looks_like_player_action(part: &str) -> bool {
    let s = part.trim_matches(|c: char| c.is_whitespace() || "，。；:：、".contains(c));
    if s.is_empty() {
        return false;
    }
    if let Some((head, rest)) = s.split_once('，') {
        if head.chars().count() <= 16 && branch_looks_like_player_action(rest) {
            return true;
        }
    }
    let action_starts = [
        "先",
        "再",
        "继续",
        "有人",
        "下车",
        "试着",
        "尝试",
        "冒险",
        "顺着",
        "确认",
        "切断",
        "断线",
        "撬开",
        "靠近",
        "贴",
        "冲",
        "转向",
        "追",
        "开火",
        "射击",
        "喊话",
        "撤",
        "进入",
        "压制",
        "破坏",
        "扑近",
        "沿墙",
        "摸到",
        "想办法",
    ];
    if action_starts.iter().any(|prefix| s.starts_with(prefix)) {
        return true;
    }
    s.starts_with('对')
        && ["喊话", "发号施令", "开火", "射击"]
            .iter()
            .any(|verb| s.contains(verb))
}

fn text_has_prose_branch_action_menu(text: &str) -> bool {
    let frames = ["无论你是想", "无论你想", "无论你是要", "无论你要"];
    if !frames.iter().any(|cue| text.contains(cue)) || !text.contains("还是") {
        return false;
    }
    let branch_count = text.matches('、').count()
        + text.matches('，').count()
        + text.matches('；').count()
        + text.matches("或者").count()
        + text.matches("或是").count()
        + text.matches("还是").count();
    if branch_count < 3 {
        return false;
    }
    let action_verbs = [
        "继续", "试着", "尝试", "冒险", "顺着", "确认", "切断", "断线", "撬开", "靠近", "贴", "冲",
        "开火", "射击", "喊话", "撤", "进入", "压制", "破坏", "扑近", "沿墙", "摸到",
    ];
    action_verbs
        .iter()
        .filter(|verb| text.contains(**verb))
        .count()
        >= 3
}

fn detect_success_without_information(turn: &EvalTurn, findings: &mut Vec<EvalFinding>) {
    if turn
        .gm_response
        .check_outcome
        .as_deref()
        .map(|s| s.eq_ignore_ascii_case("success"))
        .unwrap_or(false)
        && turn.gm_response.facts_exposed_to_player.is_empty()
        && turn.gm_response.state_delta.is_empty()
        && turn.gm_response.choices_opened.is_empty()
    {
        findings.push(finding(
            FindingCategory::SuccessWithoutInformation,
            FindingSeverity::S1,
            vec![turn.turn],
            "Rules / Narrator",
            "A successful check should expose the success payload, commit a state change, open a concrete next step, or explain why success cannot reveal more.",
            "The check succeeded but exposed no concrete information or state delta.",
            vec![
                format!("player_action: {}", turn.player_decision.declared_action),
                format!("gm_response: {}", turn.gm_response.text),
            ],
            0.93,
        ));
    }
}

fn detect_mechanical_debt(turn: &EvalTurn, findings: &mut Vec<EvalFinding>) {
    let status_pending = matches!(
        turn.gm_response.resolution_status.as_str(),
        "pending" | "invalid"
    );
    let debt_marked = status_pending
        || !turn.trace.mechanical_debt_markers.is_empty()
        || contains_debt_language(&turn.gm_response.text);
    if debt_marked && turn.trace.pending_id.is_none() && turn.gm_response.pending.is_empty() {
        findings.push(finding(
            FindingCategory::PendingMechanicalDebt,
            FindingSeverity::S1,
            vec![turn.turn],
            "Rules / Reporter",
            "Unresolved mechanics must be resolved now or registered as pending with a pending_id, reason, and trigger condition.",
            "The turn leaves unresolved language or pending status without a pending_id.",
            vec![
                format!("resolution_status: {}", turn.gm_response.resolution_status),
                format!("markers: {:?}", turn.trace.mechanical_debt_markers),
                format!("gm_response: {}", turn.gm_response.text),
            ],
            0.96,
        ));
    }
}

fn contains_debt_language(text: &str) -> bool {
    ["待结算", "未落定", "等待", "悬着", "pending", "unresolved"]
        .iter()
        .any(|needle| text.contains(needle))
}

fn detect_semantic_noop(turn: &EvalTurn, findings: &mut Vec<EvalFinding>) {
    if no_progress(turn) && turn.trace.pending_id.is_none() {
        findings.push(finding(
            FindingCategory::SemanticNoop,
            FindingSeverity::S2,
            vec![turn.turn],
            "Resolution / Narrator",
            "Every turn should add a new fact, state change, concrete consequence, valid pending item, clarification request, or meaningful choice.",
            "The turn contains no recorded new fact, state delta, consequence, choice, or tracked pending item.",
            vec![
                format!("player_action: {}", turn.player_decision.declared_action),
                format!("gm_response: {}", turn.gm_response.text),
            ],
            0.88,
        ));
    }
}

fn no_progress(turn: &EvalTurn) -> bool {
    turn.gm_response.facts_added.is_empty()
        && turn.gm_response.facts_exposed_to_player.is_empty()
        && turn.gm_response.state_delta.is_empty()
        && turn.gm_response.choices_opened.is_empty()
        && turn.gm_response.pending.is_empty()
}

fn detect_continuity(turn: &EvalTurn, findings: &mut Vec<EvalFinding>) {
    if let Some(violation) = turn.trace.continuity_violation.as_ref() {
        if !violation.trim().is_empty() {
            findings.push(finding(
                FindingCategory::StateContinuityFail,
                FindingSeverity::S1,
                vec![turn.turn],
                "World / Director",
                "Scene, location, NPC, object, resource, and known-fact state must remain continuous unless the GM commits an explicit transition.",
                violation,
                vec![format!("continuity_violation: {violation}")],
                0.9,
            ));
        }
    }
}

fn detect_uncommitted_narrated_state(turn: &EvalTurn, findings: &mut Vec<EvalFinding>) {
    if !text_claims_position_change(&turn.gm_response.text) {
        return;
    }
    if !turn.gm_response.state_delta.is_empty()
        || !turn.trace.rolls.is_empty()
        || !turn.gm_response.pending.is_empty()
        || turn.trace.pending_id.is_some()
    {
        return;
    }
    findings.push(finding(
        FindingCategory::StateContinuityFail,
        FindingSeverity::S1,
        vec![turn.turn],
        "World / Reporter",
        "A narrated PC location or posture change must be backed by a committed state_delta, a resolved check trace, or a tracked pending/clarification item.",
        "GM narrated a completed position change without any committed state or mechanical trace.",
        vec![
            format!("player_action: {}", turn.player_decision.declared_action),
            format!("gm_response: {}", turn.gm_response.text),
        ],
        0.9,
    ));
}

fn text_claims_position_change(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    [
        "移动到",
        "转移到",
        "挪到",
        "挪开",
        "收进",
        "藏进",
        "藏住",
        "到达",
        "抵达",
        "来到",
        "紧贴着",
        "身位已经",
        "已经从先前",
        "门框旁的盲区",
        "doorframe's blind side",
        "doorframe blind side",
        "no longer stranded",
    ]
    .iter()
    .any(|cue| lowered.contains(&cue.to_ascii_lowercase()))
}

fn detect_rules_trace(turn: &EvalTurn, findings: &mut Vec<EvalFinding>) {
    for roll in &turn.trace.rolls {
        let missing = missing_roll_fields(roll);
        if !missing.is_empty() {
            findings.push(finding(
                FindingCategory::RulesTraceIncomplete,
                FindingSeverity::S1,
                vec![turn.turn],
                "Rules / Reporter",
                "Each mechanical result must expose skill/stat, die, die_result, total, target, outcome, source_refs, and state_delta when applicable.",
                "A roll trace is incomplete and cannot be independently audited.",
                vec![
                    format!("action: {}", roll.action),
                    format!("missing_fields: {:?}", missing),
                ],
                0.94,
            ));
        }
    }
}

fn missing_roll_fields(roll: &RollTrace) -> Vec<&'static str> {
    let mut missing = Vec::new();
    if roll.skill.as_deref().unwrap_or("").trim().is_empty()
        && roll.stat.as_deref().unwrap_or("").trim().is_empty()
    {
        missing.push("skill_or_stat");
    }
    if roll.die.as_deref().unwrap_or("").trim().is_empty() {
        missing.push("die");
    }
    if roll.die_result.is_none() {
        missing.push("die_result");
    }
    if roll.total.is_none() {
        missing.push("total");
    }
    if roll.target.as_deref().unwrap_or("").trim().is_empty() {
        missing.push("target");
    }
    if roll.outcome.as_deref().unwrap_or("").trim().is_empty() {
        missing.push("outcome");
    }
    if roll.source_refs.is_empty() {
        missing.push("source_refs");
    }
    if roll.state_delta.is_empty() {
        missing.push("state_delta");
    }
    missing
}

fn detect_player_script_loop(fixture: &EvalFixture, findings: &mut Vec<EvalFinding>) {
    let mut runs: HashMap<&str, Vec<&EvalTurn>> = HashMap::new();
    for turn in &fixture.turns {
        if no_progress(turn) {
            let intent = turn.player_decision.response_contract.intent.as_str();
            if !intent.trim().is_empty() {
                runs.entry(intent).or_default().push(turn);
            }
        }
    }
    for (intent, turns) in runs {
        if turns.len() >= 3 {
            findings.push(finding(
                FindingCategory::PlayerScriptLoop,
                FindingSeverity::S2,
                turns.iter().map(|t| t.turn).collect(),
                "PlayerSimulator",
                "The player simulator should change strategy, ask for clarification, retreat, or update goals after repeated non-progress.",
                "The fixture repeats the same functional intent across multiple no-progress turns.",
                turns
                    .iter()
                    .map(|t| format!("turn {} action: {}", t.turn, t.player_decision.declared_action))
                    .chain(std::iter::once(format!("repeated_intent: {intent}")))
                    .collect(),
                0.84,
            ));
        }
    }
}

fn finding(
    category: FindingCategory,
    severity: FindingSeverity,
    turns: Vec<u32>,
    root_layer: &str,
    expected: &str,
    actual: &str,
    evidence: Vec<String>,
    confidence: f32,
) -> EvalFinding {
    EvalFinding {
        finding_id: String::new(),
        category,
        severity,
        turns,
        root_layer: root_layer.to_string(),
        expected: expected.to_string(),
        actual: actual.to_string(),
        evidence,
        confidence,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EvalTurn, GmResponse, PlayerDecision, ResponseContract, TraceObservation};

    fn complete_player_decision(declared_action: &str, intent: &str) -> PlayerDecision {
        PlayerDecision {
            gm_visible_reply: "GM描述了玩家可见局面：仓库门前有火线、线缆和伤员。".to_string(),
            perceived_facts: vec![
                "仓库门前有火线压力".to_string(),
                "一根线缆连到仓库内部".to_string(),
            ],
            active_goal: "确认线缆和无人机控制源，同时避免暴露在火线中".to_string(),
            hypotheses: vec![
                "无人机可能被仓库内设备供电或控制".to_string(),
                "正面开阔地风险很高".to_string(),
            ],
            last_action_result: "上一轮观察获得了线缆和火力扇区信息".to_string(),
            risk_assessment: vec!["正面移动可能被无人机火力覆盖".to_string()],
            resource_assessment: vec!["可利用仓库外墙与门框作掩体".to_string()],
            open_questions: vec!["线缆是否通向可断开的控制源".to_string()],
            candidate_actions: vec![
                "沿掩体观察线缆走向".to_string(),
                "向伤员确认无人机开火规律".to_string(),
                "尝试从侧面接近仓库门".to_string(),
            ],
            selection_rationale: "谨慎调查者优先选择低暴露、能增加可见事实的行动".to_string(),
            declared_action: declared_action.to_string(),
            sent_to_gm: declared_action.to_string(),
            response_contract: ResponseContract {
                intent: intent.to_string(),
                requested_information: vec!["visible facts".to_string()],
                acceptable_resolutions: vec![
                    "facts exposed".to_string(),
                    "check resolved".to_string(),
                    "blocked with reason".to_string(),
                ],
                unacceptable: vec!["only atmosphere".to_string()],
            },
        }
    }

    #[test]
    fn complete_player_decision_protocol_can_pass_with_resolved_gm_response() {
        let fixture = EvalFixture {
            fixture_id: "complete-player-protocol".to_string(),
            title: "Complete Player Protocol".to_string(),
            turns: vec![EvalTurn {
                turn: 1,
                player_decision: complete_player_decision(
                    "我借仓库外墙掩护观察线缆进入门内后的走向。",
                    "observe_cable_route",
                ),
                gm_response: GmResponse {
                    text: "你看清线缆贴着门内左侧地面延伸，尽头接进一台仍亮着红灯的服务器柜。"
                        .to_string(),
                    facts_exposed_to_player: vec!["facts exposed".to_string()],
                    ..Default::default()
                },
                trace: TraceObservation::default(),
            }],
        };

        let report = evaluate_fixture(&fixture);

        assert_eq!(report.verdict, Verdict::Pass);
        assert_eq!(report.score, 100);
        assert!(!report.has_category(FindingCategory::PlayerPolicyViolation));
    }

    #[test]
    fn incomplete_player_decision_protocol_is_a_player_policy_violation() {
        let fixture = EvalFixture {
            fixture_id: "incomplete-player-protocol".to_string(),
            title: "Incomplete Player Protocol".to_string(),
            turns: vec![EvalTurn {
                turn: 1,
                player_decision: PlayerDecision {
                    declared_action: "我继续靠近仓库。".to_string(),
                    response_contract: ResponseContract {
                        intent: "approach_warehouse".to_string(),
                        acceptable_resolutions: vec!["movement resolved".to_string()],
                        ..Default::default()
                    },
                    ..Default::default()
                },
                gm_response: GmResponse {
                    text: "你靠近了一点。".to_string(),
                    state_delta: vec!["pc_closer_to_warehouse".to_string()],
                    ..Default::default()
                },
                trace: TraceObservation::default(),
            }],
        };

        let report = evaluate_fixture(&fixture);

        assert_eq!(report.verdict, Verdict::Fail);
        assert!(report.has_category(FindingCategory::PlayerPolicyViolation));
        let player_score = report
            .rubric_scores
            .iter()
            .find(|score| score.dimension == RubricDimension::PlayerSimulation)
            .unwrap();
        assert_eq!(player_score.weight, 15);
        assert_eq!(player_score.score, 0);
    }

    #[test]
    fn score_breakdown_uses_requested_rubric_weights() {
        let report = evaluate_fixture(&EvalFixture {
            fixture_id: "rubric-weights".to_string(),
            title: "Rubric Weights".to_string(),
            turns: vec![EvalTurn {
                turn: 1,
                player_decision: complete_player_decision(
                    "我观察仓库门口、线缆和无人机火线。",
                    "observe_scene",
                ),
                gm_response: GmResponse {
                    text: "你确认线缆通向门内左侧，火线主要覆盖正前方，两名警员仍在车后。"
                        .to_string(),
                    facts_exposed_to_player: vec!["facts exposed".to_string()],
                    ..Default::default()
                },
                trace: TraceObservation::default(),
            }],
        });

        let weights: Vec<(RubricDimension, i32)> = report
            .rubric_scores
            .iter()
            .map(|score| (score.dimension, score.weight))
            .collect();
        assert_eq!(
            weights,
            vec![
                (RubricDimension::RulesResolution, 25),
                (RubricDimension::Continuity, 20),
                (RubricDimension::Responsiveness, 20),
                (RubricDimension::NarrativeAgency, 15),
                (RubricDimension::PlayerSimulation, 15),
                (RubricDimension::Language, 5),
            ]
        );
        assert_eq!(report.score, 100);
    }

    #[test]
    fn explicit_option_menu_is_a_player_agency_violation() {
        let fixture = EvalFixture {
            fixture_id: "menu-regression".to_string(),
            title: "Menu Regression".to_string(),
            turns: vec![EvalTurn {
                turn: 7,
                player_decision: PlayerDecision {
                    declared_action: "我贴着墙检查仓库侧门和把手。".to_string(),
                    response_contract: ResponseContract {
                        intent: "probe_entry_point".to_string(),
                        requested_information: vec!["entry_affordance".to_string()],
                        acceptable_resolutions: vec!["visible obstacle or check".to_string()],
                        unacceptable: vec!["explicit option menu".to_string()],
                    },
                    ..Default::default()
                },
                gm_response: GmResponse {
                    text: "你现在可以立刻选择其一：\n- 从窗口翻进去\n- 走通风口\n- 继续死磕这扇门\n- 转去后勤通道".to_string(),
                    facts_exposed_to_player: vec!["warehouse has multiple possible entrances".to_string()],
                    ..Default::default()
                },
                trace: TraceObservation::default(),
            }],
        };

        let report = evaluate_fixture(&fixture);

        assert_eq!(report.verdict, Verdict::Fail);
        assert!(report.has_category(FindingCategory::PlayerAgencyViolation));
    }

    #[test]
    fn inline_followup_opportunities_are_a_player_agency_violation() {
        let fixture = EvalFixture {
            fixture_id: "inline-menu-regression".to_string(),
            title: "Inline Menu Regression".to_string(),
            turns: vec![EvalTurn {
                turn: 3,
                player_decision: PlayerDecision {
                    declared_action: "我撬开维护盖板确认里面是不是活线。".to_string(),
                    response_contract: ResponseContract {
                        intent: "inspect_live_wiring".to_string(),
                        requested_information: vec!["wire_status".to_string()],
                        acceptable_resolutions: vec!["wire status".to_string()],
                        unacceptable: vec!["inline option menu".to_string()],
                    },
                    ..Default::default()
                },
                gm_response: GmResponse {
                    text: "你已经摸到真东西。你现在有几个很近、很实际的后续机会：继续顺着这道缝看清哪根线是电；试着从这里做更精细的技术处理；或者冒险再往前贴一点，确认这组线是不是直接通向接口。".to_string(),
                    facts_exposed_to_player: vec!["live concealed wiring".to_string()],
                    ..Default::default()
                },
                trace: TraceObservation::default(),
            }],
        };

        let report = evaluate_fixture(&fixture);

        assert_eq!(report.verdict, Verdict::Fail);
        assert!(report.has_category(FindingCategory::PlayerAgencyViolation));
    }

    #[test]
    fn prose_rewritten_action_lists_are_a_player_agency_violation() {
        let fixture = EvalFixture {
            fixture_id: "rewritten-action-menu-regression".to_string(),
            title: "Rewritten Action Menu Regression".to_string(),
            turns: vec![EvalTurn {
                turn: 7,
                player_decision: PlayerDecision {
                    declared_action: "我楔住接头观察无人机反应。".to_string(),
                    response_contract: ResponseContract {
                        intent: "stabilize_signal_disconnect".to_string(),
                        requested_information: vec!["drone_reaction".to_string()],
                        acceptable_resolutions: vec!["drone reaction".to_string()],
                        unacceptable: vec!["rewritten action menu".to_string()],
                    },
                    ..Default::default()
                },
                gm_response: GmResponse {
                    text: "你现在有了一个短窗口。你把这些观察串在一起：趁它还没完全停摆，冲出去控制、拖拽或缴械；转向仓库内侧，追服务器和那根线的源头；对警员发号施令，争取把他们从射界里撤出来；先观察它接下来几秒还保不保持火控与瞄准能力。".to_string(),
                    facts_exposed_to_player: vec!["drone control is disrupted".to_string()],
                    ..Default::default()
                },
                trace: TraceObservation::default(),
            }],
        };

        let report = evaluate_fixture(&fixture);

        assert_eq!(report.verdict, Verdict::Fail);
        assert!(report.has_category(FindingCategory::PlayerAgencyViolation));
    }

    #[test]
    fn trailing_chinese_what_do_you_do_prompt_is_not_a_player_agency_violation() {
        let fixture = EvalFixture {
            fixture_id: "chinese-what-do-you-do-regression".to_string(),
            title: "Chinese What Do You Do Regression".to_string(),
            turns: vec![EvalTurn {
                turn: 1,
                player_decision: PlayerDecision {
                    declared_action: "我观察仓库门前的开阔带。".to_string(),
                    response_contract: ResponseContract {
                        intent: "observe_fire_lane".to_string(),
                        requested_information: vec!["visible lane facts".to_string()],
                        acceptable_resolutions: vec!["facts exposed".to_string()],
                        unacceptable: vec!["menu-like next action prompt".to_string()],
                    },
                    ..Default::default()
                },
                gm_response: GmResponse {
                    text: "你看清了仓库门前的开阔带。接下来你要怎么做？".to_string(),
                    facts_exposed_to_player: vec!["warehouse door fire lane is open".to_string()],
                    ..Default::default()
                },
                trace: TraceObservation::default(),
            }],
        };

        let report = evaluate_fixture(&fixture);

        assert!(!report.has_category(FindingCategory::PlayerAgencyViolation));
    }

    #[test]
    fn trailing_chinese_do_what_prompt_is_not_a_player_agency_violation() {
        let fixture = EvalFixture {
            fixture_id: "chinese-do-what-regression".to_string(),
            title: "Chinese Do What Regression".to_string(),
            turns: vec![EvalTurn {
                turn: 1,
                player_decision: PlayerDecision {
                    declared_action: "我把车停在加油站外缘观察。".to_string(),
                    response_contract: ResponseContract {
                        intent: "observe_gas_station".to_string(),
                        requested_information: vec!["visible station facts".to_string()],
                        acceptable_resolutions: vec!["facts exposed".to_string()],
                        unacceptable: vec!["menu-like next action prompt".to_string()],
                    },
                    ..Default::default()
                },
                gm_response: GmResponse {
                    text: "你把车停在加油站外缘，退路还在。接下来你要做什么？".to_string(),
                    facts_exposed_to_player: vec!["retreat lane remains open".to_string()],
                    ..Default::default()
                },
                trace: TraceObservation::default(),
            }],
        };

        let report = evaluate_fixture(&fixture);

        assert!(!report.has_category(FindingCategory::PlayerAgencyViolation));
    }

    #[test]
    fn prose_prepared_action_menu_is_a_player_agency_violation() {
        let fixture = EvalFixture {
            fixture_id: "prepared-action-menu-regression".to_string(),
            title: "Prepared Action Menu Regression".to_string(),
            turns: vec![EvalTurn {
                turn: 4,
                player_decision: PlayerDecision {
                    declared_action: "我打开油箱盖，问为什么晚上别在路上磨蹭。".to_string(),
                    response_contract: ResponseContract {
                        intent: "open_fuel_cap_probe_warning".to_string(),
                        requested_information: vec!["fueling state".to_string()],
                        acceptable_resolutions: vec!["fueling starts".to_string()],
                        unacceptable: vec!["prepared action menu".to_string()],
                    },
                    ..Default::default()
                },
                gm_response: GmResponse {
                    text: "油开始往车里灌。接下来你是准备继续坐在车里指挥他们加油，还是有人下车去开油箱、买水，或者再追问晚上别在路上的事？"
                        .to_string(),
                    facts_exposed_to_player: vec!["fueling starts".to_string()],
                    ..Default::default()
                },
                trace: TraceObservation::default(),
            }],
        };

        let report = evaluate_fixture(&fixture);

        assert_eq!(report.verdict, Verdict::Fail);
        assert!(report.has_category(FindingCategory::PlayerAgencyViolation));
    }

    #[test]
    fn trailing_how_to_take_next_step_prompt_is_not_a_player_agency_violation() {
        let fixture = EvalFixture {
            fixture_id: "chinese-next-step-regression".to_string(),
            title: "Chinese Next Step Regression".to_string(),
            turns: vec![EvalTurn {
                turn: 1,
                player_decision: PlayerDecision {
                    declared_action: "我观察仓库门和线缆。".to_string(),
                    response_contract: ResponseContract {
                        intent: "observe_warehouse_threshold".to_string(),
                        requested_information: vec!["visible threshold facts".to_string()],
                        acceptable_resolutions: vec!["facts exposed".to_string()],
                        unacceptable: vec!["menu-like next action prompt".to_string()],
                    },
                    ..Default::default()
                },
                gm_response: GmResponse {
                    text: "仓库门口仍在火线边缘，线缆钻进里面的设备区。眼下你要怎么接下一步？"
                        .to_string(),
                    facts_exposed_to_player: vec!["cable enters equipment area".to_string()],
                    ..Default::default()
                },
                trace: TraceObservation::default(),
            }],
        };

        let report = evaluate_fixture(&fixture);

        assert!(!report.has_category(FindingCategory::PlayerAgencyViolation));
    }

    #[test]
    fn english_next_move_could_be_menu_is_a_player_agency_violation() {
        let fixture = EvalFixture {
            fixture_id: "english-next-move-regression".to_string(),
            title: "English Next Move Regression".to_string(),
            turns: vec![EvalTurn {
                turn: 2,
                player_decision: PlayerDecision {
                    declared_action: "I move to the warehouse doorframe blind side.".to_string(),
                    response_contract: ResponseContract {
                        intent: "move_to_cover".to_string(),
                        requested_information: vec!["new position state".to_string()],
                        acceptable_resolutions: vec!["position resolved".to_string()],
                        unacceptable: vec!["explicit menu".to_string()],
                    },
                    ..Default::default()
                },
                gm_response: GmResponse {
                    text: concat!(
                        "You make the doorframe's blind side. ",
                        "From here, your next move could be to peek deeper into the warehouse, ",
                        "go for the server side, try to help the cops, or make a play on the cable."
                    )
                    .to_string(),
                    facts_exposed_to_player: vec!["warehouse doorframe reached".to_string()],
                    ..Default::default()
                },
                trace: TraceObservation::default(),
            }],
        };

        let report = evaluate_fixture(&fixture);

        assert_eq!(report.verdict, Verdict::Fail);
        assert!(report.has_category(FindingCategory::PlayerAgencyViolation));
    }

    #[test]
    fn either_or_action_branches_are_a_player_agency_violation() {
        let fixture = EvalFixture {
            fixture_id: "either-or-action-branches-regression".to_string(),
            title: "Either Or Action Branches Regression".to_string(),
            turns: vec![EvalTurn {
                turn: 1,
                player_decision: PlayerDecision {
                    declared_action: "我观察仓库门前的危险路线。".to_string(),
                    response_contract: ResponseContract {
                        intent: "observe_approach_route".to_string(),
                        requested_information: vec!["route risk".to_string()],
                        acceptable_resolutions: vec!["route risk".to_string()],
                        unacceptable: vec!["either-or action menu".to_string()],
                    },
                    ..Default::default()
                },
                gm_response: GmResponse {
                    text: "最危险的是仓库门前那段开阔带。要么继续借掩体贴过去，要么想办法先让那台 drone 转火、断电，或者失去行动能力。".to_string(),
                    facts_exposed_to_player: vec!["warehouse fire lane is dangerous".to_string()],
                    ..Default::default()
                },
                trace: TraceObservation::default(),
            }],
        };

        let report = evaluate_fixture(&fixture);

        assert_eq!(report.verdict, Verdict::Fail);
        assert!(report.has_category(FindingCategory::PlayerAgencyViolation));
    }

    #[test]
    fn fact_summary_after_observation_frame_is_not_a_player_agency_violation() {
        let fixture = EvalFixture {
            fixture_id: "fact-summary-regression".to_string(),
            title: "Fact Summary Regression".to_string(),
            turns: vec![EvalTurn {
                turn: 1,
                player_decision: complete_player_decision(
                    "我观察无人机、线缆和警员位置。",
                    "observe_scene",
                ),
                gm_response: GmResponse {
                    text: "你把这些观察串在一起：那台人形无人机和仓库之间，确实连着一根明显的线缆；它的火力扇区主要咬着正前方开阔地；两名警员都还活着，但伤得不轻；它和仓库里的东西之间，很可能不只是拴着，更像是被供电、控制，或者两者都有。".to_string(),
                    facts_exposed_to_player: vec!["facts exposed".to_string()],
                    ..Default::default()
                },
                trace: TraceObservation::default(),
            }],
        };

        let report = evaluate_fixture(&fixture);

        assert_eq!(report.verdict, Verdict::Pass);
        assert!(!report.has_category(FindingCategory::PlayerAgencyViolation));
    }

    #[test]
    fn no_colon_prose_branch_menus_are_a_player_agency_violation() {
        let fixture = EvalFixture {
            fixture_id: "no-colon-branch-menu-regression".to_string(),
            title: "No Colon Branch Menu Regression".to_string(),
            turns: vec![EvalTurn {
                turn: 2,
                player_decision: PlayerDecision {
                    declared_action: "我冲到仓库外墙，观察无人机背面和墙面异常。".to_string(),
                    response_contract: ResponseContract {
                        intent: "change_cover_and_observe".to_string(),
                        requested_information: vec!["new vantage facts".to_string()],
                        acceptable_resolutions: vec!["new position and visible facts".to_string()],
                        unacceptable: vec!["branch menu".to_string()],
                    },
                    ..Default::default()
                },
                gm_response: GmResponse {
                    text: "你现在已经贴上仓库外墙，离那台东西更近。下一步，无论你是想继续沿墙摸到门口、扑近它后背那块异常装配区、冲进仓库内侧，还是先朝警员喊话配合，时机都比刚才好多了。".to_string(),
                    facts_exposed_to_player: vec!["warehouse wall reached".to_string()],
                    ..Default::default()
                },
                trace: TraceObservation::default(),
            }],
        };

        let report = evaluate_fixture(&fixture);

        assert_eq!(report.verdict, Verdict::Fail);
        assert!(report.has_category(FindingCategory::PlayerAgencyViolation));
    }

    #[test]
    fn narrated_position_change_without_state_delta_is_a_continuity_failure() {
        let fixture = EvalFixture {
            fixture_id: "uncommitted-position-regression".to_string(),
            title: "Uncommitted Position Regression".to_string(),
            turns: vec![EvalTurn {
                turn: 2,
                player_decision: PlayerDecision {
                    declared_action:
                        "我移动到仓库门框旁的盲区，但不碰线缆也不进门。".to_string(),
                    response_contract: ResponseContract {
                        intent: "move_to_cover".to_string(),
                        requested_information: vec!["position state".to_string()],
                        acceptable_resolutions: vec![
                            "committed position change or check".to_string()
                        ],
                        unacceptable: vec!["narrated location change without state delta".to_string()],
                    },
                    ..Default::default()
                },
                gm_response: GmResponse {
                    text: "等你停住时，身位已经从先前暴露的位置挪开，整个人紧贴着仓库门框旁的盲区藏住。"
                        .to_string(),
                    facts_exposed_to_player: vec!["doorframe blind side reached".to_string()],
                    ..Default::default()
                },
                trace: TraceObservation::default(),
            }],
        };

        let report = evaluate_fixture(&fixture);

        assert_eq!(report.verdict, Verdict::Fail);
        assert!(report.has_category(FindingCategory::StateContinuityFail));
    }
}
