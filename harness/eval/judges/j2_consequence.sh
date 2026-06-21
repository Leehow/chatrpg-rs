#!/usr/bin/env bash
# J2 — CONSEQUENCE/AGENCY judge (blocking, DB-grounded).
# A turn is CONSEQUENTIAL if it changed the world: a parameter_impact, a
# committed check patch, a resource delta (value != cap), a tick-advanced
# parameter state, a non-empty turn state_patch, or a newly written world_fact.
# RED if the consequential ratio is below threshold (most turns ended with no
# world-state change / "nothing happened" / "待结算").
#
# Usage: j2_consequence.sh --session <sid> [--min-ratio 0.34]
set -uo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/../lib_eval.sh"

SID=""; MIN_RATIO="${EVAL_J2_MIN_RATIO:-0.34}"
while [[ $# -gt 0 ]]; do case "$1" in
  --session) SID="$2"; shift 2;;
  --min-ratio) MIN_RATIO="$2"; shift 2;;
  *) shift;;
esac; done
[[ -n "$SID" ]] || { echo "j2: --session required" >&2; exit 64; }

q() { dbq "$1" | tr -d '[:space:]'; }
nturns=$(q "select count(*) from turns where session_id='$SID'")
nturns="${nturns:-0}"

# distinct turn_ids that carry a real world-state change (union of evidence sources)
cons=$(q "
with cs as (
  select turn_id from parameter_impacts where session_id='$SID' and turn_id is not null
  union
  select cc.turn_id from check_results cr join check_contracts cc on cr.check_id=cc.check_id
    where cc.session_id='$SID' and cr.committed_patches is not null
      and cr.committed_patches::text not in ('null','[]','{}')
  union
  select turn_id from domain_events where session_id='$SID' and kind='ResourceChanged'
    and (data ? 'cap') and (data->>'value') is not null
    and (data->>'value')::int <> (data->>'cap')::int
  union
  select turn_id from turns where session_id='$SID'
    and state_patches::text not in ('null','[]','{}','')
  union
  select turn_id from world_facts where session_id='$SID'
)
select count(distinct turn_id) from cs where turn_id is not null")
cons="${cons:-0}"

# context: how many turns the narration itself flags as unresolved
pending=$(q "select count(*) from turns where session_id='$SID' and assistant_output ~ '待结算'")

ratio=$(python3 -c "n=$nturns; print(round($cons/n,3) if n else 0.0)")
status=$(python3 -c "print('RED' if ($nturns>=8 and $ratio < $MIN_RATIO) else 'GREEN')")

ev=()
if [[ "$status" == "RED" ]]; then
  ev+=("only $cons/$nturns turns changed world-state (ratio=$ratio < $MIN_RATIO) — most turns had no consequence")
  [[ "${pending:-0}" -ge 1 ]] && ev+=("$pending turns narrate an unresolved '待结算' with no committed effect")
else
  ev+=("$cons/$nturns turns were consequential (ratio=$ratio >= $MIN_RATIO)")
fi

ev_json=$(printf '%s\n' "${ev[@]}" | jq -R . | jq -s .)
jq -nc --arg s "$status" \
  --argjson m "{\"turns\":$nturns,\"consequential_turns\":$cons,\"consequential_ratio\":$ratio,\"pending_unresolved_turns\":${pending:-0}}" \
  --argjson e "$ev_json" --arg thr "$MIN_RATIO" \
  '{judge:"J2",name:"consequence-agency",status:$s,blocking:true,threshold:{min_ratio:($thr|tonumber)},metrics:$m,evidence:$e}'
