//! P1-3 live DB 实测:在真实 homecoming 解析图上派生运行时 ProgressionProgram +
//! AdvancementFrontier。证明 J3 根因(运行时缺进度维度)被真实补上 —— `program_from_
//! module_graph`(live nav 走的同一函数)从真模组已授权 flow-link ridge 产出可执行
//! ProgressRule,引擎消费后 frontier **非空**(Director 可选焦的合法下一拍),全程零
//! ruleset 分支、trigger/branch 未标准化条件 fail-closed 存 opaque 永不自动 fire。
//!
//! Run (rulesets DB :54347):
//!   DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!     CARGO_TARGET_DIR=target-air cargo test -p trpg-runtime \
//!     --test live_progression_program -- --nocapture
//!
//! 无 `DATABASE_URL` ⇒ SKIP(fail-closed,不阻塞 CI)。**反假绿**:真跑必打印 `RAN:`
//! + 计数 + `PASS`;只见 `SKIP` = 没验证。
use trpg_db::Db;
use trpg_runtime::progression::{
    compute_frontier, derive_threat_objective, evaluate, program_from_module_graph, ProgressEvent,
    ProgressionState,
};

const HOMECOMING: &str = "cyberpunk_red.homecoming";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn homecoming_yields_nonempty_program_and_frontier() {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP: DATABASE_URL unset (need live rulesets DB :54347)");
            return;
        }
    };
    let db = Db::connect(&url).await.expect("connect live DB");
    let graph = db
        .load_module_graph(HOMECOMING)
        .await
        .expect("query ok")
        .expect("homecoming module graph present in parsed_bundles");

    let program = program_from_module_graph(&graph);
    let executable = program.rules.iter().filter(|r| r.may_autofire()).count();
    let opaque = program.rules.len() - executable;
    eprintln!(
        "RAN: scenes={}, program rules={} (executable={}, opaque/fail-closed={})",
        graph.scenes.len(),
        program.rules.len(),
        executable,
        opaque
    );
    for r in &program.rules {
        let note = r
            .source_evidence
            .first()
            .and_then(|s| s.note.as_deref())
            .unwrap_or("(no anchor)");
        eprintln!(
            "  rule {} | on={:?} | autofire={} | evidence={}",
            r.id,
            r.on,
            r.may_autofire(),
            note
        );
    }

    // 真模组已含 8 条已授权 ridge(4 sequential + 3 trigger + 1 branch);structural pass
    // ⇒ sequential→executable Activate 规则,trigger/branch→opaque(永不自动 fire)。
    assert!(
        executable >= 1,
        "homecoming 应至少产出 1 条可执行 sequential ridge 规则(实测 0 = 无运行时进度维度)"
    );
    // 每条规则都带真 anchor 证据(逐字摘自源文,禁造)。
    assert!(
        program.rules.iter().all(|r| !r.source_evidence.is_empty()),
        "每条 ridge 规则必须带 source_evidence(真 anchor)"
    );

    // 引擎消费 program:对每条可执行规则的 from 节点 seed Entered → 该规则激活 to 节点 ⇒
    // frontier 非空(这正是 J3 缺的『合法下一拍』)。取第一条可执行规则验证。
    let exec_rule = program
        .rules
        .iter()
        .find(|r| r.may_autofire())
        .expect("≥1 executable rule");
    let from = match &exec_rule.on {
        trpg_model::adventure_ir::EventPattern::Entered(id) => id.clone(),
        other => panic!("executable ridge rule 的 ON 应为 Entered,实际 {other:?}"),
    };
    let mut state = ProgressionState::default();
    let signals = evaluate(&mut state, &[ProgressEvent::Entered(from.clone())], &program.borrow());
    let frontier = compute_frontier(&state, &program.objectives, &program.trackers);
    eprintln!(
        "RAN: seed Entered({from}) → signals={} frontier.active_units={:?}",
        signals.len(),
        frontier.active_units
    );
    assert!(
        !frontier.is_empty(),
        "seed 当前场景后 frontier 必须非空(Director 有合法下一拍可选焦)"
    );
    assert!(
        !signals.is_empty(),
        "引擎应发出 ProgressSignal(至少 BeatActivated/LocationChanged)"
    );

    eprintln!("PASS: live homecoming program 非空 + 引擎 frontier 非空 + ProgressSignal 真发");
}

