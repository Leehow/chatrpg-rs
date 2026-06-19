//! KnowledgeEdge v1 live 验收：把「事实身份/真相」与「谁知道/相信/误信它」分离。
//! 覆盖必需验收测试：
//!   - knowledge_edge_roundtrip            通用 durable upsert 往返（gm/system + v1 字段）
//!   - player_projection_hides_unknown_fact 玩家投影只见 knows_true，未知/信念态隐藏
//!   - npc_projection_is_holder_specific    durable NPC 投影按 holder 隔离（TC-KNOW-04）
//!   - npc_durable_upsert_identity_gated    通用 upsert npc：稳定 id 成功、非法 id fail-closed 无行
//!   - pc_faction_durable_upsert_identity_gated  通用 upsert pc/faction：稳定 id 成功、非法 id fail-closed 无行
//!   - npc_belief_not_world_truth_or_known  NPC 信念不进其已知真相，更不是 GM 世界真相
//!   - npc_learned_fact_replay_is_idempotent 重放同一 NpcLearnedFact 不重复边/事件
//!   - false_belief_not_world_truth         false belief 是信念，绝不进 GM 世界真相视图
//!   - player_reveal_updates_player_party_only  reveal 只更新 player_party，不碰 gm/npc
//! TC-KNOW-04 已打开 durable NPC knowledge_edges（holder_kind='npc'）；本任务在 actor-identity
//! 契约落地后补齐 pc/faction durable holder（migration 0037）——带身份 holder 写库前均经稳定 id 校验。
//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!      cargo test -p trpg-db --test live_knowledge_edges -- --nocapture
//! 无 DATABASE_URL 时 SKIP（fail-closed，不卡 CI）。
use trpg_db::{Db, KnowledgeEdgeInput};

/// 连接 + migrate；无 DATABASE_URL 或连不上即 None（调用方 SKIP）。
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
    // DATABASE_URL 已设且连接成功后，migrate 失败必须让测试 FAIL，绝不伪装成 SKIP——
    // 否则迁移竞争会让必需验收断言被静默跳过而 cargo 仍报 ok。
    // 仅当 DATABASE_URL 缺失或库连不上时才允许 SKIP（保留既有 fail-closed 模式）。
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
    sqlx::query("delete from domain_events where session_id=$1")
        .bind(session)
        .execute(&db.pool)
        .await
        .unwrap();
}

/// P0b 兼容回归：player_party knows_true 投影只取 knows_true，believes_false 不入，重 upsert 幂等。
#[tokio::test]
async fn knowledge_projection_returns_player_party_knows_true_only() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_ke_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    db.upsert_knowledge_edge_player_party(
        &session,
        "turn1",
        "npc_butler",
        "knows_true",
        Some("diary"),
    )
    .await
    .unwrap();
    db.upsert_knowledge_edge_player_party(
        &session,
        "turn1",
        "npc_secret_false",
        "believes_false",
        Some("rumor"),
    )
    .await
    .unwrap();

    let got = db.list_player_known_fact_ids(&session).await.unwrap();
    assert_eq!(
        got,
        vec!["npc_butler".to_string()],
        "只投影 knows_true，believes_false 不入"
    );

    db.upsert_knowledge_edge_player_party(
        &session,
        "turn7",
        "npc_butler",
        "knows_true",
        Some("再次确认"),
    )
    .await
    .unwrap();
    let again = db.list_player_known_fact_ids(&session).await.unwrap();
    assert_eq!(
        again,
        vec!["npc_butler".to_string()],
        "重 upsert 幂等不重复"
    );

    purge(&db, &session).await;
}

