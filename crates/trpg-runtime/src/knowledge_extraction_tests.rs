//! R1 知识抽取 kernel 单测（纯函数，无 DB / 无 LLM）。覆盖 R1 验收项：
//! (1) C1 分档映射 witnessed/deceived/heard；(2) C2 原子配对（同 fact_id、同 evidence、
//! 两者皆 `.validated().is_ok()`，且无法经本 API 产出孤立 KnowledgeUpdate）；以及知识闸
//! 的 should-run / should-skip。
use super::*;
use trpg_model::{KnowledgeState, MemoryExtractionProposal, ProposalHolder};

fn sample_input() -> KnowledgePairInput {
    KnowledgePairInput {
        fact_id: "fact_dagger_is_poisoned".to_string(),
        subject: "dagger".to_string(),
        predicate: "is".to_string(),
        object: "poisoned".to_string(),
        summary: "The ceremonial dagger is poisoned.".to_string(),
        source_event_ids: vec!["de_turn_7_observe".to_string()],
        confidence: Some(0.9),
        turn_id: Some("turn_7".to_string()),
        reason: Some("witnessed firsthand".to_string()),
    }
}

// ---- C1 分档映射 ----

#[test]
fn c1_witnessed_grades_to_knows_true() {
    assert_eq!(
        KnowledgeLearningMode::Witnessed.grade(),
        KnowledgeState::KnowsTrue
    );
    assert_eq!(
        grade_knowledge_state(KnowledgeLearningMode::Witnessed),
        KnowledgeState::KnowsTrue
    );
}

#[test]
fn c1_deceived_grades_to_believes_false() {
    assert_eq!(
        KnowledgeLearningMode::Deceived.grade(),
        KnowledgeState::BelievesFalse
    );
    assert_eq!(
        grade_knowledge_state(KnowledgeLearningMode::Deceived),
        KnowledgeState::BelievesFalse
    );
}

#[test]
fn c1_heard_grades_to_heard_about() {
    assert_eq!(
        KnowledgeLearningMode::Heard.grade(),
        KnowledgeState::HeardAbout
    );
    assert_eq!(
        grade_knowledge_state(KnowledgeLearningMode::Heard),
        KnowledgeState::HeardAbout
    );
}

// ---- C2 原子配对 ----

#[test]
fn c2_builder_always_emits_both_world_fact_and_knowledge_update() {
    let pair = build_atomic_knowledge_pair(
        sample_input(),
        ProposalHolder::player_party(),
        grade_knowledge_state(KnowledgeLearningMode::Witnessed),
    );
    assert_eq!(pair.len(), 2, "builder must emit exactly the two paired proposals");

    let kinds: Vec<&str> = pair.iter().map(|p| p.kind_token()).collect();
    assert!(kinds.contains(&"world_fact"), "missing paired WorldFact");
    assert!(
        kinds.contains(&"knowledge_update"),
        "missing paired KnowledgeUpdate"
    );
}

#[test]
fn c2_pair_shares_fact_id_and_evidence_and_both_validate() {
    let input = sample_input();
    let expected_fact_id = input.fact_id.clone();
    let expected_evidence = input.source_event_ids.clone();

    let pair = build_atomic_knowledge_pair(
        input,
        ProposalHolder::player_party(),
        grade_knowledge_state(KnowledgeLearningMode::Deceived),
    );

    let mut saw_fact = false;
    let mut saw_update = false;
    for p in &pair {
        // 每个候选都必须通过 fail-closed 校验。
        let validated = p.validated().expect("paired candidate must validate");
        assert_eq!(validated.evidence_refs(), expected_evidence);
        match p {
            MemoryExtractionProposal::WorldFact(c) => {
                saw_fact = true;
                assert_eq!(c.fact_id, expected_fact_id);
                assert_eq!(c.source_event_ids, expected_evidence);
            }
            MemoryExtractionProposal::KnowledgeUpdate(c) => {
                saw_update = true;
                assert_eq!(c.fact_id, expected_fact_id, "同 fact_id");
                assert_eq!(c.source_event_ids, expected_evidence, "同 evidence");
                // C1×C2：deceived 学习模式分档应落在 BelievesFalse。
                assert_eq!(c.knowledge_state, KnowledgeState::BelievesFalse);
            }
            _ => panic!("unexpected proposal kind in atomic pair"),
        }
    }
    assert!(saw_fact && saw_update, "原子配对必含 WorldFact 与 KnowledgeUpdate 两者");
}

/// C2 的 API 形态保证：唯一构造口返回 `Vec`，且总含其配对 WorldFact——无法经本 API 拿到
/// 孤立 KnowledgeUpdate。这里以「每个 update 必有同 fact_id 的 fact 与之配对」断言之。
#[test]
fn c2_cannot_produce_lone_knowledge_update() {
    let pair = build_atomic_knowledge_pair(
        sample_input(),
        ProposalHolder::player_party(),
        grade_knowledge_state(KnowledgeLearningMode::Heard),
    );
    let update_fact_ids: Vec<&str> = pair
        .iter()
        .filter_map(|p| match p {
            MemoryExtractionProposal::KnowledgeUpdate(c) => Some(c.fact_id.as_str()),
            _ => None,
        })
        .collect();
    for fid in update_fact_ids {
        let has_paired_fact = pair.iter().any(|p| {
            matches!(p, MemoryExtractionProposal::WorldFact(c) if c.fact_id == fid)
        });
        assert!(
            has_paired_fact,
            "every KnowledgeUpdate must ship with its paired WorldFact (same fact_id)"
        );
    }
}

// ---- 知识闸 ----

#[test]
fn gate_runs_when_new_entity_surfaced() {
    assert!(knowledge_gate_should_run(true, "", ""));
}

#[test]
fn gate_runs_on_social_signal_without_new_entity() {
    // 玩家审问 NPC：无新实体，但有明确获知/社交信号 → 开闸。
    assert!(knowledge_gate_should_run(
        false,
        "I interrogate the guard about the missing key.",
        ""
    ));
}

#[test]
fn gate_skips_plain_turn_without_signal_or_new_entity() {
    // 纯环境念白、无社交信号、无新实体 → 跳过（fail-closed 成本闸）。
    assert!(!knowledge_gate_should_run(
        false,
        "I walk north along the empty road.",
        "Wind rustles the dry grass."
    ));
}
