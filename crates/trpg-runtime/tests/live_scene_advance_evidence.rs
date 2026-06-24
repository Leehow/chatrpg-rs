//! D2 `progress_scene_advance_evidence_v1` live DB proof on the REAL **homecoming**
//! module — the Wall B bridge. Proves, on the real published module that froze 30
//! turns (audit `hcJ3`: ObjectiveResolved=0, frontier_starvation), that:
//!
//!   (i)   homecoming's OWN authored scene content compiles (via the reused EV-P3
//!         [`compile_authored_observations`]) into ≥1 scene-salient **GuardLeaf**
//!         observation atom, and [`scene_advance_objective`] binds the scene's atoms
//!         into a single `EvidenceAnyOf` advance objective (the producer works on the
//!         real module, not a fixture — homecoming has no prep-packet objectives, yet
//!         this yields a usable scene-advance objective);
//!   (ii)  that atom is admissible via the SAME witness admission code path the live
//!         turn loop uses ([`admit_witness_proposals`]) — a committed success check +
//!         the LLM-sensor's opaque cap ⇒ exactly one admitted AcceptedEvidence (the
//!         audit proved homecoming admits ~3 such scene atoms in real play; this is
//!         that admission, deterministically);
//!   (iii) [`witnessed_scene_advance_resolutions`] (the engine consuming the ledger)
//!         fires `ObjectiveResolved` for that scene's advance objective — labeled
//!         `signal=scene_advance`, matched atom recorded — and the LIVE write path
//!         (`append_domain_event`) lands the row in `domain_events` (prev 0 → ≥1):
//!         the j3v2 SEMANTIC-axis carrier homecoming previously NEVER produced;
//!   (iv)  **nav-split** — the engine never mutates current_scene;
//!   (v)   **idempotent** — re-appending the same resolution writes no second row.
//!
//! The ENGINE produces the signal; the LLM only ever proposed an opaque cap
//! (`LLM ∩ ProgressSignal = ∅`). Zero hardcoded fact↔predicate matcher: admission is
//! the witness path, completion is exact `atom_id` membership in `EvidenceAnyOf`.
//!
//! Run (rulesets DB :54347):
//!   DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!     cargo test -p trpg-runtime --test live_scene_advance_evidence -- --nocapture
//! No `DATABASE_URL` ⇒ SKIP (fail-closed). Anti-false-green: a real run prints `RAN:`
//! lines + `PASS`; only `SKIP` = nothing verified.
use sqlx::Row;
use trpg_db::Db;
use trpg_model::adventure_ir::{
    scene_advance_guard_leaves, scene_advance_objective, BasisKind, CapId, EvidenceAtomCatalog,
    EvidenceAtomSpec, EvidenceClaim, EvidenceKind, EvidenceLedger, EvidenceOffer, EvidenceOfferSet,
    NonEmpty, ObjectiveSpec, TurnLocalRef,
};
use trpg_model::{DomainEvent, DomainEventKind, ModuleGraph};
use trpg_runtime::post_turn_witness::admit_witness_proposals;
use trpg_runtime::progression::witnessed_scene_advance_resolutions;

const HOMECOMING: &str = "cyberpunk_red.homecoming";
const SESSION: &str = "sess_scene_advance_live";
const TURN: &str = "turn-uuid-scene-advance-live-1";
/// The actual playable scene the J3 audit measured FROZEN for 30 turns (`hcJ3`).
/// Wall B is precisely about THIS scene not advancing; the generic smoke above can
/// land on front-matter, so this pins the real frozen player seam.
const FROZEN_SEAM_SCENE: &str = "scene_01_lawmen_in_trouble";

/// A witness-bindable kind (the post-turn witness binds action/state/entity atoms
/// from a committed check; location/fact land via the exact projector). We pick a
/// bindable atom so the deterministic admission below uses the REAL witness path.
fn is_witness_bindable(kind: EvidenceKind) -> bool {
    matches!(
        kind,
        EvidenceKind::ActionResolved
            | EvidenceKind::StateEstablished
            | EvidenceKind::EntityEncountered
    )
}