/// 验收：通用 durable upsert 往返。gm / system holder 边落库后能按 holder 投影读回，
/// v1 字段（confidence / learned_at_turn_id / disclosure_policy）持久化，重 upsert 幂等。
/// 未知 holder token fail-closed 拒绝、绝不写库（npc/pc/faction 身份门单测见下）。
#[tokio::test]
async fn knowledge_edge_roundtrip() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_rt_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    db.upsert_knowledge_edge(KnowledgeEdgeInput {
        session_id: &session,
        holder_kind: "gm",
        holder_id: "",
        fact_id: "fact_villain_identity",
        knowledge_state: "knows_true",
        confidence: Some(1.0),
        learned_at_turn_id: Some("t_setup"),
        disclosure_policy: Some("gm_only"),
        source_event_id: Some("ev_seed"),
        reason: Some("module seed truth"),
    })
    .await
    .unwrap();
    db.upsert_knowledge_edge(KnowledgeEdgeInput {
        session_id: &session,
        holder_kind: "system",
        holder_id: "",
        fact_id: "fact_ruling_log",
        knowledge_state: "knows_true",
        confidence: None,
        learned_at_turn_id: None,
        disclosure_policy: None,
        source_event_id: None,
        reason: None,
    })
    .await
    .unwrap();

    assert_eq!(
        db.gm_truth_view(&session).await.unwrap(),
        vec!["fact_villain_identity".to_string()],
        "GM 真相视图只含 gm holder 的 knows_true"
    );
    assert_eq!(
        db.list_holder_known_fact_ids(&session, "system", "")
            .await
            .unwrap(),
        vec!["fact_ruling_log".to_string()],
        "system holder 投影按 holder 隔离"
    );

    // v1 字段确有持久化。
    let (conf, turn, pol): (Option<f64>, Option<String>, Option<String>) = sqlx::query_as(
        "select confidence, learned_at_turn_id, disclosure_policy from knowledge_edges \
         where session_id=$1 and holder_kind='gm' and fact_id='fact_villain_identity'",
    )
    .bind(&session)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(conf, Some(1.0));
    assert_eq!(turn.as_deref(), Some("t_setup"));
    assert_eq!(pol.as_deref(), Some("gm_only"));

    // 幂等 + on-conflict 更新态：再 upsert 同 (session,gm,'',fact) 不增行。
    db.upsert_knowledge_edge(KnowledgeEdgeInput {
        session_id: &session,
        holder_kind: "gm",
        holder_id: "",
        fact_id: "fact_villain_identity",
        knowledge_state: "knows_true",
        confidence: Some(0.9),
        learned_at_turn_id: None,
        disclosure_policy: None,
        source_event_id: None,
        reason: None,
    })
    .await
    .unwrap();
    let cnt: i64 = sqlx::query_scalar(
        "select count(*) from knowledge_edges where session_id=$1 and holder_kind='gm'",
    )
    .bind(&session)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(cnt, 1, "重 upsert 幂等：仍一条 gm 边");

    // fail-closed：未知 holder token 被拒，且不写任何边（pc/faction 已开，身份门见专测）。
    assert!(
        db.upsert_knowledge_edge(KnowledgeEdgeInput {
            session_id: &session,
            holder_kind: "dragon",
            holder_id: "",
            fact_id: "fact_x",
            knowledge_state: "knows_true",
            confidence: None,
            learned_at_turn_id: None,
            disclosure_policy: None,
            source_event_id: None,
            reason: None,
        })
        .await
        .is_err(),
        "未知 holder token 必须 fail-closed 拒绝"
    );
    let cnt: i64 = sqlx::query_scalar(
        "select count(*) from knowledge_edges where session_id=$1 and holder_kind='dragon'",
    )
    .bind(&session)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(cnt, 0, "fail-closed：未知 holder token 边一条都没写");

    purge(&db, &session).await;
}

