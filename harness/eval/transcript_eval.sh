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
  echo "[transcript-smoke] cargo test -p trpg-eval (negative-golden + discrimination)"
  cargo test -p trpg-eval --quiet
  echo "[transcript-smoke] PASS — bad reports FAIL with all root causes, clean control PASSES"
  exit 0
fi

if [[ $# -lt 1 ]]; then
  echo "usage: transcript_eval.sh <report.md> [--json] | --smoke" >&2
  exit 2
fi

cargo run -q -p trpg-eval --bin trpg-eval -- "$@"
