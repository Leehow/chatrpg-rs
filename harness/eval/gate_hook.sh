#!/usr/bin/env bash
# Thin FACTORY GATE hook (E6). Drop-in completion gate for the autonomous
# factory: a build is NOT done while any blocking judge is RED on the latest
# eval run. This is the ONLY engine-adjacent integration point — it reads the DB
# and the aggregate verdict; it never mutates engine state.
#
# Resolves the session to gate on (in priority order):
#   $1 / --session <sid>  |  $EVAL_GATE_SESSION  |  most recent session in DB
#
# Exit 0 = PASS (gate open), 1 = FAIL (gate closed: dead/amnesiac game).
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$HERE/lib_eval.sh"

SID="${1:-${EVAL_GATE_SESSION:-}}"
[[ "$SID" == "--session" ]] && SID="${2:-}"
if [[ -z "$SID" ]]; then
  SID="$(dbq "select session_id from turns group by session_id order by max(created_at) desc limit 1")"
fi
[[ -n "$SID" ]] || { echo "gate: no session to evaluate" >&2; exit 64; }

BOARD="${EVAL_REDBOARD_PATH:-$EVAL_ROOT/.tmp/exam/EVAL_REDBOARD_latest.md}"
bash "$HERE/aggregate.sh" --session "$SID" --label "factory-gate" \
  ${EVAL_GATE_SIGNALS:+--signals "$EVAL_GATE_SIGNALS"} \
  --redboard "$BOARD" >/dev/null
rc=$?
if [[ $rc -eq 0 ]]; then
  echo "FACTORY GATE: PASS — $SID (red-board: $BOARD)"
else
  echo "FACTORY GATE: FAIL — $SID has blocking judge reds (red-board: $BOARD)"
fi
exit $rc
