//! 等价验证：MaterialNeedResolver.resolve() 产块 == 旧 materialization_blocks_for_turn 产块。
//! 需：DATABASE_URL 指向含活跃 session 的 CoC 库（:54347）。缺则 SKIP。
//! Run:
//!   DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!   cargo test -p trpg-runtime --test material_need_resolver_equiv -- --nocapture

use trpg_db::Db;
use trpg_model::{ContextRequest, TokenBudget, VisibilityProfile};
use trpg_need::{MaterialNeed, Need, NeedResolver, NeedScopes};
use trpg_runtime::{material_need_resolver::MaterialNeedResolver, RuntimeEngine};

const RULESET: &str = "call_of_cthulhu_7e";

#[tokio::test]
async fn material_resolver_blocks_byte_equiv_to_direct() {
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
    let session_id: Option<String> = sqlx::query_scalar("select session_id from sessions limit 1")
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

    let engine = RuntimeEngine::new(db.clone());
    let request = ContextRequest {
        ruleset_id: RULESET.into(),
        module_id: None,
        session_id: session_id.clone(),
        turn_id: "turn_test_mat_equiv".into(),
        viewer: VisibilityProfile::gm(),
        token_budget: TokenBudget::default(),
    };

    // 旧路径：直接调 materialization_blocks_for_turn
    let old_blocks = engine
        .materialization_blocks_for_turn_pub(&request)
        .await
        .expect("old path ok");

    // 新路径：经 MaterialNeedResolver
    let scopes = NeedScopes {
        ruleset_id: request.ruleset_id.clone(),
        module_id: request.module_id.clone(),
        session_id: request.session_id.clone(),
        turn_id: request.turn_id.clone(),
        scene_id: None,
    };
    let resolver = MaterialNeedResolver::new(db.clone());
    let need = Need::Material(MaterialNeed { scopes, user_input: None });
    let outcome = resolver.resolve(&need).await.expect("resolver ok");

    // 块数量等价
    assert_eq!(
        old_blocks.len(),
        outcome.blocks.len(),
        "block count mismatch: old={} new={}",
        old_blocks.len(),
        outcome.blocks.len()
    );
    // block_id 等价（content_hash 可能因两次 world_tick 查询的极小时机差异略有偏差，
    // 实际上两次都在同一进程内同步完成、world_tick 不会进位，因此也断言 content_hash）
    for (old, new) in old_blocks.iter().zip(outcome.blocks.iter()) {
        assert_eq!(old.block_id, new.block_id, "block_id mismatch");
        assert_eq!(
            old.content_hash, new.content_hash,
            "content_hash mismatch for block_id={}; old and new resolver must produce identical blocks",
            old.block_id
        );
    }
    println!(
        "[material_equiv] {} block(s) verified byte-equivalent",
        outcome.blocks.len()
    );
}