/// 验收：通用 upsert_knowledge_edge 的 pc / faction 身份门（本任务 + TC-KNOW-00）。
/// 稳定 pc / faction id（带首尾空白）→ 契约 trim 规范化后写入成功且按 holder 投影读回；
/// 非法 id（空 / 占位 / 展示名形态）→ fail-closed，绝不写任何 pc / faction 边。
#[tokio::test]
async fn pc_faction_durable_upsert_identity_gated() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_pcfac_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    // 稳定 id（带首尾空白）→ 契约 trim 规范化后成功落库，按 holder 投影读回。
    for (kind, raw_id, norm_id, fact) in [
        ("pc", "  pc_hero  ", "pc_hero", "fact_pc_secret"),
        (
            "faction",
            " guild_thieves ",
            "guild_thieves",
            "fact_faction_plan",
        ),
    ] {
        db.upsert_knowledge_edge(KnowledgeEdgeInput {
            session_id: &session,
            holder_kind: kind,
            holder_id: raw_id,
            fact_id: fact,
            knowledge_state: "knows_true",
            confidence: Some(0.7),
            learned_at_turn_id: Some("t1"),
            disclosure_policy: None,
            source_event_id: None,
            reason: Some("本任务打开"),
        })
        .await
        .unwrap_or_else(|e| panic!("稳定 {kind} id 必须成功: {e}"));
        assert_eq!(
            db.list_holder_known_fact_ids(&session, kind, norm_id)
                .await
                .unwrap(),
            vec![fact.to_string()],
            "规范化 {kind} id（{norm_id}）按 holder 投影读回"
        );
    }

    // 非法 pc / faction id 全部 fail-closed，且不写任何边。
    for kind in ["pc", "faction"] {
        for bad in ["", "  ", "unknown", "n/a", "The Hero", "Thieves Guild"] {
            assert!(
                db.upsert_knowledge_edge(KnowledgeEdgeInput {
                    session_id: &session,
                    holder_kind: kind,
                    holder_id: bad,
                    fact_id: "fact_x",
                    knowledge_state: "knows_true",
                    confidence: None,
                    learned_at_turn_id: None,
                    disclosure_policy: None,
                    source_event_id: None,
                    reason: None,
                })
                .await
                .is_err(),
                "非法 {kind} id 必须 fail-closed: {bad:?}"
            );
        }
        // 只应有先前那一条合法边（非法写入一条都没落）。
        let cnt: i64 = sqlx::query_scalar(
            "select count(*) from knowledge_edges where session_id=$1 and holder_kind=$2",
        )
        .bind(&session)
        .bind(kind)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(cnt, 1, "非法 {kind} id 不写边，仅合法一条");
    }

    purge(&db, &session).await;
}

/// 验收：玩家投影隐藏未知事实。player_party 仅对 fact_known 是 knows_true；
/// fact_unknown 完全无边，fact_belief 是 believes_false。player_knowledge_view 只见 fact_known。
#[tokio::test]
async fn player_projection_hides_unknown_fact() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_hide_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    db.upsert_knowledge_edge_player_party(&session, "t1", "fact_known", "knows_true", None)
        .await
        .unwrap();
    db.upsert_knowledge_edge_player_party(&session, "t1", "fact_belief", "believes_false", None)
        .await
        .unwrap();
    // fact_unknown 故意不写任何边（玩家不知道）。
    // GM 知道全部三个真相，但 GM 真相绝不自动进玩家视图。
    db.upsert_knowledge_edge(KnowledgeEdgeInput {
        session_id: &session,
        holder_kind: "gm",
        holder_id: "",
        fact_id: "fact_unknown",
        knowledge_state: "knows_true",
        confidence: None,
        learned_at_turn_id: None,
        disclosure_policy: None,
        source_event_id: None,
        reason: None,
    })
    .await
    .unwrap();

    let player = db.player_knowledge_view(&session).await.unwrap();
    assert_eq!(
        player,
        vec!["fact_known".to_string()],
        "玩家只见 knows_true 事实"
    );
    assert!(
        !player.contains(&"fact_unknown".to_string()),
        "未知事实隐藏"
    );
    assert!(
        !player.contains(&"fact_belief".to_string()),
        "信念态不入玩家投影"
    );

    // GM 视图含 fact_unknown，证明「GM 真相 ≠ 玩家可见」。
    assert!(
        db.gm_truth_view(&session)
            .await
            .unwrap()
            .contains(&"fact_unknown".to_string()),
        "GM 知道 fact_unknown，但它不在玩家视图里"
    );

    purge(&db, &session).await;
}

