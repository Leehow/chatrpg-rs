//! NPC Mind View v1 live projection (TC-NPC-03): the durable `knowledge_edges` →
//! per-NPC mind-fact projection, against a real Postgres. Proves the query filters to
//! ONE NPC's own knowledge/beliefs and excludes GM world truth, player_party, and other
//! NPCs — the foundation a prompt-safe speech context is built on. Covers:
//!   - npc_mind_facts_include_known_and_belief    knows_true → Known; believes_false /
//!                                                misinformed → Belief; weak states dropped
//!   - npc_mind_facts_exclude_gm_truth_and_other_holders  gm / player_party / other-npc
//!                                                edges never enter this NPC's mind facts
//!   - npc_mind_view_excludes_gm_truth_from_speech  end-to-end: a GM truth the NPC lacks
//!                                                never reaches the speech context
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!      cargo test -p trpg-db --test live_npc_mind_view -- --nocapture
//! 无 DATABASE_URL 时 SKIP（fail-closed，不卡 CI）。
use trpg_db::{Db, KnowledgeEdgeInput};
use trpg_model::{
    KnowledgeState, NpcFactStanding, NpcKnowledgeEntry, NpcMindView, NpcProfile, NpcRelationship,
    NpcRelationshipDelta, NpcRelationshipTarget,
};

async fn connect_or_skip() -> Option<Db> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP: DATABASE_URL unset");
            return None;
        }
    };
    let db = match Db::connect(&url).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("SKIP: connect failed: {e}");
            return None;
        }
    };
    db.migrate()
        .await
        .expect("migrate failed after successful connect");
    Some(db)
}

async fn purge(db: &Db, session: &str) {
    sqlx::query("delete from knowledge_edges where session_id=$1")
        .bind(session)
        .execute(&db.pool)
        .await
        .unwrap();
}