/// PL-1 (PHASE 2) live DB 实测:真 homecoming `module_config` 的 npc/tech matcher 词表
/// 派生出 scene_01 威胁目标 ⇒ frontier 在**入场即非空**(修复 scene_01 frontier 饥饿:
/// frontier_total=0 for t01–t14),且真活体 fact_id `encounter.hacking_server` 经
/// AnyFactMatches 对齐 ⇒ 目标 completed + 网关结局 RevelationUnlocked。零 ruleset 分支
/// (词表纯从 config 结构抽),fail-closed(无 config/空词表 ⇒ 无目标)。镜像 tiered.rs 接线。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn homecoming_threat_objective_ends_scene01_starvation() {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP: DATABASE_URL unset (need live rulesets DB :54347)");
            return;
        }
    };
    let db = Db::connect(&url).await.expect("connect live DB");
    let cfg = db
        .load_module_config(HOMECOMING)
        .await
        .expect("homecoming module_config present");

    // 与 tiered.rs 接线逐字一致:词表 = npc 绑定 matcher + 技术选项 matcher。
    let mut vocab: Vec<String> = Vec::new();
    for b in &cfg.npc_actor_bindings {
        vocab.extend(b.matcher.iter().cloned());
    }
    if let Some(rows) = &cfg.technical_option_table {
        for row in rows {
            vocab.extend(row.matcher.iter().cloned());
        }
    }
    eprintln!("RAN: module_config vocab tokens={} -> {:?}", vocab.len(), vocab);
    assert!(
        !vocab.is_empty(),
        "真 homecoming module_config 应含 npc/tech matcher 词表(实测空 = 配置缺失)"
    );

    let d = derive_threat_objective(
        "obj.neutralize_threat",
        &vocab,
        "rev.threat_outcome",
        "module_config: npc_actor_bindings + technical_option_table matchers",
    )
    .expect("非空词表派生威胁目标");
    assert!(
        !d.objective.source_evidence.is_empty() && !d.outcome_rule.source_evidence.is_empty(),
        "目标+结局规则都带真 anchor 证据(禁造)"
    );

    // 入场即非空:仅有威胁目标、无任何 fact 时,frontier.open_objectives 已含它。
    let objectives = vec![d.objective.clone()];
    let rules = vec![d.outcome_rule.clone()];
    let fresh = ProgressionState::default();
    let f0 = compute_frontier(&fresh, &objectives, &[]);
    eprintln!(
        "RAN: scene_01 入场 frontier.open_objectives={:?} (饥饿前 frontier_total={})",
        f0.open_objectives,
        f0.len()
    );
    assert!(
        f0.open_objectives.contains(&"obj.neutralize_threat".to_string()),
        "scene_01 入场 frontier 必须非空(开放威胁目标 = 饥饿修复)"
    );

    // 真活体 fact_id 经词表对齐 ⇒ 目标 completed + 网关结局 reveal。
    use trpg_model::adventure_ir::{IrValue, ProgressSignalKind};
    use trpg_runtime::progression::ProgressionProgram;
    let program = ProgressionProgram {
        rules: &rules,
        objectives: &objectives,
        trackers: &[],
    };
    let mut state = ProgressionState::default();
    let signals = evaluate(
        &mut state,
        &[ProgressEvent::WorldFactChanged {
            fact: "encounter.hacking_server".into(), // 真活体 homecoming fact_id
            value: IrValue::Bool(true),
        }],
        &program,
    );
    let completed = signals
        .iter()
        .any(|s| s.kind == ProgressSignalKind::ObjectiveCompleted && s.id == "obj.neutralize_threat");
    let revealed = signals
        .iter()
        .any(|s| s.kind == ProgressSignalKind::RevelationUnlocked && s.id == "rev.threat_outcome");
    eprintln!(
        "RAN: live fact `encounter.hacking_server` -> ObjectiveCompleted={completed} RevelationUnlocked={revealed}"
    );
    assert!(completed, "真活体 fact_id 应经 AnyFactMatches 对齐使目标 completed");
    assert!(revealed, "网关结局规则应在中和向量 fact 上 reveal coords");

    eprintln!("PASS: 真 module_config 词表 → scene_01 入场 frontier 非空 + 活体 fact 对齐目标完成");
}
