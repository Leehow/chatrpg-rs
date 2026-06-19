//! TC-D3-03 Memory Proposal Commit Pipeline — live acceptance (runtime-owned commit).
//!
//! Exercises [`trpg_runtime::review_and_commit_proposals`]: a validated proposal batch is
//! reviewed by runtime and committed to the right durable owner — `memory_facts`,
//! `knowledge_edges` / domain events, or `npc_relationships` — while invalid batches write
//! nothing. Plugins/agents never reach this path; they only build proposals.
//!
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!      cargo test -p trpg-runtime --test memory_proposal_commit -- --nocapture
//! No DATABASE_URL (or DB unreachable) → SKIP (fail-closed, never blocks CI). The pure
//! atomicity gate is additionally covered DB-free by the `memory_proposal::tests` unit test
//! `invalid_proposal_batch_commits_nothing`.
//!
//! Tests are wrapped in `mod memory_proposal` so the `cargo test -p trpg-runtime
//! memory_proposal` name filter selects them.
mod memory_proposal {
    use serde_json::json;
    use tokio::sync::{Mutex, MutexGuard};
    use trpg_db::Db;
    use trpg_runtime::{review_and_commit_proposals, try_proposals_from_json, CommitContext};

    /// Serialize live DB tests in this binary. Under cargo's default parallel runner,
    /// concurrent `migrate()` DDL (AccessExclusiveLock) racing other tests' DML deadlocks
    /// on a fresh DB; holding this for each test's DB work makes them run one at a time.
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

    /// Connect, take the process-wide serialization lock, migrate (idempotent, now safe
    /// under the lock), and prepare a clean session. Returns the guard so the caller holds
    /// the lock for the whole test body. `None` (SKIP) when no DB is reachable.
    async fn setup(session: &str) -> Option<(Db, MutexGuard<'static, ()>)> {
        let db = connect_or_skip().await?;
        let guard = DB_SERIAL.lock().await;
        db.migrate().await.expect("migrate after connect");
        prepare(&db, session).await;
        Some((db, guard))
    }

