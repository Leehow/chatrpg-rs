//! F1 `progress_scene_npc_knowledge_v1` live DB proof on the REAL **homecoming** module — the
//! TALK-scene coverage closer. The diagnostic (DIAG_RESIDUAL_REDS) pinned the residual J3 red to
//! `scene_02_questions_for_athena` freezing T7–T21 (max_frozen_run=15): a TALK/interrogation scene
//! with `scene_mechanics=0`, `gm_notes=null`, no clue/location/encounter refs ⇒ EV-P3 compiled ZERO
//! scene-tagged atoms ⇒ `scene_advance_objective(scene_02)` was `None` ⇒ it could NEVER earn its
//! advance, no matter how much real interrogation play happened. F1 derives the scene's salient atom
//! from its typed `referenced_npc_ids` ∩ NPC authored knowledge (npc_athena's authored body/secrets).
//!
//! This proves, end-to-end on the REAL parsed graph (:54347), that scene_02 now EARNS its advance
//! WITHOUT an LLM (the supervisor's 30-turn A/B exercises the live LLM loop):
//!
//!   (A) **OFF == frozen baseline** — with the F1 flag OFF, `compile_authored_observations` produces
//!       ZERO `scene:scene_02` atoms and `scene_advance_objective(scene_02)` is `None` — the EXACT
//!       freeze condition the diagnostic found (and the investigative scene_01 atoms are unchanged).
//!   (B) **ON ⇒ source-grounded talk atom** — with F1 ON the compiler derives ONE EntityEncountered
//!       atom bound to `npc_athena`, tagged `scene:scene_02`, source-grounded (cites the scene page +
//!       authored NPC), and `scene_advance_objective(scene_02)` becomes `EvidenceAnyOf` over it. ZERO
//!       hardcoded scene/NPC map — keyed on the scene's typed NPC ref + the NPC's authored content.
//!   (C) **offered in-scene** — `build_evidence_atom_catalog` folds it in and `derive_offer_set` for
//!       current_scene=scene_02 OFFERS it (EntityEncountered is an observable-action kind), so the
//!       live witness can claim it. (Other scenes do NOT see it — scene-scoped.)
//!   (D) **earned ⇒ ObjectiveResolved** — admitting the atom via the SAME witness/binding path the
//!       live loop uses (a committed CheckResolved success) ⇒ the engine fires
//!       `obj.scene_advance.scene_02_questions_for_athena` ObjectiveResolved (D2 consume). scene_02
//!       EARNS its advance from talk play — the frozen seam unfreezes.
//!   (E) **the_vault no-regression** — F1 ON, the_vault (anthology) still `resolve_next_scene = None`
//!       (no nonsense teleport); its NPC atoms cannot teleport across independent missions.
//!
//! Run (rulesets DB :54347):
//!   DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!     cargo test -p trpg-runtime --test live_f1_npc_knowledge_scene_advance -- --nocapture --test-threads=1
//! No `DATABASE_URL` ⇒ SKIP (fail-closed). Anti-false-green: a real run prints `RAN:` lines + `PASS`.
use trpg_db::Db;
use trpg_model::adventure_ir::{
    scene_advance_guard_leaves, scene_advance_objective, BasisKind, CapId, EvidenceClaim,
    EvidenceKind, EvidenceLedger, EvidenceOffer, EvidenceOfferSet, NonEmpty, PredicateExpr,
    ProgressRole, TurnLocalRef, SCENE_NPC_KNOWLEDGE_ENV,
};
use trpg_model::{DomainEvent, DomainEventKind};
use trpg_runtime::evidence_offers::derive_offer_set;
use trpg_runtime::evidence_projection::build_evidence_atom_catalog;
use trpg_runtime::progression::resolve_next_scene;

