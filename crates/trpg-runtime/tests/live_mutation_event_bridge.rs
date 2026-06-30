//! CL-EV1b live DB proof: EV-1 `mutation_event_bridge_v1` — every persistent commit emits one
//! canonical `DomainEvent` (single commit write-path seam), on the REAL rulesets DB.
//!
//! What this proves deterministically (no LLM, no judged eval — EV-1 changes no play quality):
//!   1. **OFF (baseline)**: committing a world_fact + player-knowledge batch yields ZERO
//!      `WorldFactChanged` rows in `domain_events` (the previously-empty carrier). `PlayerLearnedFact`
//!      rows DO appear at baseline — honestly, because `record_revealed_fact` already emits one for a
//!      knows_true reveal (the bridge does NOT double-emit there).
//!   2. **ON (flag set)**: the same batch now ALSO writes a `WorldFactChanged` row (previously 0),
//!      with a resolvable `fact_id` in its data, PLUS an extra `PlayerLearnedFact` carrier for the
//!      non-true player belief (PlayerPartyEdge) under a distinct `de_pp_edge_*` key.
//!
//! Run (rulesets DB :54347, this worktree's target dir):
//!   DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!     CARGO_TARGET_DIR=target-air cargo test -p trpg-runtime \
//!     --test live_mutation_event_bridge -- --nocapture
//!
//! No `DATABASE_URL` ⇒ SKIP (fail-closed, never blocks CI). A real run prints `RAN:` + counts +
//! `PASS`; only seeing `SKIP` = not verified. The pure per-arm seam mapping is additionally covered
//! DB-free by the `memory_proposal::tests::bridge_event_*` unit tests.
mod bridge {
    use serde_json::{json, Value};
    use tokio::sync::{Mutex, MutexGuard};
    use trpg_db::Db;
    use trpg_runtime::{review_and_commit_proposals, try_proposals_from_json, CommitContext};

    static DB_SERIAL: Mutex<()> = Mutex::const_new(());

    async fn setup(session: &str) -> Option<(Db, MutexGuard<'static, ()>)> {
        let url = match std::env::var("DATABASE_URL") {
            Ok(u) => u,
            Err(_) => {
                eprintln!("SKIP: DATABASE_URL unset (need live rulesets DB :54347)");
                return None;
            }
        };
        let db = match Db::connect(&url).await {
            Ok(db) => db,
            Err(e) => {
                eprintln!("SKIP: connect failed: {e}");
                return None;
            }
        };
        let guard = DB_SERIAL.lock().await;
        db.migrate().await.expect("migrate after connect");
        purge(&db, session).await;
        db.create_session(session, "call_of_cthulhu_7e", None)
            .await
            .unwrap();
        Some((db, guard))
    }

    async fn purge(db: &Db, session: &str) {
        for sql in [
            "delete from memory_facts where session_id=$1",
            "delete from world_facts where session_id=$1",
            "delete from knowledge_edges where session_id=$1",
            "delete from domain_events where session_id=$1",
            "delete from sessions where session_id=$1",
        ] {
            sqlx::query(sql)
                .bind(session)
                .execute(&db.pool)
                .await
                .unwrap();
        }
    }

    /// A batch hitting three silent/baseline arms: WorldFact (silent → WorldFactChanged when ON),
    /// PlayerLearned/knows_true (already emits PlayerLearnedFact), PlayerPartyEdge/suspects (silent
    /// → PlayerLearnedFact when ON, distinct key).
    fn batch() -> Value {
        json!({"proposals": [
            {
                "proposal_kind": "world_fact",
                "fact_id": "f_wfc", "subject": "gate", "predicate": "is", "object": "open",
                "truth_status": "true", "source_event_ids": ["ev1"]
            },
            {
                "proposal_kind": "knowledge_update",
                "fact_id": "f_known", "holder": {"holder_kind": "player_party"},
                "knowledge_state": "knows_true", "source_event_ids": ["ev1"]
            },
            {
                "proposal_kind": "knowledge_update",
                "fact_id": "f_susp", "holder": {"holder_kind": "player_party"},
                "knowledge_state": "suspects", "source_event_ids": ["ev1"]
            }
        ]})
    }

    async fn commit(db: &Db, session: &str) {
        let proposals = try_proposals_from_json(&batch()).expect("valid batch");
        let ctx = CommitContext {
            session_id: session,
            turn_id: "turn_ev1",
        };
        let report = review_and_commit_proposals(db, &ctx, &proposals)
            .await
            .expect("commit ok");
        assert!(
            report.committed() >= 3,
            "all three proposals must commit (got {})",
            report.committed()
        );
    }

