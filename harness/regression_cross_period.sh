#!/usr/bin/env bash
# 批5 跨期回归脚本 — 一期+二期+三期框架全测试套件
# 用途：验证三期 mode-skills 实施后，一期/二期所有单元测试仍照绿（缓存稳定原则）。
# 运行：bash harness/regression_cross_period.sh
# 要求：Rust toolchain；无 DB 测试不需要 postgres（需 DB 的测试在 CI 中标 #[ignore]）。
#
# 包覆盖（按一期→二期→三期框架顺序）：
#   一期：trpg-model trpg-agent trpg-llm trpg-gm（turn_loop + cache + obligations）
#   二期：trpg-mechanics trpg-rule-agent（mechanic catalog + watcher + scene intent）
#   三期框架：trpg-gm（mode 推导/四级合并/for_mode/节拍参数/退出义务/缓存维度）
set -euo pipefail
cd "$(dirname "$0")/.."

PASS=0
FAIL=0
ERRORS=()

run_suite() {
    local pkg="$1"
    printf "  %-22s " "$pkg"
    if cargo test -p "$pkg" --quiet 2>&1 | tail -1 | grep -q "^test result: ok"; then
        echo "PASS"
        PASS=$((PASS + 1))
    else
        echo "FAIL"
        FAIL=$((FAIL + 1))
        ERRORS+=("$pkg")
    fi
}

echo "=== 跨期回归：一期+二期+三期框架 ==="
echo "--- 一期 核心模型/代理/流式/GM loop ---"
run_suite trpg-model
run_suite trpg-agent
run_suite trpg-llm
run_suite trpg-gm

echo "--- 二期 规则感知 GM ---"
run_suite trpg-mechanics
run_suite trpg-rule-agent

echo ""
echo "=== 汇总 PASS=$PASS  FAIL=$FAIL ==="
if [ "${#ERRORS[@]}" -gt 0 ]; then
    echo "FAILED: ${ERRORS[*]}"
    exit 1
fi
echo "ALL GREEN"
