//! Run: DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54345/chatrpg \
//!      TRPG_DATA_DIR=$PWD/data \
//!      cargo test -p trpg-db --test live_module_config -- --nocapture
//!
//! LIVE 证明 `load_module_config` 的 **extracted ⊕ override** 合并整条链路(load_module_graph
//! SQL → merge → ModuleConfig.director):
//! ① 无 sidecar 的模组 → director = module reader 自动抽取并存进 `module_graph.director_facilitation`
//!    的值(本任务退役 REQUIRED sidecar 的核心 —— 验收 1 的读侧);
//! ② 有 sidecar(homecoming,hand-authored 文件 / embedded)→ sidecar 胜过 extracted(验收 2)。
//! 无 DATABASE_URL 时 SKIP(与既有 live_*.rs 一致),`cargo test --workspace` 干净。
use trpg_db::Db;
use trpg_model::{DirectorModuleConfig, DirectorPressureItem, DirectorSceneFact, ModuleGraph};

fn director_is_empty(d: &DirectorModuleConfig) -> bool {
    d.scene_facts.is_empty()
        && d.pressure_items.is_empty()
        && d.affordance_items.is_empty()
        && d.risk_items.is_empty()
        && d.npc_advice.is_empty()
        && d.known_facts.is_empty()
        && d.open_question.is_none()
        && d.place_summary_fallback.is_none()
}

/// §6.4 读+合并:无 sidecar 的模组,load_module_config 返回 module_graph 里抽取的 director。
/// 用 throwaway module_id 直接 SQL 注入一个带 `director_facilitation` 的 module bundle
/// (不依赖 LLM、不动真实模组),证明 `load_module_graph → merge → ModuleConfig.director` LIVE 链路。
#[tokio::test]
async fn extracted_facilitation_served_when_no_sidecar() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect: {e}"); return; } };
    const MID: &str = "ut_facil_live_demo_zzz"; // throwaway:无 sidecar、无 embedded
    // 用真实 ModuleGraph 序列化(全字段齐全,镜像 parser 写入的形态),只填 director_facilitation。
    let graph = ModuleGraph {
        module_id: MID.into(),
        director_facilitation: Some(DirectorModuleConfig {
            scene_facts: vec![DirectorSceneFact { text: "门半开着".into(), source: "read_aloud".into() }],
            pressure_items: vec![DirectorPressureItem { text: "夜色渐深".into(), severity: 2, ..Default::default() }],
            ..Default::default()
        }),
        ..Default::default()
    };
    let content = serde_json::json!({ "module_id": MID, "module_graph": graph });
    sqlx::query(
        r#"insert into parsed_bundles
           (id, bundle_id, bundle_kind, title, schema_version, source_hash, parse_config_hash, content_json)
           values (gen_random_uuid(), $1, 'module', 'ut facil demo', 'test', 'h', 'h', $2)
           on conflict (bundle_id) do update set content_json = excluded.content_json, updated_at = now()"#,
    )
    .bind(MID)
    .bind(&content)
    .execute(&db.pool)
    .await
    .expect("seed throwaway module bundle");

    let cfg = db.load_module_config(MID).await;
    // cleanup BEFORE asserting so a failure still leaves a clean DB.
    sqlx::query("delete from parsed_bundles where bundle_id = $1").bind(MID).execute(&db.pool).await.ok();

    let director = cfg.and_then(|c| c.director).expect("无 sidecar → director 应来自抽取的 director_facilitation");
    assert!(!director_is_empty(&director), "extracted director 必须非空");
    assert_eq!(director.scene_facts.first().map(|f| f.text.as_str()), Some("门半开着"));
    assert_eq!(director.pressure_items.len(), 1, "pressure 也应从 graph 读到");
}

/// §6.5 override 胜:homecoming 有 hand-authored(`{TRPG_DATA_DIR}/modules/...module_config.json`)
/// / embedded sidecar → load_module_config 返回 sidecar 的 director,即使 module_graph 也带了
/// re-parse 抽取的 director_facilitation(sidecar 整块盖掉 extracted)。
#[tokio::test]
async fn override_sidecar_wins_for_homecoming() {
    let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
    let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect: {e}"); return; } };
    const MID: &str = "cyberpunk_red.homecoming";
    let cfg = match db.load_module_config(MID).await {
        Some(c) => c,
        None => { eprintln!("SKIP: no {MID} config (need TRPG_DATA_DIR with sidecar, or embedded)"); return; }
    };
    let director = cfg.director.expect("homecoming 必有 sidecar director");
    // hand-authored homecoming 配置带 scene_facts / npc_advice,非空 → 证明 override 胜出(非空抽取被盖掉)。
    assert!(
        !director.scene_facts.is_empty() || !director.npc_advice.is_empty(),
        "homecoming sidecar director 应非空(override 胜过 extracted)"
    );
}