    /// (event_id, kind) rows for the session, seq order.
    async fn events(db: &Db, session: &str) -> Vec<(String, String, Value)> {
        let rows = sqlx::query_as::<_, (String, String, Value)>(
            "select event_id, kind, data from domain_events where session_id=$1 order by seq",
        )
        .bind(session)
        .fetch_all(&db.pool)
        .await
        .unwrap();
        rows
    }

    fn count_kind(evs: &[(String, String, Value)], kind: &str) -> usize {
        evs.iter().filter(|(_, k, _)| k == kind).count()
    }

    #[tokio::test]
    async fn bridge_off_zero_world_fact_changed_then_on_emits_canonical_events() {
        let off_session = format!("sess_ev1off_{}", uuid::Uuid::new_v4().simple());
        // ---- OFF phase: flag unset → baseline ----
        std::env::remove_var("TRPG_MUTATION_EVENT_BRIDGE_V1");
        std::env::remove_var("TRPG_PROGRESS_EVIDENCE_V1");
        let Some((db, guard)) = setup(&off_session).await else {
            return;
        };
        commit(&db, &off_session).await;
        let off = events(&db, &off_session).await;
        let off_wfc = count_kind(&off, "WorldFactChanged");
        let off_plf = count_kind(&off, "PlayerLearnedFact");
        eprintln!(
            "RAN: OFF session WorldFactChanged={off_wfc} PlayerLearnedFact={off_plf} total={}",
            off.len()
        );
        assert_eq!(
            off_wfc, 0,
            "OFF must emit ZERO WorldFactChanged (baseline carrier was empty)"
        );
        assert_eq!(
            off_plf, 1,
            "OFF baseline: exactly the knows_true reveal's PlayerLearnedFact (no bridge double-emit)"
        );
        purge(&db, &off_session).await;

        // ---- ON phase: bridge flag set → canonical events appear ----
        let on_session = format!("sess_ev1on_{}", uuid::Uuid::new_v4().simple());
        std::env::set_var("TRPG_MUTATION_EVENT_BRIDGE_V1", "1");
        purge(&db, &on_session).await;
        db.create_session(&on_session, "call_of_cthulhu_7e", None)
            .await
            .unwrap();
        commit(&db, &on_session).await;
        let on = events(&db, &on_session).await;
        std::env::remove_var("TRPG_MUTATION_EVENT_BRIDGE_V1");

        let on_wfc = count_kind(&on, "WorldFactChanged");
        let on_plf = count_kind(&on, "PlayerLearnedFact");
        eprintln!(
            "RAN: ON  session WorldFactChanged={on_wfc} PlayerLearnedFact={on_plf} total={}",
            on.len()
        );
        for (id, k, d) in &on {
            eprintln!("  ev kind={k} id={id} data={d}");
        }
        // Headline: WorldFactChanged now present (was 0), with a resolvable fact_id.
        assert!(
            on_wfc >= 1,
            "ON must emit ≥1 WorldFactChanged (previously 0)"
        );
        let wfc = on
            .iter()
            .find(|(_, k, _)| k == "WorldFactChanged")
            .expect("a WorldFactChanged row");
        assert_eq!(
            wfc.2.get("fact_id").and_then(Value::as_str),
            Some("f_wfc"),
            "WorldFactChanged carries a resolvable fact_id"
        );
        assert_eq!(wfc.0, format!("de_worldfact_{on_session}_f_wfc_turn_ev1"));
        // The non-true player belief adds a SECOND PlayerLearnedFact under the de_pp_edge key —
        // distinct from the knows_true reveal, carrying its knowledge_state.
        assert_eq!(
            on_plf, 2,
            "ON: knows_true reveal + bridged PlayerPartyEdge = 2 PlayerLearnedFact"
        );
        let pp = on
            .iter()
            .find(|(id, _, _)| id.starts_with("de_pp_edge_"))
            .expect("a bridged PlayerPartyEdge PlayerLearnedFact");
        assert_eq!(pp.1, "PlayerLearnedFact");
        assert_eq!(
            pp.2.get("knowledge_state").and_then(Value::as_str),
            Some("suspects")
        );

        purge(&db, &on_session).await;
        drop(guard);
        eprintln!("PASS: OFF==0 WorldFactChanged; ON emits canonical WorldFactChanged + PlayerLearnedFact carriers");
    }
}
