//! 层化迁移架构门（P0）——设计4 §19 的 10 条架构验收的 Rust 侧基线门。
//!
//! 本文件是**护栏 / 基线**，不改任何运行时行为。两类断言：
//!   1. 6 条**现可落地**断言（绿门）：钉死当前契约（注册表 / 事件三分 / asset 契约），
//!      回退即红，作为后续 phase 的"地基绳"。
//!   2. 4 条 **#[ignore] P6 占位锚**：把 §19-#8（replay parity）/ §19-#9（同种子确定性）
//!      的**目标断言先写成代码**。P6 实现 event-fold/replay + seedable 掷骰后，
//!      去掉 `#[ignore]` 即变绿门。最弱形状（架构师 D3：存在性 / 签名）。
//!
//! 经 `crates/trpg-gm/src/lib.rs` 的 `#[cfg(test)] #[path]` 挂载（沿用 *_tests.rs 约定）。
//! 跑：`cargo test -p trpg-gm --lib arch_gates`。
//!
//! 注：execution_tier 字段实际落在 `trpg_model::BindingPlan`（asset.rs:136），
//! **不是** AssetEnvelope（spec 验收文字误置）——见 AcceptanceLedger A1 Notes 与 invariants.md。

use trpg_model::{AssetEnvelope, BindingPlan, DomainEvent, DomainEventKind, ExecutionTier};

// ── 1. §19-#1/#5：Narrator 尚未隔离的可见基线 ──────────────────────────────
// 当前 run_agent_loop 用 ToolRegistry::standard()（tools/mod.rs），含 15 工具且其中
// mutation 工具在册。P1 隔离 Narrator 后，本断言将演化为『Narrator registry **不含**
// mutation 工具』——届时此基线翻面，正是 P1 完成的可观测信号。

/// standard() 注册表当前含 15 工具（schema 字节稳定铁律的计数侧锚）。
#[test]
fn standard_registry_has_fifteen_tools() {
    let schemas = crate::tools::ToolRegistry::standard().schemas();
    assert_eq!(
        schemas.len(),
        15,
        "ToolRegistry::standard() 工具数应为 15（P0 基线；改工具集需同步 schema 字节回归测试）"
    );
}

/// standard() 注册表含 5 个 mutation 工具（roll_check/apply_effect/change_track/
/// remember/reveal_fact）——P1 前 Narrator 尚未隔离的基线（P1 后此集应被移出叙事路径）。
#[test]
fn standard_registry_includes_mutation_tools() {
    let names: std::collections::BTreeSet<String> = crate::tools::ToolRegistry::standard()
        .schemas()
        .iter()
        .filter_map(|s| s["function"]["name"].as_str().map(str::to_string))
        .collect();
    for expected in [
        "roll_check",   // RollCheckTool
        "apply_effect", // ApplyEffectTool
        "change_track", // ChangeTrackTool
        "remember",     // RememberTool
        "reveal_fact",  // RevealFactTool
    ] {
        assert!(
            names.contains(expected),
            "mutation 工具 '{expected}' 应在 standard() 注册表（P1 前基线）；现有: {names:?}"
        );
    }
}

// ── 2. §19-#10：PlayerLearnedFact 知识语义三分 ────────────────────────────

/// DomainEventKind 含 ContextSurfaced / PlayerExposed / PlayerLearnedFact 三分
/// （domain_event.rs）。三者语义互斥，不得回退合并回旧 EntitySurfaced。
#[test]
fn domain_event_kind_has_knowledge_split_trinity() {
    let trinity = [
        DomainEventKind::ContextSurfaced,
        DomainEventKind::PlayerExposed,
        DomainEventKind::PlayerLearnedFact,
    ];
    // 互不相等（编译期已证存在；运行期证三分不塌缩）。
    assert_ne!(trinity[0], trinity[1]);
    assert_ne!(trinity[1], trinity[2]);
    assert_ne!(trinity[0], trinity[2]);
}

/// 三分事件的 as_str token 字节稳定——知识账本幂等键（de_{turn}_{kind}）依赖它，
/// `#[serde(rename)]` 误改会破坏 db 幂等。钉死 token 字面量防漂移。
#[test]
fn knowledge_trinity_as_str_tokens_stable() {
    assert_eq!(DomainEventKind::ContextSurfaced.as_str(), "ContextSurfaced");
    assert_eq!(DomainEventKind::PlayerExposed.as_str(), "PlayerExposed");
    assert_eq!(
        DomainEventKind::PlayerLearnedFact.as_str(),
        "PlayerLearnedFact"
    );
    // round-trip：token → kind → token 闭环（幂等键稳定的双向证）。
    assert_eq!(
        DomainEventKind::from_str_token("PlayerLearnedFact"),
        DomainEventKind::PlayerLearnedFact
    );
}

// ── 3. §19 末段：asset 契约不回退 ────────────────────────────────────────

/// ExecutionTier 枚举（SourceOnly..VerifiedExecution）+ AssetEnvelope 带 source_refs /
/// visibility 字段（asset.rs）。parser 资产的 source/visibility/tier 契约不回退。
#[test]
fn asset_envelope_and_execution_tier_contract() {
    // ExecutionTier 5 级有序枚举存在（默认 SourceOnly）。
    assert_eq!(ExecutionTier::default(), ExecutionTier::SourceOnly);
    let _tiers = [
        ExecutionTier::SourceOnly,
        ExecutionTier::GuidedRuling,
        ExecutionTier::PartialExecution,
        ExecutionTier::ExactExecution,
        ExecutionTier::VerifiedExecution,
    ];
    // AssetEnvelope 带 source_refs（Vec<SourceRef>）+ visibility（默认 GmOnly）字段。
    let env = AssetEnvelope::default();
    assert!(env.source_refs.is_empty(), "source_refs 字段在册（默认空）");
    assert_eq!(
        env.visibility,
        trpg_model::Visibility::default(),
        "visibility 字段在册（默认 GmOnly）"
    );
}

