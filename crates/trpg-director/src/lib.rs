use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use trpg_combat::ConflictTurnResult;
use trpg_model::*;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DirectorMode {
    Disabled,
    OnDemand,
    EveryTurn,
}

#[derive(Debug, Clone)]
pub struct ActionableSituationDirector {
    pub mode: DirectorMode,
}

#[derive(Debug, Clone, Copy)]
pub struct DirectorInput<'a> {
    pub request: &'a ContextRequest,
    pub state: &'a RuntimeState,
    pub compiled: &'a CompiledContext,
    pub user_input: &'a str,
    pub conflict: Option<&'a ConflictTurnResult>,
    /// Module-level engine config loaded by the caller (runtime). The director's
    /// module-specific scene facts / NPC advice / place summary come from
    /// `module_config.director`; None → generic path (fail-soft). This replaces
    /// the old is_homecoming() ruleset/module/input name sniffing — the director
    /// now branches on DATA presence, never on names.
    pub module_config: Option<&'a ModuleConfig>,
}

impl<'a> DirectorInput<'a> {
    /// The module's director facilitation overlay, if any.
    fn director_cfg(&self) -> Option<&'a DirectorModuleConfig> {
        self.module_config.and_then(|m| m.director.as_ref())
    }
}

/// Parse data-supplied ActionVector serde names into enum values, dropping
/// unknown names (fail-soft). Empty/all-unknown → caller's neutral default.
fn parse_vectors(names: &[String]) -> Vec<ActionVector> {
    names
        .iter()
        .filter_map(|n| serde_json::from_value::<ActionVector>(json!(n)).ok())
        .collect()
}

impl ActionableSituationDirector {
    pub fn from_env_or_default() -> Self {
        let enabled = env_bool("TRPG_ACTIONABLE_DIRECTOR_ENABLE_V13", true);
        let every_turn = env_bool("TRPG_DIRECTOR_BRIEF_EVERY_TURN", false);
        let mode = if !enabled { DirectorMode::Disabled } else if every_turn { DirectorMode::EveryTurn } else { DirectorMode::OnDemand };
        Self { mode }
    }

    pub fn prepare(&self, input: DirectorInput<'_>) -> DirectorTurnResult {
        if self.mode == DirectorMode::Disabled {
            return DirectorTurnResult::default();
        }
        let active_frame = input.conflict.and_then(|c| c.frame.as_ref());
        let should_present = self.mode == DirectorMode::EveryTurn || needs_director_brief(input.user_input, active_frame, input.conflict);
        if !should_present {
            return DirectorTurnResult::default();
        }
        let level = choose_guidance_level(input.user_input, active_frame, input.conflict);
        let mut brief = build_brief(input, active_frame, level);
        if let Some(conflict) = input.conflict { if let Some(novelty) = &conflict.novelty { brief.fresh_change = novelty.fresh_change.clone(); } }
        enforce_guidance_policy(&mut brief);
        repair_brief_anchors(&mut brief, input, active_frame);
        let novelty = input.conflict.and_then(|c| c.novelty.clone()).or_else(|| brief.fresh_change.clone().map(novelty_from_fresh_change));
        let clue_board = maybe_build_clue_board(input, &brief);
        let consequence = Some(build_consequence_contract(input, active_frame, &brief));
        let clock_ticks = maybe_tick_clocks(input, active_frame, &brief);
        let spotlight = build_spotlight(input);
        let mut phases = vec!["actionable_situation_brief".into(), "guidance_ladder".into()];
        if clue_board.is_some() { phases.push("clue_board_updated".into()); }
        if consequence.is_some() { phases.push("consequence_contract_created".into()); }
        if !clock_ticks.is_empty() { phases.push("clock_tick".into()); }
        if novelty.is_some() { phases.push("fresh_change".into()); phases.push("novelty_director".into()); }
        if !spotlight.is_empty() { phases.push("spotlight_state_updated".into()); }
        DirectorTurnResult {
            handled: true,
            phases,
            narration_context: Some(render_director_context(&brief, clue_board.as_ref(), consequence.as_ref(), &clock_ticks, novelty.as_ref())),
            brief: Some(brief),
            clue_board,
            consequence,
            clock_ticks,
            spotlight,
            novelty,
        }
    }
}

fn needs_director_brief(input: &str, frame: Option<&StateFrame>, conflict: Option<&ConflictTurnResult>) -> bool {
    let t = input.to_lowercase();
    if contains_any(&t, &["不知道", "能做什么", "怎么办", "卡住", "整理", "已知", "线索", "目标", "风险", "代价", "方向", "what can", "what do we know", "stuck", "summarize", "options", "clue", "怎么看", "npc", "警察", "雇主", "立场"]) { return true; }
    if frame.is_some() && contains_any(&t, &["继续等", "继续讨论", "观察", "看看", "犹豫", "不行动", "wait", "observe", "look around", "discuss"]) { return true; }
    if let Some(c) = conflict {
        if c.handled && (c.check.is_none() || c.narration_context.is_some()) { return true; }
        if c.stalemate_contract.is_some() || c.exit_contract.is_some() || c.novelty.is_some() { return true; }
    }
    false
}

