//! Knowledge P0c — Reveal Events v1 活测：四个 reveal 语义是分离的领域概念/投影输入。
//! ContextSurfaced（进 GM/context，隐藏装载）≠ PlayerExposed（玩家可见暴露，不揭秘）≠
//! PlayerLearnedFact（player_party 确知事实，写穿 KnowledgeEdge）≠ NpcLearnedFact（某个
//! 具体 NPC 习得事实，只作用于目标 NPC，TC-KNOW-04 起写穿 durable npc 边并保留事件账本）。
//!
//! 无 DATABASE_URL 时 SKIP（fail-closed，不卡 CI）。模型层（trpg-model domain_event）与
//! truthgraph 纯函数另有 DB-free 单测覆盖 kind 三/四分与 context≠exposure。
//! 运行：DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!       cargo test -p trpg-db --test live_semantic_events -- --nocapture
use chrono::{DateTime, Utc};
use serde_json::json;
use trpg_db::Db;
use trpg_model::{DomainEvent, DomainEventKind};

/// 连接 + migrate；无 DATABASE_URL 或连不上即返回 None（调用方 SKIP）。
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
            eprintln!("SKIP: connect: {e}");
            return None;
        }
    };
    if let Err(e) = db.migrate().await {
        eprintln!("SKIP: migrate: {e}");
        return None;
    }
    Some(db)
}

fn ev(
    session: &str,
    turn: &str,
    event_id: &str,
    kind: DomainEventKind,
    entity_id: &str,
    ek: &str,
) -> DomainEvent {
    DomainEvent {
        event_id: event_id.into(),
        session_id: session.into(),
        turn_id: turn.into(),
        kind,
        data: json!({ "entity_id": entity_id, "entity_kind": ek, "scene_id": "sc01" }),
        source_refs: Vec::new(),
        created_at: DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
    }
}

async fn purge(db: &Db, session: &str) {
    sqlx::query("delete from domain_events where session_id=$1")
        .bind(session)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("delete from knowledge_edges where session_id=$1")
        .bind(session)
        .execute(&db.pool)
        .await
        .unwrap();
}

/// 验收①：ContextSurfaced（进 GM/context）**不**授予玩家知识——既不计入玩家暴露投影，
/// 也不进 revealed-facts；只在 context 投影里可见。
#[tokio::test]
async fn context_surfaced_does_not_grant_player_knowledge() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_ctx_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    // 纯 ContextSurfaced（场景把线索装进 GM context）+ 一条噪声事件。
    db.append_domain_event(&ev(
        &session,
        "t_ctx",
        &format!("de_surfaced_{session}_clue_ctx"),
        DomainEventKind::ContextSurfaced,
        "clue_ctx",
        "clue",
    ))
    .await
    .unwrap();
    db.append_domain_event(&DomainEvent {
        event_id: format!("de_noise_{session}"),
        session_id: session.clone(),
        turn_id: "t_noise".into(),
        kind: DomainEventKind::TurnStarted,
        data: json!({ "entity_id": "should_not_appear" }),
        source_refs: Vec::new(),
        created_at: DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
    })
    .await
    .unwrap();

    // ContextSurfaced 投影：含该实体（隐藏 context 装载可溯源）。
    let ctx = db.list_context_surfaced_entities(&session).await.unwrap();
    assert_eq!(
        ctx,
        vec![("clue_ctx".to_string(), "clue".to_string())],
        "ContextSurfaced 投影含 context 装载实体"
    );

    // 玩家暴露投影：绝不含 ContextSurfaced。
    let exposed = db.list_surfaced_entities(&session).await.unwrap();
    assert!(
        exposed.is_empty(),
        "ContextSurfaced 不计入玩家暴露：{exposed:?}"
    );
    // has_entity_surfaced_in_turn = 玩家暴露语义：纯 ContextSurfaced 回合 → false。
    assert!(
        !db.has_entity_surfaced_in_turn(&session, "t_ctx")
            .await
            .unwrap(),
        "纯 ContextSurfaced 回合不算玩家暴露"
    );
    // revealed-facts（玩家知识）：空——进 context 不等于玩家习得事实。
    assert!(
        db.list_revealed_facts(&session).await.unwrap().is_empty(),
        "ContextSurfaced 不授予 revealed-facts"
    );
    assert!(
        db.list_player_known_fact_ids(&session)
            .await
            .unwrap()
            .is_empty(),
        "ContextSurfaced 不授予 player_party 知识"
    );

    purge(&db, &session).await;
}

