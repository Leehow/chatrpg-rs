//! Standalone prototype/audit runner for the mechanics-catalog compile pass
//! (the rule agent's THIRD pass) — no DB/engine, mirrors module_reader_proto.
//! Feeds an already-parsed rulebook's semantic units + a rule-kernel artifact
//! through `compile_mechanics_catalog` and prints an audit summary: entry
//! count, kind / expressiveness-tier distributions, validation warnings, gap
//! notes and wall-clock. Used by the six-ruleset batch + appendix-A coverage
//! audit (plan task A7). `--out` writes the upgraded kernel JSON (no DB —
//! real persistence goes through a parse re-run).
//!
//! Usage: mechanics_proto <units.jsonl> <kernel.json> [sidecar.md] [budget]
//!          [--out <path>] [--audit <path>] [--missing <json>]
//!   kernel.json = data/parsed/rules/{ruleset}.rule_kernel.json artifact
//!                 (or a dump of rule_kernels.content_json from the real DB)
//!   --audit   dump the compiled catalog as audit-friendly JSON lines
//!             (one entry per line: id/name/kind/tier/when_to_use)
//!   --missing OFFLINE mode (no LLM): read the coverage-audit gap list
//!             (JSON array of strings or {item,note} objects) and append each
//!             as a code=catalog_coverage_gap warning to the kernel's
//!             validation_report — the machine-visible half of the audit.
//!             Combine with --out to persist; --audit also works offline.
//! Env: TRPG_LLM_BASE_URL / TRPG_LLM_API_KEY
//!      + TRPG_MECHANICS_COMPILE_MODEL (default gpt-5.4 — same model as
//!        production, otherwise the audit is meaningless)

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;
use trpg_llm::{LlmConfig, OpenAiCompatibleClient};
use trpg_model::{expressiveness_tier, ExpressivenessTier, RuleKernel};
use trpg_rule_agent::reader::{compile_mechanics_catalog, load_units, MechCompileCtx};

const USAGE: &str = "usage: mechanics_proto <units.jsonl> <kernel.json> [sidecar.md] [budget] [--out <path>] [--audit <path>] [--missing <json>]";

/// Audit-friendly dump: one JSON object per line (id/name/kind/tier/when_to_use).
fn write_audit_dump(path: &PathBuf, kernel: &RuleKernel) -> Result<()> {
    let mut lines = Vec::with_capacity(kernel.mechanics_catalog.len());
    for e in &kernel.mechanics_catalog {
        let kind = serde_json::to_value(&e.kind)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        lines.push(serde_json::to_string(&serde_json::json!({
            "id": e.id, "name": e.name, "kind": kind,
            "tier": tier_label(expressiveness_tier(e)),
            "when_to_use": e.when_to_use,
        }))?);
    }
    std::fs::write(path, lines.join("\n") + "\n")
        .with_context(|| format!("write audit dump to {}", path.display()))?;
    eprintln!(
        "audit dump ({} entries) written to {}",
        kernel.mechanics_catalog.len(),
        path.display()
    );
    Ok(())
}

/// `--missing` half: append each audited gap as a `catalog_coverage_gap`
/// warning. Accepts a JSON array of strings or of {item, note} objects.
fn append_coverage_gaps(kernel: &mut RuleKernel, raw: &str) -> Result<usize> {
    let items: Vec<serde_json::Value> =
        serde_json::from_str(raw).context("parse --missing JSON array")?;
    let mut n = 0;
    for it in items {
        let (target, message) = match &it {
            serde_json::Value::String(s) => (s.clone(), String::new()),
            serde_json::Value::Object(o) => (
                o.get("item")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                o.get("note")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            ),
            _ => continue, // fail-closed: unrecognized shape is skipped, not invented
        };
        if target.is_empty() {
            continue;
        }
        kernel
            .validation_report
            .warnings
            .push(trpg_model::ValidationMessage {
                code: "catalog_coverage_gap".to_string(),
                message: if message.is_empty() {
                    "missing from mechanics_catalog (appendix-A coverage audit)".to_string()
                } else {
                    message
                },
                target: Some(target),
            });
        n += 1;
    }
    Ok(n)
}

fn tier_label(t: ExpressivenessTier) -> &'static str {
    match t {
        ExpressivenessTier::Procedure => "procedure",
        ExpressivenessTier::Hook => "hook",
        ExpressivenessTier::PassiveModifier => "passive_modifier",
        ExpressivenessTier::Semantic => "semantic",
    }
}