fn choose_guidance_level(input: &str, frame: Option<&StateFrame>, conflict: Option<&ConflictTurnResult>) -> GuidanceLevel {
    let t = input.to_lowercase();
    if contains_any(&t, &["具体", "举例", "给几个", "examples", "具体例子"]) { return GuidanceLevel::OfferCostedExamples; }
    if contains_any(&t, &["能做什么", "怎么办", "方向", "options", "what can", "stuck"]) { return GuidanceLevel::OfferActionCategories; }
    if contains_any(&t, &["目标", "想做什么", "what should we focus", "priority"]) { return GuidanceLevel::AskGoal; }
    if contains_any(&t, &["整理", "已知", "线索", "what do we know", "summarize", "recap"]) { return GuidanceLevel::SummarizeKnownInfo; }
    if conflict.and_then(|c| c.stalemate_contract.as_ref()).is_some() { return GuidanceLevel::OfferActionCategories; }
    if frame.is_some() { GuidanceLevel::AskGoal } else { default_guidance_level() }
}

fn build_brief(input: DirectorInput<'_>, frame: Option<&StateFrame>, level: GuidanceLevel) -> ActionableSituationBrief {
    let frame_id = frame.map(|f| f.frame_id.clone());
    let mut visible_facts = base_visible_facts(input, frame);
    let mut pressure = base_pressure(input, frame);
    let mut affordances = base_affordances(input, frame);
    let mut risks = base_risks(input, frame);
    let mut known_facts = vec![KnownFact { fact_id: id("known"), text: "玩家只需要声明目标和手段；GM 会说明风险、检定和后果。".into(), source: "director_policy".into(), public: true }];
    let mut open_questions = vec![OpenQuestion { question_id: id("question"), text: "你们现在最想改变哪件事？".into(), points_to: vec!["goal".into(), "risk".into(), "approach".into()] }];

    if let Some(cfg) = input.director_cfg() {
        for sf in &cfg.scene_facts {
            visible_facts.push(VisibleFact { fact_id: id("fact"), text: sf.text.clone(), source_refs: vec![], confidence: RulingConfidence::Medium });
        }
        for pi in &cfg.pressure_items {
            pressure.push(PressureItem { pressure_id: id("pressure"), text: pi.text.clone(), clock_id: pi.clock_id.clone(), severity: pi.severity, consequence_hint: pi.consequence_hint.clone() });
        }
        for ai in &cfg.affordance_items {
            let mut vectors = parse_vectors(&ai.implies_vectors);
            if vectors.is_empty() { vectors = vec![ActionVector::Observe, ActionVector::Technical, ActionVector::Tactical]; }
            affordances.push(Affordance { affordance_id: id("affordance"), description: ai.description.clone(), implies_vectors: vectors, visible_to_players: true, source_refs: vec![] });
        }
        for ri in &cfg.risk_items {
            let mut vectors = parse_vectors(&ri.related_vectors);
            if vectors.is_empty() { vectors = vec![ActionVector::Tactical]; }
            risks.push(RiskItem { risk_id: id("risk"), text: ri.text.clone(), related_vectors: vectors, severity: ri.severity });
        }
        for kf in &cfg.known_facts {
            known_facts.push(KnownFact { fact_id: id("known"), text: kf.clone(), source: "module_director_config".into(), public: true });
        }
        if let Some(oq) = &cfg.open_question {
            open_questions.push(OpenQuestion { question_id: id("question"), text: oq.clone(), points_to: cfg.open_question_points_to.clone() });
        }
    }

    if let Some(conflict) = input.conflict {
        if let Some(intent) = &conflict.intent {
            known_facts.push(KnownFact { fact_id: id("known"), text: format!("系统判断当前玩家意图为 {:?} / {:?}，但这不是路线命令，只是裁决辅助。", intent.relation_to_active_frame, intent.action_kind), source: "semantic_situation_router".into(), public: true });
        }
    }

    let examples = if level == GuidanceLevel::OfferCostedExamples { costed_examples(input) } else { vec![] };
    let min_costed = if level == GuidanceLevel::OfferCostedExamples { env_usize("TRPG_DIRECTOR_MIN_COSTED_EXAMPLES", 3) } else { 0 };
    let prompt_kind = match level {
        GuidanceLevel::ObservableFactsOnly | GuidanceLevel::SummarizeKnownInfo | GuidanceLevel::AskGoal => PlayerDecisionPromptKind::ChooseGoal,
        GuidanceLevel::OfferActionCategories => PlayerDecisionPromptKind::ChooseApproachVector,
        GuidanceLevel::OfferCostedExamples => PlayerDecisionPromptKind::ChooseRiskTolerance,
    };

    ActionableSituationBrief {
        brief_id: id("brief"),
        session_id: input.request.session_id.clone(),
        turn_id: input.request.turn_id.clone(),
        frame_id,
        where_are_we: frame.map(|f| format!("当前局部局势：{}。{}", f.title, f.objective)).unwrap_or_else(|| current_place_summary(input)),
        visible_facts,
        pressure,
        affordances,
        risks,
        known_facts,
        open_questions,
        guidance: GuidanceDecision { level, prompt_kind, reason: "director detected a need to convert scene description into player-actionable situation information".into(), forbid_single_correct_path: true, min_costed_examples: min_costed },
        goal_prompt: goal_prompt_for_level(level),
        costed_examples: examples,
        npc_advice: biased_npc_advice(input),
        fresh_change: None,
        scene_purpose: Some(SceneFramePurpose { scene_id: input.state.scene_id.clone(), scene_purpose: infer_scene_purpose(input, frame), enter_condition: None, progress_signals: vec!["玩家明确目标".into(), "局势压力变化".into(), "线索或后果落地".into()], exit_conditions: vec!["目标完成".into(), "玩家选择离开".into(), "倒计时触发后果".into(), "场景转为另一个 frame".into()], fail_forward_options: vec!["给线索但推进 clock".into(), "成功但有代价".into(), "失败揭示新风险".into()], pacing_budget_turns: Some(4) }),
        avoid_single_correct_path: true,
        player_facing: true,
        created_at: Utc::now(),
    }
}

