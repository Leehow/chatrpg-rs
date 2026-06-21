//! CLI entry:
//!   `trpg-eval <report.md> [--json]`      — static-transcript gate (exit≠0 on FAIL)
//!   `trpg-eval player "<scene>" [--json]` — run all §三 personas on a GM scene
//!                                           (the §六 counterfactual building block)

use std::process::ExitCode;
use trpg_eval::player::{audit_player_reads_gm, deliberate, PlayerPersona};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("player") {
        return run_player(&args);
    }
    if args.get(1).map(|s| s.as_str()) == Some("counterfactual") {
        return run_counterfactual(&args);
    }
    let path = match args.iter().skip(1).find(|a| !a.starts_with("--")) {
        Some(p) => p,
        None => {
            eprintln!("usage: trpg-eval <report.md> [--json]");
            return ExitCode::from(2);
        }
    };
    let md = match std::fs::read_to_string(path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("read {path}: {e}");
            return ExitCode::from(2);
        }
    };
    let transcript = trpg_eval::parse_transcript(&md);
    let verdict = trpg_eval::evaluate(&transcript);

    if args.iter().any(|a| a == "--json") {
        let out = serde_json::json!({
            "verdict": verdict,
            "score": trpg_eval::score_card(&verdict),
            "response_contracts": trpg_eval::contracts(&transcript),
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        print!("{}", trpg_eval::redboard(&transcript, &verdict));
    }
    if verdict.is_fail() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// `trpg-eval player "<scene>"` — deliberate every persona on one GM scene.
/// Demonstrates §二/§三: each persona's chosen action + structured trajectory.
fn run_player(args: &[String]) -> ExitCode {
    let scene = match args.iter().skip(2).find(|a| !a.starts_with("--")) {
        Some(s) => s.clone(),
        None => {
            eprintln!("usage: trpg-eval player \"<gm scene text>\" [--json] [--goal G] [--seed N]");
            return ExitCode::from(2);
        }
    };
    let goal = flag_value(args, "--goal").unwrap_or_else(|| "查清真相并活着离开".into());
    let seed: u64 = flag_value(args, "--seed").and_then(|v| v.parse().ok()).unwrap_or(0);

    let decisions: Vec<_> = PlayerPersona::PRESETS
        .iter()
        .map(|&kind| {
            let st = trpg_eval::SimulatedPlayerState::new(PlayerPersona::preset(kind), goal.clone());
            (kind, deliberate(&st, &scene, seed))
        })
        .collect();

    if args.iter().any(|a| a == "--json") {
        let rows: Vec<_> = decisions
            .iter()
            .map(|(k, d)| serde_json::json!({ "persona": k.id(), "decision": d }))
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows).unwrap());
    } else {
        println!("# Persona deliberation (蓝图 §二/§三)\nscene: {scene}\n");
        for (k, d) in &decisions {
            println!(
                "- {:<22} → {:<22} (conf {:.2}){}",
                k.id(),
                d.selected.id(),
                d.confidence,
                d.repeat_justification
                    .as_ref()
                    .map(|r| format!("  // {r}"))
                    .unwrap_or_default(),
            );
        }
    }
    ExitCode::SUCCESS
}

fn flag_value(args: &[String], key: &str) -> Option<String> {
    args.iter().position(|a| a == key).and_then(|i| args.get(i + 1)).cloned()
}

/// `trpg-eval counterfactual` — run the §六 fork battery over all personas and
/// PROVE the player-sim reads the GM (验收2). Exits non-zero if any persona fails.
fn run_counterfactual(args: &[String]) -> ExitCode {
    let goal = flag_value(args, "--goal").unwrap_or_else(|| "查清真相并活着离开".into());
    let mut all_pass = true;
    let mut rows = Vec::new();
    for &kind in PlayerPersona::PRESETS.iter() {
        let r = audit_player_reads_gm(kind, &goal);
        all_pass &= r.passed;
        rows.push((kind, r));
    }
    if args.iter().any(|a| a == "--json") {
        let j: Vec<_> = rows
            .iter()
            .map(|(k, r)| {
                serde_json::json!({
                    "persona": k.id(),
                    "passed": r.passed,
                    "sensitivity_diverged": r.sensitivity_diverged,
                    "invariance_stable": r.invariance_stable,
                    "memory_alerted": r.memory_alerted,
                    "no_spoiler_leak": r.no_spoiler_leak,
                    "no_response_escalated": r.no_response_escalated,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&j).unwrap());
    } else {
        println!("# Counterfactual: does the player read the GM? (蓝图 §六 / 验收2)\n");
        for (k, r) in &rows {
            println!(
                "- {:<22} {}  [敏感性{} 不变性{} 记忆{} 防剧透{} 无响应{}]",
                k.id(),
                if r.passed { "PASS" } else { "FAIL" },
                tick(r.sensitivity_diverged),
                tick(r.invariance_stable),
                tick(r.memory_alerted),
                tick(r.no_spoiler_leak),
                tick(r.no_response_escalated),
            );
        }
        println!("\nVERDICT: {}", if all_pass { "PASS" } else { "FAIL" });
    }
    if all_pass {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn tick(b: bool) -> char {
    if b {
        '✓'
    } else {
        '✗'
    }
}