/// 验收：durable NPC 投影按 holder 隔离（TC-KNOW-04）。NpcLearnedFact 写穿 durable
/// knowledge_edges(holder_kind='npc')：npc_a 知道的 fact 不泄露给 npc_b，也绝不进
/// player_party；durable npc 边确有落库（不再 event-only），且 per-NPC holder_id 隔离。
#[tokio::test]
async fn npc_projection_is_holder_specific() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_npcspec_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    db.record_npc_learned_fact(&session, "t1", "npc_a", "fact_poison", Some("目睹下毒"))
        .await
        .unwrap();
    db.record_npc_learned_fact(&session, "t2", "npc_b", "fact_letter", Some("读了信"))
        .await
        .unwrap();

    assert_eq!(
        db.list_npc_known_fact_ids(&session, "npc_a").await.unwrap(),
        vec!["fact_poison".to_string()],
        "npc_a 只知 fact_poison"
    );
    assert_eq!(
        db.list_npc_known_fact_ids(&session, "npc_b").await.unwrap(),
        vec!["fact_letter".to_string()],
        "npc_b 只知 fact_letter，不含 npc_a 的事实"
    );
    assert!(
        db.list_npc_known_fact_ids(&session, "npc_c")
            .await
            .unwrap()
            .is_empty(),
        "无关 NPC 投影为空"
    );
    // NPC 习得绝不泄露给玩家。
    assert!(
        db.player_knowledge_view(&session).await.unwrap().is_empty(),
        "NPC 习得不进 player_party"
    );
    // TC-KNOW-04：durable NPC 边确有落库——两个 npc holder 各一条 knows_true 边。
    let npc_edges: i64 = sqlx::query_scalar(
        "select count(*) from knowledge_edges where session_id=$1 and holder_kind='npc'",
    )
    .bind(&session)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(npc_edges, 2, "durable NPC 边落库（npc_a / npc_b 各一条）");
    // holder_id 隔离：npc_a 的边只挂 npc_a。
    let a_edges: i64 = sqlx::query_scalar(
        "select count(*) from knowledge_edges where session_id=$1 and holder_kind='npc' and holder_id='npc_a' and fact_id='fact_poison'",
    )
    .bind(&session)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(a_edges, 1, "npc_a 的 durable 边只挂 npc_a");
    // 绝不写 gm / player_party 边。
    let other: i64 = sqlx::query_scalar(
        "select count(*) from knowledge_edges where session_id=$1 and holder_kind in ('gm','player_party')",
    )
    .bind(&session)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(other, 0, "NPC 习得不写 gm/player_party 边");

    purge(&db, &session).await;
}

/// 验收：通用 upsert_knowledge_edge 的 NPC 身份门（TC-KNOW-04 + TC-KNOW-00）。
/// 稳定 npc id → 写入成功且按 holder 投影读回；非法 npc id（空/占位/展示名形态）→ fail-closed，
/// 绝不写任何 npc 边。pc / faction 仍整体 gated（在 knowledge_edge_roundtrip 覆盖）。
#[tokio::test]
async fn npc_durable_upsert_identity_gated() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_npcid_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    // 稳定 id（带首尾空白）→ 契约 trim 规范化后成功落库。
    db.upsert_knowledge_edge(KnowledgeEdgeInput {
        session_id: &session,
        holder_kind: "npc",
        holder_id: "  npc_alice  ",
        fact_id: "fact_secret",
        knowledge_state: "knows_true",
        confidence: Some(0.8),
        learned_at_turn_id: Some("t1"),
        disclosure_policy: None,
        source_event_id: None,
        reason: Some("亲眼所见"),
    })
    .await
    .expect("稳定 npc id 必须成功");
    assert_eq!(
        db.list_npc_known_fact_ids(&session, "npc_alice")
            .await
            .unwrap(),
        vec!["fact_secret".to_string()],
        "规范化 id（npc_alice）按 holder 投影读回"
    );

    // 非法 npc id 全部 fail-closed，且不写任何 npc 边。
    for bad in ["", "  ", "unknown", "enemy", "The Butler", "Goblin #2"] {
        assert!(
            db.upsert_knowledge_edge(KnowledgeEdgeInput {
                session_id: &session,
                holder_kind: "npc",
                holder_id: bad,
                fact_id: "fact_x",
                knowledge_state: "knows_true",
                confidence: None,
                learned_at_turn_id: None,
                disclosure_policy: None,
                source_event_id: None,
                reason: None,
            })
            .await
            .is_err(),
            "非法 npc id 必须 fail-closed: {bad:?}"
        );
    }
    // 只应有先前那一条合法 npc 边（非法写入一条都没落）。
    let cnt: i64 = sqlx::query_scalar(
        "select count(*) from knowledge_edges where session_id=$1 and holder_kind='npc'",
    )
    .bind(&session)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(cnt, 1, "非法 npc id 不写边，仅合法一条");

    purge(&db, &session).await;
}

