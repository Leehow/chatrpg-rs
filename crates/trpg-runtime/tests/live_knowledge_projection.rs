//! Runtime KnowledgeProjection live 测试（P0b）：record_revealed_fact 写穿后，
//! player_knowledge_projection 能从 KnowledgeEdge 账本读回该 fact_id。
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!      cargo test -p trpg-runtime player_projection_reads_knowledge_edges -- --nocapture
//! 无 DATABASE_URL 时 SKIP（fail-closed，不卡 CI）。
use trpg_db::Db;
use trpg_runtime::knowledge_projection::player_knowledge_projection;
use uuid::Uuid;

#[tokio::test]
async fn player_projection_reads_knowledge_edges() {
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

    let session = format!("sess_kproj_{}", Uuid::new_v4().simple());
    // 写穿路径：record_revealed_fact 落 domain_events + knowledge_edges。
    db.record_revealed_fact(&session, "turn1", "npc_butler", Some("diary"))
        .await
        .unwrap();

    let proj = player_knowledge_projection(&db, &session).await.unwrap();
    assert!(
        proj.revealed_fact_ids.contains("npc_butler"),
        "投影应含写穿的 npc_butler，got={:?}",
        proj.revealed_fact_ids
    );
    // 未揭示事实不在集内。
    assert!(!proj.revealed_fact_ids.contains("npc_never_revealed"));

    sqlx::query("delete from knowledge_edges where session_id=$1")
        .bind(&session)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("delete from domain_events where session_id=$1")
        .bind(&session)
        .execute(&db.pool)
        .await
        .unwrap();
}