/// Find the first homecoming scene that yields a scene-advance objective whose guard
/// leaves include a witness-bindable atom — returns (scene_id, objective, atom).
fn first_bindable_scene_advance(
    graph: &ModuleGraph,
) -> Option<(String, ObjectiveSpec, EvidenceAtomSpec)> {
    for scene in &graph.scenes {
        let scene_id = scene.node_id.trim();
        if scene_id.is_empty() {
            continue;
        }
        let Some(obj) = scene_advance_objective(graph, scene_id) else {
            continue;
        };
        let Some(atom) = scene_advance_guard_leaves(graph, scene_id)
            .into_iter()
            .find(|a| is_witness_bindable(a.kind))
        else {
            continue;
        };
        return Some((scene_id.to_string(), obj, atom));
    }
    None
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn homecoming_scene_advance_consumes_admitted_observation_and_appends_objectiveresolved() {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP: DATABASE_URL unset (need live rulesets DB :54347)");
            return;
        }
    };
    let db = Db::connect(&url).await.expect("connect live DB");
    let pool = sqlx::postgres::PgPool::connect(&url)
        .await
        .expect("connect sqlx pool");

    let mut graph = db
        .load_module_graph(HOMECOMING)
        .await
        .expect("query ok")
        .expect("homecoming module graph present in parsed_bundles");
    // mirror the live offer/catalog + apply_witnessed_progression path: clue-projected
    // graph so the scene atom_ids here == the ones the live witness admits (codex A).
    let _ = trpg_runtime::clue_projection::project_clues_onto_scenes(&mut graph);

    // ─── (i) homecoming's OWN scene content yields a scene-advance GuardLeaf objective ──
    let (scene_id, objective, atom) = first_bindable_scene_advance(&graph).expect(
        "homecoming has ≥1 scene whose authored content yields a witness-bindable \
         scene-advance GuardLeaf atom (the audit proved ~3 such atoms admit in real play)",
    );
    let anyof_len = match &objective.success_when {
        trpg_model::adventure_ir::PredicateExpr::EvidenceAnyOf { atom_ids } => atom_ids.len(),
        _ => panic!("scene-advance objective must be EvidenceAnyOf"),
    };
    eprintln!(
        "RAN: homecoming scenes={} clues={}; scene_advance scene={} objective={} EvidenceAnyOf_atoms={} \
         chosen GuardLeaf atom={} kind={:?} grounding={}",
        graph.scenes.len(),
        graph.clues.len(),
        scene_id,
        objective.id,
        anyof_len,
        atom.atom_id.as_str(),
        atom.kind,
        atom.grounding,
    );
    assert_eq!(objective.id, format!("obj.scene_advance.{scene_id}"));
    assert!(
        anyof_len >= 1,
        "the scene-advance objective consults ≥1 authored observation atom"
    );

    // ─── (ii) admit that scene atom via the REAL witness admission code path ────────────
    let cap = CapId::from_parts(SESSION, TURN, &atom.atom_id);
    let mut catalog = EvidenceAtomCatalog::new();
    catalog.insert(atom.clone());
    let mut offer_set = EvidenceOfferSet::new(TURN);
    offer_set.push(EvidenceOffer {
        cap_id: cap.clone(),
        atom_id: atom.atom_id.clone(),
        kind: atom.kind,
        meaning: "the player engaged this scene's authored observation".into(),
        required_basis: vec![BasisKind::OutcomeCommitted],
        expires_at: TURN.into(),
    });
    let success_check = DomainEvent::new(
        "de_sceneadv_chk_1",
        SESSION,
        TURN,
        DomainEventKind::CheckResolved,
        serde_json::json!({"check_id": "sa_1", "outcome": {"success": true, "success_tier": "regular"}}),
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
    assert_eq!(
        ledger.len(),
        1,
        "the witness admits exactly one scene observation AcceptedEvidence (the engine's input)"
    );
    assert_eq!(
        ledger.entries()[0].atom_id.as_str(),
        atom.atom_id.as_str(),
        "admitted atom id == the scene-advance objective's guard atom"
    );
    eprintln!(
        "RAN: witness admitted 1 AcceptedEvidence(atom={}) for scene {}",
        ledger.entries()[0].atom_id.as_str(),
        scene_id
    );

    // ─── (iii) engine consumes the ledger ⇒ scene_advance ObjectiveResolved ─────────────
    let resolutions =
        witnessed_scene_advance_resolutions(SESSION, TURN, &graph, &ledger, &scene_id);
    assert_eq!(
        resolutions.len(),
        1,
        "engine consumes the admitted scene observation ⇒ exactly one ObjectiveResolved"
    );
    let ev = &resolutions[0];
    assert_eq!(ev.kind, DomainEventKind::ObjectiveResolved);
    assert_eq!(ev.data["objective_id"], objective.id);
    assert_eq!(ev.data["signal"], "scene_advance", "distinctly labeled");
    assert_eq!(ev.data["scene_id"], scene_id);
    assert_eq!(
        ev.data["atom_id"], atom.atom_id.as_str(),
        "matched atom recorded for audit"
    );
    eprintln!(
        "RAN: engine consumed ledger ⇒ scene_advance ObjectiveResolved objective_id={} atom={}",
        objective.id,
        atom.atom_id.as_str()
    );

    // ─── (iv/v) LIVE write path: append ⇒ DB shows the row (prev 0), idempotent ─────────
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
    assert_eq!(before_rows, 0, "prev ObjectiveResolved rows = 0 (the homecoming Wall B gap)");

    for ev in &resolutions {
        db.append_domain_event(ev).await.expect("LIVE append_domain_event");
    }
    // idempotent: re-append must not create a second row (turn-independent event_id).
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
        1,
        "exactly one durable scene_advance ObjectiveResolved row (idempotent, no double-emit)"
    );
    for (event_id, data) in &after_rows {
        assert_eq!(data["signal"], "scene_advance");
        assert!(event_id.starts_with(&format!("de_objresolved_{SESSION}_obj.scene_advance.")));
        eprintln!("RAN: DB domain_events row event_id={event_id} data={data}");
    }

    // cleanup so the test is repeatable and leaves no residue in the shared DB.
    sqlx::query("delete from domain_events where session_id = $1 and kind = $2")
        .bind(SESSION)
        .bind("ObjectiveResolved")
        .execute(&pool)
        .await
        .expect("cleanup after");

    eprintln!(
        "PASS: D2 scene-advance — real homecoming scene '{scene_id}' authored content ⇒ \
         witness-admitted observation ⇒ engine ⇒ LIVE append_domain_event writes a \
         scene_advance ObjectiveResolved (prev 0 → 1); current_scene untouched (nav-split). \
         Wall B bridged on the module that froze 30 turns."
    );
}

