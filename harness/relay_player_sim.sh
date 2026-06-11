#!/usr/bin/env bash
# harness/relay_player_sim.sh
# Relay 玩家模拟器：读 harness/specs/<spec>.json，经 stdin 喂 trpg play --agent 持久进程。
#
# 用法：
#   ./harness/relay_player_sim.sh --spec harness/specs/combat_dnd.json \
#                                  [--bin /path/to/trpg] \
#                                  [--out-dir /tmp/run_xxx] \
#                                  [--dry-run] \
#                                  [--model gpt-5.5]
#
# 架构：trpg play --agent 是交互循环；本脚本用命名管道维持会话，
#         每回合写一行输入→等提示符静默后读输出→喂下一条。
#
# spec 字段兼容：支持 turns[].player_input.{source,text,relay_prompt,max_tokens}
#   和 turns[].user_input（直接字符串，scenarios/ 用法）两种形态。
# Claude 额度零消耗：player 输入全走本地 relay（非 Anthropic API）。
# 行级时间戳：每行 stderr 前缀 [HH:MM:SS]。
# DB 不可用时报清晰错误（fail-closed）。
#
# 依赖：bash ≥5, jq, curl

set -euo pipefail

# ── 工具函数 ────────────────────────────────────────────────────────────────

ts()  { date '+%H:%M:%S'; }
log() { printf '[%s] %s\n' "$(ts)" "$*" >&2; }
die() { printf '[%s] ERROR: %s\n' "$(ts)" "$*" >&2; exit 1; }

command -v jq   >/dev/null 2>&1 || die "jq is required (brew install jq)"
command -v curl >/dev/null 2>&1 || die "curl is required"

# ── 引数解析 ────────────────────────────────────────────────────────────────

SPEC_FILE=""
TRPG_BIN="${TRPG_BIN:-}"
OUT_DIR=""
DRY_RUN=0
MODEL_OVERRIDE=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --spec)    SPEC_FILE="$2";      shift 2 ;;
    --bin)     TRPG_BIN="$2";      shift 2 ;;
    --out-dir) OUT_DIR="$2";       shift 2 ;;
    --dry-run) DRY_RUN=1;          shift   ;;
    --model)   MODEL_OVERRIDE="$2"; shift 2 ;;
    *) die "unknown argument: $1" ;;
  esac
done

[[ -n "$SPEC_FILE" ]] || die "--spec is required"
[[ -f "$SPEC_FILE" ]] || die "spec file not found: $SPEC_FILE"

# ── 環境変数 ─────────────────────────────────────────────────────────────────

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
_CALLER_DATABASE_URL="${DATABASE_URL:-}"
if [[ -f "$REPO_ROOT/.env" ]]; then
  set -o allexport
  # shellcheck disable=SC1091
  source "$REPO_ROOT/.env"
  set +o allexport
fi
if [[ -n "$_CALLER_DATABASE_URL" ]]; then
  DATABASE_URL="$_CALLER_DATABASE_URL"
  export DATABASE_URL
fi

if [[ -z "$TRPG_BIN" ]]; then
  CACHE_BIN="/Users/haoli/.cache/cargo-target/chatrpg-rs-v1.20-formula/debug/trpg"
  if [[ -f "$CACHE_BIN" ]]; then
    TRPG_BIN="$CACHE_BIN"
  else
    TRPG_BIN="$REPO_ROOT/target/debug/trpg"
  fi
fi

DATABASE_URL="${DATABASE_URL:-postgres://chatrpg:chatrpg@localhost:54346/chatrpg}"
RELAY_BASE="${TRPG_LLM_BASE_URL:-http://127.0.0.1:18888/v1}"
RELAY_KEY="${TRPG_LLM_API_KEY:-codex-relay-local}"
# --model 覆盖或用 gpt-5.5（三期 mode 框架需要能调工具的模型）
if [[ -n "$MODEL_OVERRIDE" ]]; then
  RELAY_MODEL="$MODEL_OVERRIDE"
else
  RELAY_MODEL="${TRPG_LLM_MODEL:-gpt-5.5}"
  export TRPG_LLM_MODEL="$RELAY_MODEL"
fi

# DB 容器名从 DATABASE_URL 端口推算
DB_PORT="$(printf '%s' "$DATABASE_URL" | grep -oE ':[0-9]+/' | tr -d ':/' | head -1 || echo "54346")"
case "$DB_PORT" in
  54341) DB_CONTAINER="chatrpg-postgres-v1154" ;;
  54344) DB_CONTAINER="chatrpg-postgres-v1160" ;;
  54345) DB_CONTAINER="chatrpg-postgres-v1161" ;;
  54346) DB_CONTAINER="chatrpg-postgres-v1162" ;;
  54347) DB_CONTAINER="chatrpg-postgres-rulesets" ;;
  *)     DB_CONTAINER="chatrpg-postgres-v1162" ;;
