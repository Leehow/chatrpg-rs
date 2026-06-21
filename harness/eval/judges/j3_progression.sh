#!/usr/bin/env bash
# J3 — PROGRESSION judge (blocking, DB-grounded).
# RED if the scene is frozen (a long run of turns with no SceneTransitioned),
# the scene-transition rate is near zero, OR the GM keeps re-describing a
# near-identical establishing tableau (the warehouse-tableau pattern).
#
# Usage: j3_progression.sh --session <sid> [--max-frozen 15] [--min-rate 0.05]
set -uo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/../lib_eval.sh"

SID=""; MAX_FROZEN="${EVAL_J3_MAX_FROZEN:-15}"; MIN_RATE="${EVAL_J3_MIN_RATE:-0.05}"
MAX_REPEAT="${EVAL_J3_MAX_REPEAT:-4}"
while [[ $# -gt 0 ]]; do case "$1" in
  --session) SID="$2"; shift 2;;
  --max-frozen) MAX_FROZEN="$2"; shift 2;;
  --min-rate) MIN_RATE="$2"; shift 2;;
  *) shift;;
esac; done
[[ -n "$SID" ]] || { echo "j3: --session required" >&2; exit 64; }

# Per-turn rows: turn_id | had_transition(t/f) | normalized 120-char opening.
ROWS="$(dbq "
select t.turn_id || '|' ||
  (exists(select 1 from domain_events d where d.session_id=t.session_id
          and d.turn_id=t.turn_id and d.kind='SceneTransitioned'))::text || '|' ||
  left(regexp_replace(coalesce(t.assistant_output,''), '[[:space:]]+', '', 'g'), 120)
from turns t where t.session_id='$SID' order by t.created_at")"

EVAL_ROWS="$ROWS" MAX_FROZEN="$MAX_FROZEN" MIN_RATE="$MIN_RATE" MAX_REPEAT="$MAX_REPEAT" \
python3 - <<'PY'
import os,json,collections
rows=[r for r in os.environ.get('EVAL_ROWS','').splitlines() if r.strip()]
n=len(rows); trans=0; frozen=cur=0; openings=collections.Counter()
for r in rows:
    parts=r.split('|',2)
    had = parts[1].startswith('t') if len(parts)>1 else False
    op  = parts[2] if len(parts)>2 else ''
    if had: trans+=1; cur=0
    else: cur+=1; frozen=max(frozen,cur)
    if len(op)>=40: openings[op[:80]]+=1
max_rep = max(openings.values()) if openings else 0
rate = round(trans/n,3) if n else 0.0
mf=int(os.environ['MAX_FROZEN']); mr=float(os.environ['MIN_RATE']); mrep=int(os.environ['MAX_REPEAT'])
ev=[]; red=False
if n>=8 and frozen>=mf: red=True; ev.append(f"scene frozen for {frozen} consecutive turns with no transition (>= {mf})")
if n>=8 and rate<mr: red=True; ev.append(f"scene-transition rate {rate} < {mr} ({trans} transitions / {n} turns)")
if max_rep>=mrep: red=True; ev.append(f"near-identical establishing prose repeated {max_rep}x (tableau loop)")
if not red: ev.append(f"{trans} transitions / {n} turns, max frozen run {frozen}, max prose repeat {max_rep}")
print(json.dumps({"judge":"J3","name":"progression","status":"RED" if red else "GREEN","blocking":True,
  "threshold":{"max_frozen":mf,"min_rate":mr,"max_prose_repeat":mrep},
  "metrics":{"turns":n,"scene_transitions":trans,"transition_rate":rate,"max_frozen_run":frozen,"max_prose_repeat":max_rep},
  "evidence":ev},ensure_ascii=False))
PY
