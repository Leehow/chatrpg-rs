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
    compute_frontier, evaluate, program_from_module_graph, ProgressEvent, ProgressionState,
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
