#!/usr/bin/env bash
# J4 — CHECK -> NARRATION COHERENCE judge (blocking, DB-grounded).
# The engine resolves checks (CheckResolved / DiceRolled events) but a dead game
# never surfaces the outcome to the player: the narration shows a "待结算"
# (pending) stub and no [roll]/stated result. RED if resolved checks far exceed
# the outcomes that actually appear in narration, or orphan "待结算" stubs remain.
#
# Usage: j4_check_coherence.sh --session <sid> [--min-coverage 0.5]
set -uo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/../lib_eval.sh"

SID=""; MIN_COV="${EVAL_J4_MIN_COVERAGE:-0.5}"; MAX_ORPHAN="${EVAL_J4_MAX_ORPHAN:-1}"
while [[ $# -gt 0 ]]; do case "$1" in
  --session) SID="$2"; shift 2;;
  --min-coverage) MIN_COV="$2"; shift 2;;
  *) shift;;
esac; done
[[ -n "$SID" ]] || { echo "j4: --session required" >&2; exit 64; }

q() { dbq "$1" | tr -d '[:space:]'; }
checks=$(q "select count(*) from domain_events where session_id='$SID' and kind='CheckResolved'")
dice=$(q "select count(*) from domain_events where session_id='$SID' and kind='DiceRolled'")
# turns that actually ran a check (apples-to-apples with narration coverage)
check_turns=$(q "select count(distinct turn_id) from domain_events where session_id='$SID' and kind='CheckResolved' and turn_id is not null")
# narration coverage: turns whose visible output shows a resolved roll/outcome
surfaced=$(q "select count(*) from turns where session_id='$SID' and (assistant_output ~ '\[roll' or assistant_output ~ '\[/roll\]')")
# orphan pending stubs: narration says 待结算 with no surfaced resolution
orphans=$(q "select count(*) from turns where session_id='$SID' and assistant_output ~ '待结算'")
# committed effects from the resolution layer (how many checks actually wrote patches)
committed=$(q "select count(*) from check_results cr join check_contracts cc on cr.check_id=cc.check_id where cc.session_id='$SID' and cr.committed_patches is not null and cr.committed_patches::text not in ('null','[]','{}')")

checks="${checks:-0}"; check_turns="${check_turns:-0}"; surfaced="${surfaced:-0}"
orphans="${orphans:-0}"; committed="${committed:-0}"; dice="${dice:-0}"

cov=$(python3 -c "ct=$check_turns; print(round($surfaced/ct,3) if ct else 1.0)")
status=$(python3 -c "print('RED' if ($check_turns>=3 and ($cov < $MIN_COV or $orphans > $MAX_ORPHAN)) else 'GREEN')")

ev=()
if [[ "$status" == "RED" ]]; then
  ev+=("$checks CheckResolved events across $check_turns turns, but only $surfaced turns surfaced a [roll]/outcome (coverage=$cov < $MIN_COV)")
  [[ "$orphans" -gt "$MAX_ORPHAN" ]] && ev+=("$orphans turns left an orphan '待结算' stub with no resolution surfaced")
  ev+=("only $committed/$checks resolved checks committed any state patch")
else
  ev+=("$surfaced/$check_turns check-turns surfaced an outcome (coverage=$cov), orphans=$orphans")
fi

ev_json=$(printf '%s\n' "${ev[@]}" | jq -R . | jq -s .)
jq -nc --arg s "$status" \
  --argjson m "{\"checks_resolved\":$checks,\"dice_rolled\":$dice,\"check_turns\":$check_turns,\"surfaced_turns\":$surfaced,\"narration_coverage\":$cov,\"orphan_pending_turns\":$orphans,\"committed_checks\":$committed}" \
  --argjson e "$ev_json" --arg mc "$MIN_COV" \
  '{judge:"J4",name:"check-narration-coherence",status:$s,blocking:true,threshold:{min_coverage:($mc|tonumber),max_orphan:'"$MAX_ORPHAN"'},metrics:$m,evidence:$e}'
