//! TC-D3-03 commit pipeline — DB-layer acceptance for the durable owner APIs the
//! runtime commit pipeline drives. The runtime crate ([`trpg_runtime::memory_proposal`])
//! decides *which* path a validated proposal takes; this suite proves the underlying
//! `trpg-db` helpers (`upsert_memory_fact`, `record_npc_learned_fact`,
//! `upsert_npc_relationship`) durably persist what the pipeline commits — independent of
//! the runtime layer, so the durable contract is covered from the DB side too.
//!
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!      cargo test -p trpg-db --test memory_proposal -- --nocapture
//! No DATABASE_URL (or DB unreachable) → SKIP (fail-closed, never blocks CI).
//!
//! Wrapped in `mod memory_proposal` so the `cargo test -p trpg-db memory_proposal` filter
//! selects these.
mod memory_proposal {
    use tokio::sync::{Mutex, MutexGuard};
    use trpg_db::Db;
    use trpg_model::{MemoryFact, NpcRelationship, NpcRelationshipDelta, NpcRelationshipTarget};

    /// Serialize live DB tests in this binary: concurrent `migrate()` DDL racing other
    /// tests' DML deadlocks on a fresh DB under cargo's default parallel runner.
    static DB_SERIAL: Mutex<()> = Mutex::const_new(());

    async fn connect_or_skip() -> Option<Db> {
        let url = match std::env::var("DATABASE_URL") {
            Ok(u) => u,
            Err(_) => {
                eprintln!("SKIP: DATABASE_URL unset");
                return None;
            }
        };
        match Db::connect(&url).await {
            Ok(db) => Some(db),
            Err(e) => {
                eprintln!("SKIP: connect failed: {e}");
                None
            }
        }
    }

    /// Connect, take the serialization lock, migrate (idempotent, safe under the lock), and
    /// prepare a clean session. Caller holds the returned guard for the whole test body.
    async fn setup(session: &str) -> Option<(Db, MutexGuard<'static, ()>)> {
        let db = connect_or_skip().await?;
        let guard = DB_SERIAL.lock().await;
        db.migrate().await.expect("migrate after connect");
        prepare(&db, session).await;
        Some((db, guard))
    }

    async fn purge(db: &Db, session: &str) {
        // Child tables first, then the sessions row last (FK parent). Scoped to the session.
        for sql in [
            "delete from memory_facts where session_id=$1",
            "delete from knowledge_edges where session_id=$1",
            "delete from domain_events where session_id=$1",
            "delete from npc_relationships where session_id=$1",
            "delete from sessions where session_id=$1",
        ] {
            sqlx::query(sql)
                .bind(session)
                .execute(&db.pool)
                .await
                .unwrap();
        }
    }

    /// `memory_facts` / `domain_events` carry a FK to `sessions(session_id)`; create the
    /// session row before committing facts/learned-edges for it.
    async fn prepare(db: &Db, session: &str) {
        purge(db, session).await;
        db.create_session(session, "call_of_cthulhu_7e", None)
            .await
            .unwrap();
    }

    /// A world-fact-shaped `MemoryFact` built via serde so the test needs no chrono dep.
    fn world_fact(session: &str) -> MemoryFact {
        serde_json::from_value(serde_json::json!({
            "fact_id": "f_world_db",
            "session_id": session,
            "scope": {"scope_type": "session", "scope_id": session},
            "visibility": "gm_only",
            "subject": "raul", "predicate": "wrote", "object": "letter",
            "summary": "Raul wrote the letter.",
            "status": "active", "confidence": 0.5,
            "source_event_ids": ["ev_w1"],
            "tags": ["memory", "proposal", "world_fact"],
            "importance": 1, "turn_id": "turn1",
            "created_at": "2026-06-18T00:00:00Z", "updated_at": "2026-06-18T00:00:00Z"
        }))
        .expect("valid MemoryFact json")
    }

    #[tokio::test]
    async fn world_fact_commit_persists_to_memory_facts() {
        let session = format!("sess_mpdb_{}", uuid::Uuid::new_v4().simple());
        let Some((db, _guard)) = setup(&session).await else {
            return;
        };

        db.upsert_memory_fact(&world_fact(&session)).await.unwrap();
        // Idempotent re-commit keeps a single row (the pipeline upserts on replay).
        db.upsert_memory_fact(&world_fact(&session)).await.unwrap();

        let count: i64 = sqlx::query_scalar(
            "select count(*) from memory_facts where session_id=$1 and fact_id='f_world_db'",
        )
        .bind(&session)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(count, 1, "world fact upsert is idempotent");

        purge(&db, &session).await;
    }

    #[tokio::test]
    async fn npc_learned_and_relationship_commit_durably() {
        let session = format!("sess_mpdb_{}", uuid::Uuid::new_v4().simple());
        let Some((db, _guard)) = setup(&session).await else {
            return;
        };

        // KnowledgeUpdate(npc, knows_true) commit path.
        db.record_npc_learned_fact(&session, "turn1", "npc_alice", "f_known_db", None)
            .await
            .unwrap();
        let known = db
            .list_npc_known_fact_ids(&session, "npc_alice")
            .await
            .unwrap();
        assert!(
            known.contains(&"f_known_db".to_string()),
            "durable npc knows_true edge"
        );

        // NpcRelationshipDelta commit path (load-default + bounded/evidence-gated apply).
        let mut rel =
            NpcRelationship::new(&session, "npc_lars", NpcRelationshipTarget::PlayerParty).unwrap();
        NpcRelationshipDelta::help(vec!["ev_help".into()])
            .apply_to(&mut rel)
            .unwrap();
        db.upsert_npc_relationship(&rel).await.unwrap();
        let loaded = db
            .load_npc_relationship(&session, "npc_lars", "player_party", "")
            .await
            .unwrap()
            .expect("relationship persisted");
        assert!(loaded.trust > 0 && loaded.debt > 0);

        purge(&db, &session).await;
    }
}
