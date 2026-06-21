//! 声明式回合管线契约（R1）：回合表示为有序 phase 列表，execute_turn 解释它而非过程式写死。
//! 回合 phases 不随规则集变 → in-code 常量即足够（做成 config 文件是过度工程）。

/// phase 在管线中的执行段：确定性头部 / agent 工具轮主体 / 后置尾部。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseKind {
    Deterministic,
    AgentLoop,
    Postprocess,
}

/// 规范回合的 16 个 phase 标识。顺序即 CANONICAL_TURN_PLAN 顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseId {
    // —— 10 个确定性头部（含 1 个条件 Materialize，默认 OFF）——
    RecordPlayerAction,
    RefreshLiveDerived,
    Reconcile,
    Gate,
    StimulusPass,
    OpposedPrepass,
    ModeInference,
    DebtLoad,
    // 条件确定性头：default-OFF（TRPG_MATERIALIZE_PHASE_ENABLE）。flag 未设时 execute 头部循环
    // 整段跳过（不 push phases_run、不 dispatch）⇒ 与 15-phase 基线字节级一致；ON 时在
    // ContextAssembly 之前跑 per-turn 物化 producer，使同回合 demand 对 prepare_turn_context 可见。
    Materialize,
    ContextAssembly,
    // —— 1 个 agent 主体 ——
    AgentLoop,
    // —— 5 个后置尾部 ——
    VerifyAfterStream,
    Finalize,
    AuditLearning,
    SceneNavigate,
    CarryoverDebt,
}

/// 单个 phase 的声明：执行器按 id 调对应 phase_* 方法；conditional=true 表示按运行期条件可跳过。
#[derive(Debug, Clone, Copy)]
pub struct TurnPhasePlan {
    pub id: PhaseId,
    pub kind: PhaseKind,
    pub conditional: bool,
}

/// 规范回合管线：execute_turn 按此顺序解释。10 确定性头（含条件 Materialize）+ 1 agent 主体 + 5 后置尾。
/// conditional=true：Materialize（仅 TRPG_MATERIALIZE_PHASE_ENABLE 置真）/ SceneNavigate（仅
/// module_id.is_some()）/ CarryoverDebt（仅工具轮耗尽且有未决义务），余皆 false。
pub const CANONICAL_TURN_PLAN: &[TurnPhasePlan] = &[
    det(PhaseId::RecordPlayerAction),
    det(PhaseId::RefreshLiveDerived),
    det(PhaseId::Reconcile),
    det(PhaseId::Gate),
    det(PhaseId::StimulusPass),
    det(PhaseId::OpposedPrepass),
    det(PhaseId::ModeInference),
    det(PhaseId::DebtLoad),
    // 条件确定性头：default-OFF。位于 DebtLoad 与 ContextAssembly 之间，使同回合 producer 写入
    // 对该回合 prepare_turn_context 可见。flag OFF 时头部循环跳过 ⇒ 字节级等价 15-phase 基线。
    TurnPhasePlan {
        id: PhaseId::Materialize,
        kind: PhaseKind::Deterministic,
        conditional: true,
    },
    det(PhaseId::ContextAssembly),
    TurnPhasePlan {
        id: PhaseId::AgentLoop,
        kind: PhaseKind::AgentLoop,
        conditional: false,
    },
    post(PhaseId::VerifyAfterStream, false),
    post(PhaseId::Finalize, false),
    post(PhaseId::AuditLearning, false),
    post(PhaseId::SceneNavigate, true),
    post(PhaseId::CarryoverDebt, true),
];

const fn det(id: PhaseId) -> TurnPhasePlan {
    TurnPhasePlan {
        id,
        kind: PhaseKind::Deterministic,
        conditional: false,
    }
}
const fn post(id: PhaseId, conditional: bool) -> TurnPhasePlan {
    TurnPhasePlan {
        id,
        kind: PhaseKind::Postprocess,
        conditional,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_plan_has_sixteen_phases() {
        assert_eq!(CANONICAL_TURN_PLAN.len(), 16);
    }

    #[test]
    fn first_phase_is_record_player_action() {
        assert_eq!(CANONICAL_TURN_PLAN[0].id, PhaseId::RecordPlayerAction);
        assert_eq!(CANONICAL_TURN_PLAN[0].kind, PhaseKind::Deterministic);
    }

    #[test]
    fn head_ten_are_deterministic() {
        for p in &CANONICAL_TURN_PLAN[0..10] {
            assert_eq!(
                p.kind,
                PhaseKind::Deterministic,
                "head phase {:?} must be Deterministic",
                p.id
            );
        }
    }

    #[test]
    fn agent_loop_is_the_single_middle_body() {
        // 第 11 项（index 10）是唯一 AgentLoop body。
        assert_eq!(CANONICAL_TURN_PLAN[10].id, PhaseId::AgentLoop);
        assert_eq!(CANONICAL_TURN_PLAN[10].kind, PhaseKind::AgentLoop);
        let agent_count = CANONICAL_TURN_PLAN
            .iter()
            .filter(|p| p.kind == PhaseKind::AgentLoop)
            .count();
        assert_eq!(agent_count, 1, "exactly one AgentLoop body");
    }

    #[test]
    fn tail_five_are_postprocess() {
        for p in &CANONICAL_TURN_PLAN[11..16] {
            assert_eq!(
                p.kind,
                PhaseKind::Postprocess,
                "tail phase {:?} must be Postprocess",
                p.id
            );
        }
    }

    #[test]
    fn only_materialize_scene_navigate_and_carryover_debt_are_conditional() {
        for p in CANONICAL_TURN_PLAN {
            let expect_conditional = matches!(
                p.id,
                PhaseId::Materialize | PhaseId::SceneNavigate | PhaseId::CarryoverDebt
            );
            assert_eq!(
                p.conditional, expect_conditional,
                "phase {:?} conditional flag wrong",
                p.id
            );
        }
    }

    #[test]
    fn plan_order_matches_phase_id_declaration() {
        let ids: Vec<PhaseId> = CANONICAL_TURN_PLAN.iter().map(|p| p.id).collect();
        assert_eq!(
            ids,
            vec![
                PhaseId::RecordPlayerAction,
                PhaseId::RefreshLiveDerived,
                PhaseId::Reconcile,
                PhaseId::Gate,
                PhaseId::StimulusPass,
                PhaseId::OpposedPrepass,
                PhaseId::ModeInference,
                PhaseId::DebtLoad,
                PhaseId::Materialize,
                PhaseId::ContextAssembly,
                PhaseId::AgentLoop,
                PhaseId::VerifyAfterStream,
                PhaseId::Finalize,
                PhaseId::AuditLearning,
                PhaseId::SceneNavigate,
                PhaseId::CarryoverDebt,
            ]
        );
    }
}
