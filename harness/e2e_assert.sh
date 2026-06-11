#!/usr/bin/env bash
# harness/e2e_assert.sh
# SQL 断言库：按 harness/specs/*.json 的 assertions 执行查询+期望比对，输出 PASS/FAIL 清单。
#
# 用法：
#   ./harness/e2e_assert.sh --spec harness/specs/combat_coc.json \
#                            --session SESSION_ID \
#                            [--player-text /tmp/trpg_sim_.../player_visible.txt] \
#                            [--verbose]
#
# 断言类型：
#   sql              — 执行 SQL，比对 COUNT(*) ≥ expect_min
#   grep_required    — 在 player_visible_text 中必须匹配 pattern
#   grep_forbidden   — 在 player_visible_text 中不得匹配 pattern
#
# 退出码：0 = 全部 PASS；1 = ≥1 FAIL；2 = 配置/依赖错误

set -euo pipefail

# ── 工具函数 ────────────────────────────────────────────────────────────────

ts()  { date '+%H:%M:%S'; }
die() { printf '[%s] ERROR: %s\n' "$(ts)" "$*" >&2; exit 2; }

command -v jq >/dev/null 2>&1 || die "jq is required (brew install jq)"

# ── 引数解析 ────────────────────────────────────────────────────────────────

SPEC_FILE=""
SESSION_ID=""
PLAYER_TEXT_FILE=""
VERBOSE=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --spec)        SPEC_FILE="$2";        shift 2 ;;
    --session)     SESSION_ID="$2";       shift 2 ;;
    --player-text) PLAYER_TEXT_FILE="$2"; shift 2 ;;
    --verbose|-v)  VERBOSE=1;             shift   ;;
    *) die "unknown argument: $1" ;;
  esac
done

[[ -n "$SPEC_FILE" ]]  || die "--spec is required"
[[ -f "$SPEC_FILE" ]]  || die "spec file not found: $SPEC_FILE"
[[ -n "$SESSION_ID" ]] || die "--session is required"

# ── 環境変数 ─────────────────────────────────────────────────────────────────

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# Save caller-supplied DATABASE_URL before sourcing .env (which would clobber it).
_CALLER_DATABASE_URL="${DATABASE_URL:-}"
if [[ -f "$REPO_ROOT/.env" ]]; then
  set -o allexport
  # shellcheck disable=SC1091
  source "$REPO_ROOT/.env"
  set +o allexport
fi
# Restore caller override (non-empty caller value wins over .env).
if [[ -n "$_CALLER_DATABASE_URL" ]]; then
  DATABASE_URL="$_CALLER_DATABASE_URL"
  export DATABASE_URL
fi

DATABASE_URL="${DATABASE_URL:-postgres://chatrpg:chatrpg@localhost:54346/chatrpg}"

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

# ── DB チェック ──────────────────────────────────────────────────────────────

db_check() {
  docker exec "$DB_CONTAINER" psql "$DB_CONN" -At -c "SELECT 1" >/dev/null 2>&1 || {
    printf '\n[%s] ERROR: DB not reachable in container %s\n' "$(ts)" "$DB_CONTAINER" >&2
    printf '[%s] Check: docker ps | grep %s\n' "$(ts)" "$DB_CONTAINER" >&2
    exit 2
  }
}

db_query() {
  local sql="${1/\$SESSION_ID/$SESSION_ID}"
  local result exit_code
  result="$(docker exec "$DB_CONTAINER" psql "$DB_CONN" -At -c "$sql" 2>&1)"
  exit_code=$?
  if [[ $exit_code -ne 0 ]]; then
    printf 'SQL_ERROR: %s' "$result"
  else
    printf '%s' "$result"
  fi
}

# ── カウンター ───────────────────────────────────────────────────────────────

PASS_COUNT=0
FAIL_COUNT=0
SKIP_COUNT=0

# ── 断言ヘルパー ─────────────────────────────────────────────────────────────

assert_pass() {
  local id="$1" desc="$2" detail="${3:-}"
  printf 'PASS  [%s] %s\n' "$id" "$desc"
  [[ "$VERBOSE" -eq 1 && -n "$detail" ]] && printf '      detail: %s\n' "$detail"
  PASS_COUNT=$((PASS_COUNT + 1))
}

assert_fail() {
  local id="$1" desc="$2" detail="${3:-}"
  printf 'FAIL  [%s] %s\n' "$id" "$desc"
  [[ -n "$detail" ]] && printf '      detail: %s\n' "$detail"
  FAIL_COUNT=$((FAIL_COUNT + 1))
}

assert_skip() {
  local id="$1" reason="$2"
  printf 'SKIP  [%s] %s\n' "$id" "$reason"
  SKIP_COUNT=$((SKIP_COUNT + 1))
}

# ── SQL 断言 ─────────────────────────────────────────────────────────────────

run_sql() {
  local id="$1" query="$2" expect_min="$3" desc="$4"
  local result count
  result="$(db_query "$query")"
  if [[ "$result" == SQL_ERROR:* ]]; then
    assert_fail "$id" "$desc" "SQL error: ${result:0:120}"
    return
  fi
  count="$(printf '%s' "$result" | head -1 | tr -d '[:space:]')"
  if [[ ! "$count" =~ ^[0-9]+$ ]]; then
    assert_fail "$id" "$desc" "non-numeric result: '$count' (expect int ≥ $expect_min)"
    return
  fi
  if [[ "$count" -ge "$expect_min" ]]; then
    assert_pass "$id" "$desc" "count=$count ≥ expect_min=$expect_min"
  else
    assert_fail "$id" "$desc" "count=$count < expect_min=$expect_min"
  fi
}