const HOMECOMING: &str = "cyberpunk_red.homecoming";
const VAULT: &str = "triangle_agency.the_vault";
const SESSION: &str = "sess_f1_npc_knowledge_live";
const TURN: &str = "turn-uuid-f1-npc-knowledge-live-1";
/// The exact scene the diagnostic measured FROZEN T7–T21 (the talk seam F1 unfreezes).
const FROZEN_TALK_SCENE: &str = "scene_02_questions_for_athena";
const SALIENT_NPC: &str = "npc_athena";
const CATALOG_FLAG: &str = "TRPG_PROGRESS_OBSERVABLE_LEAF_CATALOG_V1";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn talk_scene_earns_its_advance_from_npc_knowledge() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("SKIP: DATABASE_URL unset (need live rulesets DB :54347)");
        return;
    };
    let db = Db::connect(&url).await.expect("connect live DB");

    let mut graph = db
        .load_module_graph(HOMECOMING)
        .await
        .expect("query ok")
        .expect("homecoming module graph present in parsed_bundles");
    let _ = trpg_runtime::clue_projection::project_clues_onto_scenes(&mut graph);

    // ─── (A) OFF == frozen baseline: no scene_02 atom, no advance objective ──────────────────────
    std::env::remove_var(SCENE_NPC_KNOWLEDGE_ENV);
    assert!(
        scene_advance_guard_leaves(&graph, FROZEN_TALK_SCENE).is_empty(),
        "OFF: scene_02 has ZERO scene-tagged atoms (investigative-only baseline — the freeze)"
    );
    assert!(
        scene_advance_objective(&graph, FROZEN_TALK_SCENE).is_none(),
        "OFF: scene_02 has NO scene-advance objective ⇒ can never earn its advance (frozen)"
    );
    // investigative scene_01 is unaffected (D2 unchanged OFF or ON).
    assert!(
        scene_advance_objective(&graph, "scene_01_lawmen_in_trouble").is_some(),
        "scene_01 (investigative, scene_mechanics=5) still has its advance objective"
    );
    eprintln!("RAN: (A) OFF baseline — scene_02 has 0 atoms / None objective (frozen); scene_01 unchanged");

    // ─── (B) ON ⇒ source-grounded EntityEncountered atom for scene_02 from npc_athena ────────────
    std::env::set_var(SCENE_NPC_KNOWLEDGE_ENV, "1");
    let leaves = scene_advance_guard_leaves(&graph, FROZEN_TALK_SCENE);
    let atom = leaves
        .iter()
        .find(|a| {
            a.kind == EvidenceKind::EntityEncountered && a.bindings.iter().any(|b| b == SALIENT_NPC)
        })
        .cloned()
        .expect("ON: scene_02 gains an EntityEncountered atom bound to npc_athena");
    assert_eq!(
        atom.progress_role,
        ProgressRole::GuardLeaf,
        "re-roled GuardLeaf for the objective"
    );
    assert!(
        !atom.source_refs.is_empty(),
        "source-grounded (cites the scene + authored NPC)"
    );
    assert!(
        atom.source_refs[0]
            .note
            .as_deref()
            .unwrap_or("")
            .to_lowercase()
            .contains("athena"),
        "source note cites the authored NPC (Athena)"
    );
    let objective = scene_advance_objective(&graph, FROZEN_TALK_SCENE)
        .expect("ON: scene_02 now HAS a scene-advance objective");
    assert_eq!(
        objective.id,
        "obj.scene_advance.scene_02_questions_for_athena"
    );
    match &objective.success_when {
        PredicateExpr::EvidenceAnyOf { atom_ids } => {
            assert!(
                atom_ids.iter().any(|a| a == atom.atom_id.as_str()),
                "the objective's EvidenceAnyOf consults the admitted-pipeline atom_id"
            );
        }
        other => panic!("expected EvidenceAnyOf, got {other:?}"),
    }
    eprintln!(
        "RAN: (B) ON — scene_02 atom EntityEncountered/{SALIENT_NPC} grounding='{}' ⇒ {} (EvidenceAnyOf)",
        atom.grounding, objective.id
    );

    // ─── (C) offered in-scene by the live catalog/offer path (other scenes do NOT see it) ────────
    std::env::set_var(CATALOG_FLAG, "1");
    let catalog = build_evidence_atom_catalog(&graph);
    let offers = derive_offer_set(&graph, &catalog, FROZEN_TALK_SCENE, SESSION, TURN);
    let offered = offers
        .offers()
        .iter()
        .find(|o| o.atom_id == atom.atom_id)
        .expect("scene_02's NPC atom is OFFERED when current_scene = scene_02");
    assert_eq!(offered.kind, EvidenceKind::EntityEncountered);
    let other_scene_offers =
        derive_offer_set(&graph, &catalog, "scene_03_foxwell_services", SESSION, TURN);
    assert!(
        !other_scene_offers
            .offers()
            .iter()
            .any(|o| o.atom_id == atom.atom_id),
        "scene-scoped: scene_02's atom is NOT offered in scene_03"
    );
    eprintln!(
        "RAN: (C) scene_02 atom OFFERED in-scene (cap='{}'); scene-scoped (absent in scene_03)",
        offered.cap_id.as_str()
    );

    // ─── (D) DECISIVE: admit via the real witness path ⇒ obj.scene_advance.scene_02 fires ────────
    let cap = CapId::from_parts(SESSION, TURN, &atom.atom_id);
    let mut offer_set = EvidenceOfferSet::new(TURN);
    offer_set.push(EvidenceOffer {
        cap_id: cap.clone(),
        atom_id: atom.atom_id.clone(),
        kind: atom.kind,
        meaning: "the player interrogated Athena and elicited her authored knowledge".into(),
        required_basis: vec![BasisKind::OutcomeCommitted],
        expires_at: TURN.into(),
    });
    let success_check = DomainEvent::new(
        "de_f1_talk_chk_1",
        SESSION,
        TURN,
        DomainEventKind::CheckResolved,
        serde_json::json!({"check_id": "f1_talk_1", "outcome": {"success": true, "success_tier": "regular"}}),
    );
    let proposals = vec![EvidenceClaim {
        cap_id: cap.clone(),
        basis: NonEmpty::from_vec(vec![TurnLocalRef::Commit(0)]).unwrap(),
    }];
    let (ledger, _) = trpg_runtime::post_turn_witness::admit_witness_proposals(
        SESSION,
        TURN,
        &offer_set,
        &catalog,
        &[success_check],
        &proposals,
        &EvidenceLedger::new(),
    );
    assert_eq!(
        ledger.len(),
        1,
        "witness admits scene_02's NPC-knowledge atom from a committed check success"
    );
    let resolutions = trpg_runtime::progression::witnessed_scene_advance_resolutions(
        SESSION,
        TURN,
        &graph,
        &ledger,
        FROZEN_TALK_SCENE,
    );
    assert_eq!(
        resolutions.len(),
        1,
        "scene_02's advance objective RESOLVES from real talk play (D2)"
    );
    assert_eq!(resolutions[0].data["objective_id"], objective.id);
    assert_eq!(resolutions[0].data["scene_id"], FROZEN_TALK_SCENE);
    assert_eq!(resolutions[0].data["signal"], "scene_advance");
    assert_eq!(resolutions[0].kind, DomainEventKind::ObjectiveResolved);
    eprintln!(
        "RAN: (D) EARNED — admitted scene_02 NPC atom ⇒ ObjectiveResolved {} (signal=scene_advance); the frozen talk seam unfreezes",
        objective.id
    );

    // ─── (E) the_vault no-regression: F1 ON, still no anthology teleport ──────────────────────────
    let vault = db
        .load_module_graph(VAULT)
        .await
        .expect("query ok")
        .expect("the_vault module graph present");
    let vault_first = vault
        .scenes
        .iter()
        .min_by_key(|s| s.page_start.unwrap_or(u32::MAX))
        .map(|s| s.node_id.clone())
        .expect("the_vault has scenes");
    assert_eq!(
        resolve_next_scene(&vault, &vault_first),
        None,
        "the_vault ({:?}) anthology: F1 ON still no spine teleport across independent missions",
        vault.module_type
    );
    eprintln!("RAN: (E) the_vault first scene '{vault_first}' resolve_next_scene = None (F1 ON, no teleport)");

    std::env::remove_var(SCENE_NPC_KNOWLEDGE_ENV);
    std::env::remove_var(CATALOG_FLAG);
    eprintln!(
        "PASS: F1 — the FROZEN talk scene {FROZEN_TALK_SCENE} now EARNS obj.scene_advance.* from its \
         authored NPC knowledge ({SALIENT_NPC}): source-grounded EntityEncountered atom, offered \
         in-scene, admitted via the real witness path ⇒ ObjectiveResolved. OFF == frozen baseline; \
         the_vault no-teleport. The talk-scene coverage gap is closed."
    );
}
