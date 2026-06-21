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
    if args.get(1).map(|s| s.as_str()) == Some("flight") {
        return run_flight(&args);
    }
    if args.get(1).map(|s| s.as_str()) == Some("ab") {
        return run_ab(&args);
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

/// `trpg-eval flight <recording.json>` — layered attribution (验收3): read the
/// Flight Recorder contribution trail and blame the producing layer. Exits
/// non-zero if any defect is attributed.
fn run_flight(args: &[String]) -> ExitCode {
    let path = match args.iter().skip(2).find(|a| !a.starts_with("--")) {
        Some(p) => p.clone(),
        None => {
            eprintln!("usage: trpg-eval flight <flight-recorder.json> [--json]");
            return ExitCode::from(2);
        }
    };
    let json = match std::fs::read_to_string(&path) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("read {path}: {e}");
            return ExitCode::from(2);
        }
    };
    let rec = match trpg_eval::FlightRecording::parse(&json) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("parse {path}: {e}");
            return ExitCode::from(2);
        }
    };
    let verdict = trpg_eval::attribute(&rec);
    if args.iter().any(|a| a == "--json") {
        println!("{}", serde_json::to_string_pretty(&verdict).unwrap());
    } else {
        print!("{}", trpg_eval::flight_redboard(&rec, &verdict));
    }
    if verdict.is_fail() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// `trpg-eval ab <baseline.md> <candidate.md>` — blind paired comparison (验收4).
/// Holds the input fixed and reports which arm is stably better; reproducible.
fn run_ab(args: &[String]) -> ExitCode {
    let files: Vec<&String> = args.iter().skip(2).filter(|a| !a.starts_with("--")).collect();
    let (base_p, cand_p) = match (files.first(), files.get(1)) {
        (Some(a), Some(b)) => (a.as_str(), b.as_str()),
        _ => {
            eprintln!("usage: trpg-eval ab <baseline.md> <candidate.md> [--json]");
            return ExitCode::from(2);
        }
    };
    let read = |p: &str| std::fs::read_to_string(p).map_err(|e| format!("read {p}: {e}"));
    let (base_md, cand_md) = match (read(base_p), read(cand_p)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let base = trpg_eval::parse_transcript(&base_md);
    let cand = trpg_eval::parse_transcript(&cand_md);
    // A = baseline, B = candidate. The comparator stays blind to these labels.
    let c = trpg_eval::compare(&base, &cand);
    let reproducible = trpg_eval::is_reproducible(&base, &cand, 20);
    if args.iter().any(|a| a == "--json") {
        let out = serde_json::json!({ "comparison": c, "reproducible": reproducible });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        print!("{}", trpg_eval::ab_board(base_p, cand_p, &c));
        println!("reproducible(20x)={reproducible}");
    }
    // Exit 0 when the candidate (B) is stably at least as good as the baseline.
    if matches!(c.overall, trpg_eval::Arm::B | trpg_eval::Arm::Tie) {
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
