#!/bin/bash
# Adaptive play helper: run ONE turn, print GM narration + compact mechanics.
# Usage: _play.sh <ruleset> <session_id> <data_dir> <db_url> "<player input>"
cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
set -a; source .env; set +a
RS="$1"; SID="$2"; DD="$3"; DBURL="$4"; INPUT="$5"
export DATABASE_URL="$DBURL"
export TRPG_DATA_DIR="$DD"
export TRPG_GM_AGENTIC_RETRIEVE=true TRPG_ALLOW_SYNTHETIC_ACTOR_SEEDS=true
export TRPG_FAIL_ON_MISSING_SOURCE_BACKED_PARAMS=false TRPG_CHARACTER_ONBOARDING_REQUIRED=false
# Table-default skill for roll-under games (CoC) — per-check skill binding is a known gap; harmless for other families.
export TRPG_CONTEST_DEFAULT_PERCENTILE_SKILL=50
OUT="/tmp/play_last.jsonl"
target/debug/trpg turn --ruleset "$RS" --session-id "$SID" --input "$INPUT" --stream-format jsonl 2>/dev/null > "$OUT"
python3 - "$OUT" <<'PY'
import json,sys
f=sys.argv[1]; deltas=[]; checks=[]; res=[]; route=None
for line in open(f):
    line=line.strip()
    if not line: continue
    try: o=json.loads(line)
    except: continue
    if not isinstance(o,dict): continue
    if o.get('event')=='delta' and isinstance(o.get('data'),str): deltas.append(o['data']); continue
    p=o.get('phase'); d=o.get('data',{}) if isinstance(o.get('data'),dict) else {}
    if p=='turn_route': r=d.get('route',{}); route=r.get('route_kind')
    if p=='check_contract_created':
        t=d.get('target') or {}
        checks.append((d.get('dice_expression'), t.get('kind') if isinstance(t,dict) else t, t.get('value') if isinstance(t,dict) else None, d.get('intent_kind')))
    if p=='pending_check_resolved':
        res.append((d.get('success'), d.get('success_count'), d.get('pool_miss_count'), d.get('total'), d.get('target')))
print("=== GM ===")
print(''.join(deltas).strip()[:1800])
print("\n=== MECH ===")
print("route:", route)
for c in checks: print("  check dice=%s target=%s val=%s intent=%s" % c)
for r in res: print("  resolved success=%s 3s=%s miss=%s total=%s target=%s" % r)
PY
echo "--- chaos track ---"
docker exec -e PGPASSWORD=chatrpg chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg -tA -c "select parameter_path||'='||value_json from generic_parameter_states where session_id='$SID' and parameter_path like 'tracks.%';" 2>/dev/null