fn enforce_guidance_policy(brief: &mut ActionableSituationBrief) {
    brief.avoid_single_correct_path = true;
    brief.guidance.forbid_single_correct_path = true;
    if brief.guidance.level == GuidanceLevel::OfferCostedExamples {
        while brief.costed_examples.len() < brief.guidance.min_costed_examples.max(3) {
            let i = brief.costed_examples.len();
            let vector = [ActionVector::Observe, ActionVector::Social, ActionVector::Tactical, ActionVector::Retreat][i % 4];
            brief.costed_examples.push(CostedExample { example_id: id("example"), approach: vector, example: format!("用 {} 方向提出你自己的做法", vector.as_str()), likely_check: None, success_hint: "推进目标或获得信息".into(), failure_or_cost_hint: "推进局势压力或付出资源/位置代价".into() });
        }
    }
}


fn repair_brief_anchors(brief: &mut ActionableSituationBrief, input: DirectorInput<'_>, frame: Option<&StateFrame>) {
    while brief.visible_facts.len() < 2 {
        brief.visible_facts.push(VisibleFact { fact_id: id("fact"), text: "你们能确认：这里有至少一个可观察事实尚未转化为行动。先说目标，再说手段。".into(), source_refs: vec![], confidence: RulingConfidence::Medium });
    }
    if brief.pressure.is_empty() {
        brief.pressure.push(PressureItem { pressure_id: id("pressure"), text: "如果没有行动或新目标，局势压力会推进：NPC 耐心、危险 clock 或对手目标不会暂停。".into(), clock_id: Some("clock.scene_pressure".into()), severity: 50, consequence_hint: Some("clock_tick / npc_tactic_shift".into()) });
    }
    while brief.affordances.len() < 2 {
        brief.affordances.push(Affordance { affordance_id: id("affordance"), description: "你可以选择目标和手段，而不是猜 GM 的正确路线。观察、社交、技术、战术、资源或撤退都可以成为方向。".into(), implies_vectors: vec![ActionVector::Observe, ActionVector::Social, ActionVector::Technical, ActionVector::Tactical, ActionVector::Resource, ActionVector::Retreat], visible_to_players: true, source_refs: vec![] });
    }
    if brief.risks.is_empty() {
        brief.risks.push(RiskItem { risk_id: id("risk"), text: "任何方向都有代价：快可能暴露，稳可能耗时，保守会让其他势力或危险推进。".into(), related_vectors: vec![ActionVector::Tactical, ActionVector::Retreat, ActionVector::Observe], severity: 45 });
    }
    if brief.known_facts.is_empty() {
        brief.known_facts.push(KnownFact { fact_id: id("known"), text: "目前只整理玩家可见事实；隐藏真相不会进入玩家线索板。".into(), source: "director_repair".into(), public: true });
    }
    if brief.open_questions.is_empty() {
        brief.open_questions.push(OpenQuestion { question_id: id("question"), text: "你们现在最想改变什么？".into(), points_to: vec!["goal".into(), "approach".into(), "risk".into()] });
    }
    if brief.fresh_change.is_none() {
        let summary = if let Some(frame) = frame { format!("当前 {} frame 需要产生新的事实、压力、风险或 NPC 战术变化，不能重复上一轮回应。", frame.frame_kind.as_str()) } else { "场景需要给玩家一个新的可行动变化，而不是重复氛围描述。".into() };
        brief.fresh_change = Some(FreshChange { change_id: id("fresh"), change_type: FreshChangeType::NewPressure, summary, caused_by: input.user_input.chars().take(80).collect(), source_refs: vec![], created_at: Utc::now() });
    }
}

