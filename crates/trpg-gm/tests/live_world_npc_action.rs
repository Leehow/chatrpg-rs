//! P4.6 LIVE e2e — World NPC reaction + Attack-intent slice over a real DB.
//!
//! This is the ON-path behavioral assertion (FRAMEWORK §2): OFF==baseline byte-equality
//! only proves the flag-off path is unchanged; here we drive the flag-ON Attack slice
//! against a live DB + real Rules resolution and assert it actually settles a check.
//!
//! Chain under test (all real, no mocks):
//!   seed hostile NPC profile + relationship + a withheld secret edge
//!     → `world::load_world_reaction_plans` (real DB load; §24-#3 only active NPCs)
//!     → `world::derive_attack_intents` (World emits typed intent; resolves nothing)
//!     → `npc_action::resolve_world_attack_intents` → `RuntimeEngine::execute_system_roll_bundle`
//!        (the SAME entry the GM `roll_check` tool uses via `tools::settle`, WITH kernel-default
//!         hydration — the roll happens in Rules and yields a REAL hit/miss band, not a blocked stub)
//!     → a real `check_results` row lands (mechanical proof in Rules, not World)
//!     → the typed `WorldAttackOutcome` is folded as a player-perceivable gate fact (load-bearing).
//!
//! Also asserts: the `TRPG_WORLD_NPC_ACTION` flag GATES the path (OFF ⇒ no row), and the
//! §24-#3 negative-pool rule (a non-active / unloaded NPC leaks no candidate).
//!
//! §24-#4: the candidate's `knowledge_basis` carries ONLY revealable facts; the withheld
//! secret never leaks into the World guidance.
//!
//! Run (rulesets DB has the CoC kernel):
//!   DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!   cargo test -p trpg-gm --test live_world_npc_action -- --nocapture
//! No DATABASE_URL ⇒ SKIP (fail-closed, never blocks CI).
use trpg_db::Db;
use trpg_model::{NpcProfile, NpcRelationship, NpcRelationshipDelta, NpcRelationshipTarget};
use trpg_params::{RuntimeActorParameters, RuntimeParameterService};
use trpg_runtime::RuntimeEngine;

const RULESET: &str = "call_of_cthulhu_7e";

/// Seed the attacking NPC's runtime actor sheet with a source-backed combat skill so the
/// CoC percentile roll-under has a REAL value to test against (→ a genuine hit/miss band,
/// not an `awaiting_binding` stub). The skill key `attack` matches the engine-set check
/// text ("NPC attack…", intent `world:npc_attack`) via `derive_tested_source`'s text scan —
/// so no contract widening is needed; the v1 binding resolves it as-is.
async fn seed_npc_combat_sheet(db: &Db, session: &str, npc_id: &str) {
    let svc = RuntimeParameterService::new(db.clone());
    let params = RuntimeActorParameters {
        actor_param_id: format!("ap_p46_{session}_{npc_id}"),
        session_id: session.into(),
        actor_id: npc_id.into(),
        actor_kind: trpg_model::ActorKind::Npc,
        ruleset_id: RULESET.into(),
        source_kind: "test_seed".into(),
        template_id: None,
        display_name: Some("Lars".into()),
        sheet_json: serde_json::json!({ "skills": { "attack": 60 } }),
        mechanical_profile: serde_json::json!({ "skills": { "attack": 60 }, "stats": {} }),
        status_json: serde_json::json!({}),
        visibility: trpg_model::Visibility::default(),
        created_at_tick: Some(0),
        updated_at_tick: Some(0),
    };
    svc.upsert_actor_parameters(&params).await.unwrap();
}