/// Write one durable edge for an arbitrary durable holder via the generic upsert.
async fn edge(db: &Db, session: &str, kind: &str, holder_id: &str, fact: &str, state: &str) {
    db.upsert_knowledge_edge(KnowledgeEdgeInput {
        session_id: session,
        holder_kind: kind,
        holder_id,
        fact_id: fact,
        knowledge_state: state,
        confidence: None,
        learned_at_turn_id: None,
        disclosure_policy: None,
        source_event_id: None,
        reason: None,
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn npc_mind_facts_include_known_and_belief() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_mind_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    edge(
        &db,
        &session,
        "npc",
        "npc_alice",
        "fact_known",
        "knows_true",
    )
    .await;
    edge(
        &db,
        &session,
        "npc",
        "npc_alice",
        "fact_false",
        "believes_false",
    )
    .await;
    edge(
        &db,
        &session,
        "npc",
        "npc_alice",
        "fact_misinfo",
        "misinformed",
    )
    .await;
    // Weak / unknown states must NOT project as mind facts.
    edge(
        &db,
        &session,
        "npc",
        "npc_alice",
        "fact_rumor",
        "heard_about",
    )
    .await;
    edge(&db, &session, "npc", "npc_alice", "fact_susp", "suspects").await;

    let facts = db.list_npc_mind_facts(&session, "npc_alice").await.unwrap();
    let find = |id: &str| facts.iter().find(|f: &&NpcKnowledgeEntry| f.fact_id == id);
    assert_eq!(
        find("fact_known").map(|f| f.state),
        Some(KnowledgeState::KnowsTrue)
    );
    assert_eq!(
        find("fact_false").map(|f| f.state),
        Some(KnowledgeState::BelievesFalse)
    );
    assert_eq!(
        find("fact_misinfo").map(|f| f.state),
        Some(KnowledgeState::Misinformed)
    );
    assert!(
        find("fact_rumor").is_none(),
        "heard_about is not a mind fact"
    );
    assert!(find("fact_susp").is_none(), "suspects is not a mind fact");

    purge(&db, &session).await;
}

#[tokio::test]
async fn npc_mind_facts_exclude_gm_truth_and_other_holders() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_mind_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    edge(
        &db,
        &session,
        "npc",
        "npc_alice",
        "alice_fact",
        "knows_true",
    )
    .await;
    // Other holders' edges that must never appear in npc_alice's mind facts.
    edge(&db, &session, "gm", "", "gm_secret_fact", "knows_true").await;
    edge(
        &db,
        &session,
        "player_party",
        "",
        "party_fact",
        "knows_true",
    )
    .await;
    edge(&db, &session, "npc", "npc_bob", "bob_fact", "knows_true").await;

    let facts = db.list_npc_mind_facts(&session, "npc_alice").await.unwrap();
    let ids: Vec<&str> = facts.iter().map(|f| f.fact_id.as_str()).collect();
    assert_eq!(ids, vec!["alice_fact"], "only this NPC's own edges project");

    purge(&db, &session).await;
}

/// DA-KNOW-03 (live, canonical): the SAME fact diverges across two NPC holders and the
/// player. For one fact `F`: GM `knows_true` (world truth), NPC A `knows_true`, NPC B
/// `believes_false`, and player_party has NO edge (unknown). Each NPC's mind view is
/// projected ONLY from its own holder edges, so A reports F as Known while B reports it as
/// a (false) Belief, and the player projection stays empty. Negative controls: a player
/// reveal of a DIFFERENT fact does not enter either NPC's mind facts, and the NPC-held
/// fact never enters the player projection.
#[tokio::test]
async fn two_npcs_diverge_on_same_fact_player_unknown() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_mind_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;
    // also clear domain_events for the reveal negative control's idempotent key.
    sqlx::query("delete from domain_events where session_id=$1")
        .bind(&session)
        .execute(&db.pool)
        .await
        .unwrap();

    let shared = "fact_who_poisoned_the_well";
    edge(&db, &session, "gm", "", shared, "knows_true").await; // world truth
    edge(&db, &session, "npc", "npc_a", shared, "knows_true").await; // A knows
    edge(&db, &session, "npc", "npc_b", shared, "believes_false").await; // B falsely believes
                                                                         // player_party: no edge for `shared` → unknown.

    let a_profile = NpcProfile {
        actor_id: "npc_a".into(),
        name: "Avis".into(),
        ..Default::default()
    };
    let b_profile = NpcProfile {
        actor_id: "npc_b".into(),
        name: "Bex".into(),
        ..Default::default()
    };
    let a_facts = db.list_npc_mind_facts(&session, "npc_a").await.unwrap();
    let b_facts = db.list_npc_mind_facts(&session, "npc_b").await.unwrap();
    let a_view = NpcMindView::build(&session, "npc_a", &a_profile, &[], &a_facts).unwrap();
    let b_view = NpcMindView::build(&session, "npc_b", &b_profile, &[], &b_facts).unwrap();

    // Holder-specific divergence on the SAME fact id.
    assert_eq!(
        a_view.known_fact_ids(),
        vec![shared],
        "NPC A knows the shared fact"
    );
    assert!(a_view.belief_fact_ids().is_empty());
    assert_eq!(
        b_view.belief_fact_ids(),
        vec![shared],
        "NPC B only (falsely) believes it"
    );
    assert!(
        b_view.known_fact_ids().is_empty(),
        "NPC B's false belief is never reported as known truth"
    );
    let b_standing = b_view
        .facts
        .iter()
        .find(|f| f.fact_id == shared)
        .map(|f| f.standing);
    assert_eq!(b_standing, Some(NpcFactStanding::Belief));

    // Player remains unknown: NPC-held knowledge never enters the player projection.
    assert!(
        db.player_knowledge_view(&session).await.unwrap().is_empty(),
        "player_party must not inherit NPC knowledge"
    );

    // Negative control: a player reveal of a DIFFERENT fact does not pollute NPC minds,
    // and the player's known fact is not the NPC-held one.
    db.record_revealed_fact(
        &session,
        "t_reveal",
        "fact_player_only",
        Some("read handout"),
    )
    .await
    .unwrap();
    assert_eq!(
        db.player_knowledge_view(&session).await.unwrap(),
        vec!["fact_player_only".to_string()],
        "player learns only its own revealed fact"
    );
    let a_after = db.list_npc_mind_facts(&session, "npc_a").await.unwrap();
    let b_after = db.list_npc_mind_facts(&session, "npc_b").await.unwrap();
    assert!(
        a_after.iter().all(|f| f.fact_id != "fact_player_only"),
        "player reveal does not enter NPC A mind facts"
    );
    assert!(
        b_after.iter().all(|f| f.fact_id != "fact_player_only"),
        "player reveal does not enter NPC B mind facts"
    );
    assert!(
        !db.player_knowledge_view(&session)
            .await
            .unwrap()
            .contains(&shared.to_string()),
        "the NPC-held fact never becomes player-known via NPC updates"
    );

    purge(&db, &session).await;
    sqlx::query("delete from domain_events where session_id=$1")
        .bind(&session)
        .execute(&db.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn npc_mind_view_excludes_gm_truth_from_speech() {
    // End-to-end against the shared store: a GM world truth the NPC does NOT hold must
    // never reach the prompt-safe speech context built from the mind view.
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_mind_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    edge(
        &db,
        &session,
        "npc",
        "npc_alice",
        "alice_knows",
        "knows_true",
    )
    .await;
    edge(&db, &session, "gm", "", "gm_secret_fact", "knows_true").await;

    let mut rel =
        NpcRelationship::new(&session, "npc_alice", NpcRelationshipTarget::PlayerParty).unwrap();
    rel.apply_delta(&NpcRelationshipDelta::threat(vec!["e".into()]))
        .unwrap();

    let profile = NpcProfile {
        actor_id: "npc_alice".into(),
        name: "Alice".into(),
        role: Some("innkeeper".into()),
        ..Default::default()
    };
    let facts = db.list_npc_mind_facts(&session, "npc_alice").await.unwrap();
    let view = NpcMindView::build(&session, "npc_alice", &profile, &[rel], &facts).unwrap();
    let ctx = view.speech_context();

    assert!(ctx.contains("alice_knows"));
    assert!(
        !ctx.contains("gm_secret_fact"),
        "GM truth must not leak into NPC speech context"
    );
    assert_eq!(view.known_fact_ids(), vec!["alice_knows"]);
    assert!(view
        .facts
        .iter()
        .all(|f| f.standing == NpcFactStanding::Known || f.standing == NpcFactStanding::Belief));

    purge(&db, &session).await;
}