fn novelty_from_fresh_change(change: FreshChange) -> NoveltyDecision {
    NoveltyDecision { decision_id: id("novelty"), force_tactic_shift: matches!(change.change_type, FreshChangeType::NpcTacticShift | FreshChangeType::ContinuedPressureWithCost), selected_tactic_id: None, previous_tactic_id: None, novelty_score: 0.75, reason: "fresh_change_from_director".into(), fresh_change: Some(change), beat: None }
}

fn maybe_build_clue_board(input: DirectorInput<'_>, brief: &ActionableSituationBrief) -> Option<PlayerFacingClueBoard> {
    let t = input.user_input.to_lowercase();
    if !contains_any(&t, &["整理", "已知", "线索", "recap", "what do we know", "clue", "不知道", "能做什么", "怎么办", "stuck", "options"]) && !matches!(brief.guidance.level, GuidanceLevel::SummarizeKnownInfo | GuidanceLevel::OfferActionCategories) { return None; }
    Some(PlayerFacingClueBoard { board_id: id("clue_board"), session_id: input.request.session_id.clone(), turn_id: input.request.turn_id.clone(), known_facts: brief.known_facts.clone(), open_leads: brief.affordances.iter().take(6).map(|a| Lead { lead_id: id("lead"), text: a.description.clone(), target_ref: None, exhausted: false }).collect(), unresolved_questions: brief.open_questions.clone(), exhausted_nodes: vec![], pressure_notes: brief.pressure.clone(), updated_at: Utc::now() })
}

fn build_consequence_contract(input: DirectorInput<'_>, frame: Option<&StateFrame>, brief: &ActionableSituationBrief) -> ConsequenceContract {
    ConsequenceContract { consequence_id: id("consequence"), session_id: input.request.session_id.clone(), turn_id: input.request.turn_id.clone(), frame_id: frame.map(|f| f.frame_id.clone()), on_success: "玩家的目标推进；把新的事实、资源变化或位置变化写入 frame/world state。".into(), on_failure: "失败仍然推进局势：给信息、推进 clock、改变 NPC 态度或产生资源/位置代价。".into(), fail_forward: true, cost_options: vec!["推进一个公开压力 clock".into(), "暴露位置或意图".into(), "消耗时间、金钱、弹药、信誉或机会".into()], clue_safety_policy: "关键线索不应被单次失败永久锁死；可以转为带代价获得或换路径获得。".into(), source_refs: brief.visible_facts.iter().flat_map(|f| f.source_refs.clone()).collect() }
}

fn maybe_tick_clocks(input: DirectorInput<'_>, _frame: Option<&StateFrame>, _brief: &ActionableSituationBrief) -> Vec<ClockTick> {
    if !env_bool("TRPG_DIRECTOR_CLOCK_ON_STALL", true) { return vec![]; }
    let t = input.user_input.to_lowercase();
    if contains_any(&t, &["继续等", "继续讨论", "不行动", "犹豫", "等着", "继续开火", "继续攻击", "还是打", "wait", "keep discussing", "do nothing", "keep firing", "continue attacking"]) {
        vec![ClockTick { clock_id: "clock.scene_pressure".into(), label: "局势压力".into(), previous: 0, current: 1, max: 4, reason: "玩家选择等待/讨论/不推进，世界继续行动。".into(), visible_to_players: true }]
    } else { vec![] }
}

fn build_spotlight(input: DirectorInput<'_>) -> Vec<SpotlightState> {
    vec![SpotlightState { player_id: "current_player".into(), character_id: Some("pc.current".into()), last_spotlight_turn: Some(input.request.turn_id.clone()), spotlight_count: 1, character_strengths: vec![], preferred_playstyle: vec![], pending_personal_hook: None }]
}

fn render_director_context(brief: &ActionableSituationBrief, board: Option<&PlayerFacingClueBoard>, consequence: Option<&ConsequenceContract>, ticks: &[ClockTick], novelty: Option<&NoveltyDecision>) -> String {
    let mut out = String::new();
    out.push_str("\n\n[Actionable Situation Director v1.3]\n");
    out.push_str("Use this as player-facing facilitation context. Do not output a single official answer. Illuminate facts, pressure, affordances, risks, and ask for the player's goal/approach. Avoid saying 'you should'.\n");
    out.push_str("```json\n");
    out.push_str(&serde_json::to_string_pretty(&json!({"brief": brief, "clue_board": board, "consequence": consequence, "clock_ticks": ticks, "novelty": novelty})).unwrap_or_default());
    out.push_str("\n```\n");
    out
}

