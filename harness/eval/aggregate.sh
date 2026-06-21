#!/usr/bin/env bash
# E6 — Aggregate verdict + FACTORY GATE.
# Runs J1-J4 (DB-grounded blocking judges) plus the player-sim STUCK/AMNESIA
# signals, combines them: ANY blocking judge RED => run FAIL. Emits a red-board
# markdown with DB evidence and a machine verdict JSON. Exit code: 0 = PASS,
# 1 = FAIL (a build that ships this game is NOT done).
#
# Usage:
#   aggregate.sh --session <sid> [--signals <signals.jsonl>] \
#                [--redboard <out.md>] [--json <out.json>] [--label <text>]
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$HERE/lib_eval.sh"

SID=""; SIGNALS=""; REDBOARD=""; JSON_OUT=""; LABEL=""
while [[ $# -gt 0 ]]; do case "$1" in
  --session) SID="$2"; shift 2;;
  --signals) SIGNALS="$2"; shift 2;;
  --redboard) REDBOARD="$2"; shift 2;;
  --json) JSON_OUT="$2"; shift 2;;
  --label) LABEL="$2"; shift 2;;
  *) shift;;
esac; done
[[ -n "$SID" ]] || { echo "aggregate: --session required" >&2; exit 64; }

run_judge() {
  local script="$1"; shift
  local out; out="$(bash "$HERE/judges/$script" --session "$SID" "$@" 2>/dev/null)"
  [[ -n "$out" ]] && jq -e . >/dev/null 2>&1 <<<"$out" && { echo "$out"; return; }
  jq -nc --arg j "${script%%_*}" '{judge:($j|ascii_upcase),name:"errored",status:"ERROR",blocking:true,metrics:{},evidence:["judge failed to produce JSON"]}'
}

J1="$(run_judge j1_memory.sh ${SIGNALS:+--signals "$SIGNALS"})"
J2="$(run_judge j2_consequence.sh)"
J3="$(run_judge j3_progression.sh)"
J4="$(run_judge j4_check_coherence.sh)"

# player-sim run-level signals (STUCK / AMNESIA) — context, not a 5th judge
stuck=0; amnesia=0
if [[ -n "$SIGNALS" && -f "$SIGNALS" ]]; then
  stuck=$(grep -c '"kind":"STUCK"' "$SIGNALS" 2>/dev/null || echo 0)
  amnesia=$(grep -c '"kind":"AMNESIA"' "$SIGNALS" 2>/dev/null || echo 0)
fi

ALL="$(jq -nc --argjson a "$J1" --argjson b "$J2" --argjson c "$J3" --argjson d "$J4" '[$a,$b,$c,$d]')"
reds=$(jq '[.[]|select(.status=="RED")]|length' <<<"$ALL")
errs=$(jq '[.[]|select(.status=="ERROR")]|length' <<<"$ALL")
verdict=$(( reds > 0 || errs > 0 ? 1 : 0 ))
VSTR=$([[ $verdict -eq 0 ]] && echo PASS || echo FAIL)

VERDICT_JSON="$(jq -nc --arg sid "$SID" --arg label "$LABEL" --arg v "$VSTR" \
  --argjson judges "$ALL" --argjson reds "$reds" \
  --argjson stuck "${stuck:-0}" --argjson amn "${amnesia:-0}" \
  '{session:$sid,label:$label,verdict:$v,red_count:$reds,
    player_sim_signals:{stuck:$stuck,amnesia:$amn},judges:$judges}')"
[[ -n "$JSON_OUT" ]] && echo "$VERDICT_JSON" > "$JSON_OUT"

# ── red-board markdown ───────────────────────────────────────────────────────
emit_board() {
  echo "# EVAL RED-BOARD — ${LABEL:-$SID}"
  echo
  echo "- session: \`$SID\`"
  echo "- engine: \`$(cd "$EVAL_ROOT" && git rev-parse --short HEAD 2>/dev/null)\`  db: \`$DB_CONTAINER\`  model: \`$TRPG_LLM_MODEL\`"
  echo "- **VERDICT: $VSTR** ($reds blocking judge(s) RED)"
  echo "- player-sim signals: STUCK=$stuck  AMNESIA=$amnesia"
  echo
  echo "| judge | name | status | key metrics |"
  echo "|-------|------|--------|-------------|"
  jq -r '.[] | "| \(.judge) | \(.name) | \(.status) | \(.metrics|to_entries|map("\(.key)=\(.value)")|join(", ")) |"' <<<"$ALL"
  echo
  echo "## Evidence (DB-grounded)"
  jq -r '.[] | "\n### \(.judge) \(.name) — \(.status)\n" + (.evidence|map("- " + .)|join("\n"))' <<<"$ALL"
}
if [[ -n "$REDBOARD" ]]; then
  mkdir -p "$(dirname "$REDBOARD")"; emit_board > "$REDBOARD"
  eval_log "red-board written: $REDBOARD"
fi

echo "$VERDICT_JSON"
exit $verdict
