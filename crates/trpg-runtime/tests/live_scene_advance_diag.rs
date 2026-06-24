//! Throwaway live diagnostic (D2): list every homecoming scene with its scene-advance
//! atom count + kinds, and the module's entry scene, so the inline smoke targets a
//! fireable scene. SKIP without DATABASE_URL.
use trpg_db::Db;
use trpg_model::adventure_ir::{scene_advance_guard_leaves, scene_advance_objective};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diag_homecoming_scene_advance_atoms() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("SKIP: DATABASE_URL unset");
        return;
    };
    let db = Db::connect(&url).await.expect("connect");
    let mut graph = db
        .load_module_graph("cyberpunk_red.homecoming")
        .await
        .expect("q")
        .expect("homecoming present");
    let _ = trpg_runtime::clue_projection::project_clues_onto_scenes(&mut graph);
    let entry = graph
        .scenes
        .iter()
        .filter(|s| !s.node_id.trim().is_empty())
        .min_by_key(|s| s.page_start.unwrap_or(u32::MAX))
        .map(|s| s.node_id.clone());
    eprintln!("ENTRY scene (lowest page): {entry:?}");
    for s in &graph.scenes {
        let id = s.node_id.trim();
        if id.is_empty() {
            continue;
        }
        let leaves = scene_advance_guard_leaves(&graph, id);
        let kinds: Vec<String> = leaves.iter().map(|a| format!("{:?}", a.kind)).collect();
        let has_obj = scene_advance_objective(&graph, id).is_some();
        eprintln!(
            "scene={id} page={:?} advance_objective={has_obj} atoms={} kinds={:?}",
            s.page_start,
            leaves.len(),
            kinds
        );
    }
}
