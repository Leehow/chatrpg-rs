#!/usr/bin/env bash
# E0 — Run driver. Create a character against a REAL DB, then drive N turns with
# the human-like reactive player-sim (E1), capturing transcript + domain_events
# + STUCK/AMNESIA signals. Reuses the proven create-character / `trpg turn`
# pattern. The engine is exercised only through its public CLI (read-only).
#
# Usage:
#   run_eval.sh --ruleset cyberpunk_red --module cyberpunk_red.homecoming \
#               --turns 30 [--objective "..."] [--label oc1] [--out-dir DIR] \
#               [--reuse-session SID] [--judge]
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$HERE/lib_eval.sh"
source "$HERE/player_sim.sh"

RULESET=""; MODULE=""; TURNS=30; OBJECTIVE=""; LABEL="eval"; OUT_DIR=""
REUSE=""; DO_JUDGE=0
while [[ $# -gt 0 ]]; do case "$1" in
  --ruleset) RULESET="$2"; shift 2;;
  --module) MODULE="$2"; shift 2;;
  --turns) TURNS="$2"; shift 2;;
  --objective) OBJECTIVE="$2"; shift 2;;
  --label) LABEL="$2"; shift 2;;
  --out-dir) OUT_DIR="$2"; shift 2;;
  --reuse-session) REUSE="$2"; shift 2;;
  --judge) DO_JUDGE=1; shift;;
  *) echo "unknown arg: $1" >&2; exit 64;;
esac; done
[[ -n "$RULESET" ]] || { echo "run_eval: --ruleset required" >&2; exit 64; }
[[ -n "$OBJECTIVE" ]] || OBJECTIVE="你重返多年未归的故乡街区，要查清并铲除正在威胁这个街区和你家人的势力，保护好家人，最终守住这片地方。"

# ── preflight ────────────────────────────────────────────────────────────────
[[ -x "$BIN" ]] || { echo "run_eval: binary not found: $BIN" >&2; exit 70; }
curl -sf -m 8 "$RELAY_BASE/models" >/dev/null 2>&1 || { echo "run_eval: relay down at $RELAY_BASE" >&2; exit 70; }
dbq "select 1" >/dev/null 2>&1 || { echo "run_eval: DB unreachable ($DB_CONTAINER)" >&2; exit 70; }

TS="$(date +%Y%m%d_%H%M%S)"
[[ -z "$OUT_DIR" ]] && OUT_DIR="$EVAL_ROOT/.tmp/exam/runs/${LABEL}_${TS}"
mkdir -p "$OUT_DIR"; export EVAL_OUT_DIR="$OUT_DIR"
: > "$OUT_DIR/transcript.md"; : > "$OUT_DIR/tried.txt"; : > "$OUT_DIR/facts.txt"
: > "$OUT_DIR/signals.jsonl"; : > "$OUT_DIR/location.txt"
printf '%s' "$OBJECTIVE" > "$OUT_DIR/objective.txt"
eval_log "run dir: $OUT_DIR  ruleset=$RULESET module=${MODULE:-none} turns=$TURNS"

# ── character ────────────────────────────────────────────────────────────────
if [[ -n "$REUSE" ]]; then
  SID="$REUSE"
else
  SID="session_${LABEL}$(date +%s)"
  eval_create_pc "$SID" "$RULESET" "$MODULE" || eval_log "WARN: create-character non-zero rc"
  # binding guard: pc.current must carry a real sheet (stats/skills/resources)
  ok="$(dbq "select count(*) from runtime_actor_parameters where session_id='$SID' and actor_id='pc.current' and (sheet_json ?| array['stats','skills','resources'])" | tr -d '[:space:]')"
  if [[ "${ok:-0}" != "1" ]]; then
    eval_log "WARN: pc.current has no real sheet in $SID — mechanics may be vacuous (recorded)"
    echo '{"turn":0,"kind":"SETUP","detail":"pc.current sheet missing stats/skills/resources"}' >> "$OUT_DIR/signals.jsonl"
  fi
fi
echo "$SID" > "$OUT_DIR/session_id.txt"
eval_log "session=$SID"

# ── turn loop ────────────────────────────────────────────────────────────────
stall=0; prev_vis=""
for ((i=1; i<=TURNS; i++)); do
  tag="$(printf 't%02d' "$i")"
  last_gm="$OUT_DIR/last_gm.txt"; [[ -f "$OUT_DIR/${tag_prev:-none}.vis" ]] || true
  # player reads the previous GM output (empty on turn 1) and chooses
  action="$(ps_choose "$OUT_DIR" "$i" "$last_gm" "$stall")"
  eval_log "=== turn $i (stall=$stall) player: ${action:0:80}"
  printf '\n## turn %s\n**玩家:** %s\n' "$i" "$action" >> "$OUT_DIR/transcript.md"

  vis="$(eval_run_turn "$SID" "$RULESET" "$MODULE" "$action" "$OUT_DIR/${tag}.jsonl")"
  printf '%s' "$vis" > "$last_gm"
  printf '**GM:** %s\n' "$(printf '%s' "$vis" | cut -c1-1600)" >> "$OUT_DIR/transcript.md"

  # advance / stall detection from player-visible output (non-DB; player POV)
  adv="$(VIS="$vis" PREV="$prev_vis" python3 - <<'PY'
import os
v=os.environ.get('VIS','').strip(); p=os.environ.get('PREV','').strip()
def grams(s): return set(s[i:i+3] for i in range(max(0,len(s)-2)))
non=False
if '待结算' in v: non=True
if len(v) < 80: non=True
if p:
    a,b=grams(v),grams(p)
    if a and b:
        j=len(a&b)/len(a|b)
        if j>0.85: non=True
print('NONADV' if non else 'ADV')
PY
)"
  if [[ "$adv" == "NONADV" ]]; then
    stall=$((stall+1))
    if [[ "$stall" -eq 3 ]]; then
      jq -nc --argjson t "$i" '{turn:$t,kind:"STUCK",detail:"3+ consecutive turns with no consequence/advance"}' >> "$OUT_DIR/signals.jsonl"
      eval_log "  ⚠ STUCK@turn$i (player will escalate)"
    fi
  else
    stall=0
  fi
  prev_vis="$vis"

  ps_detect "$OUT_DIR" "$i" "$last_gm" || true
done

eval_log "=== run done: $SID ($TURNS turns) — signals: $(wc -l < "$OUT_DIR/signals.jsonl" | tr -d ' ')"

if [[ "$DO_JUDGE" -eq 1 ]]; then
  bash "$HERE/aggregate.sh" --session "$SID" --signals "$OUT_DIR/signals.jsonl" \
    --label "$LABEL" --redboard "$OUT_DIR/redboard.md" --json "$OUT_DIR/verdict.json" || true
  eval_log "verdict: $(jq -r .verdict "$OUT_DIR/verdict.json" 2>/dev/null)"
fi
echo "$SID"
