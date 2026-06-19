#!/usr/bin/env bash
# 层化迁移护栏 (P0-5): Narrator 路径不得直接出现状态变更裸调用。
#
# 设计4 §9.5/§14：Narrator（叙事层）只读不写——commit 边界归 runtime-owned typed
# services。本守卫**预置**该禁令：扫 trpg-gm/src，禁明确非法裸调用——
#   - apply_damage(            : 直接施加伤害（应经 typed service / 工具裁定）
#   - apply_effect_roll(       : 直接结算效果掷骰（同上）
#   - insert_knowledge_edge(   : 直写知识账本（应经 PlayerLearnedFact/NpcLearnedFact 事件）
#   - KnowledgeEdge {          : 直接构造 KnowledgeEdge 写结构（绕过事件账本）
# 命中 -> 退出码 1; 干净 -> 退出码 0。
#
# P0 现状：Narrator 尚未拆分，trpg-gm 现无上述裸调用 → **对基线 0 命中**（已 grep 验证）。
# 本脚本是 P1/P2 拆分后『Narrator 无状态工具』的预置门：届时若有人把状态变更裸调用
# 塞进叙事路径，立刻红灯。精确全边界（每个状态写接口）留 P1/P2 Narrator 真拆后细化。
#
# 白名单：注释行 + #[cfg(test)] 内联块 + *_tests.rs（沿用 no_engine_ruleset_hardcode.sh
# 的 brace-depth 感知剥离，避免测试 / 文档 mention 误报）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "${SCRIPT_DIR}/.." && pwd)"

# 扫描范围：当前业务总汇 crate（Narrator 路径将从此处拆出）。
SCAN_CRATES=(trpg-gm)

# 禁止的状态变更裸调用模式（正则；明确非法集，P0 最弱形状）。
BANNED='\bapply_damage\s*\(|\bapply_effect_roll\s*\(|\binsert_knowledge_edge\s*\(|\bKnowledgeEdge\s*\{'

REPORT="$(
python3 - "$REPO" "$BANNED" "${SCAN_CRATES[@]}" <<'PY'
import os, re, sys, glob

repo   = sys.argv[1]
banned = re.compile(sys.argv[2])
crates = sys.argv[3:]

def _sanitize_for_braces(line):
    """去掉 // 行注释 + 字符串/字符字面量内容, 防其中 {} 干扰 brace 计数。"""
    line = re.sub(r'"(\\.|[^"\\])*"', '""', line)
    line = re.sub(r"'(\\.|[^'\\])'", "''", line)
    line = re.sub(r'//.*', '', line)
    return line

def strip_test_modules(lines):
    """剥离内联 #[cfg(test)] {...} 块 (brace-depth 感知) + 注释行。返回 [(lineno, text)]。"""
    out, i, n = [], 0, len(lines)
    while i < n:
        s = lines[i].strip()
        if re.match(r'#\[cfg\(test\)\]', s):
            depth, started, j = 0, False, i
            while j < n:
                clean = _sanitize_for_braces(lines[j])
                if not started and '{' not in clean and ';' in clean:
                    j += 1
                    break
                depth += clean.count('{') - clean.count('}')
                if '{' in clean:
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
        # *_tests.rs = 纯测试文件 (经 #[cfg(test)] #[path] mod 引入), 整体豁免。
        if f.endswith('_tests.rs'):
            continue
        with open(f, encoding='utf-8') as fh:
            lines = fh.readlines()
        for ln, txt in strip_test_modules(lines):
            # 去掉行内 // 注释后再匹配（避免注释里 mention 触发）。
            code = re.sub(r'//.*', '', txt)
            if banned.search(code):
                rel = os.path.relpath(f, repo)
                hits.append(f"[BOUNDARY] {rel}:{ln}: {txt.strip()[:140]}")

print('\n'.join(hits))
PY
)"

if [[ -n "${REPORT//[$'\n\t ']/}" ]]; then
  echo "===== no-layer-boundary-violation: FAIL ====="
  printf '%s\n' "$REPORT"
  echo "Narrator/业务路径出现状态变更裸调用。改经 runtime-owned typed service / 事件账本"
  echo "（PlayerLearnedFact / NpcLearnedFact），叙事层只读不写（设计4 §9.5/§14）。"
  exit 1
fi

echo "no-layer-boundary-violation: OK (0 hits)"
exit 0