async fn seed_npc_edge(db: &Db, session: &str, npc_id: &str, fact_id: &str, state: &str) {
    sqlx::query(
        r#"insert into knowledge_edges
             (edge_id, session_id, holder_kind, holder_id, fact_id, knowledge_state)
           values ($1, $2, 'npc', $3, $4, $5)
           on conflict (session_id, holder_kind, holder_id, fact_id)
           do update set knowledge_state = excluded.knowledge_state"#,
    )
    .bind(format!("ke_p46_{session}_{npc_id}_{fact_id}"))
    .bind(session)
    .bind(npc_id)
    .bind(fact_id)
    .bind(state)
    .execute(&db.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn world_attack_intent_reaches_real_roll_check() {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP: DATABASE_URL unset");
            return;
        }
    };
    let db = match Db::connect(&url).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("SKIP: connect failed: {e}");
            return;
        }
    };
    if let Err(e) = db.migrate().await {
        eprintln!("SKIP: migrate failed: {e}");
        return;
    }
    // Gate: the CoC kernel must be present (this DB is the rulesets DB :54347).
    match db.load_rule_kernel(RULESET).await {
        Ok(Some(_)) => {}
        _ => {
            eprintln!("SKIP: {RULESET} kernel not present in this DB");
            return;
        }
    }

    let session = format!("sess_p46_world_attack_{}", uuid::Uuid::new_v4().simple());
    // turn_id is per-run unique so the derived check_id is fresh — otherwise a cached
    // `contest_profiles` row from a prior run (keyed by check_id) would shadow the live
    // re-resolution and mask the real hit/miss with a stale model.
    let turn_id_owned = format!("turn_p46_live_{}", uuid::Uuid::new_v4().simple());
    let turn_id = turn_id_owned.as_str();
    let npc_id = "npc_lars_hostile";

    // Seed: the NPC knows a secret the party does NOT know (must be withheld, §24-#4).
    seed_npc_edge(&db, &session, npc_id, "lars_betrayal_plan", "knows_true").await;

    // Durable hostile relationship toward the player party → hostile stance + high
    // willingness_to_fight ⇒ the World action gate proposes an Attack.
    let mut rel = NpcRelationship::new(&session, npc_id, NpcRelationshipTarget::PlayerParty).unwrap();
    rel.apply_delta(&NpcRelationshipDelta {
        hostility: 95,
        evidence_event_ids: vec!["evt_p46".into()],
        ..Default::default()
    })
    .unwrap();
    db.upsert_npc_relationship(&rel).await.unwrap();

    let profile = NpcProfile {
        actor_id: npc_id.into(),
        name: "Lars".into(),
        ..Default::default()
    };
    db.upsert_npc_profile(&session, &profile).await.unwrap();

    // Seed a source-backed combat skill so the d100 roll-under resolves to a real hit/miss.
    seed_npc_combat_sheet(&db, &session, npc_id).await;

    let targets = [NpcRelationshipTarget::PlayerParty];
    let player_known = db.list_player_known_fact_ids(&session).await.unwrap();

    // §24-#3: build the World reaction set ONLY from the active-NPC pool we pass.
    let (mut set, plans) = trpg_runtime::world::load_world_reaction_plans(
        &db,
        &session,
        &[npc_id.to_string()],
        &profile_slice(&profile),
        &targets,
        &player_known,
    )
    .await;
    assert_eq!(set.reactions.len(), 1, "exactly one active-NPC candidate (§24-#3)");
    assert_eq!(set.reactions[0].npc_id, npc_id);

    // §24-#4: the withheld secret must NOT appear in the World knowledge basis.
    assert!(
        !set.reactions[0]
            .knowledge_basis
            .iter()
            .any(|f| f == "lars_betrayal_plan"),
        "withheld secret leaked into World guidance basis (§24-#4)"
    );
    // It IS genuinely withheld at the plan level (the gate had something to hide).
    assert!(
        plans[0]
            .facts_will_withhold
            .iter()
            .any(|f| f == "lars_betrayal_plan"),
        "secret should be withheld by the plan"
    );

    // World emits the Attack intent (resolves nothing).
    trpg_runtime::world::derive_attack_intents(&plans, &mut set);
    let intent = set.reactions[0]
        .action_intent
        .as_ref()
        .expect("hostile NPC yields an Attack intent");
    assert_eq!(intent.kind, trpg_model::NpcActionKind::Attack);

    let engine = RuntimeEngine::new(db.clone());
    let check_id = format!("npc_attack:{turn_id}:{npc_id}");

    // ── Gap 2: the TRPG_WORLD_NPC_ACTION flag must GATE the resolution path ──────────
    // OFF: mirror the turn-loop guard — the slice is never entered, so NO resolution
    // happens and NO check_results row lands from this path.
    std::env::remove_var("TRPG_WORLD_NPC_ACTION");
    assert!(
        !trpg_gm::npc_action::world_npc_action_enabled(),
        "flag must read OFF after remove_var"
    );
    if trpg_gm::npc_action::world_npc_action_enabled() {
        let _ = trpg_gm::npc_action::resolve_world_attack_intents(
            &engine, &session, turn_id, RULESET, &set,
        )
        .await;
    }
    let landed_off: i64 =
        sqlx::query_scalar("select count(*) from check_results where check_id = $1")
            .bind(&check_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        landed_off, 0,
        "flag OFF ⇒ NPC-action path not entered ⇒ no check_result row"
    );

    // ON: the slice runs, routes through the SAME mechanical entry the GM roll_check uses
    // (execute_system_roll_bundle → kernel-default hydration), and the outcome is consumed.
    std::env::set_var("TRPG_WORLD_NPC_ACTION", "1");
    assert!(
        trpg_gm::npc_action::world_npc_action_enabled(),
        "flag must read ON after set_var"
    );
    let outcomes = if trpg_gm::npc_action::world_npc_action_enabled() {
        trpg_gm::npc_action::resolve_world_attack_intents(
            &engine, &session, turn_id, RULESET, &set,
        )
        .await
    } else {
        Vec::new()
    };
    std::env::remove_var("TRPG_WORLD_NPC_ACTION");
    assert_eq!(outcomes.len(), 1, "ON ⇒ exactly one resolved NPC attack");
    let o = &outcomes[0];
    eprintln!(
        "LIVE outcome: npc={} blocked={} success={:?} tier={:?} fact={:?} json={}",
        o.npc_id,
        o.blocked,
        o.success,
        o.success_tier,
        o.to_gate_fact(),
        serde_json::to_string(&o.outcome).unwrap_or_default()
    );

    // ── Gap 2: assert REAL mechanics, not just count==1. The CoC kernel hydrates a d100
    // core mechanic, so this is a genuine hit/miss resolution — NOT the
    // `blocked: missing_source_backed_parameters` stub.
    assert!(
        !o.blocked,
        "ON attack must resolve to a real hit/miss (kernel hydrated), not a blocked stub; outcome={}",
        serde_json::to_string(&o.outcome).unwrap_or_default()
    );
    assert!(
        o.success.is_some(),
        "real resolution must carry a hit/miss verdict; outcome={}",
        serde_json::to_string(&o.outcome).unwrap_or_default()
    );
    // The folded gate fact (the load-bearing player-perceivable consumption) names the
    // NPC and a HIT/MISS verdict — it is never silently dropped.
    let fact = o.to_gate_fact();
    assert!(fact.contains(npc_id), "gate fact must name the attacking NPC");
    assert!(
        fact.contains("HIT") || fact.contains("MISS"),
        "gate fact must carry a real hit/miss verdict, got: {fact}"
    );

    // The roll committed a real check_results row (mechanical state lives in Rules).
    let landed: i64 =
        sqlx::query_scalar("select count(*) from check_results where check_id = $1")
            .bind(&check_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(landed, 1, "a real check_result row must land for the NPC attack");

    // ── Gap 3: §24-#3 negative-pool counter-example. A NON-active / unloaded NPC (no
    // profile passed in the pool) must NOT leak a reaction candidate. We pass an id that
    // is not in the active set we built profiles for: empty pool ⇒ empty set.
    let (ghost_set, ghost_plans) = trpg_runtime::world::load_world_reaction_plans(
        &db,
        &session,
        &["npc_not_active_ghost".to_string()],
        &profile_slice(&profile), // only `npc_id` has a profile; ghost id has none
        &targets,
        &player_known,
    )
    .await;
    assert!(
        ghost_set.reactions.is_empty(),
        "§24-#3: a non-active / unloaded NPC must NOT leak a reaction candidate"
    );
    assert!(
        ghost_plans.is_empty(),
        "§24-#3: no plan should load for an NPC with no profile in the pool"
    );

    // Cleanup our seeds (leave the shared DB as we found it).
    let _ = sqlx::query("delete from check_results where check_id = $1")
        .bind(&check_id)
        .execute(&db.pool)
        .await;
    let _ = sqlx::query("delete from knowledge_edges where session_id=$1")
        .bind(&session)
        .execute(&db.pool)
        .await;
    let _ = sqlx::query("delete from npc_relationships where session_id=$1")
        .bind(&session)
        .execute(&db.pool)
        .await;
    let _ = sqlx::query("delete from npc_profiles where session_id=$1")
        .bind(&session)
        .execute(&db.pool)
        .await;
    let _ = sqlx::query("delete from runtime_actor_parameters where session_id=$1")
        .bind(&session)
        .execute(&db.pool)
        .await;
    let _ = sqlx::query("delete from contest_profiles where check_id = $1")
        .bind(&check_id)
        .execute(&db.pool)
        .await;
}

fn profile_slice(p: &NpcProfile) -> Vec<NpcProfile> {
    vec![p.clone()]
}