/// 验收②：PlayerExposed 把实体标记为"玩家见过/听说过"（计入玩家暴露投影），但**不**揭示
/// 其隐藏身份/秘密事实（不进 revealed-facts）。
#[tokio::test]
async fn player_exposed_entity_does_not_reveal_secret_fact() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_exp_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    // 玩家在念白里见到管家这个 NPC（暴露），但其"管家其实是凶手"的秘密事实未揭示。
    db.record_player_exposed_entity(
        &session,
        "t_exp",
        "npc_butler",
        "npc",
        Some("玩家进客厅见到管家"),
    )
    .await
    .unwrap();

    // 暴露投影命中该实体。
    let exposed = db.list_surfaced_entities(&session).await.unwrap();
    assert_eq!(
        exposed,
        vec![("npc_butler".to_string(), "npc".to_string())],
        "PlayerExposed 进玩家暴露投影"
    );
    assert!(
        db.has_entity_surfaced_in_turn(&session, "t_exp")
            .await
            .unwrap(),
        "PlayerExposed 回合算玩家暴露"
    );

    // 但秘密事实未揭示：revealed-facts / player_known 不含该实体的 secret fact_id。
    assert!(
        db.list_revealed_facts(&session).await.unwrap().is_empty(),
        "仅暴露实体不得揭示其秘密事实（revealed-facts 必须为空）"
    );
    assert!(
        db.list_player_known_fact_ids(&session)
            .await
            .unwrap()
            .is_empty(),
        "暴露 ≠ player_party 习得秘密"
    );
    // 暴露也绝不写 knowledge_edges。
    let edge_cnt: i64 =
        sqlx::query_scalar("select count(*) from knowledge_edges where session_id=$1")
            .bind(&session)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(edge_cnt, 0, "PlayerExposed 不写任何 KnowledgeEdge");

    purge(&db, &session).await;
}

/// 验收③：PlayerLearnedFact 更新 player_party 知识——写 PlayerLearnedFact 域事件并写穿
/// KnowledgeEdge(player_party, knows_true)，revealed-facts/player_known 投影命中。
#[tokio::test]
async fn player_learned_fact_updates_party_knowledge() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_learned_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    db.record_revealed_fact(&session, "turn_reveal", "npc_butler", Some("玩家读完日记"))
        .await
        .unwrap();

    // 域事件层：写 PlayerLearnedFact（取代旧 FactRevealed 写路径），event_id 幂等键不变。
    let kind: String = sqlx::query_scalar("select kind from domain_events where event_id = $1")
        .bind(format!("de_revealed_{session}_npc_butler"))
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        kind, "PlayerLearnedFact",
        "reveal 写 PlayerLearnedFact 域事件"
    );

    // player_party KnowledgeEdge 写穿 → 投影命中。
    assert_eq!(
        db.list_player_known_fact_ids(&session).await.unwrap(),
        vec!["npc_butler".to_string()],
        "player_party 习得事实"
    );
    assert_eq!(
        db.list_revealed_facts(&session).await.unwrap(),
        vec!["npc_butler".to_string()],
        "revealed-facts 兼容投影命中"
    );
    // 写穿确有 player_party knows_true 边。
    let edge_cnt: i64 = sqlx::query_scalar(
        "select count(*) from knowledge_edges where session_id=$1 and holder_kind='player_party' and knowledge_state='knows_true'",
    ).bind(&session).fetch_one(&db.pool).await.unwrap();
    assert_eq!(
        edge_cnt, 1,
        "PlayerLearnedFact 写穿 1 条 player_party knows_true 边"
    );

    purge(&db, &session).await;
}

