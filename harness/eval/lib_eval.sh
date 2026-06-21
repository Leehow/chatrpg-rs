#!/usr/bin/env bash
# harness/eval/lib_eval.sh
# Shared library for the EVAL-HARNESS: the dedicated test system that FAILS
# dead/amnesiac games. Reuses the proven create-character + `trpg turn` driver
# pattern (ex .tmp/exam/lib_s14.sh) but is parameterized and DB-grounded.
#
# Provides:
#   dbq <sql>                      -> run SQL in the live Postgres container (-At)
#   extract_narration <jsonl>      -> concatenated visible GM deltas (stdout)
#   eval_create_pc <sid> <rs> <mod>
#   eval_run_turn <sid> <rs> <mod> <input> <out.jsonl>  -> echoes narration
#
# Engine is READ-ONLY here: we only drive the public CLI and query the DB.
set -uo pipefail

EVAL_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# ── binary / DB resolution ───────────────────────────────────────────────────
BIN="${EVAL_BIN:-/Users/haoli/.cache/cargo-target/chatrpg-rs-v1.20-formula/debug/trpg}"
[[ -x "$BIN" ]] || BIN="$EVAL_ROOT/target/debug/trpg"

# DB host port -> docker container (judges query via docker exec; binary via TCP)
DB_HOST_PORT="${EVAL_DB_PORT:-54346}"
case "$DB_HOST_PORT" in
  54341) DB_CONTAINER="chatrpg-postgres-v1154" ;;
  54344) DB_CONTAINER="chatrpg-postgres-v1160" ;;
  54345) DB_CONTAINER="chatrpg-postgres-v1161" ;;
  54346) DB_CONTAINER="chatrpg-postgres-v1162" ;;
  54347) DB_CONTAINER="chatrpg-postgres-rulesets" ;;
  *)     DB_CONTAINER="chatrpg-postgres-v1162" ;;
esac
DB_CONTAINER="${EVAL_DB_CONTAINER:-$DB_CONTAINER}"
DB_CONN_INTERNAL="postgres://chatrpg:chatrpg@localhost/chatrpg"
export DATABASE_URL="${DATABASE_URL:-postgres://chatrpg:chatrpg@localhost:${DB_HOST_PORT}/chatrpg}"

# ── GM LLM (local relay; zero Anthropic spend) ───────────────────────────────
export TRPG_LLM_BASE_URL="${TRPG_LLM_BASE_URL:-http://127.0.0.1:18888/v1}"
export TRPG_LLM_API_KEY="${TRPG_LLM_API_KEY:-codex-relay-local}"
export TRPG_LLM_MODEL="${TRPG_LLM_MODEL:-gpt-5.5}"
export TRPG_LLM_SEND_TEMPERATURE="${TRPG_LLM_SEND_TEMPERATURE:-false}"
RELAY_BASE="$TRPG_LLM_BASE_URL"
RELAY_KEY="$TRPG_LLM_API_KEY"

# ── layered-ON engine profile (binary reads TRPG_*) ─────────────────────────
# Mirrors the profile that produced the reference runs; nothing ruleset-specific.
export TRPG_GM_AGENTIC_RETRIEVE="${TRPG_GM_AGENTIC_RETRIEVE:-true}"
export TRPG_ALLOW_SYNTHETIC_ACTOR_SEEDS="${TRPG_ALLOW_SYNTHETIC_ACTOR_SEEDS:-true}"
export TRPG_FAIL_ON_MISSING_SOURCE_BACKED_PARAMS="${TRPG_FAIL_ON_MISSING_SOURCE_BACKED_PARAMS:-false}"
export TRPG_CHARACTER_ONBOARDING_REQUIRED="${TRPG_CHARACTER_ONBOARDING_REQUIRED:-false}"
export TRPG_CONTEST_DEFAULT_PERCENTILE_SKILL="${TRPG_CONTEST_DEFAULT_PERCENTILE_SKILL:-50}"
export TRPG_CONTEST_DEFAULT_ATTACK_DV="${TRPG_CONTEST_DEFAULT_ATTACK_DV:-14}"
export TRPG_CONTEST_DEFAULT_STATIC_TARGET="${TRPG_CONTEST_DEFAULT_STATIC_TARGET:-14}"
export TRPG_NARRATOR_SPLIT="${TRPG_NARRATOR_SPLIT:-1}"
export TRPG_PRESENTATION_GATE="${TRPG_PRESENTATION_GATE:-1}"
export TRPG_SEEDED_ROLLS="${TRPG_SEEDED_ROLLS:-1}"
export TRPG_KNOWLEDGE_KERNEL="${TRPG_KNOWLEDGE_KERNEL:-shadow}"
export TRPG_DIRECTOR_PACKET="${TRPG_DIRECTOR_PACKET:-1}"
export TRPG_WORLD_NPC_ACTION="${TRPG_WORLD_NPC_ACTION:-1}"
export TRPG_SCENE_SENSORY_FLOOR="${TRPG_SCENE_SENSORY_FLOOR:-1}"
export TRPG_GM_CRAFT="${TRPG_GM_CRAFT:-1}"

