#!/usr/bin/env bash
# Static-transcript evaluator arm (蓝图 §十 第一阶段 / 里程碑1).
#
# Complements the DB-grounded live judges (j1..j4): those need a live session,
# this FAILs a *recorded* 战报 markdown with root-cause + turn evidence.
#
#   transcript_eval.sh <report.md> [--json]   # judge one report; exit 1 = FAIL
#   transcript_eval.sh --smoke                # FAIL-FAST tripwire (<2s, deterministic)
#
# --smoke is the cheap tripwire the factory runs BEFORE any expensive live run:
# it asserts the two negative golden fixtures FAIL with every documented root
# cause AND the clean control PASSES (proving metric-driven, not hardcoded RED).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

if [[ "${1:-}" == "--smoke" ]]; then
  echo "[transcript-smoke] cargo test -p trpg-eval (golden + flight + ab + counterfactual)"
  cargo test -p trpg-eval --quiet
  # Real-bin tripwires across all arms: M1 static (exit 1/1/0), P5 flight layered
  # attribution (good PASS / bad FAIL), P6 A/B (candidate clean beats bad baseline).
  BIN="cargo run -q -p trpg-eval --bin trpg-eval --"
  F=crates/trpg-eval/fixtures
  $BIN "$F/bad/cyber_repetition.md"   >/dev/null 2>&1 && { echo "FAIL: cyber should FAIL"; exit 1; } || true
  $BIN "$F/good/clean_min.md"         >/dev/null 2>&1 || { echo "FAIL: clean should PASS"; exit 1; }
  $BIN flight "$F/flight/good_provenance.json" >/dev/null 2>&1 || { echo "FAIL: flight good should PASS"; exit 1; }
  $BIN flight "$F/flight/bad_failbranch.json"  >/dev/null 2>&1 && { echo "FAIL: flight bad should FAIL"; exit 1; } || true
  $BIN ab "$F/bad/cyber_repetition.md" "$F/good/clean_min.md" >/dev/null 2>&1 || { echo "FAIL: A/B clean candidate should win"; exit 1; }
  echo "[transcript-smoke] PASS — static M1 + flight layered-attribution + A/B all green on real bin"
  exit 0
fi

if [[ $# -lt 1 ]]; then
  echo "usage: transcript_eval.sh <report.md> [--json] | --smoke" >&2
  exit 2
fi

cargo run -q -p trpg-eval --bin trpg-eval -- "$@"
