//! Standalone prototype: run `compile_starter_pack` on a parsed units.jsonl +
//! duotext markdown — no DB. Validates the starter-pack extraction + guardrail
//! quickly (vs a ~10-min full staged parse).
//!
//! Usage: onboarding_proto <units.jsonl> <merged.md> [budget] [role_field] [skills_csv]
//! Env: TRPG_LLM_BASE_URL / TRPG_LLM_MODEL / TRPG_LLM_API_KEY (+ SEND_TEMPERATURE=false)

use anyhow::Result;
use std::path::PathBuf;
use trpg_llm::{LlmConfig, OpenAiCompatibleClient};
use trpg_rule_agent::reader::{compile_starter_pack, load_units, OnboardingCtx};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();
    let args: Vec<String> = std::env::args().collect();
    let units = load_units(&PathBuf::from(args.get(1).expect(
        "usage: onboarding_proto <units.jsonl> <merged.md> [budget] [role_field] [skills_csv]",
    )))?;
    let sidecar = args.get(2).and_then(|p| std::fs::read_to_string(p).ok());
    let budget: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(10);
    let role_field = args.get(4).filter(|s| !s.is_empty()).cloned();
    let skill_fields: Vec<String> = args
        .get(5)
        .map(|s| {
            s.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        })
        .unwrap_or_default();
    eprintln!(
        "units={} sidecar={}b role={:?} skills={}",
        units.len(),
        sidecar.as_ref().map(|s| s.len()).unwrap_or(0),
        role_field,
        skill_fields.len()
    );
    let client = OpenAiCompatibleClient::new(LlmConfig::from_env()?)?;
    let ctx = OnboardingCtx {
        units: &units,
        sidecar_text: sidecar,
        role_field,
        skill_fields,
        option_catalogs: serde_json::json!([]),
    };
    let pack = compile_starter_pack(&client, &ctx, budget).await;
    println!("{}", serde_json::to_string_pretty(&pack)?);
    Ok(())
}