esac
DB_CONN="postgres://chatrpg:chatrpg@localhost/chatrpg"

# ── Spec 读取 ─────────────────────────────────────────────────────────────────

SPEC="$(jq . "$SPEC_FILE")"
SPEC_ID="$(jq -r '.spec_id // .name // "unnamed"' <<< "$SPEC")"
RULESET_ID="$(jq -r '.ruleset_id' <<< "$SPEC")"
MODULE_ID="$(jq -r '.module_id // empty' <<< "$SPEC")"
TURN_COUNT="$(jq '.turns | length' <<< "$SPEC")"

log "spec=$SPEC_ID  ruleset=$RULESET_ID  module=${MODULE_ID:-<none>}  turns=$TURN_COUNT"
log "relay_model=$RELAY_MODEL  db_port=$DB_PORT"

if [[ "$DRY_RUN" -eq 1 ]]; then
  log "[dry-run] spec parsed OK — skipping binary + DB calls"
  log "[dry-run] trpg_bin=$TRPG_BIN"
  exit 0
fi

[[ -f "$TRPG_BIN" ]] || die "trpg binary not found: $TRPG_BIN (run: cargo build -p trpg-cli)"

# ── 出力ディレクトリ ─────────────────────────────────────────────────────────

if [[ -z "$OUT_DIR" ]]; then
  OUT_DIR="/tmp/trpg_sim_${SPEC_ID}_$(date '+%Y%m%d_%H%M%S')"
fi
mkdir -p "$OUT_DIR"
log "output dir: $OUT_DIR"

PLAYER_TEXT_LOG="$OUT_DIR/player_visible.txt"
SESSION_FILE="$OUT_DIR/session_id.txt"
AGENT_STDOUT="$OUT_DIR/agent_stdout.txt"
AGENT_STDERR="$OUT_DIR/agent_stderr.txt"

# ── DB チェック ──────────────────────────────────────────────────────────────

db_check() {
  docker exec "$DB_CONTAINER" psql "$DB_CONN" -At -c "SELECT 1" >/dev/null 2>&1 || {
    die "DB not reachable in container $DB_CONTAINER — check docker ps"
  }
}
db_check

# ── relay LLM 生成（player input source=relay_generate） ──────────────────

relay_generate() {
  local prompt="$1"
  local payload
  payload="$(jq -n --arg model "$RELAY_MODEL" --arg prompt "$prompt" \
    '{"model":$model,"messages":[{"role":"user","content":$prompt}]}')"
  local response
  response="$(curl -sf -X POST "$RELAY_BASE/chat/completions" \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer $RELAY_KEY" \
    -d "$payload")" || die "relay call failed — check $RELAY_BASE is running"
  jq -r '.choices[0].message.content // empty' <<< "$response" \
    || die "relay response parse failed"
}

log "checking relay at $RELAY_BASE..."
relay_health="$(curl -sf "$RELAY_BASE/models" 2>&1 | head -c 60 || echo "UNREACHABLE")"
[[ "$relay_health" != "UNREACHABLE" ]] || die "relay not reachable at $RELAY_BASE"
log "relay OK"

# ── turn_input 解析：兼容两种 spec 形态 ──────────────────────────────────────
# 形态A (specs/): turns[i].player_input.{source, text, relay_prompt}
# 形态B (scenarios/): turns[i].user_input (字符串直接使用)

get_turn_input() {
  local i="$1"
  local turn
  turn="$(jq ".turns[$i]" <<< "$SPEC")"

  # 形态B：user_input 字段存在时直接使用
  local user_input
  user_input="$(jq -r '.user_input // empty' <<< "$turn")"
  if [[ -n "$user_input" ]]; then
    printf '%s' "$user_input"
    return
  fi

  # 形态A：player_input.source
  local source
  source="$(jq -r '.player_input.source // "literal"' <<< "$turn")"
  case "$source" in
    literal)
      jq -r '.player_input.text' <<< "$turn"
      ;;
    relay_generate)
      local relay_prompt
      relay_prompt="$(jq -r '.player_input.relay_prompt' <<< "$turn")"
      log "  calling relay for player input..."
      local generated
      generated="$(relay_generate "$relay_prompt")"
      log "  relay: ${generated:0:80}"
      printf '%s' "$generated"
      ;;
    *)
      die "turn $i: unknown player_input.source: $source"
      ;;
  esac
}

# ── trpg play --agent 持久进程 ─────────────────────────────────────────────
# 用命名管道(FIFO)向进程 stdin 注入输入；stdout 追加写到文件后轮询。