run_sql_optional() {
  local id="$1" query="$2" desc="$3" note="$4"
  local result count
  result="$(db_query "$query")"
  if [[ "$result" == SQL_ERROR:* ]]; then
    assert_skip "$id" "sql error (optional): ${result:0:80}"
    return
  fi
  count="$(printf '%s' "$result" | head -1 | tr -d '[:space:]')"
  if [[ "$count" =~ ^[0-9]+$ ]] && [[ "$count" -ge 1 ]]; then
    assert_pass "$id" "$desc" "count=$count"
  else
    assert_skip "$id" "optional: count=$count; note: $note"
  fi
}

# ── grep 断言 ────────────────────────────────────────────────────────────────

require_file_or_skip() {
  local id="$1" file="$2"
  if [[ -z "$file" || ! -f "$file" ]]; then
    assert_skip "$id" "player_text file not provided or missing: ${file:-<unset>}"
    return 1
  fi
  return 0
}

run_grep_required() {
  local id="$1" pattern="$2" file="$3" desc="$4"
  require_file_or_skip "$id" "$file" || return
  if grep -qE "$pattern" "$file" 2>/dev/null; then
    assert_pass "$id" "$desc"
  else
    assert_fail "$id" "$desc" "pattern not found: $pattern"
  fi
}

run_grep_forbidden() {
  local id="$1" pattern="$2" file="$3" desc="$4"
  require_file_or_skip "$id" "$file" || return
  local hits
  hits="$(grep -cE "$pattern" "$file" 2>/dev/null || true)"
  if [[ "$hits" -eq 0 ]]; then
    assert_pass "$id" "$desc" "pattern absent (correct)"
  else
    local examples
    examples="$(grep -oE "$pattern" "$file" 2>/dev/null | head -3 | tr '\n' ' ')"
    assert_fail "$id" "$desc" "forbidden pattern found ($hits hits): $examples"
  fi
}

run_grep_required_count() {
  local id="$1" patterns_json="$2" min_count="$3" file="$4" desc="$5"
  require_file_or_skip "$id" "$file" || return
  local matched=0 total
  total="$(jq 'length' <<< "$patterns_json")"
  for pi in $(seq 0 $((total - 1))); do
    local pat
    pat="$(jq -r ".[$pi]" <<< "$patterns_json")"
    if grep -qE "$pat" "$file" 2>/dev/null; then
      matched=$((matched + 1))
    fi
  done
  if [[ "$matched" -ge "$min_count" ]]; then
    assert_pass "$id" "$desc" "$matched/$total patterns matched (need ≥$min_count)"
  else
    assert_fail "$id" "$desc" "only $matched/$total patterns matched (need ≥$min_count)"
  fi
}

# ── Spec 読み込み＋断言実行 ─────────────────────────────────────────────────

SPEC="$(jq . "$SPEC_FILE")"
SPEC_ID="$(jq -r '.spec_id' <<< "$SPEC")"
ASSERTION_COUNT="$(jq '.assertions | length' <<< "$SPEC")"

printf '\n=== e2e_assert: %s ===\n' "$SPEC_ID"
printf '    session_id:  %s\n' "$SESSION_ID"
printf '    player_text: %s\n' "${PLAYER_TEXT_FILE:-<not provided>}"
printf '    assertions:  %s\n\n' "$ASSERTION_COUNT"

db_check

for i in $(seq 0 $((ASSERTION_COUNT - 1))); do
  A="$(jq ".assertions[$i]" <<< "$SPEC")"
  A_ID="$(jq -r '.id' <<< "$A")"
  A_TYPE="$(jq -r '.type' <<< "$A")"
  A_DESC="$(jq -r '.description // .id' <<< "$A")"

  case "$A_TYPE" in
    sql)
      A_OPT="$(jq -r '.expect_min_or_note // empty' <<< "$A")"
      A_QUERY="$(jq -r '.query' <<< "$A")"
      if [[ -n "$A_OPT" ]]; then
        run_sql_optional "$A_ID" "$A_QUERY" "$A_DESC" "$A_OPT"
      else
        A_MIN="$(jq -r '.expect_min // 1' <<< "$A")"
        run_sql "$A_ID" "$A_QUERY" "$A_MIN" "$A_DESC"
      fi
      ;;
    grep_required)
      A_MIN_PAT="$(jq -r '.pattern_count_min // empty' <<< "$A")"
      if [[ -n "$A_MIN_PAT" ]]; then
        A_PATTERNS="$(jq '.patterns' <<< "$A")"
        run_grep_required_count "$A_ID" "$A_PATTERNS" "$A_MIN_PAT" \
          "$PLAYER_TEXT_FILE" "$A_DESC"
      else
        A_PATTERN="$(jq -r '.pattern' <<< "$A")"
        run_grep_required "$A_ID" "$A_PATTERN" "$PLAYER_TEXT_FILE" "$A_DESC"
      fi
      ;;
    grep_forbidden)
      A_PATTERN="$(jq -r '.pattern' <<< "$A")"
      run_grep_forbidden "$A_ID" "$A_PATTERN" "$PLAYER_TEXT_FILE" "$A_DESC"
      ;;
    *)
      assert_skip "$A_ID" "unknown assertion type: $A_TYPE"
      ;;
  esac
done

# ── 結果サマリー ─────────────────────────────────────────────────────────────

printf '\n=== SUMMARY ===\n'
printf '  PASS: %s\n' "$PASS_COUNT"
printf '  FAIL: %s\n' "$FAIL_COUNT"
printf '  SKIP: %s\n\n' "$SKIP_COUNT"

if [[ "$FAIL_COUNT" -gt 0 ]]; then
  printf 'RESULT: FAIL (%s assertion(s) failed)\n' "$FAIL_COUNT"
  exit 1
else
  printf 'RESULT: PASS\n'
  exit 0
fi
