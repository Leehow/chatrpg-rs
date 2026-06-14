//! R2 收口后：ParameterNeedResolver 是唯一的 actor-parameter 获取路径。
//! 旧直连 `actor_parameter_blocks_for_turn(_pub)` 已删，A/B 比对不再可能；
//! 本测试改为断言 bus 路径（resolver）能产出块且 fail-closed 不 panic。
//! 需：DATABASE_URL 指向含活跃 session 的 CoC 库（:54347）。缺则 SKIP。
//! Run:
//!   DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!   cargo test -p trpg-runtime --test parameter_resolver_equiv -- --nocapture

use trpg_db::Db;
use trpg_need::{Need, NeedResolver, NeedScopes, ParameterNeed};
use trpg_runtime::parameter_need_resolver::ParameterNeedResolver;

const RULESET: &str = "call_of_cthulhu_7e";

#[tokio::test]
async fn parameter_resolver_produces_blocks() {
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
            eprintln!("SKIP: db connect failed: {e}");
            return;
        }
    };

    // Pick any active session from the DB; SKIP gracefully if none found.
    let session_id: Option<String> =
        sqlx::query_scalar("select session_id from sessions limit 1")
            .fetch_optional(&db.pool)
            .await
            .unwrap_or(None);
    let session_id = match session_id {
        Some(s) => s,
        None => {
            eprintln!("SKIP: no sessions in DB");
            return;
        }
    };

    // bus 路径（唯一获取路径）：经 ParameterNeedResolver。
    let scopes = NeedScopes {
        ruleset_id: RULESET.into(),
        module_id: None,
        session_id: session_id.clone(),
        turn_id: "turn_test_param".into(),
        scene_id: None,
    };
    let resolver = ParameterNeedResolver::new(db.clone());
    let need = Need::Parameter(ParameterNeed {
        scopes,
        actor_id: None, // None → 退化为 "pc.current"
        current_input: None,
    });
    let outcome = resolver.resolve(&need).await.expect("resolver ok");

    // actor-parameter 投影恒产 1 个块（actor_parameters_context_block）。
    assert_eq!(
        outcome.blocks.len(),
        1,
        "ParameterNeedResolver must project exactly one actor-parameter block"
    );
    eprintln!(
        "[parameter_bus] {} block(s) produced via the bus path",
        outcome.blocks.len()
    );
}

#[tokio::test]
async fn parameter_resolver_fail_closed() {
    // Resolver 对不存在的 session 不 panic、返回空 outcome（fail-closed）。
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
            eprintln!("SKIP: db connect failed: {e}");
            return;
        }
    };
    use trpg_need::NeedBus;

    let resolver = ParameterNeedResolver::new(db);
    let mut bus = NeedBus::new();
    bus.register(Box::new(resolver));
    bus.emit(Need::Parameter(ParameterNeed {
        scopes: NeedScopes {
            ruleset_id: "nonexistent".into(),
            module_id: None,
            session_id: "session_does_not_exist_xyz".into(),
            turn_id: "t0".into(),
            scene_id: None,
        },
        actor_id: None,
        current_input: None,
    }));
    let outcomes = bus.resolve_all().await;
    // fail-closed: 返回 vec，长度=1，不 panic
    assert_eq!(outcomes.len(), 1, "fail-closed: must return 1 outcome entry");
    eprintln!("[parameter_fail_closed] PASS: {} outcome(s) returned", outcomes.len());
}
