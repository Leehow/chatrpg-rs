//! NPC mind DB-bound read live test (P1 slice-1, DB layer).
//!
//! Demonstrates the first durable NPC-as-holder read slice at the DB boundary: an
//! `npc` holder `knowledge_edges` row (seeded directly via SQL — there is no gameplay
//! writer by design) is read by `list_npc_knowledge_entries` and feeds
//! `NpcMindView::build`, while the `npc.opposition` placeholder is rejected at both the
//! DB read API and the DB CHECK backstop. The runtime `load_npc_mind_view` /
//! `load_npc_behavior_plan` wrappers are covered end-to-end in
//! `trpg-runtime/tests/live_npc_mind_view.rs`.
//!
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!      cargo test -p trpg-db --test live_npc_mind_view -- --nocapture
//! 无 DATABASE_URL 时 SKIP（fail-closed，不卡 CI）。
use trpg_db::Db;
use trpg_model::{NpcFactStanding, NpcMindView, NpcProfile};

/// Seed one npc-holder knowledge_edge directly (no gameplay writer exists by design).
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
async fn npc_edges_read_into_mind_view_and_opposition_rejected() {
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

    let session = format!("sess_npcmind_{}", uuid::Uuid::new_v4().simple());
    let npc_id = "npc_alice";

    // This NPC's own edges: a known truth, a false belief, and an excluded weak state.
    seed_npc_edge(&db, &session, npc_id, "hidden_passage", "knows_true").await;
    seed_npc_edge(&db, &session, npc_id, "rumor_about_lord", "believes_false").await;
    seed_npc_edge(&db, &session, npc_id, "half_overheard", "exposed").await;
    // Another NPC's edge in the same session must NOT leak into Alice's read.
    seed_npc_edge(&db, &session, "npc_bob", "bobs_secret", "knows_true").await;

    // 1) Raw read API returns only this NPC's edges, fact-id ordered.
    let entries = db.list_npc_knowledge_entries(&session, npc_id).await.unwrap();
    let fact_ids: Vec<&str> = entries.iter().map(|e| e.fact_id.as_str()).collect();
    assert_eq!(
        fact_ids,
        vec!["half_overheard", "hidden_passage", "rumor_about_lord"],
        "only this NPC's edges, ordered by fact_id"
    );

    // 2) Feed the DB-read entries + relationships into NpcMindView::build: known + belief
    //    survive, the weak state drops, and the false belief is labeled Belief.
    let relationships = db.list_npc_relationships(&session, npc_id).await.unwrap();
    let profile = NpcProfile {
        actor_id: npc_id.into(),
        name: "Alice".into(),
        role: Some("innkeeper".into()),
        ..Default::default()
    };
    let view = NpcMindView::build(&session, npc_id, &profile, &relationships, &entries).unwrap();
    assert_eq!(view.npc_id, npc_id);
    assert_eq!(view.known_fact_ids(), vec!["hidden_passage"]);
    assert_eq!(view.belief_fact_ids(), vec!["rumor_about_lord"]);
    assert!(
        view.facts.iter().all(|f| f.fact_id != "half_overheard"),
        "exposed (weak) state must not project into the mind view"
    );
    assert!(
        matches!(
            view.facts.iter().find(|f| f.fact_id == "rumor_about_lord").unwrap().standing,
            NpcFactStanding::Belief
        ),
        "false belief is labeled Belief, never world truth"
    );

    // 3) npc.opposition placeholder is rejected at the DB read API (fail-closed id gate).
    assert!(
        db.list_npc_knowledge_entries(&session, "npc.opposition").await.is_err(),
        "npc.opposition must be rejected by the read API, never queried as a holder"
    );
    assert!(
        db.list_npc_knowledge_entries(&session, "").await.is_err(),
        "empty id must be rejected by the read API"
    );

    // 4) DB CHECK backstop: a direct write of an npc.opposition holder row is refused even
    //    if Rust validation were bypassed; an empty npc holder_id is likewise refused.
    let opp = sqlx::query(
        r#"insert into knowledge_edges
             (edge_id, session_id, holder_kind, holder_id, fact_id, knowledge_state)
           values ($1, $2, 'npc', 'npc.opposition', 'x', 'knows_true')"#,
    )
    .bind(format!("ke_bad_opp_{session}"))
    .bind(&session)
    .execute(&db.pool)
    .await;
    assert!(opp.is_err(), "DB CHECK must reject npc holder anchored to npc.opposition");

    let empty = sqlx::query(
        r#"insert into knowledge_edges
             (edge_id, session_id, holder_kind, holder_id, fact_id, knowledge_state)
           values ($1, $2, 'npc', '', 'x', 'knows_true')"#,
    )
    .bind(format!("ke_bad_empty_{session}"))
    .bind(&session)
    .execute(&db.pool)
    .await;
    assert!(empty.is_err(), "DB CHECK must reject npc holder with an empty holder_id");

    // Cleanup.
    sqlx::query("delete from knowledge_edges where session_id=$1")
        .bind(&session)
        .execute(&db.pool)
        .await
        .unwrap();
}
