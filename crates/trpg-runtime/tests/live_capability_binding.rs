//! CL-P4b live DB 实测:**EV-P4 capability binding + GuardLeaf-from-prep-packet** 在真
//! the_vault 上确定性工作(Rust 评、LLM 永不、零造)。证两件 EV-P3 暴露的缺口已闭合:
//!
//!   (i)  **GuardLeaf > 0**:the_vault 的 module_graph 无 objectives 字段(被压扁),但任务
//!        authored objectives 活在 `module_prep_packets` 的 `current_session_packet.
//!        mission_briefing.optional_objectives` 里。`compile_prep_packet_guard_leaves`
//!        从中编出 ≥1 个 **GuardLeaf** ActionResolved 原子(绑到该任务 authored 的 Anomaly/
//!        场景实体——anti-fabrication:目标须由 authored mission-context 解析到真 graph ref),
//!        每个带逐字 source span(打印验真,反造假)。
//!   (ii) **bound check-success ⇒ 1 admitted**:把一个 GuardLeaf 原子做成 offer(铸 cap),
//!        造一个**已提交成功** CheckResolved 事件 + GM 的 `[evidence_attempts]` tag,
//!        `bind_capability_evidence` ⇒ **1 条 AcceptedEvidence(ActionResolved, ExactDomain)**
//!        ——正是 EV-P3 observed=2/admitted=0 的修复(action 原子的 required_basis 现由真
//!        committed check-outcome 满足,而非 gateway 解不了的自由 GM-cited basis)。
//!
//! 仍 **shadow**(引擎不消费 = EV-APPLY);完整 8 回合 LLM eval(admitted>0 真实播 + GuardLeaf
//! 映射任务目标)是主管步(见 READY_FOR_FULL.sentinel)。
//!
//! Run (rulesets DB :54347):
//!   DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!     CARGO_TARGET_DIR=target-air cargo test -p trpg-runtime \
//!     --test live_capability_binding -- --nocapture
//!
//! 无 `DATABASE_URL` ⇒ SKIP(fail-closed,不阻塞 CI)。**反假绿**:真跑必打印 `RAN:` + 计数 +
//! 逐字 source span + `PASS`;只见 `SKIP` = 没验证。
use trpg_db::Db;
use trpg_model::adventure_ir::{
    compile_prep_packet_guard_leaves, BasisKind, CapId, EvidenceAtomCatalog, EvidenceAuthority,
    EvidenceKind, EvidenceLedger, EvidenceOffer, EvidenceOfferSet, ProgressRole,
};
use trpg_model::{DomainEvent, DomainEventKind};
use trpg_runtime::evidence_binding::bind_capability_evidence;

const VAULT: &str = "triangle_agency.the_vault";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_vault_guard_leaves_and_bound_check_success_admits() {
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
    let csp = db
        .load_module_prep_packet_session(VAULT)
        .await
        .expect("query ok")
        .expect("the_vault prep packet present (current_session_packet)");

    // ─── (i) GuardLeaf > 0 from prep-packet objectives, source-grounded ──────────────
    let guard_leaves = compile_prep_packet_guard_leaves(&graph, &csp);
    eprintln!(
        "RAN: the_vault module_graph scenes={} npcs={} clues={}; prep-packet GuardLeaf atoms={}",
        graph.scenes.len(),
        graph.npcs.len(),
        graph.clues.len(),
        guard_leaves.len()
    );
    assert!(
        !guard_leaves.is_empty(),
        "EV-P4(B): the prep-packet's authored mission objectives must compile to ≥1 GuardLeaf atom \
         (the module_graph itself carries none — they live in current_session_packet)"
    );
    for (n, a) in guard_leaves.iter().enumerate() {
        // Anti-fabrication: every atom is a GuardLeaf, source-grounded, with a verbatim span.
        assert_eq!(a.progress_role, ProgressRole::GuardLeaf, "objective leaf ⇒ GuardLeaf");
        assert!(!a.source_refs.is_empty(), "source-grounded: carries ≥1 SourceRef");
        let span = a
            .source_refs
            .first()
            .and_then(|s| s.note.clone().or_else(|| s.anchor_id.clone()))
            .unwrap_or_default();
        eprintln!(
            "  GuardLeaf[{n}] kind={:?} grounding={} source_span={:?}",
            a.kind, a.grounding, span
        );
        assert!(
            !span.trim().is_empty(),
            "anti-fabrication: every GuardLeaf carries a verbatim authored source span"
        );
    }

    // ─── (ii) a bound check-success ⇒ 1 admitted AcceptedEvidence(ActionResolved) ─────
    // Pick the first GuardLeaf and make it an offered capability (mint a turn-scoped cap).
    let atom = guard_leaves
        .iter()
        .find(|a| a.kind == EvidenceKind::ActionResolved)
        .cloned()
        .expect("≥1 GuardLeaf is an ActionResolved action atom (bindable by a check)");
    const SESSION: &str = "sess_p4_live";
    const TURN: &str = "turn-uuid-live-1";
    let cap = CapId::from_parts(SESSION, TURN, &atom.atom_id);
    let mut catalog = EvidenceAtomCatalog::new();
    catalog.insert(atom.clone());
    let mut offer_set = EvidenceOfferSet::new(TURN);
    offer_set.push(EvidenceOffer {
        cap_id: cap.clone(),
        atom_id: atom.atom_id.clone(),
        kind: atom.kind,
        meaning: "the player resolved an authored mission action".into(),
        required_basis: vec![BasisKind::OutcomeCommitted],
        expires_at: TURN.into(),
    });

    // A real committed successful check this turn (the GM tagged the cap as the attempt).
    let check = DomainEvent::new(
        "de_check_live_1",
        SESSION,
        TURN,
        DomainEventKind::CheckResolved,
        serde_json::json!({"check_id": "live_1", "outcome": {"success": true, "success_tier": "regular"}}),
    );
    let (ledger, decisions) = bind_capability_evidence(
        SESSION,
        TURN,
        &offer_set,
        &catalog,
        &[check],
        &[cap],
        &EvidenceLedger::new(),
    );
    assert_eq!(
        ledger.len(),
        1,
        "EV-P4(A): a bound action cap + a committed successful check ⇒ exactly one admitted evidence"
    );
    let ev = &ledger.entries()[0];
    assert_eq!(ev.evidence_kind, EvidenceKind::ActionResolved);
    assert_eq!(ev.authority, EvidenceAuthority::ExactDomain, "deterministic producer (not GM-witnessed)");
    assert_eq!(ev.basis_event_ids, vec!["de_check_live_1".to_string()], "basis = the committed check");
    assert!(decisions[0].result.is_ok());
    eprintln!(
        "RAN: bound check-success ⇒ admitted AcceptedEvidence(ActionResolved) atom={} basis={:?} authority={:?}",
        ev.atom_id.as_str(),
        ev.basis_event_ids,
        ev.authority
    );
    eprintln!("PASS: GuardLeaf>0 from prep-packet objectives + bound check-success admits (EV-P4 shadow)");
}
