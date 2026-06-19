//! Knowledge P0a — DB-gated 标准注册表 dispatch 活测：经 ToolRegistry::standard()
//! 真派发 reveal_fact，证明工具结果 `revealed:true` 且 db.list_revealed_facts 落账。
//! DATABASE_URL 缺失时打印 SKIP 并返回（绝不伪 PASS）。
//! 运行：DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!       cargo test -p trpg-gm reveal_fact_tool_writes_fact_revealed_ledger -- --nocapture

#[tokio::test]
async fn reveal_fact_tool_writes_fact_revealed_ledger() {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP: DATABASE_URL unset");
            return;
        }
    };
    let db = match trpg_db::Db::connect(&url).await {
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

    let session = format!("session_reveal_tool_{}", uuid::Uuid::new_v4().simple());
    let engine = trpg_runtime::RuntimeEngine::new(db.clone());
    let request = trpg_model::ContextRequest {
        ruleset_id: "call_of_cthulhu_7e".to_string(),
        module_id: None,
        session_id: session.clone(),
        turn_id: "turn_reveal_1".to_string(),
        viewer: trpg_model::VisibilityProfile::gm(),
        token_budget: trpg_model::TokenBudget::default(),
    };
    let state = trpg_model::RuntimeState {
        ruleset_id: "call_of_cthulhu_7e".to_string(),
        ..Default::default()
    };
    let ctx = trpg_gm::ToolCtx {
        engine: &engine,
        request: &request,
        state: &state,
        scene_extractor: None,
        obligations: None,
        data_dir: None,
        current_mode: None,
        opposed_binding: None,
        nominated_reveals: None,
        rejected_nominations: None,
    };
    let registry = trpg_gm::ToolRegistry::standard();
    let mut ledger = trpg_gm::TurnLedger::new();
    let out = registry
        .dispatch(
            &ctx,
            &mut ledger,
            &trpg_llm::AggregatedToolCall {
                id: "call_reveal_1".to_string(),
                name: "reveal_fact".to_string(),
                arguments: serde_json::json!({"fact_id":"npc_butler","reason":"玩家读完日记"})
                    .to_string(),
            },
        )
        .await;

    let content: serde_json::Value = serde_json::from_str(&out.content).unwrap();
    assert_eq!(
        content.pointer("/revealed").and_then(|v| v.as_bool()),
        Some(true),
        "{content}"
    );
    assert_eq!(
        db.list_revealed_facts(&session).await.unwrap(),
        vec!["npc_butler".to_string()]
    );

    // P0c：reveal_fact 写穿的域事件 kind 是 PlayerLearnedFact（取代旧 FactRevealed）。
    let kind: String = sqlx::query_scalar("select kind from domain_events where event_id = $1")
        .bind(format!("de_revealed_{session}_npc_butler"))
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        kind, "PlayerLearnedFact",
        "reveal_fact 经派发后写 PlayerLearnedFact 域事件"
    );
}
