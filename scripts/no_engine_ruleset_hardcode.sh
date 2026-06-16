#!/usr/bin/env bash
# CI 守卫 (P0-2): 引擎 crate 的 src/ 不得出现规则集 / 模组名字面量。
#   命中 -> 退出码 1; 干净 -> 退出码 0。
#
# 白名单 (不计入命中):
#   1. #[cfg(test)] 内联块 (brace-depth 感知剥离) 及 *_tests.rs 测试文件
#      (本仓约定: 全部经 #[cfg(test)] #[path=...] mod 引入, 纯测试)。
#   2. 注释行 (// /// //! /* * 开头)。
#   3. override 数据加载键 / env-var 引用所在行 (ALLOWLIST_FN_PATTERNS)。
# 设计哲学: 引擎零硬编码; 规则集 / 模组差异一律下沉到 kernel / module 数据层。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "${SCRIPT_DIR}/.." && pwd)"

ENGINE_CRATES=(
  trpg-runtime trpg-gm trpg-combat trpg-referee trpg-director
  trpg-object trpg-mechanics trpg-material trpg-contest
  trpg-orchestrator trpg-semantics trpg-interaction
)

# 禁止字面量 (规则集名 / 模组名 / 模组专属 NPC id)。大小写不敏感子串。
BANNED='call_of_cthulhu|cyberpunk|cthulhu|sword_world|剑世界|homecoming|nyarlathotep|scav_boss|athena_drone|"coc"|"brp"|"dnd"|"d&d"|"5e"|"triangle"|"fate"|"masks"|"vault"'

# 行级白名单: 命中行若含下列子串则豁免 (override 加载键 / env-var 引用)。
ALLOWLIST='read_kernel_override_file|load_dir|TRPG_RULESET_ADVICE_DIR|TRPG_DATA_DIR'

REPORT="$(
python3 - "$REPO" "$BANNED" "$ALLOWLIST" "${ENGINE_CRATES[@]}" <<'PY'
import os, re, sys, glob

repo      = sys.argv[1]
banned    = re.compile(sys.argv[2], re.IGNORECASE)
allowlist = re.compile(sys.argv[3])
crates    = sys.argv[4:]

def strip_test_modules(lines):
    """剥离内联 #[cfg(test)] {...} 块 (brace-depth 感知), 注释行返回 None。
    返回 [(lineno, text_or_None)]。"""
    out, i, n = [], 0, len(lines)
    while i < n:
        s = lines[i].strip()
        if re.match(r'#\[cfg\(test\)\]', s):
            # 找到 attr 后的首个 { , 计深度直到归零。
            depth, started, j = 0, False, i
            while j < n:
                depth += lines[j].count('{') - lines[j].count('}')
                if '{' in lines[j]:
                    started = True
                j += 1
                if started and depth <= 0:
                    break
            i = j
            continue
        if s.startswith('//') or s.startswith('/*') or s.startswith('*'):
            i += 1
            continue
        out.append((i + 1, lines[i].rstrip('\n')))
        i += 1
    return out

hits = []
for crate in crates:
    src = os.path.join(repo, 'crates', crate, 'src')
    if not os.path.isdir(src):
        continue
    for f in sorted(glob.glob(src + '/**/*.rs', recursive=True)):
        # *_tests.rs = 纯测试文件 (经 #[cfg(test)] #[path=...] mod 引入), 整体豁免。
        if f.endswith('_tests.rs'):
            continue
        with open(f, encoding='utf-8') as fh:
            lines = fh.readlines()
        for ln, txt in strip_test_modules(lines):
            if banned.search(txt) and not allowlist.search(txt):
                rel = os.path.relpath(f, repo)
                hits.append(f"[BANNED] {rel}:{ln}: {txt.strip()[:140]}")

print('\n'.join(hits))
PY
)"

if [[ -n "${REPORT//[$'\n\t ']/}" ]]; then
  echo "===== no-engine-ruleset-hardcode: FAIL ====="
  printf '%s\n' "$REPORT"
  echo "引擎 crate src 含规则集/模组名字面量。迁入 kernel/module 数据层, 或确认应入白名单。"
  exit 1
fi

echo "no-engine-ruleset-hardcode: OK (0 hits)"
exit 0