IN_FIFO="$OUT_DIR/agent_in.fifo"
mkfifo "$IN_FIFO"

log "starting trpg play --agent (ruleset=$RULESET_ID module=${MODULE_ID:-none} model=$RELAY_MODEL)..."

# 启动持久进程：stdin 接 FIFO，stdout/stderr 写文件
PLAY_CMD=("$TRPG_BIN" play --ruleset "$RULESET_ID" --agent)
[[ -n "$MODULE_ID" ]] && PLAY_CMD+=(--module "$MODULE_ID")

(
  cd "$REPO_ROOT"
  export TRPG_LLM_MODEL="$RELAY_MODEL"
  "${PLAY_CMD[@]}" < "$IN_FIFO" > "$AGENT_STDOUT" 2> "$AGENT_STDERR"
) &
AGENT_PID=$!
log "agent pid=$AGENT_PID"

# 保持 FIFO writer 打开（防止进程收到 EOF 退出）
exec 9>"$IN_FIFO"

# ── 等待启动：读到 "agent session:" 行或提示符 ─────────────────────────────

SESSION_ID=""
wait_for_prompt() {
  local max_secs="${1:-60}" desc="${2:-startup}"
  local elapsed=0
  while [[ $elapsed -lt $max_secs ]]; do
    if ! kill -0 "$AGENT_PID" 2>/dev/null; then
      log "WARN: agent process exited early (${desc})"
      break
    fi
    # 提取 session id（如果还没有）
    if [[ -z "$SESSION_ID" && -f "$AGENT_STDOUT" ]]; then
      SESSION_ID="$(grep -m1 'agent session:' "$AGENT_STDOUT" 2>/dev/null \
        | sed 's/.*agent session: *//' | tr -d '[:space:]' || true)"
    fi
    # 等到 prompt 出现（[chatrpg:agent]>）
    if grep -q '\[chatrpg:agent\]>' "$AGENT_STDOUT" 2>/dev/null; then
      return 0
    fi
    sleep 1
    elapsed=$((elapsed + 1))
  done
  log "WARN: wait_for_prompt timed out after ${max_secs}s (${desc})"
  return 0
}

log "waiting for agent startup..."
wait_for_prompt 90 "startup"

if [[ -n "$SESSION_ID" ]]; then
  log "session_id=$SESSION_ID"
  echo "$SESSION_ID" > "$SESSION_FILE"
else
  log "WARN: could not extract session_id from startup output"
fi

# ── 预建卡（pre_session.create_character=true） ──────────────────────────────
# spec 里声明 "pre_session": {"create_character": true} 时，在 turn 序列开始前
# 使用当前 ruleset 自动建卡并绑到 session，使 GM 能做机械结算。
PRE_CC="$(jq -r '.pre_session.create_character // false' <<< "$SPEC")"
if [[ "$PRE_CC" == "true" && -n "$SESSION_ID" ]]; then
  log "pre_session: running create-character --auto --session-id $SESSION_ID ..."
  CC_OUT="$OUT_DIR/create_char_out.txt"
  CC_ERR="$OUT_DIR/create_char_err.txt"
  CC_CMD=("$TRPG_BIN" create-character --ruleset "$RULESET_ID" --auto --session-id "$SESSION_ID")
  [[ -n "$MODULE_ID" ]] && CC_CMD+=(--module "$MODULE_ID")
  set +e
  (cd "$REPO_ROOT" && TRPG_LLM_MODEL="$RELAY_MODEL" "${CC_CMD[@]}" > "$CC_OUT" 2> "$CC_ERR") &
  CC_PID=$!
  CC_ELAPSED=0
  until ! kill -0 "$CC_PID" 2>/dev/null || [[ $CC_ELAPSED -ge 300 ]]; do
    sleep 5; CC_ELAPSED=$((CC_ELAPSED + 5))
    [[ $((CC_ELAPSED % 30)) -eq 0 ]] && log "  create-character still running (${CC_ELAPSED}s)..."
  done
  kill "$CC_PID" 2>/dev/null || true; wait "$CC_PID" 2>/dev/null || true
  set -e
  log "pre_session: create-character done (${CC_ELAPSED}s)"
  # 短暂等 agent prompt 恢复（create-char 不干扰 play 进程，但 DB 写入需稳定）
  sleep 2
fi

# ── 回合循环 ────────────────────────────────────────────────────────────────

# 记录每回合开始时 stdout 的字节偏移，用于切出本回合输出
prev_size=0
if [[ -f "$AGENT_STDOUT" ]]; then
  prev_size="$(wc -c < "$AGENT_STDOUT" | tr -d ' ')"
fi

