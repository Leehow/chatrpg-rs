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
//!     → `npc_action::resolve_world_attack_intents` → `RuntimeEngine::resolve_check_with_input`
//!        (the SAME entry the GM `roll_check` tool uses — the roll happens in Rules)
//!     → a real `check_results` row lands (mechanical proof in Rules, not World).
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
use trpg_runtime::RuntimeEngine;

const RULESET: &str = "call_of_cthulhu_7e";

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
    let turn_id = "turn_p46_live";
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

    // Rules resolves it: route through the SAME mechanical entry the GM roll_check uses.
    let engine = RuntimeEngine::new(db.clone());
    let facts =
        trpg_gm::npc_action::resolve_world_attack_intents(&engine, &session, turn_id, RULESET, &set)
            .await;
    assert!(
        !facts.is_empty(),
        "Attack intent must reach a real resolution (roll_check in Rules)"
    );
    eprintln!("LIVE resolution fact: {}", facts[0]);

    // The roll committed a real check_results row (mechanical state lives in Rules).
    let check_id = format!("npc_attack:{turn_id}:{npc_id}");
    let landed: i64 =
        sqlx::query_scalar("select count(*) from check_results where check_id = $1")
            .bind(&check_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(landed, 1, "a real check_result row must land for the NPC attack");

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
}

fn profile_slice(p: &NpcProfile) -> Vec<NpcProfile> {
    vec![p.clone()]
}
