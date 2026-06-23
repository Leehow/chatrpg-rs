//! CL-P5b live DB 实测:**EV-P5 progress_post_turn_witness_v1** 在真 the_vault 上确定性
//! 工作——证 evidence 可经 PostTurnWitness 流动而**不依赖主 GM 顺手 emit inline 标签**:
//!
//!   (i)   真 the_vault prep-packet 编出 ≥1 个 **GuardLeaf** ActionResolved 原子(EV-P4),
//!         做成本回合的 offer(铸 turn-scoped cap)。
//!   (ii)  造一个**已提交成功** CheckResolved → `structural_candidates`(确定性 Rust 收窄,
//!         无 LLM)把该 GuardLeaf offer 认作 **structural candidate**(committed success ∩
//!         check-bindable offer);无成功检定则**零候选**(fail-closed)。
//!   (iii) `build_extractor_view` 只暴露候选 + committed outcome——**serialized view 不含任何
//!         objective/guard/reward/atom 词汇**(抽取器见不到进度,只能提议 cap+basis)。
//!   (iv)  一个 **mock witness proposal**(= 专职聚焦抽取器的输出,worker 侧 mock LLM;主管跑真
//!         8 回合 LLM)经 `admit_witness_proposals`(复用 EV-P4 binding 不改)⇒ **1 条 admitted
//!         AcceptedEvidence(ActionResolved, GuardLeaf)**——正是 EV-P5 的诉求:即便主 GM inline
//!         标签为 0,evidence 仍可靠 flow。
//!   (v)   `witness_trigger`:audit 全 not_observed 但有 structural candidate ⇒ 触发(recall
//!         backstop);无候选 ⇒ 不触发(fail-closed)。
//!
//! 仍 **shadow**(引擎不消费 = EV-APPLY);真 8 回合 engine-ON LLM eval(admitted>0 经真 witness)
//! 是主管步(见 READY_FOR_FULL.sentinel)。
//!
//! Run (rulesets DB :54347):
//!   DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!     CARGO_TARGET_DIR=target-air cargo test -p trpg-runtime \
//!     --test live_post_turn_witness -- --nocapture
//!
//! 无 `DATABASE_URL` ⇒ SKIP(fail-closed)。**反假绿**:真跑必打印 `RAN:` + 计数 + `PASS`;只见
//! `SKIP` = 没验证。
use trpg_db::Db;
use trpg_model::adventure_ir::{
    compile_prep_packet_guard_leaves, BasisKind, CapId, EvidenceAtomCatalog, EvidenceAuthority,
    EvidenceClaim, EvidenceKind, EvidenceLedger, EvidenceOffer, EvidenceOfferSet, NonEmpty,
    ProgressRole, TurnLocalRef,
};
use trpg_model::{DomainEvent, DomainEventKind};
use trpg_runtime::post_turn_witness::{
    admit_witness_proposals, build_extractor_view, structural_candidates, witness_trigger,
    WitnessTrigger,
};