fn base_visible_facts(input: DirectorInput<'_>, frame: Option<&StateFrame>) -> Vec<VisibleFact> {
    let mut facts = vec![VisibleFact { fact_id: id("fact"), text: "玩家面前的局势应该先被表达为可观察事实，而不是 GM 的行动建议。".into(), source_refs: vec![], confidence: RulingConfidence::High }];
    if let Some(frame) = frame { facts.push(VisibleFact { fact_id: id("fact"), text: format!("当前局部流程仍在进行：{}。目标是：{}", frame.title, frame.objective), source_refs: vec![], confidence: RulingConfidence::High }); }
    if !input.state.active_npc_ids.is_empty() { facts.push(VisibleFact { fact_id: id("fact"), text: format!("当前活跃 NPC：{}。NPC 的提示必须带有立场和偏见，不能当作官方攻略。", input.state.active_npc_ids.join(", ")), source_refs: vec![], confidence: RulingConfidence::Medium }); }
    facts
}

fn base_pressure(_input: DirectorInput<'_>, frame: Option<&StateFrame>) -> Vec<PressureItem> {
    let mut pressure = vec![PressureItem { pressure_id: id("pressure"), text: "世界不会因为玩家讨论而暂停；如果继续拖延，后台目标、NPC 耐心或环境危险会推进。".into(), clock_id: Some("clock.scene_pressure".into()), severity: 50, consequence_hint: Some("推进倒计时或改变 NPC 行动".into()) }];
    if frame.is_some() { pressure.push(PressureItem { pressure_id: id("pressure"), text: "当前 frame 不应该无限循环；没有决定性推进时应打开方向门或推进后果。".into(), clock_id: Some("clock.frame_stalemate".into()), severity: 60, consequence_hint: Some("direction gate / NPC tactic shift / frame transition".into()) }); }
    pressure
}

fn base_affordances(_input: DirectorInput<'_>, frame: Option<&StateFrame>) -> Vec<Affordance> {
    let mut aff = vec![
        Affordance { affordance_id: id("affordance"), description: "观察现场细节并确认已知事实。".into(), implies_vectors: vec![ActionVector::Observe], visible_to_players: true, source_refs: vec![] },
        Affordance { affordance_id: id("affordance"), description: "与 NPC 交流、施压、安抚、套话或交易。".into(), implies_vectors: vec![ActionVector::Social], visible_to_players: true, source_refs: vec![] },
        Affordance { affordance_id: id("affordance"), description: "用角色技能、装备或联系人创造新路径。".into(), implies_vectors: vec![ActionVector::Technical, ActionVector::Resource, ActionVector::Weird], visible_to_players: true, source_refs: vec![] },
        Affordance { affordance_id: id("affordance"), description: "改变位置、撤退、潜入、绕路或寻找掩护。".into(), implies_vectors: vec![ActionVector::Mobility, ActionVector::Stealth, ActionVector::Retreat], visible_to_players: true, source_refs: vec![] },
    ];
    if let Some(frame) = frame { aff.push(Affordance { affordance_id: id("affordance"), description: format!("当前 {} frame 可以通过完成目标、承担代价、退出或转场来推进。", frame.frame_kind.as_str()), implies_vectors: vec![ActionVector::Tactical, ActionVector::Retreat, ActionVector::Social], visible_to_players: true, source_refs: vec![] }); }
    aff
}

fn base_risks(_input: DirectorInput<'_>, _frame: Option<&StateFrame>) -> Vec<RiskItem> {
    vec![
        RiskItem { risk_id: id("risk"), text: "最快路径通常带来暴露、资源消耗或后续情报损失。".into(), related_vectors: vec![ActionVector::Tactical, ActionVector::Technical], severity: 50 },
        RiskItem { risk_id: id("risk"), text: "最安全路径可能让其他势力、倒计时或敌方目标推进。".into(), related_vectors: vec![ActionVector::Retreat, ActionVector::Observe], severity: 45 },
    ]
}

fn costed_examples(_input: DirectorInput<'_>) -> Vec<CostedExample> {
    vec![
        CostedExample { example_id: id("example"), approach: ActionVector::Observe, example: "先整理现场事实或扫描异常点。".into(), likely_check: None, success_hint: "确认抓手，降低误判。".into(), failure_or_cost_hint: "花费时间，推进压力。".into() },
        CostedExample { example_id: id("example"), approach: ActionVector::Social, example: "询问、安抚、威慑或交易，让 NPC 暴露立场。".into(), likely_check: Some("appropriate social skill".into()), success_hint: "获得线索、盟友或缓和冲突。".into(), failure_or_cost_hint: "NPC 耐心下降或条件变差。".into() },
        CostedExample { example_id: id("example"), approach: ActionVector::Technical, example: "分析设备、信号、痕迹、文件或环境机制。".into(), likely_check: Some("appropriate technical/investigation skill".into()), success_hint: "找到源头、弱点或替代路径。".into(), failure_or_cost_hint: "暴露位置、损坏材料或推进倒计时。".into() },
        CostedExample { example_id: id("example"), approach: ActionVector::Retreat, example: "撤离、绕路、重整或等待更好机会。".into(), likely_check: None, success_hint: "降低即时风险。".into(), failure_or_cost_hint: "对手或世界目标继续推进。".into() },
    ]
}