/// 验收：NPC 可持有 false/rumor 信念而不污染其已知真相，也绝不变成 GM 世界真相。
/// npc_a 对 fact_rumor 持 believes_false（durable npc 边），对 fact_true 持 knows_true。
/// list_npc_known_fact_ids 只含 fact_true；gm_truth_view 不含任一（GM 没确知）。
#[tokio::test]
async fn npc_belief_not_world_truth_or_known() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_npcbel_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    db.record_npc_learned_fact(&session, "t1", "npc_a", "fact_true", Some("确知"))
        .await
        .unwrap();
    // NPC 误信一条传闻为假（durable belief 边，非 GM 真相）。
    db.upsert_knowledge_edge(KnowledgeEdgeInput {
        session_id: &session,
        holder_kind: "npc",
        holder_id: "npc_a",
        fact_id: "fact_rumor",
        knowledge_state: "believes_false",
        confidence: Some(0.3),
        learned_at_turn_id: Some("t2"),
        disclosure_policy: None,
        source_event_id: None,
        reason: Some("听信谣言"),
    })
    .await
    .unwrap();

    assert_eq!(
        db.list_npc_known_fact_ids(&session, "npc_a").await.unwrap(),
        vec!["fact_true".to_string()],
        "NPC 已知真相只含 knows_true，believes_false 不入"
    );
    let truth = db.gm_truth_view(&session).await.unwrap();
    assert!(
        !truth.contains(&"fact_rumor".to_string()) && !truth.contains(&"fact_true".to_string()),
        "NPC 知识/信念都不自动变成 GM 世界真相"
    );

    purge(&db, &session).await;
}

/// 验收：重放同一 NpcLearnedFact 既不重复 durable 边也不重复事件（确定性幂等键）。
#[tokio::test]
async fn npc_learned_fact_replay_is_idempotent() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_npcrep_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    for _ in 0..3 {
        db.record_npc_learned_fact(&session, "t1", "npc_a", "fact_dup", Some("重放"))
            .await
            .unwrap();
    }
    let edge_cnt: i64 = sqlx::query_scalar(
        "select count(*) from knowledge_edges where session_id=$1 and holder_kind='npc' and holder_id='npc_a' and fact_id='fact_dup'",
    )
    .bind(&session)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(edge_cnt, 1, "重放只一条 durable npc 边");
    let ev_cnt: i64 = sqlx::query_scalar(
        "select count(*) from domain_events where session_id=$1 and kind='NpcLearnedFact'",
    )
    .bind(&session)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(ev_cnt, 1, "重放只一条 NpcLearnedFact 事件");
    assert_eq!(
        db.list_npc_known_fact_ids(&session, "npc_a").await.unwrap(),
        vec!["fact_dup".to_string()]
    );

    purge(&db, &session).await;
}

