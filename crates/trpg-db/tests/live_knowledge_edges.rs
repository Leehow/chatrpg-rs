//! KnowledgeEdge 投影 live 测试（P0b）：upsert player_party 边后，
//! list_player_known_fact_ids 只返回 (player_party, knows_true) 的 fact_id，
//! believes_false 等其它知识态不入兼容投影。
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!      cargo test -p trpg-db --test live_knowledge_edges -- --nocapture
//! 无 DATABASE_URL 时 SKIP（fail-closed，不卡 CI）。
use trpg_db::Db;

#[tokio::test]
async fn knowledge_projection_returns_player_party_knows_true_only() {
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

    let session = format!("sess_ke_{}", uuid::Uuid::new_v4().simple());
    db.upsert_knowledge_edge_player_party(&session, "turn1", "npc_butler", "knows_true", Some("diary"))
        .await
        .unwrap();
    db.upsert_knowledge_edge_player_party(&session, "turn1", "npc_secret_false", "believes_false", Some("rumor"))
        .await
        .unwrap();

    let got = db.list_player_known_fact_ids(&session).await.unwrap();
    assert_eq!(got, vec!["npc_butler".to_string()], "只投影 knows_true，believes_false 不入");

    // 幂等：同 (session, fact) 重 upsert 不产生重复，仍只一条。
    db.upsert_knowledge_edge_player_party(&session, "turn7", "npc_butler", "knows_true", Some("再次确认"))
        .await
        .unwrap();
    let again = db.list_player_known_fact_ids(&session).await.unwrap();
    assert_eq!(again, vec!["npc_butler".to_string()], "重 upsert 幂等不重复");

    sqlx::query("delete from knowledge_edges where session_id=$1")
        .bind(&session)
        .execute(&db.pool)
        .await
        .unwrap();
}