fn biased_npc_advice(input: DirectorInput<'_>) -> Vec<NpcBiasedAdvice> {
    if let Some(cfg) = input.director_cfg() {
        if !cfg.npc_advice.is_empty() { return cfg.npc_advice.clone(); }
    }
    vec![NpcBiasedAdvice { npc_id: "local_npc".into(), speaker_label: "现场 NPC".into(), advice_text: "我只会从自己的利益出发提醒你们。".into(), bias_or_goal: "NPC advice is biased and not the GM's official route.".into(), not_official_solution: true }]
}

fn infer_scene_purpose(input: DirectorInput<'_>, frame: Option<&StateFrame>) -> ScenePurpose {
    let t = input.user_input.to_lowercase();
    if contains_any(&t, &["整理", "已知", "线索", "recap", "clue"]) { return ScenePurpose::RevealInformation; }
    if contains_any(&t, &["不知道", "怎么办", "能做什么", "stuck", "options"]) { return ScenePurpose::ForceChoice; }
    if frame.is_some() { ScenePurpose::ResolveConflict } else { ScenePurpose::EstablishTone }
}

fn goal_prompt_for_level(level: GuidanceLevel) -> String {
    match level {
        GuidanceLevel::ObservableFactsOnly => "先看局势：你们现在最想改变哪件事？".into(),
        GuidanceLevel::SummarizeKnownInfo => "这些是你们已经知道的内容；你们想追哪条线，还是换目标？".into(),
        GuidanceLevel::AskGoal => "先不用猜正确答案：你们当前的首要目标是什么？".into(),
        GuidanceLevel::OfferActionCategories => "可以从观察、社交、技术、战术、资源、潜行或撤退等方向提出自己的做法；你们选哪个方向？".into(),
        GuidanceLevel::OfferCostedExamples => "下面只是带代价的例子，不是标准答案。你们接受哪类风险，或提出别的办法？".into(),
    }
}

fn current_place_summary(input: DirectorInput<'_>) -> String {
    if let Some(location) = &input.state.location_id { return format!("当前地点：{}", location); }
    if let Some(cfg) = input.director_cfg() {
        if let Some(summary) = &cfg.place_summary_fallback { return summary.clone(); }
    }
    "当前场景需要先转化为可行动局势。".into()
}

fn default_guidance_level() -> GuidanceLevel {
    match std::env::var("TRPG_DIRECTOR_DEFAULT_GUIDANCE_LEVEL").unwrap_or_else(|_| "ask_goal".into()).as_str() {
        "observable_facts_only" => GuidanceLevel::ObservableFactsOnly,
        "summarize_known_info" => GuidanceLevel::SummarizeKnownInfo,
        "offer_action_categories" => GuidanceLevel::OfferActionCategories,
        "offer_costed_examples" => GuidanceLevel::OfferCostedExamples,
        _ => GuidanceLevel::AskGoal,
    }
}
fn contains_any(t: &str, terms: &[&str]) -> bool { terms.iter().any(|term| t.contains(term)) }
fn env_bool(key: &str, default: bool) -> bool { std::env::var(key).ok().map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")).unwrap_or(default) }
fn env_usize(key: &str, default: usize) -> usize { std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default) }
fn id(prefix: &str) -> String { format!("{}_{}", prefix, Uuid::new_v4().simple()) }

#[cfg(test)]
mod director_dehardcode_tests {
    use super::*;

    fn minimal_request() -> ContextRequest {
        ContextRequest {
            ruleset_id: "cyberpunk_red".into(),
            module_id: Some("cyberpunk_red.homecoming".into()),
            session_id: "s1".into(),
            turn_id: "t1".into(),
            viewer: VisibilityProfile::gm(),
            token_budget: TokenBudget::default(),
        }
    }

    fn minimal_state() -> RuntimeState {
        RuntimeState {
            ruleset_id: "cyberpunk_red".into(),
            module_id: Some("cyberpunk_red.homecoming".into()),
            ..Default::default()
        }
    }

    fn compiled() -> CompiledContext { CompiledContext::default() }