const VAULT: &str = "triangle_agency.the_vault";
const SESSION: &str = "sess_p5_live";
const TURN: &str = "turn-uuid-p5-live-1";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_vault_post_turn_witness_admits_guard_leaf_without_inline_gm_tag() {
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

    // ─── (i) a GuardLeaf action offer from the real prep-packet objectives ────────────
    let guard_leaves = compile_prep_packet_guard_leaves(&graph, &csp);
    let atom = guard_leaves
        .iter()
        .find(|a| a.kind == EvidenceKind::ActionResolved)
        .cloned()
        .expect("≥1 prep-packet GuardLeaf is an ActionResolved action atom (bindable by a check)");
    assert_eq!(atom.progress_role, ProgressRole::GuardLeaf, "objective leaf ⇒ GuardLeaf");
    let span = atom
        .source_refs
        .first()
        .and_then(|s| s.note.clone().or_else(|| s.anchor_id.clone()))
        .unwrap_or_default();
    eprintln!(
        "RAN: the_vault scenes={} clues={}; GuardLeaf ActionResolved atom grounding={} source_span={:?}",
        graph.scenes.len(),
        graph.clues.len(),
        atom.grounding,
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

    // ─── (ii) deterministic candidate narrowing: committed success ⇒ candidate ────────
    let success_check = DomainEvent::new(
        "de_witness_chk_1",
        SESSION,
        TURN,
        DomainEventKind::CheckResolved,
        serde_json::json!({"check_id": "wit_1", "outcome": {"success": true, "success_tier": "regular"}}),
    );
    let events = vec![success_check];
    let candidates = structural_candidates(SESSION, TURN, &offer_set, &events);
    assert_eq!(candidates, vec![cap.clone()], "committed success ∩ check-bindable offer ⇒ candidate");
    // fail-closed: no committed success ⇒ zero candidates (the extractor gets nothing).
    let failed = vec![DomainEvent::new(
        "de_witness_chk_fail",
        SESSION,
        TURN,
        DomainEventKind::CheckResolved,
        serde_json::json!({"check_id": "wit_f", "outcome": {"success": false, "success_tier": "regular"}}),
    )];
    assert!(
        structural_candidates(SESSION, TURN, &offer_set, &failed).is_empty(),
        "fail-closed: no committed success ⇒ no candidate offered to the extractor"
    );
    eprintln!("RAN: structural_candidates={} (success) / 0 (failed) — deterministic narrowing", candidates.len());

    // ─── (iii) the extractor view leaks no progression vocabulary ─────────────────────
    let view = build_extractor_view(
        "I conduct an experiment on the anomaly",
        &offer_set,
        &events,
        vec![],
        &candidates,
    );
    assert_eq!(view.candidates.len(), 1, "only the narrowed candidate is surfaced");
    assert_eq!(view.committed_outcomes.len(), 1, "the committed success is the citable basis");
    let view_json = serde_json::to_string(&view).unwrap();
    for forbidden in ["objective", "guard", "reward", "next_scene", "atom:", "atom_id", "success_when"] {
        assert!(!view_json.contains(forbidden), "extractor view leaked `{forbidden}`");
    }

    // ─── (iv) a mock witness proposal ⇒ 1 admitted AcceptedEvidence(ActionResolved) ───
    // The focused extractor (mocked here; the supervisor runs the real LLM) proposes the
    // narrowed candidate citing the committed outcome. Rust admits via the REUSED EV-P4
    // binding — Rust decided the check succeeded, the extractor only said "this happened".
    let proposals = vec![EvidenceClaim {
        cap_id: cap.clone(),
        basis: NonEmpty::from_vec(vec![TurnLocalRef::Commit(0)]).unwrap(),
    }];
    let (ledger, admissions) = admit_witness_proposals(
        SESSION, TURN, &offer_set, &catalog, &events, &proposals, &EvidenceLedger::new(),
    );
    assert_eq!(ledger.len(), 1, "EV-P5: witness proposal ⇒ exactly one admitted AcceptedEvidence");
    let ev = &ledger.entries()[0];
    assert_eq!(ev.evidence_kind, EvidenceKind::ActionResolved);
    assert_eq!(ev.authority, EvidenceAuthority::ExactDomain, "Rust decided success (reused EV-P4 binding)");
    assert_eq!(ev.atom_id, atom.atom_id, "atom Rust-resolved from the OfferSet, never named by the extractor");
    assert_eq!(ev.basis_event_ids, vec!["de_witness_chk_1".to_string()], "basis = the committed check");
    assert!(admissions[0].result.is_ok());
    eprintln!(
        "RAN: mock witness proposal ⇒ admitted AcceptedEvidence(ActionResolved, GuardLeaf) atom={} basis={:?} authority={:?}",
        ev.atom_id.as_str(),
        ev.basis_event_ids,
        ev.authority
    );

    // ─── (v) trigger: all-not_observed + candidates ⇒ fires; no candidates ⇒ idle ─────
    assert_eq!(
        witness_trigger(false, 0, candidates.len()),
        WitnessTrigger::AllNotObservedWithCandidates,
        "GM audit observed nothing but a real success on an offered atom happened ⇒ backstop fires"
    );
    assert_eq!(witness_trigger(true, 0, 0), WitnessTrigger::NotTriggered, "no candidate ⇒ fail-closed idle");

    eprintln!(
        "PASS: EV-P5 post-turn witness admits a GuardLeaf ActionResolved on real the_vault \
         WITHOUT any inline GM tag (shadow; engine not consuming, J3 unchanged)"
    );
}