ALARM="${EVAL_ALARM:-360}"

_eval_ts() { date '+%H:%M:%S'; }
eval_log() { printf '[%s] %s\n' "$(_eval_ts)" "$*" >&2; }
_cap() { perl -e 'alarm shift; exec @ARGV' "$ALARM" "$@"; }

# dbq <sql> — single-column/-At query against the live container.
dbq() { docker exec "$DB_CONTAINER" psql "$DB_CONN_INTERNAL" -At -c "$1"; }

# extract_narration <jsonl> — concatenate visible GM delta text.
extract_narration() {
  grep '"event":"delta"' "$1" 2>/dev/null | python3 -c "import sys,json
buf=[]
for l in sys.stdin:
  try: d=json.loads(l).get('data','')
  except Exception: continue
  if isinstance(d,str): buf.append(d)
print(''.join(buf))" 2>/dev/null
}

# eval_create_pc <sid> <ruleset> <module>
eval_create_pc() {
  local sid="$1" rs="$2" mod="$3" dir="${EVAL_OUT_DIR:-/tmp}" rc
  local args=(create-character --auto --ruleset "$rs" --actor-id pc.current \
              --session-id "$sid" --stream-format jsonl)
  [[ -n "$mod" && "$mod" != "-" ]] && args+=(--module "$mod")
  _cap "$BIN" "${args[@]}" >"$dir/create.jsonl" 2>"$dir/create.err"; rc=$?
  if [[ $rc -ne 0 ]]; then
    _cap "$BIN" "${args[@]}" >"$dir/create.jsonl" 2>"$dir/create.err"; rc=$?
  fi
  eval_log "create sid=$sid rc=$rc"
  return $rc
}

# eval_run_turn <sid> <ruleset> <module> <input> <out.jsonl> ; echoes narration
eval_run_turn() {
  local sid="$1" rs="$2" mod="$3" input="$4" out="$5" rc
  local args=(turn --ruleset "$rs" --session-id "$sid" --input "$input" \
              --stream-format jsonl)
  [[ -n "$mod" && "$mod" != "-" ]] && args+=(--module "$mod")
  _cap "$BIN" "${args[@]}" >"$out" 2>"${out%.jsonl}.err"; rc=$?
  if [[ $rc -ne 0 ]]; then
    _cap "$BIN" "${args[@]}" >"$out" 2>"${out%.jsonl}.err"; rc=$?
  fi
  extract_narration "$out"
  return $rc
}

# relay_chat <prompt> — one-shot completion from the local relay (player-sim brain)
relay_chat() {
  local prompt="$1" payload resp
  payload="$(jq -n --arg m "$TRPG_LLM_MODEL" --arg p "$prompt" \
    '{model:$m,messages:[{role:"user",content:$p}]}')"
  resp="$(curl -sf -m 120 -X POST "$RELAY_BASE/chat/completions" \
    -H 'Content-Type: application/json' -H "Authorization: Bearer $RELAY_KEY" \
    -d "$payload" 2>/dev/null)" || return 1
  jq -r '.choices[0].message.content // empty' <<<"$resp"
}