/// 验收：false belief 不是世界真相。某 holder 对 fact_false 持 believes_false，
/// GM 对 fact_real 持 knows_true。gm_truth_view 只含 fact_real，绝不含任何信念态事实；
/// 玩家投影也不含 fact_false。
#[tokio::test]
async fn false_belief_not_world_truth() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_false_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    // 世界真相：GM 确知 fact_real。
    db.upsert_knowledge_edge(KnowledgeEdgeInput {
        session_id: &session,
        holder_kind: "gm",
        holder_id: "",
        fact_id: "fact_real",
        knowledge_state: "knows_true",
        confidence: Some(1.0),
        learned_at_turn_id: None,
        disclosure_policy: None,
        source_event_id: None,
        reason: Some("ground truth"),
    })
    .await
    .unwrap();
    // 错误信念：player_party 误信 fact_false（durable holder 表达 belief）。
    db.upsert_knowledge_edge_player_party(
        &session,
        "t1",
        "fact_false",
        "believes_false",
        Some("被骗"),
    )
    .await
    .unwrap();
    // GM 误信也用 misinformed 表达：仍不得变成世界真相。
    db.upsert_knowledge_edge(KnowledgeEdgeInput {
        session_id: &session,
        holder_kind: "gm",
        holder_id: "",
        fact_id: "fact_rumor",
        knowledge_state: "misinformed",
        confidence: Some(0.3),
        learned_at_turn_id: None,
        disclosure_policy: None,
        source_event_id: None,
        reason: None,
    })
    .await
    .unwrap();

    let truth = db.gm_truth_view(&session).await.unwrap();
    assert_eq!(
        truth,
        vec!["fact_real".to_string()],
        "世界真相只含确知为真，信念态不入"
    );
    assert!(
        !truth.contains(&"fact_false".to_string()),
        "false belief 不是世界真相"
    );
    assert!(
        !truth.contains(&"fact_rumor".to_string()),
        "misinformed 信念不是世界真相"
    );
    // 玩家投影也不含错误信念事实。
    assert!(
        !db.player_knowledge_view(&session)
            .await
            .unwrap()
            .contains(&"fact_false".to_string()),
        "玩家投影不含 believes_false 事实"
    );

    purge(&db, &session).await;
}

/// 验收：玩家 reveal 只更新 player_party。record_revealed_fact 写穿 player_party knows_true，
/// 不触碰 GM 视图、不触碰 NPC 事件账本投影；GM 既有真相边不受影响。
#[tokio::test]
async fn player_reveal_updates_player_party_only() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_reveal_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    // 前置：GM 已知 fact_gm_secret；某 NPC 已知 fact_npc（事件账本）。
    db.upsert_knowledge_edge(KnowledgeEdgeInput {
        session_id: &session,
        holder_kind: "gm",
        holder_id: "",
        fact_id: "fact_gm_secret",
        knowledge_state: "knows_true",
        confidence: None,
        learned_at_turn_id: None,
        disclosure_policy: None,
        source_event_id: None,
        reason: None,
    })
    .await
    .unwrap();
    db.record_npc_learned_fact(&session, "t0", "npc_a", "fact_npc", None)
        .await
        .unwrap();

    // 玩家 reveal fact_player。
    db.record_revealed_fact(&session, "t_reveal", "fact_player", Some("读完日记"))
        .await
        .unwrap();

    // player_party 只多了 fact_player。
    assert_eq!(
        db.player_knowledge_view(&session).await.unwrap(),
        vec!["fact_player".to_string()],
        "reveal 只更新 player_party"
    );
    // GM 视图不被 reveal 改变（仍只有 fact_gm_secret，没有 fact_player）。
    assert_eq!(
        db.gm_truth_view(&session).await.unwrap(),
        vec!["fact_gm_secret".to_string()],
        "玩家 reveal 不写 gm holder"
    );
    // NPC 投影不被 reveal 改变。
    assert_eq!(
        db.list_npc_known_fact_ids(&session, "npc_a").await.unwrap(),
        vec!["fact_npc".to_string()],
        "玩家 reveal 不碰 NPC 知识"
    );
    // player_party 边里没有 fact_gm_secret/fact_npc。
    let pp: i64 = sqlx::query_scalar(
        "select count(*) from knowledge_edges where session_id=$1 and holder_kind='player_party' \
         and fact_id in ('fact_gm_secret','fact_npc')",
    )
    .bind(&session)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(pp, 0, "reveal 不把 gm/npc 事实灌进 player_party");

    purge(&db, &session).await;
}