    /// EXACT pre-migration hardcoded Homecoming director values as DATA — the
    /// equivalence baseline. This is what the deleted is_homecoming() block
    /// emitted; the override JSON `cyberpunk_red.homecoming.module_config.json`
    /// must carry the same values so the Homecoming run is unchanged.
    fn legacy_homecoming_director_config() -> DirectorModuleConfig {
        DirectorModuleConfig {
            scene_facts: vec![DirectorSceneFact {
                text: "无人机、警察、仓库入口、外露电缆和内部服务器噪音形成同一个局势面：威胁、救援、技术源头和情报价值同时存在。".into(),
                source: "module_override.homecoming".into(),
            }],
            pressure_items: vec![DirectorPressureItem {
                text: "如果继续拖延，现场伤员、外部势力和设备过载都会推进局势。".into(),
                clock_id: Some("clock.homecoming_scene_pressure".into()),
                severity: 70,
                consequence_hint: Some("警察伤势恶化、敌对势力抵达、仓库内源头转移或过载".into()),
            }],
            affordance_items: vec![
                DirectorAffordanceItem { description: "外露电缆是可观察的交互抓手；它暗示供能、数据或控制关系。".into(), implies_vectors: vec!["technical".into(), "tactical".into(), "observe".into()] },
                DirectorAffordanceItem { description: "受困警察和封锁街口是救援、社交和现场资源的抓手。".into(), implies_vectors: vec!["social".into(), "resource".into(), "tactical".into()] },
                DirectorAffordanceItem { description: "仓库内部的噪音与外部威胁同步，说明源头可能不在无人机本体。".into(), implies_vectors: vec!["observe".into(), "technical".into(), "stealth".into()] },
            ],
            risk_items: vec![DirectorRiskItem {
                text: "直接摧毁威胁最简单，但可能损失情报、报酬或后续线索。".into(),
                related_vectors: vec!["tactical".into()],
                severity: 60,
            }],
            npc_advice: vec![
                NpcBiasedAdvice { npc_id: "injured_lawman".into(), speaker_label: "受伤警察".into(), advice_text: "别靠近它，把火力压住！".into(), bias_or_goal: "想活下来，优先压制威胁，不关心情报价值。".into(), not_official_solution: true },
                NpcBiasedAdvice { npc_id: "fixer_contact".into(), speaker_label: "你的联系人".into(), advice_text: "别把值钱的情报打烂，查清它从哪来的。".into(), bias_or_goal: "想要可出售的信息或技术，低估现场救援压力。".into(), not_official_solution: true },
            ],
            known_facts: vec!["现场至少不是单一战斗问题：它同时是救援、威胁控制、源头调查和资源取舍。".into()],
            open_question: Some("你们优先救人、控制威胁、追查源头、获取资源，还是撤离保命？".into()),
            open_question_points_to: vec!["rescue".into(), "control".into(), "source".into(), "loot".into(), "retreat".into()],
            place_summary_fallback: Some("当前地点：Cyberpunk RED Homecoming 开场附近；一个高压现场正在等待玩家选择目标。".into()),
        }
    }

    fn wrap(cfg: DirectorModuleConfig) -> ModuleConfig {
        ModuleConfig { director: Some(cfg), ..Default::default() }
    }

    // T1: module_config 携带 scene_facts → brief.visible_facts 含该文本
    #[test]
    fn scene_facts_from_module_config() {
        let cfg = wrap(DirectorModuleConfig {
            scene_facts: vec![DirectorSceneFact { text: "外露电缆是可观察的交互抓手".into(), source: "module_override".into() }],
            ..Default::default()
        });
        let req = minimal_request();
        let state = minimal_state();
        let input = DirectorInput { request: &req, state: &state, compiled: &compiled(), user_input: "我不知道能做什么", conflict: None, module_config: Some(&cfg) };
        let brief = build_brief(input, None, GuidanceLevel::AskGoal);
        assert!(brief.visible_facts.iter().any(|f| f.text.contains("外露电缆")), "scene_facts from module_config must appear in visible_facts");
    }

    // T2: module_config=None → 不 panic，返回通用 brief，无规则集/模组名字面量泄漏
    #[test]
    fn no_module_config_returns_generic_brief() {
        let req = minimal_request();
        let state = minimal_state();
        let input = DirectorInput { request: &req, state: &state, compiled: &compiled(), user_input: "怎么办", conflict: None, module_config: None };
        let brief = build_brief(input, None, GuidanceLevel::AskGoal);
        assert!(!brief.visible_facts.is_empty(), "generic brief must have >= 1 visible_fact");
        for f in &brief.known_facts {
            assert!(!f.source.contains("homecoming") && !f.source.contains("cyberpunk"), "known_fact.source must not contain hardcoded ruleset/module name, got: {}", f.source);
        }
        // 通用 NPC advice 不含 homecoming-specific npc
        let advice = biased_npc_advice(input);
        assert!(advice.iter().all(|a| a.npc_id != "injured_lawman" && a.npc_id != "fixer_contact"));
        // 通用 place summary 不泄漏模组名
        assert!(!current_place_summary(input).contains("Homecoming"));
    }

