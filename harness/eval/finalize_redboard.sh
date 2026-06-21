#!/usr/bin/env bash
# E8 — finalize the first real red-board diagnostic. Given a completed run dir
# (from run_eval.sh), re-run the aggregate gate with the player-sim signals and
# write the canonical EVAL_REDBOARD_v1.md plus a REVIEW_SAMPLE_READY sentinel
# that points at it.
#
# Usage: finalize_redboard.sh --run-dir <dir> [--out <md>] [--version v1]
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$HERE/lib_eval.sh"

RUN_DIR=""; OUT=""; VER="v1"
while [[ $# -gt 0 ]]; do case "$1" in
  --run-dir) RUN_DIR="$2"; shift 2;;
  --out) OUT="$2"; shift 2;;
  --version) VER="$2"; shift 2;;
  *) shift;;
esac; done
[[ -n "$RUN_DIR" && -d "$RUN_DIR" ]] || { echo "finalize: --run-dir <dir> required" >&2; exit 64; }
SID="$(cat "$RUN_DIR/session_id.txt" 2>/dev/null)"
[[ -n "$SID" ]] || { echo "finalize: no session_id in $RUN_DIR" >&2; exit 64; }
[[ -z "$OUT" ]] && OUT="$EVAL_ROOT/.tmp/exam/EVAL_REDBOARD_${VER}.md"
SIG="$RUN_DIR/signals.jsonl"

bash "$HERE/aggregate.sh" --session "$SID" --label "EVAL-HARNESS baseline-red $VER" \
  ${SIG:+--signals "$SIG"} --redboard "$OUT" --json "$RUN_DIR/verdict.json"
rc=$?

# append the player-sim signal narrative (STUCK / AMNESIA the player raised)
{
  echo
  echo "## Player-sim signals (human-like reactive player)"
  if [[ -s "$SIG" ]]; then
    jq -r '"- turn \(.turn) **\(.kind)**: \(.detail)"' "$SIG" 2>/dev/null
  else
    echo "- (none raised)"
  fi
  echo
  echo "## How to reproduce"
  echo '```'
  echo "bash harness/eval/run_eval.sh --ruleset cyberpunk_red --module cyberpunk_red.homecoming --turns 30 --judge"
  echo "bash harness/eval/aggregate.sh --session <sid> --signals <run_dir>/signals.jsonl --redboard <out.md>"
  echo '```'
  echo
  echo "_run dir: \`$RUN_DIR\`  transcript: \`$RUN_DIR/transcript.md\`_"
} >> "$OUT"

# sentinel for the reviewer
SENT="$EVAL_ROOT/.tmp/exam/REVIEW_SAMPLE_READY.sentinel"
{
  echo "redboard=$OUT"
  echo "session=$SID"
  echo "run_dir=$RUN_DIR"
  echo "verdict=$(jq -r .verdict "$RUN_DIR/verdict.json" 2>/dev/null)"
  echo "engine=$(cd "$EVAL_ROOT" && git rev-parse --short HEAD)"
} > "$SENT"

eval_log "red-board: $OUT"
eval_log "sentinel: $SENT"
echo "$OUT"
exit $rc