/// 验收④：NpcLearnedFact 只更新目标 NPC 的知识 holder——按 npc_actor_id 隔离投影，绝不
/// 触碰 player_party 或别的 NPC。TC-KNOW-04：durable NPC holder（knowledge_edges.holder_kind='npc'）
/// 已打开，故写穿 durable npc 边（每个 NPC 一条）且保留事件账本；player_party 边仍为 0。
/// 空 actor id 仍 fail-closed 被拒。
#[tokio::test]
async fn npc_learned_fact_updates_one_npc_only() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_npc_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    // 两个不同 NPC 各习得不同事实。
    db.record_npc_learned_fact(
        &session,
        "t1",
        "npc_alice",
        "fact_poison",
        Some("Alice 目睹下毒"),
    )
    .await
    .unwrap();
    db.record_npc_learned_fact(&session, "t2", "npc_bob", "fact_letter", Some("Bob 读了信"))
        .await
        .unwrap();

    // 隔离：每个 NPC 只见到自己习得的事实。
    assert_eq!(
        db.list_npc_known_fact_ids(&session, "npc_alice")
            .await
            .unwrap(),
        vec!["fact_poison".to_string()],
        "Alice 只知 fact_poison"
    );
    assert_eq!(
        db.list_npc_known_fact_ids(&session, "npc_bob")
            .await
            .unwrap(),
        vec!["fact_letter".to_string()],
        "Bob 只知 fact_letter"
    );
    // 未参与的 NPC 投影为空。
    assert!(
        db.list_npc_known_fact_ids(&session, "npc_carol")
            .await
            .unwrap()
            .is_empty(),
        "无关 NPC 不得见到任何事实"
    );

    // 绝不触碰 player_party：NPC 习得不泄露给玩家。
    assert!(
        db.list_player_known_fact_ids(&session)
            .await
            .unwrap()
            .is_empty(),
        "NpcLearnedFact 绝不更新 player_party 知识"
    );
    assert!(
        db.list_revealed_facts(&session).await.unwrap().is_empty(),
        "NpcLearnedFact 不进 revealed-facts"
    );

    // TC-KNOW-04：durable NPC 边写穿——两个 NPC 各一条 npc holder 边，但 player_party 仍 0。
    let npc_cnt: i64 = sqlx::query_scalar(
        "select count(*) from knowledge_edges where session_id=$1 and holder_kind='npc'",
    )
    .bind(&session)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        npc_cnt, 2,
        "NpcLearnedFact 写穿 durable npc 边（alice/bob 各一条）"
    );
    let pp_cnt: i64 = sqlx::query_scalar(
        "select count(*) from knowledge_edges where session_id=$1 and holder_kind='player_party'",
    )
    .bind(&session)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(pp_cnt, 0, "NpcLearnedFact 绝不写 player_party 边");

    // 空 actor id fail-closed 拒绝（绝不用展示名/对抗方标签冒充 stable actor id）。
    assert!(
        db.record_npc_learned_fact(&session, "t3", "  ", "fact_x", None)
            .await
            .is_err(),
        "空 npc_actor_id 必须 fail-closed 拒绝"
    );

    purge(&db, &session).await;
}

/// TC-KNOW-00 + TC-KNOW-04 验收：record_npc_learned_fact 经 actor-identity 契约校验稳定 holder 身份。
/// 空串/占位串/展示名/对抗方标签全部 fail-closed（既不落事件也不写边）；合法 source id 规范化
/// （trim）后落账，并写穿 durable npc 边（一条）。
#[tokio::test]
async fn npc_learned_fact_requires_stable_actor_id() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_npcid_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    // 非空但 ad-hoc 的串也必须被拒：不再是「仅空串拒绝」。
    for bad in [
        "",
        "  ",
        "unknown",
        "enemy",
        "The Butler",
        "Goblin #2",
        "n/a",
    ] {
        assert!(
            db.record_npc_learned_fact(&session, "t_bad", bad, "fact_x", None)
                .await
                .is_err(),
            "ad-hoc/占位/展示名 npc_actor_id 必须 fail-closed 拒绝: {bad:?}"
        );
    }

    // 合法 source id（带首尾空白）→ 契约 trim 规范化后落账，投影按规范化 id 命中。
    db.record_npc_learned_fact(
        &session,
        "t_ok",
        "  npc_butler  ",
        "fact_diary",
        Some("读日记"),
    )
    .await
    .unwrap();
    assert_eq!(
        db.list_npc_known_fact_ids(&session, "npc_butler")
            .await
            .unwrap(),
        vec!["fact_diary".to_string()],
        "规范化稳定 id 命中投影"
    );

    // TC-KNOW-04：合法写入写穿 durable npc 边（仅一条，挂规范化 id）；非法写入一条都没落。
    let edge_cnt: i64 = sqlx::query_scalar(
        "select count(*) from knowledge_edges where session_id=$1 and holder_kind='npc'",
    )
    .bind(&session)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(edge_cnt, 1, "仅合法 NpcLearnedFact 写穿一条 durable npc 边");
    let bad_edges: i64 = sqlx::query_scalar(
        "select count(*) from knowledge_edges where session_id=$1 and holder_id <> 'npc_butler'",
    )
    .bind(&session)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(bad_edges, 0, "非法 actor id 不写任何边");

    purge(&db, &session).await;
}