    // T3: npc_advice 来自 module_config → biased_npc_advice 返回配置 NPC
    #[test]
    fn npc_advice_from_module_config() {
        let cfg = wrap(DirectorModuleConfig {
            npc_advice: vec![NpcBiasedAdvice { npc_id: "injured_lawman".into(), speaker_label: "受伤警察".into(), advice_text: "别靠近它，把火力压住！".into(), bias_or_goal: "想活下来".into(), not_official_solution: true }],
            ..Default::default()
        });
        let req = minimal_request();
        let state = minimal_state();
        let input = DirectorInput { request: &req, state: &state, compiled: &compiled(), user_input: "怎么看", conflict: None, module_config: Some(&cfg) };
        let advice = biased_npc_advice(input);
        assert_eq!(advice.len(), 1);
        assert_eq!(advice[0].npc_id, "injured_lawman");
    }

    // 等价铁律：数据驱动的 Homecoming brief 必须复刻原 is_homecoming 硬编码的全部文本/数值。
    #[test]
    fn homecoming_data_matches_legacy_hardcode() {
        let cfg = wrap(legacy_homecoming_director_config());
        let req = minimal_request();
        let state = minimal_state();
        let input = DirectorInput { request: &req, state: &state, compiled: &compiled(), user_input: "我不知道能做什么", conflict: None, module_config: Some(&cfg) };
        let brief = build_brief(input, None, GuidanceLevel::AskGoal);

        // visible_facts: 原硬编码场景面
        assert!(brief.visible_facts.iter().any(|f| f.text.contains("外露电缆和内部服务器噪音形成同一个局势面")));
        // pressure: clock_id / severity / 文本
        let pi = brief.pressure.iter().find(|p| p.clock_id.as_deref() == Some("clock.homecoming_scene_pressure")).expect("homecoming pressure present");
        assert_eq!(pi.severity, 70);
        assert!(pi.consequence_hint.as_deref().unwrap().contains("源头转移或过载"));
        // affordances: 三条 + 精确向量组合（电缆 = technical/tactical/observe）
        let cable = brief.affordances.iter().find(|a| a.description.contains("外露电缆是可观察的交互抓手")).expect("cable affordance present");
        assert_eq!(cable.implies_vectors, vec![ActionVector::Technical, ActionVector::Tactical, ActionVector::Observe]);
        let police = brief.affordances.iter().find(|a| a.description.contains("受困警察和封锁街口")).expect("police affordance present");
        assert_eq!(police.implies_vectors, vec![ActionVector::Social, ActionVector::Resource, ActionVector::Tactical]);
        let noise = brief.affordances.iter().find(|a| a.description.contains("仓库内部的噪音与外部威胁同步")).expect("noise affordance present");
        assert_eq!(noise.implies_vectors, vec![ActionVector::Observe, ActionVector::Technical, ActionVector::Stealth]);
        // risk: severity 60 + 向量 tactical
        let risk = brief.risks.iter().find(|r| r.text.contains("直接摧毁威胁最简单")).expect("homecoming risk present");
        assert_eq!(risk.severity, 60);
        assert_eq!(risk.related_vectors, vec![ActionVector::Tactical]);
        // known_fact + open_question
        assert!(brief.known_facts.iter().any(|k| k.text.contains("它同时是救援、威胁控制、源头调查和资源取舍")));
        let oq = brief.open_questions.iter().find(|q| q.text.contains("你们优先救人、控制威胁")).expect("homecoming open_question present");
        assert_eq!(oq.points_to, vec!["rescue", "control", "source", "loot", "retreat"]);
        // npc_advice = injured_lawman + fixer_contact
        assert_eq!(brief.npc_advice.len(), 2);
        assert_eq!(brief.npc_advice[0].npc_id, "injured_lawman");
        assert_eq!(brief.npc_advice[1].npc_id, "fixer_contact");
        // place summary fallback (no location_id)
        assert_eq!(current_place_summary(input), "当前地点：Cyberpunk RED Homecoming 开场附近；一个高压现场正在等待玩家选择目标。");
    }

    // 验证落盘的 override JSON 与 legacy 基线逐字节等价（防数据文件漂移）。
    #[test]
    fn override_json_file_matches_legacy_baseline() {
        let dir = std::env::var("TRPG_DATA_DIR").unwrap_or_else(|_| {
            format!("{}/../../data", env!("CARGO_MANIFEST_DIR"))
        });
        let p = std::path::Path::new(&dir).join("modules").join("cyberpunk_red.homecoming.module_config.json");
        let text = match std::fs::read_to_string(&p) {
            Ok(t) => t,
            Err(_) => return, // data symlink absent in this checkout → skip (fail-soft)
        };
        let mc: ModuleConfig = serde_json::from_str(&text).expect("override JSON parses as ModuleConfig");
        let cfg = mc.director.expect("override carries director config");
        assert_eq!(cfg, legacy_homecoming_director_config(), "on-disk override must match the legacy is_homecoming baseline");
    }
}
