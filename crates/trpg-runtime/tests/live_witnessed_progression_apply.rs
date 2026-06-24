//! CL-WIREb live DB 实测:**EV-APPLY-WIRE witnessed_progression_apply_v1** 在真 the_vault
//! 上确定性工作——证引擎**消费**本回合 admitted GuardLeaf 证据 ⇒ 经 **LIVE 写路径**
//! (`append_domain_event`)落一条 `ObjectiveResolved` domain_event 行(先前 0)+ 目标出 frontier。
//!
//! 这是 EV-APPLY-WIRE 的最后缺口验证:EV-APPLY 已证引擎逻辑(apply_evidence_to_ctx,6 TDD),
//! 但主管 30 回合 J3 eval 发现它**没接进 live turn pipeline**(零非测试调用方)⇒ 真玩里引擎
//! 从不消费 ledger ⇒ ObjectiveCompleted=0。本测证 wiring 后:
//!
//!   (i)   真 the_vault prep-packet 编出 ≥1 个 **GuardLeaf** ActionResolved 原子(EV-P4),
//!         经复用的 EV-P5 `admit_witness_proposals` 做出**1 条 admitted AcceptedEvidence**——
//!         = 真 8 回合里 post-turn witness 实际 admit 的那条(`atom:fbf98c1c` "Conduct an experiment.")。
//!   (ii)  `witnessed_objective_resolutions`(EV-APPLY-WIRE 纯核,引擎消费 ledger)用**同一**
//!         graph+packet 把该 GuardLeaf atom 接到 `success_when=EvidencePresent(atom)` 的目标 ⇒
//!         引擎 fire ObjectiveCompleted ⇒ 产 1 条 canonical `ObjectiveResolved` DomainEvent
//!         (objective_id + basis atom;**引擎产信号,LLM 永不**)。
//!   (iii) 经 **LIVE** `db.append_domain_event` 落库(幂等 event_id),query DB:**先前
//!         ObjectiveResolved=0 → 现 ≥1 行**(=j3v2 SEMANTIC 轴读的 SEM_KIND carrier,先前恒 0)。
//!   (iv)  **目标出 frontier**:该 resolved objective 原在 open frontier,完成后离开。
//!   (v)   **nav-split**:引擎不改 current_scene。
//!
//! 这是 worker 可跑的**确定性** live 证(真 DB、真 the_vault prep-packet GuardLeaf、复用 EV-P5
//! admission + LIVE append);真 30 回合 engine-ON LLM judged eval(j3v2 语义轴定级)是主管 THE
//! J3 PAYOFF 步(见 READY_FOR_FULL.sentinel)。
//!
//! Run (rulesets DB :54347):
//!   DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!     CARGO_TARGET_DIR=target-air cargo test -p trpg-runtime \
//!     --test live_witnessed_progression_apply -- --nocapture
//!
//! 无 `DATABASE_URL` ⇒ SKIP(fail-closed)。**反假绿**:真跑必打印 `RAN:` + 计数 + `PASS`;只见
//! `SKIP` = 没验证。
use sqlx::Row;
use trpg_db::Db;
use trpg_model::adventure_ir::{
    compile_prep_packet_guard_leaves, evidence_objectives_from_prep_packet, BasisKind, CapId,
    EvidenceAtomCatalog, EvidenceClaim, EvidenceKind, EvidenceLedger, EvidenceOffer,
    EvidenceOfferSet, NonEmpty, ProgressRole, TurnLocalRef,
};
use trpg_model::{DomainEvent, DomainEventKind};
use trpg_runtime::post_turn_witness::admit_witness_proposals;
use trpg_runtime::progression::{
    apply_evidence_to_ctx, compute_frontier, evaluate, witnessed_objective_resolutions,
    ProgressionProgram, ProgressionState,
};