/// Skill-name supplement for `tested_parameter` validation: the kernel
/// artifact does not carry the reader's option catalogs, so the proto derives
/// the supplement from the sheet schema's own `skill` fields (the template
/// half of `trpg-parser::skill_ids`). Data-driven — nothing per-ruleset.
fn schema_skill_names(kernel: &RuleKernel) -> Vec<String> {
    kernel
        .character_sheet_schema
        .get("fields")
        .and_then(|f| f.as_array())
        .map(|fields| {
            fields
                .iter()
                .filter(|f| f.get("field_type").and_then(|v| v.as_str()) == Some("skill"))
                .filter_map(|f| f.get("field_id").and_then(|v| v.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::main]
async fn main() -> Result<()> {
    // Subscriber so the compile loop's per-round tracing is visible.
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(true)
        .init();

    // ---- args: strip the flag pairs first, then positionals ----
    let mut raw: Vec<String> = std::env::args().skip(1).collect();
    let take_flag = |raw: &mut Vec<String>, flag: &str| -> Result<Option<PathBuf>> {
        if let Some(i) = raw.iter().position(|a| a == flag) {
            raw.remove(i);
            anyhow::ensure!(i < raw.len(), "{flag} requires a path. {USAGE}");
            return Ok(Some(PathBuf::from(raw.remove(i))));
        }
        Ok(None)
    };
    let out_path = take_flag(&mut raw, "--out")?;
    let audit_path = take_flag(&mut raw, "--audit")?;
    let missing_path = take_flag(&mut raw, "--missing")?;
    let units_path = PathBuf::from(raw.first().with_context(|| USAGE.to_string())?);
    let kernel_path = PathBuf::from(raw.get(1).with_context(|| USAGE.to_string())?);
    let sidecar_path = raw.get(2).filter(|s| !s.is_empty()).map(PathBuf::from);
    let budget: usize = raw.get(3).and_then(|s| s.parse().ok()).unwrap_or(14);

    let units = load_units(&units_path)?;
    let sidecar_text = sidecar_path
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok());
    // serde(default) on the kernel's new regions keeps OLD artifacts loadable.
    let mut kernel: RuleKernel = serde_json::from_str(
        &std::fs::read_to_string(&kernel_path)
            .with_context(|| format!("read kernel artifact {}", kernel_path.display()))?,
    )
    .with_context(|| format!("parse RuleKernel from {}", kernel_path.display()))?;
    let pre_warnings = kernel.validation_report.warnings.len();

    // ---- OFFLINE mode (--missing): no LLM, just record audited gaps ----
    if let Some(missing) = missing_path {
        let raw = std::fs::read_to_string(&missing)
            .with_context(|| format!("read missing list {}", missing.display()))?;
        let n = append_coverage_gaps(&mut kernel, &raw)?;
        eprintln!(
            "appended {n} catalog_coverage_gap warnings ({} total) to kernel {}",
            kernel.validation_report.warnings.len(),
            kernel.ruleset_id
        );
        if let Some(audit) = audit_path {
            write_audit_dump(&audit, &kernel)?;
        }
        if let Some(out) = out_path {
            std::fs::write(&out, serde_json::to_string_pretty(&kernel)?)
                .with_context(|| format!("write upgraded kernel to {}", out.display()))?;
            eprintln!("kernel with coverage gaps written to {}", out.display());
        }
        return Ok(());
    }

    let mut cfg = LlmConfig::from_env()?;
    cfg.model =
        std::env::var("TRPG_MECHANICS_COMPILE_MODEL").unwrap_or_else(|_| "gpt-5.4".to_string());
    let model = cfg.model.clone();
    let client = OpenAiCompatibleClient::new(cfg)?;

    eprintln!(
        "loaded {} units from {}; sidecar={} bytes; kernel={} ({}); budget={budget}; model={model}",
        units.len(),
        units_path.display(),
        sidecar_text.as_ref().map(|s| s.len()).unwrap_or(0),
        kernel_path.display(),
        kernel.ruleset_id,
    );

    let skill_names = schema_skill_names(&kernel);
    let ctx = MechCompileCtx {
        units: &units,
        sidecar_text,
        located_pages: String::new(),
        skill_names,
    };

    let t0 = Instant::now();
    let gaps = compile_mechanics_catalog(&client, &mut kernel, ctx, budget).await;
    let total = t0.elapsed();

    // ---- audit summary ----
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_tier: BTreeMap<&'static str, usize> = BTreeMap::new();
    for e in &kernel.mechanics_catalog {
        let kind = serde_json::to_value(&e.kind)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "<unclassified>".into());
        *by_kind.entry(kind).or_insert(0) += 1;
        *by_tier
            .entry(tier_label(expressiveness_tier(e)))
            .or_insert(0) += 1;
    }
    eprintln!("\n==================== MECHANICS CATALOG SUMMARY ====================");
    eprintln!("⏱  COMPILE PASS WALL-CLOCK: {:.2}s", total.as_secs_f64());
    eprintln!("catalog entries: {}", kernel.mechanics_catalog.len());
    eprintln!("-- by kind --");
    for (k, n) in &by_kind {
        eprintln!("  {k}: {n}");
    }
    eprintln!("-- by expressiveness tier --");
    for (t, n) in &by_tier {
        eprintln!("  {t}: {n}");
    }
    eprintln!("-- entries (id | name | kind | tier) --");
    for e in &kernel.mechanics_catalog {
        let kind = serde_json::to_value(&e.kind)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        eprintln!(
            "  {} | {} | {} | {}",
            e.id,
            e.name,
            kind,
            tier_label(expressiveness_tier(e))
        );
    }
    eprintln!(
        "-- validation_report.warnings ({} total, {} pre-existing) --",
        kernel.validation_report.warnings.len(),
        pre_warnings
    );
    for w in &kernel.validation_report.warnings {
        eprintln!(
            "  {} [{}]: {}",
            w.code,
            w.target.as_deref().unwrap_or("-"),
            w.message
        );
    }
    eprintln!("-- gap notes ({}) --", gaps.len());
    for g in &gaps {
        eprintln!("  {g}");
    }

    if let Some(audit) = audit_path {
        write_audit_dump(&audit, &kernel)?;
    }
    if let Some(out) = out_path {
        std::fs::write(&out, serde_json::to_string_pretty(&kernel)?)
            .with_context(|| format!("write upgraded kernel to {}", out.display()))?;
        eprintln!("upgraded kernel written to {}", out.display());
    }
    Ok(())
}