for i in $(seq 0 $((TURN_COUNT - 1))); do
  TURN="$(jq ".turns[$i]" <<< "$SPEC")"
  log "=== turn $i ==="

  # 生成玩家输入
  PLAYER_INPUT="$(get_turn_input "$i")"
  [[ -n "$PLAYER_INPUT" ]] || die "turn $i: empty player input"

  log "  player: ${PLAYER_INPUT:0:100}"
  printf '[%s] PLAYER_INPUT (turn %s): %s\n' "$(ts)" "$i" "$PLAYER_INPUT" >> "$PLAYER_TEXT_LOG"

  # 保存发送前的 stdout 大小（取回合输出用）
  prev_size="$(wc -c < "$AGENT_STDOUT" 2>/dev/null | tr -d ' ' || echo 0)"

  # 把输入写入 FIFO（printf 保证不加换行符干扰，echo 加 \n 触发 readline）
  printf '%s\n' "$PLAYER_INPUT" >&9

  # 等待本回合输出：等到下一个 prompt 出现或进程退出
  TURN_TIMEOUT=240
  elapsed=0
  while [[ $elapsed -lt $TURN_TIMEOUT ]]; do
    if ! kill -0 "$AGENT_PID" 2>/dev/null; then
      log "WARN: agent exited at turn $i"
      break
    fi
    # 等 prompt 出现在本回合输出之后
    cur_size="$(wc -c < "$AGENT_STDOUT" 2>/dev/null | tr -d ' ' || echo 0)"
    if [[ "$cur_size" -gt "$prev_size" ]]; then
      # 等 [chatrpg:agent]> 出现在新内容中
      new_content="$(tail -c +$((prev_size + 1)) "$AGENT_STDOUT" 2>/dev/null || true)"
      if printf '%s' "$new_content" | grep -q '\[chatrpg:agent\]>'; then
        break
      fi
    fi
    sleep 2
    elapsed=$((elapsed + 2))
  done
  if [[ $elapsed -ge $TURN_TIMEOUT ]]; then
    log "WARN: turn $i timed out after ${TURN_TIMEOUT}s"
  fi

  # 截取本回合输出写日志
  TURN_OUT="$OUT_DIR/turn_${i}_output.txt"
  cur_size="$(wc -c < "$AGENT_STDOUT" 2>/dev/null | tr -d ' ' || echo 0)"
  if [[ "$cur_size" -gt "$prev_size" ]]; then
    tail -c "+$((prev_size + 1))" "$AGENT_STDOUT" > "$TURN_OUT" 2>/dev/null || true
    # 提取文本行（非 prompt 行）写 player_visible
    grep -v '^\[chatrpg' "$TURN_OUT" 2>/dev/null \
      | grep -v '^$' >> "$PLAYER_TEXT_LOG" || true
    printf '[%s] --- END turn %s ---\n' "$(ts)" "$i" >> "$PLAYER_TEXT_LOG"
    log "  turn output: $(wc -c < "$TURN_OUT" 2>/dev/null | tr -d ' ') bytes"
  else
    log "  WARN: no new output for turn $i"
  fi

  NOTES="$(jq -r '.notes // empty' <<< "$TURN")"
  [[ -n "$NOTES" ]] && log "  note: $NOTES"
done

# ── 优雅退出 ─────────────────────────────────────────────────────────────────

log "sending /quit to agent..."
printf '/quit\n' >&9
exec 9>&-  # 关闭 FIFO writer

# 等进程自然退出（最多 15s）
QUIT_ELAPSED=0
while kill -0 "$AGENT_PID" 2>/dev/null && [[ $QUIT_ELAPSED -lt 15 ]]; do
  sleep 1
  QUIT_ELAPSED=$((QUIT_ELAPSED + 1))
done
kill "$AGENT_PID" 2>/dev/null || true
wait "$AGENT_PID" 2>/dev/null || true

# 从 stdout 中尝试再次捕获 session_id（备用）
if [[ -z "$SESSION_ID" && -f "$AGENT_STDOUT" ]]; then
  SESSION_ID="$(grep -m1 'agent session:' "$AGENT_STDOUT" 2>/dev/null \
    | sed 's/.*agent session: *//' | tr -d '[:space:]' || true)"
  [[ -n "$SESSION_ID" ]] && echo "$SESSION_ID" > "$SESSION_FILE"
fi

# ── 完了 ─────────────────────────────────────────────────────────────────────

log "=== relay_player_sim done ==="
log "session_id=${SESSION_ID:-<unknown>}"
log "output dir: $OUT_DIR"
log "player visible: $PLAYER_TEXT_LOG"
log ""
log "Next: ./harness/e2e_assert.sh --spec '$SPEC_FILE' --session '${SESSION_ID:-<id>}' --player-text '$PLAYER_TEXT_LOG'"
