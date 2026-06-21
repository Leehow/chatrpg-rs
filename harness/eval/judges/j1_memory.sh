#!/usr/bin/env bash
# J1 — MEMORY-CONTINUITY judge (blocking, DB-grounded).
# RED if the session wrote NO durable memory (memory_facts + world_facts +
# knowledge_edges all empty) over a non-trivial run, OR the player-sim recorded
# an AMNESIA signal (GM forgot an established fact/location).
#
# Usage: j1_memory.sh --session <sid> [--signals <signals.jsonl>] [--min-turns N]
set -uo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/../lib_eval.sh"

SID=""; SIGNALS=""; MIN_TURNS="${EVAL_J1_MIN_TURNS:-8}"
while [[ $# -gt 0 ]]; do case "$1" in
  --session) SID="$2"; shift 2;;
  --signals) SIGNALS="$2"; shift 2;;
  --min-turns) MIN_TURNS="$2"; shift 2;;
  *) shift;;
esac; done
[[ -n "$SID" ]] || { echo "j1: --session required" >&2; exit 64; }

q() { dbq "$1" | tr -d '[:space:]'; }
nturns=$(q "select count(*) from turns where session_id='$SID'")
mf=$(q "select count(*) from memory_facts where session_id='$SID'")
wf=$(q "select count(*) from world_facts where session_id='$SID'")
ke=$(q "select count(*) from knowledge_edges where session_id='$SID'")
# story_state: present AND non-trivial (more than an empty/2-char json)
ss=$(q "select coalesce(length(state_json::text),0) from story_state where session_id='$SID'")
ss="${ss:-0}"
durable=$(( mf + wf + ke ))

amnesia=0
if [[ -n "$SIGNALS" && -f "$SIGNALS" ]]; then
  amnesia=$(grep -c '"kind":"AMNESIA"' "$SIGNALS" 2>/dev/null || echo 0)
fi

status="GREEN"; ev=()
if [[ "$nturns" -ge "$MIN_TURNS" && "$durable" -eq 0 ]]; then
  status="RED"
  ev+=("no durable memory over $nturns turns: memory_facts=$mf world_facts=$wf knowledge_edges=$ke")
fi
if [[ "$ss" -le 2 && "$nturns" -ge "$MIN_TURNS" && "$durable" -eq 0 ]]; then
  ev+=("story_state empty (len=$ss) — nothing persisted for recall")
fi
if [[ "${amnesia:-0}" -ge 1 ]]; then
  status="RED"
  ev+=("player-sim raised $amnesia AMNESIA signal(s): GM forgot an established fact/location")
fi
[[ "$status" == "GREEN" ]] && ev+=("durable memory accumulating: facts=$mf world=$wf edges=$ke over $nturns turns")

ev_json=$(printf '%s\n' "${ev[@]}" | jq -R . | jq -s .)
jq -nc --arg s "$status" --argjson m "{\"turns\":$nturns,\"memory_facts\":$mf,\"world_facts\":$wf,\"knowledge_edges\":$ke,\"story_state_len\":$ss,\"amnesia_signals\":${amnesia:-0}}" --argjson e "$ev_json" \
  '{judge:"J1",name:"memory-continuity",status:$s,blocking:true,metrics:$m,evidence:$e}'