const VAULT: &str = "triangle_agency.the_vault";
const SESSION: &str = "sess_wire_live";
const TURN: &str = "turn-uuid-wire-live-1";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_vault_engine_consumes_ledger_and_appends_objectiveresolved() {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP: DATABASE_URL unset (need live rulesets DB :54347)");
            return;
        }
    };
    let db = Db::connect(&url).await.expect("connect live DB");
    // A direct pool for the cleanup + the "prev 0 → now ≥1" count assertions.
    let pool = sqlx::postgres::PgPool::connect(&url)
        .await
        .expect("connect sqlx pool");

    let mut graph = db
        .load_module_graph(VAULT)
        .await
        .expect("query ok")
        .expect("the_vault module graph present in parsed_bundles");
    // mirror the live offer/catalog path (turn_loop.rs:2114): clue-projected graph so the
    // EV-P4 GuardLeaf atom_ids in the offer == the ones the evidence-objectives compile to.
    let _ = trpg_runtime::clue_projection::project_clues_onto_scenes(&mut graph);
    let csp = db
        .load_module_prep_packet_session(VAULT)
        .await
        .expect("query ok")
        .expect("the_vault prep packet present (current_session_packet)");

    // ─── (i) a real admitted GuardLeaf AcceptedEvidence (reused EV-P5 witness admission) ──
    let atom = compile_prep_packet_guard_leaves(&graph, &csp)
        .into_iter()
        .find(|a| a.kind == EvidenceKind::ActionResolved)
        .expect("≥1 prep-packet GuardLeaf ActionResolved atom (the mission 'experiment' objective)");
    assert_eq!(atom.progress_role, ProgressRole::GuardLeaf, "objective leaf ⇒ GuardLeaf");
    let span = atom
        .source_refs
        .first()
        .and_then(|s| s.note.clone().or_else(|| s.anchor_id.clone()))
        .unwrap_or_default();
    eprintln!(
        "RAN: the_vault scenes={} clues={}; GuardLeaf ActionResolved atom={} source_span={:?}",
        graph.scenes.len(),
        graph.clues.len(),
        atom.atom_id.as_str(),
        span
    );

    let cap = CapId::from_parts(SESSION, TURN, &atom.atom_id);
    let mut catalog = EvidenceAtomCatalog::new();
    catalog.insert(atom.clone());
    let mut offer_set = EvidenceOfferSet::new(TURN);
    offer_set.push(EvidenceOffer {
        cap_id: cap.clone(),
        atom_id: atom.atom_id.clone(),
        kind: atom.kind,
        meaning: "the player conducted the authored mission experiment".into(),
        required_basis: vec![BasisKind::OutcomeCommitted],
        expires_at: TURN.into(),
    });
    let success_check = DomainEvent::new(
        "de_wire_chk_1",
        SESSION,
        TURN,
        DomainEventKind::CheckResolved,
        serde_json::json!({"check_id": "wire_1", "outcome": {"success": true, "success_tier": "regular"}}),
    );
    let events = vec![success_check];
    let proposals = vec![EvidenceClaim {
        cap_id: cap.clone(),
        basis: NonEmpty::from_vec(vec![TurnLocalRef::Commit(0)]).unwrap(),
    }];
    let (ledger, _) = admit_witness_proposals(
        SESSION,
        TURN,
        &offer_set,
        &catalog,
        &events,
        &proposals,
        &EvidenceLedger::new(),
    );
    assert_eq!(ledger.len(), 1, "EV-P5 admission ⇒ exactly one admitted GuardLeaf AcceptedEvidence");
    eprintln!(
        "RAN: admitted ledger has 1 AcceptedEvidence(atom={}) — the engine's input",
        ledger.entries()[0].atom_id.as_str()
    );

    let current_scene = graph
        .scenes
        .first()
        .map(|s| s.node_id.clone())
        .unwrap_or_default();

    // ─── (iv-pre) the evidence-objective is OPEN in the frontier before consumption ──────
    let objs = evidence_objectives_from_prep_packet(&graph, &csp);
    assert!(!objs.is_empty(), "the_vault prep-packet yields ≥1 evidence-backed objective");
    let mut state = ProgressionState::default();
    if !current_scene.trim().is_empty() {
        state.ctx.entered_locations.push(current_scene.clone());
    }
    let open_before = compute_frontier(&state, &objs, &[]).open_objectives;
    // confirm the engine does NOT re-fire / completes idempotently: apply + evaluate twice.
    apply_evidence_to_ctx(&mut state.ctx, &ledger);
    let prog = ProgressionProgram { rules: &[], objectives: &objs, trackers: &[] };
    let _ = evaluate(&mut state, &[], &prog);
    let open_after = compute_frontier(&state, &objs, &[]).open_objectives;

    // ─── (ii) EV-APPLY-WIRE core: engine consumes ledger ⇒ ObjectiveResolved events ──────
    let resolutions =
        witnessed_objective_resolutions(SESSION, TURN, &graph, &csp, &ledger, &current_scene);
    assert!(
        !resolutions.is_empty(),
        "engine consumes the admitted GuardLeaf evidence ⇒ ≥1 ObjectiveResolved"
    );
    let resolved_id = resolutions[0]
        .data
        .get("objective_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    assert_eq!(resolutions[0].kind, DomainEventKind::ObjectiveResolved);
    assert!(
        open_before.contains(&resolved_id),
        "the resolved objective WAS open before (it leaves the frontier on completion)"
    );
    assert!(
        !open_after.contains(&resolved_id),
        "completed objective leaves the open frontier (advance), still: {open_after:?}"
    );
    // nav-split: building/consuming the ledger never mutated the current scene.
    assert_eq!(
        state.ctx.entered_locations,
        if current_scene.trim().is_empty() { vec![] } else { vec![current_scene.clone()] },
        "engine never teleported the scene (nav-split)"
    );
    eprintln!(
        "RAN: engine consumed ledger ⇒ ObjectiveResolved objective_id={} (was open: {}), frontier {}→{}",
        resolved_id,
        open_before.contains(&resolved_id),
        open_before.len(),
        open_after.len()
    );

    // ─── (iii) LIVE write path: append_domain_event ⇒ DB shows ObjectiveResolved (prev 0) ─
    sqlx::query("delete from domain_events where session_id = $1 and kind = $2")
        .bind(SESSION)
        .bind("ObjectiveResolved")
        .execute(&pool)
        .await
        .expect("cleanup prior test rows");
    let before_rows: i64 =
        sqlx::query("select count(*) from domain_events where session_id = $1 and kind = $2")
            .bind(SESSION)
            .bind("ObjectiveResolved")
            .fetch_one(&pool)
            .await
            .expect("count query")
            .get(0);
    assert_eq!(before_rows, 0, "prev ObjectiveResolved rows = 0 (the gap EV-APPLY-WIRE closes)");

    for ev in &resolutions {
        db.append_domain_event(ev).await.expect("LIVE append_domain_event");
    }
    // idempotent: re-appending the SAME resolution must not create a second row.
    for ev in &resolutions {
        db.append_domain_event(ev).await.expect("idempotent re-append");
    }

    let after_rows: Vec<(String, serde_json::Value)> = sqlx::query(
        "select event_id, data from domain_events where session_id = $1 and kind = $2 order by event_id",
    )
    .bind(SESSION)
    .bind("ObjectiveResolved")
    .fetch_all(&pool)
    .await
    .expect("after query")
    .into_iter()
    .map(|r| (r.get::<String, _>(0), r.get::<serde_json::Value, _>(1)))
    .collect();
    assert_eq!(
        after_rows.len(),
        resolutions.len(),
        "exactly one durable ObjectiveResolved row per resolution (idempotent, no double-emit)"
    );
    for (event_id, data) in &after_rows {
        assert!(event_id.starts_with(&format!("de_objresolved_{SESSION}_")));
        eprintln!("RAN: DB domain_events ObjectiveResolved row event_id={event_id} data={data}");
    }

    // cleanup so the test is repeatable and leaves no residue in the shared DB.
    sqlx::query("delete from domain_events where session_id = $1 and kind = $2")
        .bind(SESSION)
        .bind("ObjectiveResolved")
        .execute(&pool)
        .await
        .expect("cleanup after");

    eprintln!(
        "PASS: EV-APPLY-WIRE — engine consumes admitted GuardLeaf evidence on real the_vault ⇒ \
         LIVE append_domain_event writes ObjectiveResolved (prev 0 → {}) + objective leaves frontier; \
         current_scene untouched (nav-split)",
        resolutions.len()
    );
}
