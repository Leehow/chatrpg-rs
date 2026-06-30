//! P2-3 live DB 实测:**The Vault 的一个任务目标驱动推进**。在真 the_vault 解析图上,
//! 用与 `tiered.rs` 引擎-ON 块逐字一致的接线(`program_from_module_graph` +
//! `augment_program_with_spine`,module_config 缺失 ⇒ 威胁目标 fail-closed 跳过)派生
//! 运行时程序,证明:
//!   1. the_vault 的 12 个任务-场景(node_type="mission")每个都获得一个 scene-scoped
//!      开放推进目标(=任务目标),frontier 全程非空(不像 DB baseline 的扁平 0-objective);
//!   2. 在第一个任务(Springs Eternal)入场后 frontier 非空(任务目标浮现 + 下一任务 active);
//!   3. 一个**任务接地**的 GM fact(其 fact_id 含该任务自身 authored 标题 token,运行时从
//!      加载的场景标题结构抽取,**非模组名硬码**)经 AnyFactMatches 对齐 ⇒ 该任务目标
//!      ObjectiveCompleted **真 fire** = 目标驱动推进在 Vault 任务集结构上工作。
//!
//! 这是 P2-3 的**确定性**机制证(Rust 评 guard、LLM 永不、零造);完整 LLM eval(j3v2 +
//! J1/J2/J4 失忆定级)是主管步(见 READY_FOR_FULL.sentinel)。计分(Commendation/Demerit)
//! 是 OpaqueAuthoredText 按设计永不自动 fire(护栏#2),与 Homecoming PL-6 同一 authored-prose
//! 墙,诚实留作 OOS plumbing —— 此处证的是**任务目标驱动转场推进**,非计分实播。
//!
//! Run (rulesets DB :54347):
//!   DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!     CARGO_TARGET_DIR=target-air cargo test -p trpg-runtime \
//!     --test live_vault_mission_progression -- --nocapture
//!
//! 无 `DATABASE_URL` ⇒ SKIP(fail-closed,不阻塞 CI)。**反假绿**:真跑必打印 `RAN:` +
//! 计数 + `PASS`;只见 `SKIP` = 没验证。
use trpg_db::Db;
use trpg_model::adventure_ir::{IrValue, ProgressSignalKind};
use trpg_runtime::progression::{
    augment_program_with_spine, compute_frontier, derive_threat_objective, evaluate,
    program_from_module_graph, ProgressEvent, ProgressionState,
};

const VAULT: &str = "triangle_agency.the_vault";