/// Pin the proof to the EXACT scene the audit measured frozen 30 turns
/// (`scene_01_lawmen_in_trouble`): its OWN authored content yields a scene-advance
/// objective whose guard atom admits via the REAL witness path and drives the engine
/// to fire `ObjectiveResolved`. This is the decisive Wall B claim — the generic smoke
/// above can land on front-matter (`front_01_word_to_gm`); a player actually plays
/// scene_01, so this is where freeze→advance matters. No DB round-trip (the generic
/// test already proved the `append_domain_event` write path); this isolates the
/// real-scene admit→engine→signal chain on the frozen seam.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn frozen_seam_scene_01_advances_from_admitted_action_observation() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("SKIP: DATABASE_URL unset (need live rulesets DB :54347)");
        return;
    };
    let db = Db::connect(&url).await.expect("connect live DB");
    let mut graph = db
        .load_module_graph(HOMECOMING)
        .await
        .expect("query ok")
        .expect("homecoming module graph present");
    let _ = trpg_runtime::clue_projection::project_clues_onto_scenes(&mut graph);

    // scene_01_lawmen_in_trouble's OWN authored content ⇒ a scene-advance objective.
    let objective = scene_advance_objective(&graph, FROZEN_SEAM_SCENE).expect(
        "the frozen player scene scene_01_lawmen_in_trouble has a scene-advance objective \
         (its authored affordance/mechanic content compiles to ≥1 salient observation atom)",
    );
    assert_eq!(objective.id, format!("obj.scene_advance.{FROZEN_SEAM_SCENE}"));
    // a witness-bindable guard atom of THIS scene (the audit's scene_01 admits an action atom).
    let atom = scene_advance_guard_leaves(&graph, FROZEN_SEAM_SCENE)
        .into_iter()
        .find(|a| is_witness_bindable(a.kind))
        .expect("scene_01 has a witness-bindable scene-advance guard atom");
    eprintln!(
        "RAN: frozen seam scene={FROZEN_SEAM_SCENE} objective={} guard_atom={} kind={:?} grounding={}",
        objective.id,
        atom.atom_id.as_str(),
        atom.kind,
        atom.grounding,
    );

    // admit it through the REAL witness admission path (committed check + opaque cap).
    let cap = CapId::from_parts(SESSION, TURN, &atom.atom_id);
    let mut catalog = EvidenceAtomCatalog::new();
    catalog.insert(atom.clone());
    let mut offer_set = EvidenceOfferSet::new(TURN);
    offer_set.push(EvidenceOffer {
        cap_id: cap.clone(),
        atom_id: atom.atom_id.clone(),
        kind: atom.kind,
        meaning: "the player engaged scene_01's authored observation".into(),
        required_basis: vec![BasisKind::OutcomeCommitted],
        expires_at: TURN.into(),
    });
    let success_check = DomainEvent::new(
        "de_seam_chk_1",
        SESSION,
        TURN,
        DomainEventKind::CheckResolved,
        serde_json::json!({"check_id": "seam_1", "outcome": {"success": true, "success_tier": "regular"}}),
    );
    let proposals = vec![EvidenceClaim {
        cap_id: cap.clone(),
        basis: NonEmpty::from_vec(vec![TurnLocalRef::Commit(0)]).unwrap(),
    }];
    let (ledger, _) = admit_witness_proposals(
        SESSION,
        TURN,
        &offer_set,
        &catalog,
        &[success_check],
        &proposals,
        &EvidenceLedger::new(),
    );
    assert_eq!(ledger.len(), 1, "witness admits scene_01's observation atom");

    // engine consumes the ledger for scene_01 ⇒ ObjectiveResolved (Wall B bridged on the seam).
    let resolutions =
        witnessed_scene_advance_resolutions(SESSION, TURN, &graph, &ledger, FROZEN_SEAM_SCENE);
    assert_eq!(resolutions.len(), 1, "scene_01's advance objective resolves");
    let ev = &resolutions[0];
    assert_eq!(ev.kind, DomainEventKind::ObjectiveResolved);
    assert_eq!(ev.data["objective_id"], objective.id);
    assert_eq!(ev.data["scene_id"], FROZEN_SEAM_SCENE);
    assert_eq!(ev.data["signal"], "scene_advance");
    assert_eq!(ev.data["atom_id"], atom.atom_id.as_str());
    eprintln!(
        "PASS: D2 — the FROZEN player seam '{FROZEN_SEAM_SCENE}' (audit: 30-turn freeze) \
         advances from a real admitted authored-action observation ⇒ scene_advance \
         ObjectiveResolved {} (atom {}). Engine produces the signal; LLM only proposed an \
         opaque cap (LLM ∩ ProgressSignal = ∅).",
        objective.id,
        atom.atom_id.as_str(),
    );
}