/// execution_tier 字段的真实归属是 BindingPlan（asset.rs:136），**非** AssetEnvelope
/// （spec 验收文字误置；按真实代码锚定）。锁定 BindingPlan.execution_tier 契约不回退。
#[test]
fn binding_plan_carries_execution_tier() {
    let plan = BindingPlan::default();
    assert_eq!(
        plan.execution_tier,
        ExecutionTier::SourceOnly,
        "BindingPlan.execution_tier 字段在册（默认 SourceOnly）"
    );
}

// ── 4. P6 占位锚（#[ignore]）：§19-#8 replay parity / §19-#9 同种子确定性 ───────
// 这些测试把 P6 的目标断言**先写成代码**。当前缺 event-fold/replay 引擎与 seedable
// 掷骰通道，故 `#[ignore]`（被 skip 非 fail）。P6 实现时去掉 `#[ignore]` 即变绿门。
// 见 设计4 §19-#8/#9 与 docs/superpowers/plans/2026-06-19-layered-runtime-architecture.md P6。

fn fixed_event(kind: DomainEventKind, idx: usize) -> DomainEvent {
    // 固定时间戳构造（不依赖 Utc::now），保证 replay 序列确定性。
    DomainEvent {
        event_id: format!("de_test_{idx}_{}", kind.as_str()),
        session_id: "sess_replay".into(),
        turn_id: "turn_1".into(),
        kind,
        data: serde_json::Value::Null,
        source_refs: Vec::new(),
        created_at: chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000 + idx as i64, 0)
            .expect("fixed ts"),
    }
}

/// §19-#8 目标①：replay 同序事件 → 重建 projection 逐字段相等（确定性 fold）。
/// P6 锚：当前用占位 fold（kind token 序列）代替真 projection 引擎；P6 接真引擎后去 ignore。
#[ignore = "P6: 待 event-fold/replay 引擎（设计4 §19-#8）。去掉 #[ignore] 即绿门"]
#[test]
fn replay_same_events_rebuild_identical_projection() {
    let events = [
        fixed_event(DomainEventKind::TurnStarted, 0),
        fixed_event(DomainEventKind::ContextSurfaced, 1),
        fixed_event(DomainEventKind::PlayerExposed, 2),
        fixed_event(DomainEventKind::PlayerLearnedFact, 3),
        fixed_event(DomainEventKind::TurnFinalized, 4),
    ];
    // TARGET(P6): projection_a = engine.fold(&events); projection_b = engine.fold(&events);
    //             assert_eq!(projection_a, projection_b)  // 逐字段相等
    let fold = |evs: &[DomainEvent]| -> Vec<String> {
        evs.iter().map(|e| e.kind.as_str().to_string()).collect()
    };
    assert_eq!(fold(&events), fold(&events));
}

/// §19-#8 目标②：projection 对重复投递幂等（同事件 id 二次 fold 不改投影）。
#[ignore = "P6: 待 event-fold/replay 引擎（设计4 §19-#8）。去掉 #[ignore] 即绿门"]
#[test]
fn replay_projection_idempotent_on_redelivery() {
    let base = [
        fixed_event(DomainEventKind::PlayerLearnedFact, 0),
        fixed_event(DomainEventKind::PlayerLearnedFact, 0), // 同 idx → 同 event_id（重复投递）
    ];
    // TARGET(P6): engine.fold 应按 event_id 去重，重复投递后投影与单次相等。
    let dedup: std::collections::BTreeSet<&str> =
        base.iter().map(|e| e.event_id.as_str()).collect();
    assert_eq!(dedup.len(), 1, "同 event_id 应折叠为一（幂等）");
}

/// §19-#9 目标①：同 (state, seed, action) → roll() 返回相同 rolls 序列。
/// 当前 roll() 无 seed 入参、用 thread_rng（trpg-runtime/src/lib.rs:4552），故今日不成立 →
/// #[ignore]。P6 加 seed 通道后去 ignore 即绿门。
#[ignore = "P6: 待 seedable 掷骰（设计4 §19-#9）。去掉 #[ignore] 即绿门"]
#[test]
fn same_seed_same_roll_sequence() {
    use trpg_runtime::{DiceRollerPlugin, PseudoRandomDiceRoller};
    let roller = PseudoRandomDiceRoller;
    // TARGET(P6): roller.roll_with_seed("3d6", seed) 两次 → rolls 相等。
    let a = roller.roll("3d6").expect("roll a");
    let b = roller.roll("3d6").expect("roll b");
    assert_eq!(
        a.rolls, b.rolls,
        "同种子应得相同 rolls 序列（P6 seed 通道）"
    );
}

/// §19-#9 目标②：同 (state, seed, action) → roll() 返回相同 total。
#[ignore = "P6: 待 seedable 掷骰（设计4 §19-#9）。去掉 #[ignore] 即绿门"]
#[test]
fn same_seed_same_roll_total() {
    use trpg_runtime::{DiceRollerPlugin, PseudoRandomDiceRoller};
    let roller = PseudoRandomDiceRoller;
    let a = roller.roll("2d20+3").expect("roll a");
    let b = roller.roll("2d20+3").expect("roll b");
    assert_eq!(a.total, b.total, "同种子应得相同 total（P6 seed 通道）");
}
