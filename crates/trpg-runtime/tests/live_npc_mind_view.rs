//! Runtime NPC mind/behavior DB-bound load live test (P1 slice-1, runtime layer).
//!
//! End-to-end over a live DB: durable `npc`-holder `knowledge_edges` + relationships are
//! assembled by `load_npc_mind_view`, and `load_npc_behavior_plan` derives a plan that
//! withholds an NPC-known fact the player party does not know. The `npc.opposition`
//! placeholder fails closed at the runtime boundary.
//!
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!      cargo test -p trpg-runtime --test live_npc_mind_view -- --nocapture
//! 无 DATABASE_URL 时 SKIP（fail-closed，不卡 CI）。
use trpg_db::Db;
use trpg_model::{NpcProfile, NpcRelationshipTarget};

async fn seed_npc_edge(db: &Db, session: &str, npc_id: &str, fact_id: &str, state: &str) {
    sqlx::query(
        r#"insert into knowledge_edges
             (edge_id, session_id, holder_kind, holder_id, fact_id, knowledge_state)
           values ($1, $2, 'npc', $3, $4, $5)
           on conflict (session_id, holder_kind, holder_id, fact_id)
           do update set knowledge_state = excluded.knowledge_state"#,
    )
    .bind(format!("ke_test_{session}_{npc_id}_{fact_id}"))
    .bind(session)
    .bind(npc_id)
    .bind(fact_id)
    .bind(state)
    .execute(&db.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn load_npc_mind_and_behavior_from_durable_edges() {
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

    let session = format!("sess_rt_npcmind_{}", uuid::Uuid::new_v4().simple());
    let npc_id = "npc_alice";

    // Alice knows two true facts and holds one false belief.
    seed_npc_edge(&db, &session, npc_id, "hidden_passage", "knows_true").await;
    seed_npc_edge(&db, &session, npc_id, "alices_own_secret", "knows_true").await;
    seed_npc_edge(&db, &session, npc_id, "rumor_about_lord", "believes_false").await;
    // The party already knows one of the true facts (shared, freely revealable).
    db.upsert_knowledge_edge_player_party(&session, "t1", "hidden_passage", "knows_true", None)
        .await
        .unwrap();

    let profile = NpcProfile {
        actor_id: npc_id.into(),
        name: "Alice".into(),
        role: Some("innkeeper".into()),
        ..Default::default()
    };

    // The player party is the relationship target the runtime gates secrets against.
    let relationship_targets = [NpcRelationshipTarget::PlayerParty];

    // load_npc_mind_view assembles relationships + knowledge from the DB.
    let view =
        trpg_runtime::load_npc_mind_view(&db, &session, npc_id, &profile, &relationship_targets)
            .await
            .unwrap();
    assert_eq!(view.npc_id, npc_id);
    let mut known = view.known_fact_ids();
    known.sort();
    assert_eq!(known, vec!["alices_own_secret", "hidden_passage"]);
    assert_eq!(view.belief_fact_ids(), vec!["rumor_about_lord"]);

    // load_active_npc_guidance: the production active-NPC secret gate derives the player
    // party's known facts from durable edges, so a true fact the party does NOT know is
    // withheld; the shared fact is not; the false belief never becomes a secret.
    let player_known = db.list_player_known_fact_ids(&session).await.unwrap();
    let plan = trpg_runtime::load_active_npc_guidance(
        &db,
        &session,
        npc_id,
        &profile,
        &relationship_targets,
        &player_known,
    )
    .await
    .unwrap();
    assert!(
        plan.facts_will_withhold
            .iter()
            .any(|f| f == "alices_own_secret"),
        "NPC-known fact the party lacks becomes a withheld secret"
    );
    assert!(
        !plan
            .facts_will_withhold
            .iter()
            .any(|f| f == "hidden_passage"),
        "fact the party already knows is not withheld"
    );
    assert!(
        !plan
            .facts_will_withhold
            .iter()
            .any(|f| f == "rumor_about_lord"),
        "a mere belief is never a secret"
    );

    // npc.opposition fails closed at the runtime boundary.
    assert!(
        trpg_runtime::load_npc_mind_view(
            &db,
            &session,
            "npc.opposition",
            &profile,
            &relationship_targets
        )
        .await
        .is_err(),
        "runtime mind view must reject the opposition placeholder"
    );

    sqlx::query("delete from knowledge_edges where session_id=$1")
        .bind(&session)
        .execute(&db.pool)
        .await
        .unwrap();
}
