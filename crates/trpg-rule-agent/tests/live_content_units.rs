//! P0-2 live DB 实测:在真实的 homecoming 解析图上派生 ContentUnit 层级。证明
//! `chapters/missions` 不再恒 0 —— `derive_content_units` 从 reader 已分段的
//! ScenarioNode(node_type 结构指纹 + 标题)产出一棵真 Contains 树(Campaign→
//! Chapter→Scene/Procedure→Beat),每个非根单元带 parent + 逐字 source_evidence,
//! 且 NET/hacking 子程序场景 → Procedure(非普通 Scene)。
//!
//! Run (rulesets DB lives at :54347):
//!   DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
//!   CARGO_TARGET_DIR=target-air cargo test -p trpg-rule-agent \
//!     --test live_content_units -- --nocapture
//!
//! 无 `DATABASE_URL` ⇒ SKIP(fail-closed,绝不阻塞 CI)。**反假绿**:真跑时必打印
//! `RAN: ...` + 计数 + `PASS`;只见 `SKIP` = 没验证。
use trpg_db::Db;
use trpg_model::adventure_ir::{derive_content_units, project_chapters, UnitKind};

const HOMECOMING: &str = "cyberpunk_red.homecoming";
const SOURCE_ID: &str = "cpr_one_shot_homecoming_ver3_0_colored";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn homecoming_derives_real_contains_hierarchy() {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP: DATABASE_URL unset (need live rulesets DB :54347)");
            return;
        }
    };
    let db = Db::connect(&url).await.expect("connect live DB");
    let graph = db
        .load_module_graph(HOMECOMING)
        .await
        .expect("query ok")
        .expect("homecoming module graph present in parsed_bundles");

    // 基线事实:持久化图谱里 chapters/missions 恒 0,scenes 是平铺列表。
    eprintln!(
        "RAN: baseline scenes={}, chapters={}, missions={}",
        graph.scenes.len(),
        graph.chapters.len(),
        graph.missions.len()
    );
    assert!(!graph.scenes.is_empty(), "homecoming 应有已分段 scenes");

    // 派生(纯函数,无 LLM):从真实 scene 指纹建 Contains 树。
    let units = derive_content_units(HOMECOMING, SOURCE_ID, &graph.title, &graph.scenes);
    let chapters = project_chapters(&units);
    let roots = units.iter().filter(|u| u.parent_id.is_none()).count();
    let n_chapters = units.iter().filter(|u| u.kind == UnitKind::Chapter).count();
    let n_scene_tier = units
        .iter()
        .filter(|u| {
            matches!(
                u.kind,
                UnitKind::Scene | UnitKind::Procedure | UnitKind::Encounter
            )
        })
        .count();
    let n_beats = units.iter().filter(|u| u.kind == UnitKind::Beat).count();
    let with_evidence = units
        .iter()
        .filter(|u| !u.source_evidence.is_empty())
        .count();
    eprintln!(
        "RAN: derived content_units={} (roots={}, chapters={}, scene_tier={}, beats={}), with_evidence={}, projected_chapters={}",
        units.len(),
        roots,
        n_chapters,
        n_scene_tier,
        n_beats,
        with_evidence,
        chapters.len()
    );

    // 1) 真层级:单根 + ≥2 章 + 场景层 + 子拍,不再恒 0。
    assert_eq!(roots, 1, "恰一个 Campaign 根");
    assert!(n_chapters >= 2, "结构章(front/adventure/reference)≥2");
    assert!(
        n_scene_tier >= graph.scenes.len() / 2,
        "多数 scene 进入场景层"
    );
    assert!(
        !chapters.is_empty(),
        "compat 投影:ModuleGraph.chapters 不再恒 0"
    );

    // 2) 每个非根单元有 parent(Contains)+ 逐字 evidence(禁造)。
    assert_eq!(
        units.iter().filter(|u| u.parent_id.is_some()).count(),
        units.len() - 1,
        "每个非根单元挂在父节点下"
    );
    let scene_units: Vec<_> = units
        .iter()
        .filter(|u| u.parent_id.is_some() && u.kind != UnitKind::Chapter)
        .collect();
    assert!(
        scene_units
            .iter()
            .all(|u| !u.source_evidence.is_empty() && u.source_evidence[0].source_id == SOURCE_ID),
        "场景/拍单元全带真 source_evidence(source_id+page)"
    );

    // 3) golden:hacking/NET/escape 子程序场景 → Procedure,绝不普通 Scene。
    let procedures = units
        .iter()
        .filter(|u| u.kind == UnitKind::Procedure)
        .count();
    eprintln!("RAN: procedure-kind units={}", procedures);
    assert!(
        procedures >= 1,
        "应有 ≥1 location_procedure 场景被标 Procedure(golden:NET≠Scene)"
    );

    eprintln!("PASS: homecoming ContentUnit 层级真出数(chapters/missions 不再恒 0,带 evidence)");
}
