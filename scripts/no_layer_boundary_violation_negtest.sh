#!/usr/bin/env bash
# P7.4 负向门：证明 no_layer_boundary_violation.sh 确实**能抓到**违规——不是只会绿。
#
# 做法：在唯一临时目录里搭一个最小 fake-repo（scripts/ + crates/ 骨架），把真守卫脚本
# 复制进去（它按 SCRIPT_DIR/.. 自解析 REPO，故在 fake-repo 内运行 = 扫 fake-repo），
# 然后：
#   ① 在 Director 表示层路径（crates/trpg-director/src）放一个含禁止模式的脏 fixture
#      → 断言守卫**非零退出**（抓到）。
#   ② 在 Narrator 函数体（turn_loop.rs 的 run_narrator 体内）放禁止模式
#      → 断言守卫**非零退出**（函数体定位生效）。
#   ③ 在 runtime 合法路径（turn_loop.rs narrator 体**外**）放同一禁止模式 + 干净表示层
#      → 断言守卫**零退出**（精确收窄：不误伤合法 runtime 代码）。
# 临时目录用后即删（trap 自清理）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GUARD="${SCRIPT_DIR}/no_layer_boundary_violation.sh"

if [[ ! -f "$GUARD" ]]; then
  echo "negtest: FAIL — guard script not found at $GUARD"
  exit 1
fi

TMP="$(mktemp -d "${TMPDIR:-/tmp}/no_layer_boundary_negtest.XXXXXX")"
cleanup() { rm -rf "$TMP"; }
trap cleanup EXIT

fail() { echo "negtest: FAIL — $1"; exit 1; }

# ── 搭 fake-repo 骨架 ────────────────────────────────────────────────────
mkdir -p "$TMP/scripts" "$TMP/crates/trpg-director/src" "$TMP/crates/trpg-gm/src"
cp "$GUARD" "$TMP/scripts/no_layer_boundary_violation.sh"

# Narrator packet.rs 占位（干净）——存在即可，避免 narrator 路径缺文件。
cat > "$TMP/crates/trpg-gm/src/packet.rs" <<'RS'
// clean narrator presentation packet — no banned patterns here.
pub struct NarrationPacket;
RS

# 一个最小 turn_loop.rs：含 run_narrator 函数体（脏行可注入）+ narrator 体外 runtime 行。
write_turnloop() {
  # $1 = narrator 体内注入行；$2 = narrator 体外（runtime）注入行
  cat > "$TMP/crates/trpg-gm/src/turn_loop.rs" <<RS
pub(crate) async fn run_narrator(&self) -> Option<String> {
    let _x = 1;
    ${1:-let _clean = 0;}
    None
}

// 体外 runtime / WorldPort 合法路径：
pub(crate) async fn commit_primitive(&self) {
    ${2:-let _clean = 0;}
}
RS
}

run_guard() {
  ( cd "$TMP" && bash scripts/no_layer_boundary_violation.sh >/dev/null 2>&1 )
}

# ── ① 脏 Director fixture → 守卫必须非零 ─────────────────────────────────
cat > "$TMP/crates/trpg-director/src/lib.rs" <<'RS'
pub fn bad(db: &trpg_db::Db) { db.reveal_fact(); }
RS
write_turnloop "" ""   # narrator clean
if run_guard; then
  fail "脏 Director fixture（trpg_db::Db / reveal_fact）未被抓到（守卫错误地零退出）"
fi
echo "negtest ①: dirty Director fixture → guard non-zero (caught) OK"

# 清掉 director 脏文件，回到干净
cat > "$TMP/crates/trpg-director/src/lib.rs" <<'RS'
pub fn ok() {}
RS

# ── ② 脏 Narrator 函数体 → 守卫必须非零 ──────────────────────────────────
write_turnloop "let _ = reveal_fact();" ""
if run_guard; then
  fail "Narrator 函数体内禁止模式（reveal_fact）未被抓到（函数体定位失效）"
fi
echo "negtest ②: dirty narrator fn-body → guard non-zero (caught) OK"

# ── ③ narrator 体外 runtime 合法 reveal_fact + 干净表示层 → 守卫必须零 ────
write_turnloop "" "let _ = reveal_fact();"
if ! run_guard; then
  fail "narrator 体外 runtime 的 reveal_fact 被误伤（守卫错误地非零退出）"
fi
echo "negtest ③: clean presentation + runtime reveal_fact outside narrator → guard zero (no false positive) OK"

echo "no-layer-boundary-violation negtest: OK (catches violations, no false positives)"
exit 0
