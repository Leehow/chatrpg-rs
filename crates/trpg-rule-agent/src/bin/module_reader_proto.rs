//! Standalone prototype: run the agentic module reader against an already-parsed
//! module semantic_units.jsonl (+ optional duotext .md sidecar) — no DB/engine.
//! Validates that the production reader (real LLM + submit schema + fail-closed)
//! produces a populated ModuleGraph (skeleton + deep-extracted entry scene), and
//! RECORDS wall-clock timing for the minimal playable unit (Pass A + Pass B).
//!
//! Usage: module_reader_proto <units.jsonl> [sidecar.md] [budget] [ruleset]
//! Env: TRPG_LLM_BASE_URL / TRPG_LLM_MODEL / TRPG_LLM_API_KEY (+ SEND_TEMPERATURE=false)

use anyhow::Result;
use std::path::PathBuf;
use std::time::Instant;
use trpg_llm::{LlmConfig, OpenAiCompatibleClient};
use trpg_model::SceneExtractionStatus;
use trpg_rule_agent::reader::{load_units, run_module_reader, ModuleReaderCtx};

fn snippet(s: &Option<String>, n: usize) -> String {
    match s {
        Some(t) if !t.trim().is_empty() => {
            let t = t.trim();
            let cut: String = t.chars().take(n).collect();
            format!("{cut}{}", if t.chars().count() > n { " …" } else { "" })
        }
        _ => "<null>".into(),
    }
}

fn class_of(v: &serde_json::Value) -> &str {
    v.get("content_class").and_then(|x| x.as_str()).unwrap_or("story")
}

#[tokio::main]
async fn main() -> Result<()> {
    // subscriber so module_reader 的 per-pass tracing 计时可见
    tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).with_target(true).init();

    let args: Vec<String> = std::env::args().collect();
    let units_path = PathBuf::from(
        args.get(1).expect("usage: module_reader_proto <units.jsonl> [sidecar.md] [budget] [ruleset]"),
    );
    let sidecar_path = args.get(2).filter(|s| !s.is_empty()).map(PathBuf::from);
    let budget: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(12);
    let ruleset = args.get(4).cloned().filter(|s| !s.is_empty());

    let units = load_units(&units_path)?;
    let sidecar_text = sidecar_path.as_ref().and_then(|p| std::fs::read_to_string(p).ok());
    eprintln!(
        "loaded {} units from {}; sidecar={} bytes; budget={budget}; model={}; ruleset={:?}",
        units.len(),
        units_path.display(),
        sidecar_text.as_ref().map(|s| s.len()).unwrap_or(0),
        std::env::var("TRPG_LLM_MODEL").unwrap_or_else(|_| "<default>".into()),
        ruleset
    );

    let client = OpenAiCompatibleClient::new(LlmConfig::from_env()?)?;
    let ctx = ModuleReaderCtx { units: &units, sidecar_text, ruleset_id: ruleset };

    let t0 = Instant::now();
    let readout = run_module_reader(&client, ctx, budget).await?;
    let total = t0.elapsed();

    // ---- summary ----
    let deep = readout
        .scenes
        .iter()
        .filter(|s| s.extraction_status == SceneExtractionStatus::DeepExtracted)
        .count();
    eprintln!("\n==================== MODULE READOUT SUMMARY ====================");
    eprintln!("⏱  MINIMAL PLAYABLE UNIT PARSE: {:.2}s (wall-clock, Pass A + Pass B)", total.as_secs_f64());
    eprintln!(
        "scenes={} (deep_extracted={}, skeleton_only={})",
        readout.scenes.len(),
        deep,
        readout.scenes.len() - deep
    );
    eprintln!(
        "entities: npcs={} clues={} locations={} factions={} encounters={} handouts={} | custom_rules={}",
        readout.npcs.len(),
        readout.clues.len(),
        readout.locations.len(),
        readout.factions.len(),
        readout.encounters.len(),
        readout.handouts.len(),
        readout.module_specific_rules.len(),
    );
    let mut bp3 = 0usize;
    for v in readout.npcs.iter().chain(&readout.clues).chain(&readout.locations).chain(&readout.encounters).chain(&readout.handouts) {
        if class_of(v) == "bp3_index" { bp3 += 1; }
    }
    let bp2 = readout.module_specific_rules.iter().filter(|v| class_of(v) == "bp2_custom_rule").count();
    eprintln!("classification: bp2_custom_rule={bp2}  bp3_index={bp3}");

    eprintln!("\n---- skeleton (first 12 scene stubs) ----");
    for s in readout.scenes.iter().take(12) {
        eprintln!(
            "  [{}] {} (p{:?}-{:?}) status={:?} refs(npc={},clue={},loc={}) links={}",
            s.node_type,
            s.title,
            s.page_start,
            s.page_end,
            s.extraction_status,
            s.referenced_npc_ids.len(),
            s.referenced_clue_ids.len(),
            s.referenced_location_ids.len(),
            s.links.len(),
        );
    }

    if let Some(entry) = readout.scenes.iter().find(|s| s.extraction_status == SceneExtractionStatus::DeepExtracted) {
        eprintln!("\n---- entry scene (deep) ----");
        eprintln!("title: {}", entry.title);
        eprintln!("read_aloud: {}", snippet(&entry.read_aloud, 240));
        eprintln!("gm_notes:   {}", snippet(&entry.gm_notes, 180));
        eprintln!("referenced_npc_ids: {:?}", entry.referenced_npc_ids);
        eprintln!("links: {:?}", entry.links.iter().map(|l| format!("{}({:?})", l.to_node_id, l.link_type)).collect::<Vec<_>>());
    } else {
        eprintln!("\n!! NO deep-extracted entry scene (Pass B produced nothing) !!");
    }

    // 全量 dump（env TRPG_PROTO_DUMP=<path>）：scenes + 所有实体 vec + 自定义规则 + spine。
    if let Ok(path) = std::env::var("TRPG_PROTO_DUMP") {
        let dump = serde_json::json!({
            "spine": readout.spine,
            "scenes": readout.scenes,
            "npcs": readout.npcs,
            "clues": readout.clues,
            "locations": readout.locations,
            "factions": readout.factions,
            "encounters": readout.encounters,
            "handouts": readout.handouts,
            "module_specific_rules": readout.module_specific_rules,
        });
        std::fs::write(&path, serde_json::to_string_pretty(&dump)?)?;
        eprintln!("dumped full readout → {path}");
    }
    Ok(())
}
