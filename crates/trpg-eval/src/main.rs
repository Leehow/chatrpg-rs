//! CLI entry: `trpg-eval <report.md> [--json]`.
//! Prints the red-board and exits non-zero on FAIL — usable as a factory gate.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
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
