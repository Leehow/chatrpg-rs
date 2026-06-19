#!/usr/bin/env bash
# 层化迁移护栏汇总门 (P0-4): 单一可调用入口。
#
# 『层化迁移期每次改动必跑』——P0 立此门，P1-P7 各阶段往里**加门**（新架构验收转成
# 可执行断言后追加到本脚本与 arch_gates_tests.rs）。任一子步非零退出即整体 FAIL。
#
# 收口内容（设计4 §19 的可观测门 + 现有零硬编码守卫 + 关键缓存/契约回归）：
#   ① rustfmt --check（迁移护栏 owned 文件）          # 格式
#   ② scripts/no_engine_ruleset_hardcode.sh          # 引擎零规则集硬编码
#   ③ scripts/no_parser_seed_hardcode.sh             # parser seed 区段 ruleset-中立
#   ④ cargo test -p trpg-gm --lib arch_gates         # 本阶段新架构门（§19 Rust 侧）
#   ⑤ cargo test -p trpg-gm turn_plan                # 锁 CANONICAL_TURN_PLAN 15-phase
#   ⑥ cargo test -p trpg-gm prompts                  # 锁 BP1/BP2 prefix hash 稳定
#   ⑦ cargo test -p trpg-gm schema                   # 锁工具 schema 字节稳定
#   ⑧ scripts/no_layer_boundary_violation.sh         # Narrator 无状态工具预置门
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "$REPO"

step=0
run() {
  step=$((step + 1))
  local label="$1"; shift
  echo ""
  echo ">>> [arch_gates ${step}] ${label}"
  echo "    \$ $*"
  if ! "$@"; then
    echo ""
    echo "===== arch_gates: FAIL at step ${step} (${label}) ====="
    exit 1
  fi
}

# 格式门——只 fmt-check **本迁移护栏 owned 的文件**，不做 workspace 全量。
# 原因：基线工作区（commit 327d284）存在 5 处既有 fmt 漂移（turn_loop.rs /
# npc_behavior_prompt.rs / npc_behavior_consistency.rs / knowledge_leak_verifier.rs /
# memory_proposal.rs，均非 P0 触碰），而 P0 硬约束『逻辑文件只可加注释』禁止 P0 重排它们。
# 故此门校验 P0/后续护栏 owned 文件 fmt-clean；既有基线漂移留 P1+ 专项 fmt 清理（人定）。
# 后续阶段往 ARCH_FMT_FILES 追加各自 owned 的源文件。
ARCH_FMT_FILES=(
  crates/trpg-gm/src/arch_gates_tests.rs
  crates/trpg-gm/src/lib.rs
  crates/trpg-gm/src/execute.rs
  crates/trpg-gm/src/plugin/host.rs
  crates/trpg-gm/tests/plugin_proposal_contract.rs
  crates/trpg-orchestrator/src/lib.rs
  crates/trpg-runtime/src/lib.rs
)
# stable rustfmt 给 lib.rs 会递归进 `mod` 子文件、撞上基线既有漂移（且 --skip-children
# 仅 nightly）。故跑 rustfmt --check 后，**只对 owned 文件的 Diff 失败**——过滤掉递归
# 带出的非 owned 漂移行。owned 文件自身任何漂移即 FAIL。
fmt_check_owned() {
  local out owned_diffs
  out="$(rustfmt --check --edition 2021 "${ARCH_FMT_FILES[@]}" 2>&1 || true)"
  owned_diffs="$(printf '%s\n' "$out" | grep -E '^Diff in ' \
    | grep -F -f <(printf '%s\n' "${ARCH_FMT_FILES[@]}") || true)"
  if [[ -n "${owned_diffs//[$'\n\t ']/}" ]]; then
    echo "护栏 owned 文件存在 fmt 漂移（请 rustfmt 之）："
    printf '%s\n' "$owned_diffs"
    return 1
  fi
  echo "rustfmt(owned): OK"
  return 0
}
run "rustfmt --check (护栏 owned 文件)" fmt_check_owned
run "no_engine_ruleset_hardcode"        bash scripts/no_engine_ruleset_hardcode.sh
run "no_parser_seed_hardcode"           bash scripts/no_parser_seed_hardcode.sh
run "arch_gates_tests (§19 Rust 门)"    cargo test -p trpg-gm --lib arch_gates
run "turn_plan (15-phase 锁)"           cargo test -p trpg-gm turn_plan
run "prompts (BP1/BP2 prefix hash 锁)"  cargo test -p trpg-gm prompts
run "schema (工具 schema 字节锁)"        cargo test -p trpg-gm schema
run "no_layer_boundary_violation"       bash scripts/no_layer_boundary_violation.sh

echo ""
echo "===== arch_gates: OK (all ${step} gates green) ====="
exit 0
