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
# 含未加引号的 canonical id (dnd5e/brp_orc/triangle_agency/the_vault): 加引号的
# "dnd" 等不含 "dnd5e" 子串, 会漏掉 ruleset_id == "dnd5e" 之类; canonical id
# 足够特异不致误报。cyberpunk_red/call_of_cthulhu_7e/sword_world_2_5 已被
# cyberpunk/cthulhu/sword_world 子串覆盖。
BANNED='call_of_cthulhu|cyberpunk|cthulhu|sword_world|剑世界|homecoming|nyarlathotep|scav_boss|athena_drone|dnd5e|brp_orc|triangle_agency|the_vault|"coc"|"brp"|"dnd"|"d&d"|"5e"|"triangle"|"fate"|"masks"|"vault"'

# 行级白名单: 命中行若含下列子串则豁免 (override 加载键 / env-var 引用 /
# binary-embedded config 加载: include_str! 的 embedded_config/* 路径及其 match
# 臂 — 合法的"override 数据加载键"类; JSON 文件本身非 .rs 不被扫描)。
ALLOWLIST='read_kernel_override_file|load_dir|TRPG_RULESET_ADVICE_DIR|TRPG_DATA_DIR|embedded_config|include_str'

REPORT="$(
python3 - "$REPO" "$BANNED" "$ALLOWLIST" "${ENGINE_CRATES[@]}" <<'PY'
import os, re, sys, glob

repo      = sys.argv[1]
banned    = re.compile(sys.argv[2], re.IGNORECASE)
allowlist = re.compile(sys.argv[3])
crates    = sys.argv[4:]

# 分支谓词正则: 命中行若含 dot-contains-paren / eq-quote / neq-quote 则视为带分支
# 判定 [非纯数据加载], 不予白名单豁免。用 chr 拼装以免脚本里出现孤立的括号或引号
# 字符 [旧版 bash 3.2 在命令替换里会被这些字符干扰]。
_OP = chr(40)   # open paren
_DQ = chr(34)   # double quote
PREDICATE_RE = '\\.contains\\' + _OP + '|==\\s*' + _DQ + '|!=\\s*' + _DQ

def _sanitize_for_braces(line):
    """去掉 // 行注释 + 字符串/字符字面量内容, 防止其中的 {} 干扰 brace 计数。"""
    line = re.sub(r'"(\\.|[^"\\])*"', '""', line)
    line = re.sub(r"'(\\.|[^'\\])'", "''", line)
    line = re.sub(r'//.*', '', line)
    return line

def strip_test_modules(lines):
    """剥离内联 #[cfg(test)] {...} 块 (brace-depth 感知, 字面量/注释里的 {} 不计), 注释行返回 None。
    返回 [(lineno, text_or_None)]。"""
    out, i, n = [], 0, len(lines)
    while i < n:
        s = lines[i].strip()
        if re.match(r'#\[cfg\(test\)\]', s):
            # 找到 attr 后的首个 { , 计深度直到归零 (brace 计数前先消毒该行)。
            depth, started, j = 0, False, i
            while j < n:
                clean = _sanitize_for_braces(lines[j])
                # 无花括号声明 (如 `#[cfg(test)] mod foo;`): 进 block 前先遇 ; 即只跳该声明,
                # 不再 brace-scan 到 EOF 吞掉后续生产代码 (修潜在假阴)。
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
        # *_tests.rs = 纯测试文件 (经 #[cfg(test)] #[path=...] mod 引入), 整体豁免。
        if f.endswith('_tests.rs'):
            continue
        with open(f, encoding='utf-8') as fh:
            lines = fh.readlines()
        for ln, txt in strip_test_modules(lines):
            # 白名单仅豁免纯数据加载行: 若该行还含分支谓词 [dot-contains-open / eq-quote /
            # neq-quote] 比对 banned 字面量, 则不予豁免, 防止 load_dir 等出现在硬编码分支里被放过。
            has_predicate = re.search(PREDICATE_RE, txt)
            exempt = bool(allowlist.search(txt)) and not has_predicate
            if banned.search(txt) and not exempt:
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
