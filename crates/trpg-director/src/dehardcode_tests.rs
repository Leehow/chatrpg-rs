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

fn compiled() -> CompiledContext {
    CompiledContext::default()
}

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
        narrative_anchors: Vec::new(),
    }
}

fn wrap(cfg: DirectorModuleConfig) -> ModuleConfig {
    ModuleConfig {
        director: Some(cfg),
        ..Default::default()
    }
}

// T1: module_config 携带 scene_facts → brief.visible_facts 含该文本
#[test]
fn scene_facts_from_module_config() {
    let cfg = wrap(DirectorModuleConfig {
        scene_facts: vec![DirectorSceneFact {
            text: "外露电缆是可观察的交互抓手".into(),
            source: "module_override".into(),
        }],
        ..Default::default()
    });
    let req = minimal_request();
    let state = minimal_state();
    let input = DirectorInput {
        request: &req,
        state: &state,
        compiled: &compiled(),
        user_input: "我不知道能做什么",
        conflict: None,
        module_config: Some(&cfg),
        participants: &[],
        prior_spotlights: &[],
        leverage_npc_ids: None,
    };
    let brief = build_brief(input, None, GuidanceLevel::AskGoal);
    assert!(
        brief
            .visible_facts
            .iter()
            .any(|f| f.text.contains("外露电缆")),
        "scene_facts from module_config must appear in visible_facts"
    );
}

// T2: module_config=None → 不 panic，返回通用 brief，无规则集/模组名字面量泄漏
#[test]
fn no_module_config_returns_generic_brief() {
    let req = minimal_request();
    let state = minimal_state();
    let input = DirectorInput {
        request: &req,
        state: &state,
        compiled: &compiled(),
        user_input: "怎么办",
        conflict: None,
        module_config: None,
        participants: &[],
        prior_spotlights: &[],
        leverage_npc_ids: None,
    };
    let brief = build_brief(input, None, GuidanceLevel::AskGoal);
    assert!(
        !brief.visible_facts.is_empty(),
        "generic brief must have >= 1 visible_fact"
    );
    for f in &brief.known_facts {
        assert!(
            !f.source.contains("homecoming") && !f.source.contains("cyberpunk"),
            "known_fact.source must not contain hardcoded ruleset/module name, got: {}",
            f.source
        );
    }
    // 通用 NPC advice 不含 homecoming-specific npc
    let advice = biased_npc_advice(input);
    assert!(advice
        .iter()
        .all(|a| a.npc_id != "injured_lawman" && a.npc_id != "fixer_contact"));
    // 通用 place summary 不泄漏模组名
    assert!(!current_place_summary(input).contains("Homecoming"));
}

// T3: npc_advice 来自 module_config → biased_npc_advice 返回配置 NPC
#[test]
fn npc_advice_from_module_config() {
    let cfg = wrap(DirectorModuleConfig {
        npc_advice: vec![NpcBiasedAdvice {
            npc_id: "injured_lawman".into(),
            speaker_label: "受伤警察".into(),
            advice_text: "别靠近它，把火力压住！".into(),
            bias_or_goal: "想活下来".into(),
            not_official_solution: true,
        }],
        ..Default::default()
    });
    let req = minimal_request();
    let state = minimal_state();
    let input = DirectorInput {
        request: &req,
        state: &state,
        compiled: &compiled(),
        user_input: "怎么看",
        conflict: None,
        module_config: Some(&cfg),
        participants: &[],
        prior_spotlights: &[],
        leverage_npc_ids: None,
    };
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
    let input = DirectorInput {
        request: &req,
        state: &state,
        compiled: &compiled(),
        user_input: "我不知道能做什么",
        conflict: None,
        module_config: Some(&cfg),
        participants: &[],
        prior_spotlights: &[],
        leverage_npc_ids: None,
    };
    let brief = build_brief(input, None, GuidanceLevel::AskGoal);

    // visible_facts: 原硬编码场景面
    assert!(brief
        .visible_facts
        .iter()
        .any(|f| f.text.contains("外露电缆和内部服务器噪音形成同一个局势面")));
    // pressure: clock_id / severity / 文本
    let pi = brief
        .pressure
        .iter()
        .find(|p| p.clock_id.as_deref() == Some("clock.homecoming_scene_pressure"))
        .expect("homecoming pressure present");
    assert_eq!(pi.severity, 70);
    assert!(pi
        .consequence_hint
        .as_deref()
        .unwrap()
        .contains("源头转移或过载"));
    // affordances: 三条 + 精确向量组合（电缆 = technical/tactical/observe）
    let cable = brief
        .affordances
        .iter()
        .find(|a| a.description.contains("外露电缆是可观察的交互抓手"))
        .expect("cable affordance present");
    assert_eq!(
        cable.implies_vectors,
        vec![
            ActionVector::Technical,
            ActionVector::Tactical,
            ActionVector::Observe
        ]
    );
    let police = brief
        .affordances
        .iter()
        .find(|a| a.description.contains("受困警察和封锁街口"))
        .expect("police affordance present");
    assert_eq!(
        police.implies_vectors,
        vec![
            ActionVector::Social,
            ActionVector::Resource,
            ActionVector::Tactical
        ]
    );
    let noise = brief
        .affordances
        .iter()
        .find(|a| a.description.contains("仓库内部的噪音与外部威胁同步"))
        .expect("noise affordance present");
    assert_eq!(
        noise.implies_vectors,
        vec![
            ActionVector::Observe,
            ActionVector::Technical,
            ActionVector::Stealth
        ]
    );
    // risk: severity 60 + 向量 tactical
    let risk = brief
        .risks
        .iter()
        .find(|r| r.text.contains("直接摧毁威胁最简单"))
        .expect("homecoming risk present");
    assert_eq!(risk.severity, 60);
    assert_eq!(risk.related_vectors, vec![ActionVector::Tactical]);
    // known_fact + open_question
    assert!(brief.known_facts.iter().any(|k| k
        .text
        .contains("它同时是救援、威胁控制、源头调查和资源取舍")));
    let oq = brief
        .open_questions
        .iter()
        .find(|q| q.text.contains("你们优先救人、控制威胁"))
        .expect("homecoming open_question present");
    assert_eq!(
        oq.points_to,
        vec!["rescue", "control", "source", "loot", "retreat"]
    );
    // npc_advice = injured_lawman + fixer_contact
    assert_eq!(brief.npc_advice.len(), 2);
    assert_eq!(brief.npc_advice[0].npc_id, "injured_lawman");
    assert_eq!(brief.npc_advice[1].npc_id, "fixer_contact");
    // place summary fallback (no location_id)
    assert_eq!(
        current_place_summary(input),
        "当前地点：Cyberpunk RED Homecoming 开场附近；一个高压现场正在等待玩家选择目标。"
    );
}