/// DA-KNOW-01 验收（holder roundtrip）：同一 `fact_id` 的身份只有一个，但其 holder 状态
/// 各自独立持久化、可分别 roundtrip 读回且互不污染。一条 fact `F` 同时落五个 durable holder
/// 的不同知识态——gm=knows_true、player_party=believes_false、system=misinformed、
/// npc_a=knows_true、npc_b=believes_false——逐 holder 从库里读回各自 state（证明状态不是
/// 按 holder 复制 fact 身份，而是分离存储），再用真知识投影确认信念态绝不被当作已知真相。
#[tokio::test]
async fn same_fact_roundtrips_distinct_holder_states_across_holders() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_rtdiv_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    let fact = "fact_the_relic_is_cursed";
    // 同一 fact 身份，五个 holder 各持不同知识态（durable upsert / player_party 写穿）。
    db.upsert_knowledge_edge(KnowledgeEdgeInput {
        session_id: &session,
        holder_kind: "gm",
        holder_id: "",
        fact_id: fact,
        knowledge_state: "knows_true",
        confidence: Some(1.0),
        learned_at_turn_id: None,
        disclosure_policy: Some("gm_only"),
        source_event_id: None,
        reason: Some("module truth"),
    })
    .await
    .unwrap();
    db.upsert_knowledge_edge_player_party(&session, "t1", fact, "believes_false", Some("被骗"))
        .await
        .unwrap();
    db.upsert_knowledge_edge(KnowledgeEdgeInput {
        session_id: &session,
        holder_kind: "system",
        holder_id: "",
        fact_id: fact,
        knowledge_state: "misinformed",
        confidence: Some(0.2),
        learned_at_turn_id: None,
        disclosure_policy: None,
        source_event_id: None,
        reason: None,
    })
    .await
    .unwrap();
    db.upsert_knowledge_edge(KnowledgeEdgeInput {
        session_id: &session,
        holder_kind: "npc",
        holder_id: "npc_a",
        fact_id: fact,
        knowledge_state: "knows_true",
        confidence: Some(0.9),
        learned_at_turn_id: None,
        disclosure_policy: None,
        source_event_id: None,
        reason: Some("亲眼所见"),
    })
    .await
    .unwrap();
    db.upsert_knowledge_edge(KnowledgeEdgeInput {
        session_id: &session,
        holder_kind: "npc",
        holder_id: "npc_b",
        fact_id: fact,
        knowledge_state: "believes_false",
        confidence: Some(0.3),
        learned_at_turn_id: None,
        disclosure_policy: None,
        source_event_id: None,
        reason: Some("听信谣言"),
    })
    .await
    .unwrap();

    // 只有一个 fact 身份，但落了五条 holder 行（fact 身份未被按 holder 复制）。
    let distinct_facts: i64 = sqlx::query_scalar(
        "select count(distinct fact_id) from knowledge_edges where session_id=$1",
    )
    .bind(&session)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(distinct_facts, 1, "同一 fact 身份只有一个");
    let holder_rows: i64 = sqlx::query_scalar(
        "select count(*) from knowledge_edges where session_id=$1 and fact_id=$2",
    )
    .bind(&session)
    .bind(fact)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(holder_rows, 5, "五个 holder 各一条独立边");

    // 逐 holder roundtrip 读回各自持久化的 knowledge_state（状态与 holder 分离存储）。
    for (kind, id, want) in [
        ("gm", "", "knows_true"),
        ("player_party", "", "believes_false"),
        ("system", "", "misinformed"),
        ("npc", "npc_a", "knows_true"),
        ("npc", "npc_b", "believes_false"),
    ] {
        let got: String = sqlx::query_scalar(
            "select knowledge_state from knowledge_edges \
             where session_id=$1 and holder_kind=$2 and holder_id=$3 and fact_id=$4",
        )
        .bind(&session)
        .bind(kind)
        .bind(id)
        .bind(fact)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(got, want, "holder ({kind},{id}) 持久化态 roundtrip");
    }

    // 真知识投影：只有确知为真的 holder（gm / npc_a）把该 fact 当已知真相；
    // 信念态（player_party / system / npc_b）一律不进各自的 known 投影。
    assert!(db
        .gm_truth_view(&session)
        .await
        .unwrap()
        .contains(&fact.to_string()));
    assert!(
        db.player_knowledge_view(&session).await.unwrap().is_empty(),
        "玩家 believes_false 不是已知真相"
    );
    assert!(
        db.list_holder_known_fact_ids(&session, "system", "")
            .await
            .unwrap()
            .is_empty(),
        "system misinformed 不进真知识投影"
    );
    assert_eq!(
        db.list_npc_known_fact_ids(&session, "npc_a").await.unwrap(),
        vec![fact.to_string()],
        "npc_a knows_true"
    );
    assert!(
        db.list_npc_known_fact_ids(&session, "npc_b")
            .await
            .unwrap()
            .is_empty(),
        "npc_b believes_false 不算其已知真相"
    );

    purge(&db, &session).await;
}