/// Generic structural noise words to skip when picking a mission's authored title
/// token (mirrors the spirit of `spine::scene_vocab`'s NOISE filter — kept local so
/// the test stays a black-box check of the real objective without reaching into
/// private internals). Lowercased, ≥4 chars, alnum.
fn first_title_token(title: &str) -> Option<String> {
    const NOISE: &[&str] = &[
        "scene",
        "node",
        "beat",
        "chapter",
        "mission",
        "phase",
        "location",
        "zone",
        "encounter",
        "with",
        "from",
        "into",
        "that",
        "this",
        "they",
        "them",
        "your",
        "have",
        "will",
        "the",
        "and",
        "for",
        "npc",
        "clue",
        "loc",
    ];
    for raw in title.split(|c: char| !c.is_ascii_alphanumeric()) {
        let tok = raw.to_ascii_lowercase();
        if tok.len() >= 4
            && !tok.chars().all(|c| c.is_ascii_digit())
            && !NOISE.contains(&tok.as_str())
        {
            return Some(tok);
        }
    }
    None
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_vault_mission_objective_drives_progression() {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP: DATABASE_URL unset (need live rulesets DB :54347)");
            return;
        }
    };
    let db = Db::connect(&url).await.expect("connect live DB");
    let graph = db
        .load_module_graph(VAULT)
        .await
        .expect("query ok")
        .expect("the_vault module graph present in parsed_bundles");

    // The Vault DB graph: 12 mission-scenes (the anthology flattened to scenes; the
    // first-class Mission/score/aftermath IR is proven faithful by the P2-2 live test).
    let mission_scenes = graph
        .scenes
        .iter()
        .filter(|s| s.node_type == "mission")
        .count();
    eprintln!(
        "RAN: the_vault scenes={} (node_type=\"mission\" = {})",
        graph.scenes.len(),
        mission_scenes
    );
    assert!(
        mission_scenes >= 10,
        "the_vault DB graph should carry ~12 mission-scenes, got {mission_scenes}"
    );

    // Build the runtime program EXACTLY as the engine-ON nav block (tiered.rs) does.
    let mut program = program_from_module_graph(&graph);
    // module_config matchers → scene_01 threat objective IF present (fail-closed: the
    // Vault module has no module_config, so this is correctly skipped — never invented).
    if let Some(cfg) = db.load_module_config(VAULT).await {
        let mut vocab: Vec<String> = Vec::new();
        for b in &cfg.npc_actor_bindings {
            vocab.extend(b.matcher.iter().cloned());
        }
        if let Some(rows) = &cfg.technical_option_table {
            for row in rows {
                vocab.extend(row.matcher.iter().cloned());
            }
        }
        if let Some(d) = derive_threat_objective(
            "obj.neutralize_threat",
            &vocab,
            "rev.threat_outcome",
            "module_config: npc_actor_bindings + technical_option_table matchers",
        ) {
            program.objectives.push(d.objective);
            program.rules.push(d.outcome_rule);
        }
        eprintln!("RAN: the_vault module_config present (threat objective wired)");
    } else {
        eprintln!(
            "RAN: the_vault has NO module_config → threat objective fail-closed skipped (honest)"
        );
    }
    // PL-5 spine generalization: each mission-scene gets a scoped open objective.
    augment_program_with_spine(&mut program, &graph);

    let advance_objs: Vec<&trpg_model::adventure_ir::ObjectiveSpec> = program
        .objectives
        .iter()
        .filter(|o| o.id.starts_with("obj.advance."))
        .collect();
    let spine_rules = program
        .rules
        .iter()
        .filter(|r| r.id.starts_with("rule.spine."))
        .count();
    eprintln!(
        "RAN: program rules={} (spine={}) | mission objectives={} (one per mission-scene)",
        program.rules.len(),
        spine_rules,
        advance_objs.len()
    );
    // Each mission gets its own objective + an out-edge to the next mission (not a flat
    // 0-objective scene list) — the IR is DRIVING, not decorative.
    assert!(
        advance_objs.len() >= 10,
        "every mission-scene must get a scene-scoped objective, got {}",
        advance_objs.len()
    );
    assert!(
        spine_rules >= 9,
        "spine activation edges between adjacent missions, got {spine_rules}"
    );

    // Pick the FIRST mission by page order (Springs Eternal) and prove its objective
    // drives progression end-to-end on the live engine.
    let mut ordered: Vec<&trpg_model::ScenarioNode> = graph
        .scenes
        .iter()
        .filter(|s| !s.node_id.trim().is_empty())
        .collect();
    ordered.sort_by_key(|s| s.page_start.unwrap_or(u32::MAX));
    let m1 = ordered.first().expect("≥1 mission scene");
    let m1_id = m1.node_id.clone();
    let m1_obj = format!("obj.advance.{m1_id}");
    eprintln!(
        "RAN: first mission '{}' (id={m1_id}, page={:?})",
        m1.title, m1.page_start
    );
    assert!(
        advance_objs
            .iter()
            .any(|o| o.id == m1_obj && o.mission_id.as_deref() == Some(m1_id.as_str())),
        "first mission must carry a mission-scoped objective {m1_obj}"
    );

    // (1) Entering the first mission ⇒ frontier non-empty (mission goal surfaces +
    //     the next mission becomes a legal advancement candidate).
    let mut state = ProgressionState::default();
    let enter_signals = evaluate(
        &mut state,
        &[ProgressEvent::Entered(m1_id.clone())],
        &program.borrow(),
    );
    let frontier = compute_frontier(&state, &program.objectives, &program.trackers);
    eprintln!(
        "RAN: Entered({m1_id}) → signals={} frontier total={} active_units={:?} open_objectives={:?}",
        enter_signals.len(),
        frontier.len(),
        frontier.active_units,
        frontier.open_objectives
    );
    assert!(
        !frontier.is_empty(),
        "entering a Vault mission must give a non-empty frontier (mission goal + next mission)"
    );
    assert!(
        frontier.open_objectives.contains(&m1_obj),
        "the current mission's scoped objective must surface in its frontier"
    );

    // (2) A mission-grounded GM fact (fact_id carries the mission's OWN authored title
    //     token — extracted from the loaded scene, NOT module-name hardcoded) ⇒ the
    //     mission objective completes. This is the objective DRIVING progression.
    let token = first_title_token(&m1.title).expect("mission title yields a structural token");
    let mission_fact = format!("anomaly.{token}_resolved"); // how a GM would name the mission outcome
    eprintln!("RAN: simulating mission-grounded fact `{mission_fact}` (title token '{token}')");
    let fact_signals = evaluate(
        &mut state,
        &[ProgressEvent::WorldFactChanged {
            fact: mission_fact.clone(),
            value: IrValue::Bool(true),
        }],
        &program.borrow(),
    );
    let completed = fact_signals
        .iter()
        .any(|s| s.kind == ProgressSignalKind::ObjectiveCompleted && s.id == m1_obj);
    eprintln!(
        "RAN: fact `{mission_fact}` → signals={} ObjectiveCompleted({m1_obj})={completed}",
        fact_signals.len()
    );
    assert!(
        completed,
        "a mission-grounded fact must complete the mission objective via AnyFactMatches \
         (Rust-evaluated guard, LLM never) — this is objective-driven progression"
    );

    eprintln!(
        "PASS: The Vault mission objective drives progression — {} mission objectives + {} spine edges; \
         first mission '{}' frontier non-empty on entry and its objective completes on a mission-grounded fact \
         (scored Optional Objectives faithfully represented as OpaqueAuthoredText — never auto-fired by design).",
        advance_objs.len(),
        spine_rules,
        m1.title
    );
}
