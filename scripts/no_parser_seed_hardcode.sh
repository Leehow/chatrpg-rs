#!/usr/bin/env bash
# CI 守卫 (P0-2 同源, 范围收窄到 trpg-parser 的 chargen/mechanics fallback seed 区段):
#   该区段 (由 sentinel 注释界定) 必须 ruleset-中立 —— 不得出现规则集名字面量,
#   也就是不能再写 `ruleset.contains("cyberpunk")` 这类 per-ruleset 分支。
#   命中 -> 退出码 1; 干净 -> 退出码 0; sentinel 丢失 -> fail-closed 退出码 1。
#
# 设计哲学: 引擎零硬编码; 规则集差异一律下沉到 LLM 编译产物 (单一事实源)。
# 与 scripts/no_engine_ruleset_hardcode.sh 共用同一套禁止字面量与退出码契约,
# 只是 trpg-parser (ingest 层, 合法地按文档识别规则集身份) 整体不进引擎守卫,
# 故这里只盯住那两个 fallback 函数所在的 sentinel 区段。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "${SCRIPT_DIR}/.." && pwd)"
FILE="${REPO}/crates/trpg-parser/src/lib.rs"
BEGIN='guard:no-ruleset-name-literals BEGIN'
END='guard:no-ruleset-name-literals END'

# 禁止字面量 (规则集名 / 模组名 / 模组专属 NPC id)。大小写不敏感子串。
# 与 no_engine_ruleset_hardcode.sh 保持一致。
BANNED='call_of_cthulhu|cyberpunk|cthulhu|sword_world|剑世界|homecoming|nyarlathotep|scav_boss|athena_drone|dnd5e|brp_orc|triangle_agency|the_vault|"coc"|"brp"|"dnd"|"d&d"|"5e"|"triangle"|"fate"|"masks"|"vault"'

if [[ ! -f "$FILE" ]]; then
  echo "no-parser-seed-hardcode: FAIL (target file missing: $FILE)"
  exit 1
fi

# 抽取 sentinel 之间的区段 (不含 sentinel 行本身)。两枚 sentinel 必须都在。
REGION="$(awk -v b="$BEGIN" -v e="$END" '
  index($0, b) { f=1; seenb=1; next }
  index($0, e) { f=0; seene=1; next }
  f { print }
  END { if (!seenb || !seene) exit 3 }
' "$FILE")" || {
  echo "no-parser-seed-hardcode: FAIL (sentinels not found; expected BEGIN/END markers in $FILE)"
  exit 1
}

# 去掉注释行 (// /// //! * /* 开头), 再 grep 禁止字面量。
HITS="$(printf '%s\n' "$REGION" \
  | grep -vE '^[[:space:]]*(//|/\*|\*)' \
  | grep -inE "$BANNED" || true)"

if [[ -n "${HITS//[$'\n\t ']/}" ]]; then
  echo "===== no-parser-seed-hardcode: FAIL ====="
  printf '%s\n' "$HITS"
  echo "chargen/mechanics fallback seed 区段出现规则集名字面量。请改用 ruleset-中立 fallback,"
  echo "per-ruleset 细节交给 LLM 编译产物 (derived_formula_pack / sheet_template)。"
  exit 1
fi

echo "no-parser-seed-hardcode: OK (0 hits)"
exit 0
