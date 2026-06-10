#!/bin/bash
# Cyberpunk adaptive play helper (DB 54346): run ONE turn, print GM + combat mechanics.
cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
set -a; source .env; set +a
SID="$1"; INPUT="$2"
export DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54346/chatrpg
export TRPG_DATA_DIR=/tmp/cyber_dd
export TRPG_GM_AGENTIC_RETRIEVE=false TRPG_ALLOW_SYNTHETIC_ACTOR_SEEDS=true
export TRPG_FAIL_ON_MISSING_SOURCE_BACKED_PARAMS=false TRPG_CHARACTER_ONBOARDING_REQUIRED=false
export TRPG_CONTEST_DEFAULT_ATTACK_DV=14 TRPG_CONTEST_DEFAULT_STATIC_TARGET=14
OUT="/tmp/play_last.jsonl"
target/debug/trpg turn --ruleset cyberpunk_red --session-id "$SID" --input "$INPUT" --stream-format jsonl 2>/dev/null > "$OUT"
python3 - "$OUT" <<'PY'
import json,sys
f=sys.argv[1]; deltas=[]; rows=[]; route=None
for line in open(f):
    line=line.strip()
    if not line: continue
    try: o=json.loads(line)
    except: continue
    if not isinstance(o,dict): continue
    if o.get('event')=='delta' and isinstance(o.get('data'),str): deltas.append(o['data']); continue
    p=o.get('phase'); d=o.get('data',{}) if isinstance(o.get('data'),dict) else {}
    if p=='turn_route': route=(d.get('route') or {}).get('route_kind')
    if p=='check_contract_created':
        t=d.get('target') or {}
        rows.append("check dice=%s target=%s val=%s intent=%s"%(d.get('dice_expression'), (t.get('kind') if isinstance(t,dict) else t), (t.get('value') if isinstance(t,dict) else None), d.get('intent_kind')))
    if p in ('pending_check_resolved','damage_packet_applied','apply_effect_roll','effect_resolution_applied'):
        rows.append("%s: %s"%(p, json.dumps({k:d.get(k) for k in ('success','total','target','hp_before','hp_after','damage','sp_applied','dice') if k in d}, ensure_ascii=False)))
print("=== GM ===")
print(''.join(deltas).strip()[:1700])
print("\n=== MECH === route:",route)
for r in rows: print(" ",r)
PY
echo "--- actor HP/state (pc + npc) ---"
docker exec -e PGPASSWORD=chatrpg chatrpg-postgres-v1162 psql -U chatrpg -d chatrpg -tA -c "select actor_id||' hp='||coalesce(hp_current::text,'?')||'/'||coalesce(hp_max::text,'?')||' '||coalesce(wound_state,'') from actor_mechanical_states where session_id='$SID';" 2>/dev/null | head