// 验证落盘的 override JSON 与 legacy 基线逐字节等价（防数据文件漂移）。
#[test]
fn override_json_file_matches_legacy_baseline() {
    let dir = std::env::var("TRPG_DATA_DIR")
        .unwrap_or_else(|_| format!("{}/../../data", env!("CARGO_MANIFEST_DIR")));
    let p = std::path::Path::new(&dir)
        .join("modules")
        .join("cyberpunk_red.homecoming.module_config.json");
    let text = match std::fs::read_to_string(&p) {
        Ok(t) => t,
        Err(_) => return, // data symlink absent in this checkout → skip (fail-soft)
    };
    let mc: ModuleConfig =
        serde_json::from_str(&text).expect("override JSON parses as ModuleConfig");
    let cfg = mc.director.expect("override carries director config");
    assert_eq!(
        cfg,
        legacy_homecoming_director_config(),
        "on-disk override must match the legacy is_homecoming baseline"
    );
}

// ===== MAT.M4 (§7-#6): present vs met/engaged — Director leverage gate =====

fn state_with_active(active: &[&str]) -> RuntimeState {
    RuntimeState {
        ruleset_id: "generic".into(),
        active_npc_ids: active.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

fn input_with<'a>(
    req: &'a ContextRequest,
    state: &'a RuntimeState,
    compiled: &'a CompiledContext,
    leverage: Option<&'a [String]>,
) -> DirectorInput<'a> {
    DirectorInput {
        request: req,
        state,
        compiled,
        user_input: "look around",
        conflict: None,
        module_config: None,
        participants: &[],
        prior_spotlights: &[],
        leverage_npc_ids: leverage,
    }
}

// TDD #2: an active-but-UN-MET NPC is NOT used as proactive Director leverage; a MET one is.
#[test]
fn m4_director_leverage_excludes_unmet_npc_when_gated() {
    let req = minimal_request();
    let state = state_with_active(&["npc_met", "npc_unmet"]);
    let compiled = compiled();
    // Enforce path: runtime supplies only the met subset as leverage.
    let met: Vec<String> = vec!["npc_met".to_string()];
    let input = input_with(&req, &state, &compiled, Some(&met));
    let facts = base_visible_facts(input, None);
    let active_fact = facts
        .iter()
        .find(|f| f.text.contains("当前活跃 NPC"))
        .expect("active-NPC visible fact present");
    assert!(
        active_fact.text.contains("npc_met"),
        "MET NPC may be proactive leverage"
    );
    assert!(
        !active_fact.text.contains("npc_unmet"),
        "un-met NPC must NOT be pushed as proactive Director leverage (rule 6)"
    );
}

// TDD #3: None (Off/Shadow baseline) uses the FULL active set exactly as before.
#[test]
fn m4_director_leverage_none_is_baseline_full_active_set() {
    let req = minimal_request();
    let state = state_with_active(&["npc_met", "npc_unmet"]);
    let compiled = compiled();
    let input = input_with(&req, &state, &compiled, None);
    let facts = base_visible_facts(input, None);
    let active_fact = facts
        .iter()
        .find(|f| f.text.contains("当前活跃 NPC"))
        .expect("active-NPC visible fact present");
    // Byte-identical baseline: both ids appear, order preserved (= state.active_npc_ids).
    assert!(active_fact.text.contains("npc_met, npc_unmet"));
}

// All-met gated set behaves identically to baseline (no exclusion when everyone is met).
#[test]
fn m4_all_met_leverage_matches_full_active_set() {
    let req = minimal_request();
    let state = state_with_active(&["npc_a", "npc_b"]);
    let compiled = compiled();
    let full: Vec<String> = vec!["npc_a".to_string(), "npc_b".to_string()];
    let input = input_with(&req, &state, &compiled, Some(&full));
    let facts = base_visible_facts(input, None);
    let active_fact = facts
        .iter()
        .find(|f| f.text.contains("当前活跃 NPC"))
        .expect("active-NPC visible fact present");
    assert!(active_fact.text.contains("npc_a, npc_b"));
}
