//! Standalone prototype: run the object/ability schema compiler against an
//! already-parsed semantic_units.jsonl + its duotext markdown — no DB/engine.
//! Validates the discover→extract pass produces a usable per-category schema +
//! 2 source-backed examples, before wiring into parse-all.
//!
//! Usage: object_proto <units.jsonl> <merged.md> [budget] [skills_csv]
//! Env: TRPG_LLM_BASE_URL / TRPG_LLM_MODEL / TRPG_LLM_API_KEY (+ SEND_TEMPERATURE=false)

use anyhow::Result;
use std::path::PathBuf;
use trpg_llm::{LlmConfig, OpenAiCompatibleClient};
use trpg_rule_agent::reader::{compile_object_schemas, load_units, ObjectCtx};

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let units_path = PathBuf::from(
        args.get(1).expect("usage: object_proto <units.jsonl> <merged.md> [budget] [skills_csv]"),
    );
    let md_path = args.get(2).map(PathBuf::from);
    let budget: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(10);
    let skills: Vec<String> = args
        .get(4)
        .map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect())
        .unwrap_or_default();

    let units = load_units(&units_path)?;
    let sidecar = md_path.as_ref().and_then(|p| std::fs::read_to_string(p).ok());
    eprintln!(
        "loaded {} units; sidecar={} bytes; skills={:?}",
        units.len(),
        sidecar.as_ref().map(|s| s.len()).unwrap_or(0),
        skills
    );

    let client = OpenAiCompatibleClient::new(LlmConfig::from_env()?)?;
    let ctx = ObjectCtx { units: &units, sidecar_text: sidecar, skills, resource_tracks: vec![] };
    let schemas = compile_object_schemas(&client, &ctx, budget).await;

    eprintln!("=== produced {} categories", schemas.len());
    println!("{}", serde_json::to_string_pretty(&schemas)?);
    Ok(())
}
