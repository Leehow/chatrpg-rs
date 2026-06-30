//! P0-1 live DB 实测：homecoming 的实体共现边在 typed 旗标下标 `AssociatedByEntity`
//! （RetrievalOnly + evidence），绝不 `SpatialAdjacent` —— 共享实体证明相关性而非空间相邻。
//!
//! Run (rulesets DB lives at :54347)：
//!   DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!   CARGO_TARGET_DIR=target-air cargo test -p trpg-rule-agent \
//!     --test live_typed_cooccurrence -- --nocapture
//!
//! 无 `DATABASE_URL` ⇒ SKIP（fail-closed，绝不阻塞 CI）。**反假绿**：真跑时必打印
//! `RAN: ...` + 计数 + `PASS`；只见 `SKIP` = 没验证（见 eval-harness-cwd-false-green 教训）。
use trpg_db::Db;
use trpg_model::adventure_ir::{Enforcement, RelationKind};
use trpg_rule_agent::reader::apply_bridge_edges;
use trpg_rule_agent::reader::module_graph_edges::bridge_edges;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn homecoming_cooccurrence_is_associated_not_spatial() {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP: DATABASE_URL unset (need live rulesets DB :54347)");
            return;
        }
    };
    let db = Db::connect(&url).await.expect("connect live DB");
    let graph = db
        .load_module_graph("cyberpunk_red.homecoming")
        .await
        .expect("query ok")
        .expect("homecoming module graph present in parsed_bundles");

    let mut scenes = graph.scenes.clone();
    // 原始共现对（结构事实，与既有 links 无关）。
    let pairs = bridge_edges(&scenes);
    assert!(
        !pairs.is_empty(),
        "homecoming 应有实体共现场景对（referenced-id 已填）"
    );
    eprintln!(
        "RAN: homecoming scenes={}, cooccurrence_pairs={}",
        scenes.len(),
        pairs.len()
    );

    // 隔离桥接贡献：清掉既有 links/relations 再跑 typed=ON，看共现 pass 产什么。
    for s in &mut scenes {
        s.links.clear();
        s.relations.clear();
    }
    std::env::set_var("TRPG_TYPED_COOCCURRENCE", "1");
    let added = apply_bridge_edges(&mut scenes);
    std::env::remove_var("TRPG_TYPED_COOCCURRENCE");
    assert_eq!(added, pairs.len() * 2, "双向 typed relation");

    let rels: Vec<_> = scenes.iter().flat_map(|s| &s.relations).collect();
    eprintln!("RAN: typed relations produced={}", rels.len());
    assert!(!rels.is_empty());
    // 核心断言：共现边全 AssociatedByEntity/RetrievalOnly + 带 evidence，无一标 Spatial。
    assert!(
        rels.iter()
            .all(|r| r.kind == RelationKind::AssociatedByEntity),
        "homecoming 共现边全 AssociatedByEntity"
    );
    assert!(
        !rels.iter().any(|r| r.kind == RelationKind::SpatialAdjacent),
        "无一标 SpatialAdjacent"
    );
    assert!(rels
        .iter()
        .all(|r| r.enforcement == Enforcement::RetrievalOnly));
    assert!(rels.iter().all(|r| !r.evidence.is_empty()), "带 evidence");
    assert!(
        rels.iter().all(|r| !r.participates_in_progression()),
        "retrieval-only 永不驱动推进"
    );
    // typed 路径不产 Spatial 共现链接（nav 出口不被实体共现污染）。
    let spatial_links: usize = scenes.iter().map(|s| s.links.len()).sum();
    assert_eq!(spatial_links, 0, "typed 路径不产 Spatial 共现链接");
    eprintln!(
        "RAN: PASS — homecoming cooccurrence typed as AssociatedByEntity ({} edges), 0 Spatial",
        rels.len()
    );
}
