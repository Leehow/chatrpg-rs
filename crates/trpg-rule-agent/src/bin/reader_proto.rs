//! Standalone prototype: run the rulebook-reader agent against an already-parsed
//! semantic_units.jsonl, no full engine/DB. Validates the Rust agent reaches the
//! same GM run-kit as my hand-run, cheaply, before wiring into parse-all.
//!
//! Usage: reader_proto <semantic_units.jsonl> <ruleset> [max_tools]
//! Env: TRPG_LLM_BASE_URL / TRPG_LLM_MODEL / TRPG_LLM_API_KEY (+ SEND_TEMPERATURE=false)

use anyhow::Result;
use std::path::PathBuf;
use trpg_llm::{LlmConfig, OpenAiCompatibleClient};
use trpg_rule_agent::reader::{load_units, run_reader, run_reader_parallel};

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let units_path = PathBuf::from(
        args.get(1)
            .expect("usage: reader_proto <units.jsonl> <ruleset> [max_tools] [single|parallel]"),
    );
    let ruleset = args.get(2).cloned().unwrap_or_else(|| "unknown".into());
    let max_tools: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(12);
    let mode = args.get(4).cloned().unwrap_or_else(|| "parallel".into());

    let units = load_units(&units_path)?;
    eprintln!(
        "loaded {} units from {} (mode={mode})",
        units.len(),
        units_path.display()
    );

    let client = OpenAiCompatibleClient::new(LlmConfig::from_env()?)?;
    let res = if mode == "single" {
        run_reader(&client, &units, &ruleset, max_tools).await?
    } else {
        run_reader_parallel(&client, &units, &ruleset, max_tools).await?
    };

    eprintln!(
        "=== DONE: {} tool-calls / {} LLM-calls / {} tokens ({} prompt + {} completion)",
        res.tool_calls,
        res.llm_calls,
        res.prompt_tokens + res.completion_tokens,
        res.prompt_tokens,
        res.completion_tokens
    );
    println!("{}", serde_json::to_string_pretty(&res.run_kit)?);
    Ok(())
}