    async fn purge(db: &Db, session: &str) {
        // Child tables first, then the sessions row last (FK parent), so cleanup is safe
        // regardless of ON DELETE behavior. Scoped strictly to the test session.
        for sql in [
            "delete from memory_facts where session_id=$1",
            "delete from world_facts where session_id=$1",
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

    /// `memory_facts` / `domain_events` carry a FK to `sessions(session_id)`, so the commit
    /// pipeline can only write for a real session. Create one before committing.
    async fn prepare(db: &Db, session: &str) {
        purge(db, session).await;
        db.create_session(session, "call_of_cthulhu_7e", None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn commit_world_fact_candidate_writes_fact_store() {
        let session = format!("sess_mpc_{}", uuid::Uuid::new_v4().simple());
        let Some((db, _guard)) = setup(&session).await else {
            return;
        };
        let ctx = CommitContext {
            session_id: &session,
            turn_id: "turn1",
        };

        let proposals = try_proposals_from_json(&json!({"proposals": [{
            "proposal_kind": "world_fact",
            "fact_id": "f_world_commit",
            "subject": "raul", "predicate": "wrote", "object": "letter",
            "summary": "Raul wrote the letter.", "confidence": 0.9,
            "source_event_ids": ["ev_w1"]
        }]}))
        .expect("valid world fact proposal");

        let report = review_and_commit_proposals(&db, &ctx, &proposals)
            .await
            .unwrap();
        assert_eq!(report.committed(), 1, "world fact committed");
        assert_eq!(report.rejected(), 0);

        let (subject, predicate): (String, String) = sqlx::query_as(
            "select subject, predicate from memory_facts where session_id=$1 and fact_id=$2",
        )
        .bind(&session)
        .bind("f_world_commit")
        .fetch_one(&db.pool)
        .await
        .expect("world fact row persisted to memory_facts");
        assert_eq!(subject, "raul");
        assert_eq!(predicate, "wrote");

        // Knowledge kernel defaults OFF → baseline: NO world_facts identity row, NO GM truth
        // edge. (Off == baseline; the additive layered-runtime state is gated on the flag.)
        let wf_count: i64 =
            sqlx::query_scalar("select count(*) from world_facts where session_id=$1")
                .bind(&session)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(
            wf_count, 0,
            "OFF must not write the additive world_facts row"
        );
        assert!(
            db.gm_truth_view(&session).await.unwrap().is_empty(),
            "OFF must not derive a GM truth edge"
        );

        purge(&db, &session).await;
    }

    /// P3 ON-path (shadow) live behavior assertion (§24-#8): with TRPG_KNOWLEDGE_KERNEL=shadow,
    /// committing a `truth_status=true` world fact (a) keeps the baseline `memory_facts` row
    /// (recall preserved), (b) ALSO writes the first-class `world_facts` identity row, (c)
    /// derives a (gm, knows_true) edge so the fact enters `gm_truth_view`, and (d) does NOT
    /// reveal it to the player (player_knowledge_view stays empty — GM 确知，玩家未知).
    /// Env is mutated only while holding DB_SERIAL (all DB tests in this binary serialize on
    /// it), then restored — no env race. This is the REAL ON-path probe the loop requires
    /// (not OFF==baseline). SKIP when no DB.
    #[tokio::test]
    async fn commit_world_fact_shadow_mode_dual_writes_and_derives_gm_truth() {
        let session = format!("sess_mpc_{}", uuid::Uuid::new_v4().simple());
        let Some((db, _guard)) = setup(&session).await else {
            return;
        };
        // Guarded by DB_SERIAL (held via _guard): safe to set/restore the process-global flag.
        let prev = std::env::var("TRPG_KNOWLEDGE_KERNEL").ok();
        std::env::set_var("TRPG_KNOWLEDGE_KERNEL", "shadow");

        let ctx = CommitContext {
            session_id: &session,
            turn_id: "turn1",
        };
        let proposals = try_proposals_from_json(&json!({"proposals": [{
            "proposal_kind": "world_fact",
            "fact_id": "f_world_shadow",
            "subject": "gate", "predicate": "is", "object": "open",
            "truth_status": "true", "confidence": 0.9,
            "source_event_ids": ["ev_s1"]
        }]}))
        .expect("valid true world fact proposal");

        let report = review_and_commit_proposals(&db, &ctx, &proposals)
            .await
            .unwrap();
        assert_eq!(report.committed(), 1, "world fact committed");

        // (a) baseline memory_facts row present (recall preserved).
        let mf: i64 = sqlx::query_scalar(
            "select count(*) from memory_facts where session_id=$1 and fact_id='f_world_shadow'",
        )
        .bind(&session)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(
            mf, 1,
            "baseline memory_facts row must still be written (recall)"
        );

        // (b) first-class world_facts identity row present (additive ON path).
        let (subj, truth): (String, Option<String>) = sqlx::query_as(
            "select subject, truth_status from world_facts where session_id=$1 and fact_id=$2",
        )
        .bind(&session)
        .bind("f_world_shadow")
        .fetch_one(&db.pool)
        .await
        .expect("world_facts identity row persisted under shadow mode");
        assert_eq!(subj, "gate");
        assert_eq!(truth.as_deref(), Some("true"));

        // (c) GM truth edge derived → fact enters gm_truth_view.
        let gm = db.gm_truth_view(&session).await.unwrap();
        assert!(
            gm.contains(&"f_world_shadow".to_string()),
            "true world fact must enter gm_truth_view (GM knows), got {gm:?}"
        );
        // (d) player does NOT learn it (§24-#8: PlayerLearnedFact only when player hears).
        let player = db.player_knowledge_view(&session).await.unwrap();
        assert!(
            !player.contains(&"f_world_shadow".to_string()),
            "GM truth must NOT auto-reveal to player, got {player:?}"
        );

        match prev {
            Some(v) => std::env::set_var("TRPG_KNOWLEDGE_KERNEL", v),
            None => std::env::remove_var("TRPG_KNOWLEDGE_KERNEL"),
        }
        purge(&db, &session).await;
    }

    #[tokio::test]
    async fn commit_legacy_memory_fact_uses_context_session() {
        // A legacy relationship-triple `memory_fact` proposal whose session matches the
        // commit context commits a durable row under the context session (the runtime, not
        // the proposal, owns the destination — see the pure session-authority unit tests).
        let session = format!("sess_mpc_{}", uuid::Uuid::new_v4().simple());
        let Some((db, _guard)) = setup(&session).await else {
            return;
        };
        let ctx = CommitContext {
            session_id: &session,
            turn_id: "turn1",
        };

        let proposals = try_proposals_from_json(&json!({"proposals": [{
            "proposal_kind": "memory_fact",
            "fact": {
                "fact_id": "mf_legacy_commit", "session_id": session,
                "scope": {"scope_type": "session", "scope_id": session},
                "visibility": "gm_only",
                "subject": "raul", "predicate": "wrote", "object": "letter",
                "summary": "Raul wrote the letter.", "status": "active", "confidence": 0.9,
                "source_event_ids": ["ev_l1"], "tags": ["relationship"], "importance": 1,
                "turn_id": "turn7",
                "created_at": "2026-06-18T00:00:00Z", "updated_at": "2026-06-18T00:00:00Z"
            }
        }]}))
        .expect("valid legacy memory_fact proposal");

        let report = review_and_commit_proposals(&db, &ctx, &proposals)
            .await
            .unwrap();
        assert_eq!(report.committed(), 1, "legacy memory_fact committed");

        let scope_id: String = sqlx::query_scalar(
            "select scope_id from memory_facts where session_id=$1 and fact_id='mf_legacy_commit'",
        )
        .bind(&session)
        .fetch_one(&db.pool)
        .await
        .expect("legacy fact row persisted under the context session");
        assert_eq!(
            scope_id, session,
            "session-scope id normalized to the context session"
        );

        purge(&db, &session).await;
    }

    #[tokio::test]
    async fn commit_knowledge_update_uses_runtime_knowledge_path() {
        let session = format!("sess_mpc_{}", uuid::Uuid::new_v4().simple());
        let Some((db, _guard)) = setup(&session).await else {
            return;
        };
        let ctx = CommitContext {
            session_id: &session,
            turn_id: "turn1",
        };

        // npc + knows_true routes through the NPC-learned runtime API → durable npc edge.
        let proposals = try_proposals_from_json(&json!({"proposals": [{
            "proposal_kind": "knowledge_update",
            "fact_id": "f_known",
            "holder": {"holder_kind": "npc", "holder_id": "npc_alice"},
            "knowledge_state": "knows_true",
            "source_event_ids": ["ev_k1"]
        }]}))
        .expect("valid knowledge update proposal");

        let report = review_and_commit_proposals(&db, &ctx, &proposals)
            .await
            .unwrap();
        assert_eq!(report.committed(), 1, "knowledge update committed");

        let known = db
            .list_npc_known_fact_ids(&session, "npc_alice")
            .await
            .unwrap();
        assert!(
            known.contains(&"f_known".to_string()),
            "durable npc knows_true edge written"
        );

        // The fact is NOT visible to the player party (NPC mind ≠ player knowledge).
        let player_known = db
            .list_holder_known_fact_ids(&session, "player_party", "")
            .await
            .unwrap();
        assert!(
            !player_known.contains(&"f_known".to_string()),
            "no leak to player_party"
        );

        purge(&db, &session).await;
    }

    #[tokio::test]
    async fn commit_pc_faction_knowledge_update_writes_durable_edge() {
        // pc / faction holders are durable (this task): a valid knowledge_update commits a
        // durable knowledge edge under that holder, readable back by holder projection and
        // never leaked to player_party.
        let session = format!("sess_mpc_{}", uuid::Uuid::new_v4().simple());
        let Some((db, _guard)) = setup(&session).await else {
            return;
        };
        let ctx = CommitContext {
            session_id: &session,
            turn_id: "turn1",
        };

        let proposals = try_proposals_from_json(&json!({"proposals": [
            {
                "proposal_kind": "knowledge_update",
                "fact_id": "f_pc",
                "holder": {"holder_kind": "pc", "holder_id": "pc_hero"},
                "knowledge_state": "knows_true",
                "source_event_ids": ["ev_pc"]
            },
            {
                "proposal_kind": "knowledge_update",
                "fact_id": "f_faction",
                "holder": {"holder_kind": "faction", "holder_id": "guild_thieves"},
                "knowledge_state": "suspects",
                "source_event_ids": ["ev_fac"]
            }
        ]}))
        .expect("valid pc/faction knowledge updates");

        let report = review_and_commit_proposals(&db, &ctx, &proposals)
            .await
            .unwrap();
        assert_eq!(report.committed(), 2, "pc + faction edges committed");
        assert_eq!(report.rejected(), 0);

        // pc knows_true → durable edge readable by holder projection.
        assert_eq!(
            db.list_holder_known_fact_ids(&session, "pc", "pc_hero")
                .await
                .unwrap(),
            vec!["f_pc".to_string()],
            "durable pc knows_true edge written and projected"
        );
        // faction suspects is a belief, not known truth → NOT in its known projection,
        // but the durable edge row exists.
        assert!(
            db.list_holder_known_fact_ids(&session, "faction", "guild_thieves")
                .await
                .unwrap()
                .is_empty(),
            "faction suspects is belief, not known truth"
        );
        let faction_rows: i64 = sqlx::query_scalar(
            "select count(*) from knowledge_edges where session_id=$1 and holder_kind='faction' \
             and holder_id='guild_thieves' and fact_id='f_faction' and knowledge_state='suspects'",
        )
        .bind(&session)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(faction_rows, 1, "durable faction suspects edge written");

        // Neither leaks into player_party knowledge.
        let player_known = db
            .list_holder_known_fact_ids(&session, "player_party", "")
            .await
            .unwrap();
        assert!(
            !player_known.contains(&"f_pc".to_string())
                && !player_known.contains(&"f_faction".to_string()),
            "pc/faction edges never leak to player_party"
        );

        purge(&db, &session).await;
    }

    #[tokio::test]
    async fn commit_pc_faction_unstable_id_writes_nothing() {
        // An unstable pc/faction id fails the model gate → batch aborts before any write,
        // proving fail-closed at the live commit boundary (display-name shaped id).
        let session = format!("sess_mpc_{}", uuid::Uuid::new_v4().simple());
        let Some((db, _guard)) = setup(&session).await else {
            return;
        };
        let ctx = CommitContext {
            session_id: &session,
            turn_id: "turn1",
        };

        // Build directly so the unstable entry reaches the pipeline's pre-validation gate.
        let raw = json!({"proposals": [{
            "proposal_kind": "knowledge_update",
            "fact_id": "f_bad",
            "holder": {"holder_kind": "faction", "holder_id": "Thieves Guild"},
            "knowledge_state": "knows_true",
            "source_event_ids": ["ev_bad"]
        }]});
        let proposals: Vec<trpg_model::MemoryExtractionProposal> = raw["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| serde_json::from_value(v.clone()).unwrap())
            .collect();

        let report = review_and_commit_proposals(&db, &ctx, &proposals)
            .await
            .unwrap();
        assert!(report.committed_nothing(), "unstable id commits nothing");
        assert_eq!(report.rejected(), 1);

        let count: i64 =
            sqlx::query_scalar("select count(*) from knowledge_edges where session_id=$1")
                .bind(&session)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(
            count, 0,
            "no durable edge written for an unstable holder id"
        );

        purge(&db, &session).await;
    }

    #[tokio::test]
    async fn commit_relationship_delta_reuses_evidence_gate() {
        let session = format!("sess_mpc_{}", uuid::Uuid::new_v4().simple());
        let Some((db, _guard)) = setup(&session).await else {
            return;
        };
        let ctx = CommitContext {
            session_id: &session,
            turn_id: "turn1",
        };

        let proposals = try_proposals_from_json(&json!({"proposals": [{
            "proposal_kind": "npc_relationship_delta",
            "session_id": session,
            "npc_id": "npc_lars",
            "target": {"target_kind": "player_party"},
            "delta": {"trust": 12, "respect": 8, "debt": 15, "evidence_event_ids": ["ev_help"]}
        }]}))
        .expect("valid relationship delta proposal");

        let report = review_and_commit_proposals(&db, &ctx, &proposals)
            .await
            .unwrap();
        assert_eq!(report.committed(), 1, "relationship delta committed");

        let rel = db
            .load_npc_relationship(&session, "npc_lars", "player_party", "")
            .await
            .unwrap()
            .expect("relationship persisted through the bounded, evidence-gated path");
        assert!(rel.trust > 0 && rel.debt > 0, "delta applied");
        assert_eq!(
            rel.evidence_event_ids,
            vec!["ev_help".to_string()],
            "evidence recorded"
        );

        purge(&db, &session).await;
    }

    #[tokio::test]
    async fn invalid_batch_writes_no_rows_live() {
        // DB-level mirror of the pure atomicity unit test: one evidence-less proposal poisons
        // the batch; the valid sibling must NOT reach memory_facts.
        let session = format!("sess_mpc_{}", uuid::Uuid::new_v4().simple());
        let Some((db, _guard)) = setup(&session).await else {
            return;
        };
        let ctx = CommitContext {
            session_id: &session,
            turn_id: "turn1",
        };

        // Build directly (bypassing the strict parser) so the invalid entry reaches the
        // pipeline's own pre-validation gate inside a mixed batch.
        let raw = json!({"proposals": [
            {
                "proposal_kind": "world_fact",
                "fact_id": "f_valid_sibling", "subject": "a", "predicate": "rel", "object": "b",
                "source_event_ids": ["ev_ok"]
            },
            {
                "proposal_kind": "world_fact",
                "fact_id": "f_bad", "subject": "a", "predicate": "rel", "object": "b",
                "source_event_ids": []
            }
        ]});
        let proposals: Vec<trpg_model::MemoryExtractionProposal> = raw["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| serde_json::from_value(v.clone()).unwrap())
            .collect();

        let report = review_and_commit_proposals(&db, &ctx, &proposals)
            .await
            .unwrap();
        assert!(report.committed_nothing(), "invalid batch commits nothing");
        assert_eq!(report.rejected(), 2);

        let count: i64 =
            sqlx::query_scalar("select count(*) from memory_facts where session_id=$1")
                .bind(&session)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(
            count, 0,
            "no row written for either proposal in a poisoned batch"
        );

        purge(&db, &session).await;
    }
}