/// 验收（架构）：同一 fact_id 在不同 holder 上知识态发散——GM 确知为真、玩家未知、
/// 某 holder 误信，互不影响。NPC 维度用事件账本（durable NPC 边 deferred 到 TC-KNOW-04）。
#[tokio::test]
async fn same_fact_diverges_across_holders() {
    let Some(db) = connect_or_skip().await else {
        return;
    };
    let session = format!("sess_div_{}", uuid::Uuid::new_v4().simple());
    purge(&db, &session).await;

    let fact = "fact_butler_is_killer";
    // GM 确知为真。
    db.upsert_knowledge_edge(KnowledgeEdgeInput {
        session_id: &session,
        holder_kind: "gm",
        holder_id: "",
        fact_id: fact,
        knowledge_state: "knows_true",
        confidence: Some(1.0),
        learned_at_turn_id: None,
        disclosure_policy: Some("gm_only"),
        source_event_id: None,
        reason: None,
    })
    .await
    .unwrap();
    // 某 holder 误信同一 fact 为假（system holder 代表一条非角色信念记录）。
    db.upsert_knowledge_edge(KnowledgeEdgeInput {
        session_id: &session,
        holder_kind: "system",
        holder_id: "",
        fact_id: fact,
        knowledge_state: "believes_false",
        confidence: Some(0.2),
        learned_at_turn_id: None,
        disclosure_policy: None,
        source_event_id: None,
        reason: None,
    })
    .await
    .unwrap();
    // NPC A 习得该 fact（事件账本）。
    db.record_npc_learned_fact(&session, "t1", "npc_a", fact, None)
        .await
        .unwrap();

    // GM 知道；玩家未知；误信 holder 不进真知识投影；NPC A 知道、NPC B 不知道。
    assert!(db
        .gm_truth_view(&session)
        .await
        .unwrap()
        .contains(&fact.to_string()));
    assert!(
        db.player_knowledge_view(&session).await.unwrap().is_empty(),
        "玩家未知该 fact"
    );
    assert!(
        db.list_holder_known_fact_ids(&session, "system", "")
            .await
            .unwrap()
            .is_empty(),
        "believes_false 不进 system 真知识投影"
    );
    assert_eq!(
        db.list_npc_known_fact_ids(&session, "npc_a").await.unwrap(),
        vec![fact.to_string()]
    );
    assert!(db
        .list_npc_known_fact_ids(&session, "npc_b")
        .await
        .unwrap()
        .is_empty());

    purge(&db, &session).await;
}
