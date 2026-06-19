#!/usr/bin/env bash
# 层化迁移护栏 (P0-5 → P7.4 加固): 表示层（Narrator / Director）路径不得直接出现
# 状态变更 / 直写持久层的裸调用。
#
# 设计4 §9.5/§14/§18：Narrator（叙事层）与 Director（调度层）只读不写——commit 边界
# 归 runtime-owned typed services / 事件账本。本守卫**预置/加固**该禁令。
#
# ── 诚实定位（不得夸大）─────────────────────────────────────────────────────
# 这是 **review-time / CI 的 grep 守卫，不是编译期物理隔离**。crate 们仍依赖
# trpg-db，故模块在物理上仍能 direct-import；本 grep 只在『必须保持纯净的层路径』上
# 拦截**已知的禁止模式**。它不能证明层之间没有任何耦合，只能在 PR review / CI 时把
# 明确非法的裸调用照红。完整编译期隔离（拆 crate / sealed trait）留后续阶段。
# ───────────────────────────────────────────────────────────────────────────
#
# 两档扫描范围 + 两档禁令集：
#
# A) 窄禁令（沿用 P0 基线，扫整个 trpg-gm/src）——明确的状态写裸调用：
#      apply_damage(  apply_effect_roll(  insert_knowledge_edge(  KnowledgeEdge {
#
# B) 宽禁令（P7.4 新增，仅扫 Narrator / Director **表示层路径**）——在 A 之上再加：
#      trpg_db::Db        : 表示层直接握持持久层句柄（应经 WorldPort / runtime 注入）
#      reveal_fact        : 表示层直接揭示 fact（应经 runtime commit primitive）
#      append_domain_event: 表示层直接写领域事件账本（应经 runtime typed service）
#      apply_direct_effect: 表示层直接施加效果（应经 typed service 裁定）
#    表示层路径精确界定为：
#      Narrator = crates/trpg-gm/src/packet.rs（整文件）
#               + turn_loop.rs 内 `run_narrator` / `run_narrator_phase` 函数体
#      Director = crates/trpg-director/src 整 crate（*_tests.rs 除外）
#    **刻意不扫 WorldPort / runtime / 其余 turn_loop 主体**——那些层合法持有 Db、合法
#    调 reveal_fact（如 turn_loop.rs 的 commit primitive `engine.reveal_fact`）。把宽禁令
#    套到整文件会误伤合法 runtime 代码，故按路径 / 函数体精确收窄。
#
# 命中 -> 退出码 1; 干净 -> 退出码 0。
#
# 基线（cdf54b2）：A 与 B 两档均 0 命中（已 grep 验证）——packet.rs / director crate /
# narrator 函数体均无上述裸调用。本脚本是层拆分后『表示层无状态工具』的预置/加固门。
#
# 白名单：注释行 + #[cfg(test)] 内联块 + *_tests.rs（沿用 brace-depth 感知剥离，避免
# 测试 / 文档 mention 误报）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "${SCRIPT_DIR}/.." && pwd)"

# A 档窄禁令扫描 crate（整 trpg-gm/src）。
NARROW_CRATES=(trpg-gm)

# A 档：明确非法的状态写裸调用（正则）。
NARROW_BANNED='\bapply_damage\s*\(|\bapply_effect_roll\s*\(|\binsert_knowledge_edge\s*\(|\bKnowledgeEdge\s*\{'

# B 档：表示层额外禁令（在 A 之上叠加）。
WIDE_EXTRA='\btrpg_db::Db\b|\breveal_fact\b|\bappend_domain_event\b|\bapply_direct_effect\b'

# B 档 Director 路径：整 crate（src 下 *.rs，*_tests.rs 除外）。
DIRECTOR_SRC="crates/trpg-director/src"

# B 档 Narrator 路径：packet.rs 整文件 + turn_loop.rs 的 narrator 函数体。
NARRATOR_PACKET="crates/trpg-gm/src/packet.rs"
NARRATOR_TURNLOOP="crates/trpg-gm/src/turn_loop.rs"
# turn_loop.rs 内被视作 Narrator 表示层的函数（按签名提取函数体，brace-depth 感知；
# **不**按行号硬编码，避免文件漂移后失准）。
NARRATOR_FNS='run_narrator_phase|run_narrator'

REPORT="$(
python3 - "$REPO" "$NARROW_BANNED" "$WIDE_EXTRA" "$DIRECTOR_SRC" \
  "$NARRATOR_PACKET" "$NARRATOR_TURNLOOP" "$NARRATOR_FNS" "${NARROW_CRATES[@]}" <<'PY'
import os, re, sys, glob

repo          = sys.argv[1]
narrow        = re.compile(sys.argv[2])
director_src  = sys.argv[4]
narr_packet   = sys.argv[5]
narr_turnloop = sys.argv[6]
narr_fns_re   = re.compile(r'\bfn\s+(?:' + sys.argv[7] + r')\b')
narrow_crates = sys.argv[8:]

# B 档表示层完整禁令 = 窄禁令 | 宽追加。
wide = re.compile(sys.argv[2] + '|' + sys.argv[3])

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

def extract_fn_bodies(lines, fn_sig_re):
    """按签名定位函数 → brace-depth 感知截取其 {...} 函数体。
    返回 set(lineno)（1-based）覆盖函数体内行（含签名行至闭合 } ）。"""
    covered = set()
    n = len(lines)
    i = 0
    while i < n:
        if fn_sig_re.search(_sanitize_for_braces(lines[i])):
            # 从签名行起，找到第一个 '{' 进入函数体，再按 depth 截到闭合。
            depth, started, j = 0, False, i
            while j < n:
                clean = _sanitize_for_braces(lines[j])
                depth += clean.count('{') - clean.count('}')
                if '{' in clean:
                    started = True
                covered.add(j + 1)
                j += 1
                if started and depth <= 0:
                    break
            i = j
            continue
        i += 1
    return covered

def read_lines(path):
    with open(path, encoding='utf-8') as fh:
        return fh.readlines()

hits = []

def scan(rel_path, kept_lines, pattern, tag):
    """kept_lines: [(lineno, text)] 已剥离测试/注释。命中 pattern → 记一条。"""
    for ln, txt in kept_lines:
        code = re.sub(r'//.*', '', txt)
        if pattern.search(code):
            hits.append(f"[{tag}] {rel_path}:{ln}: {txt.strip()[:140]}")

# ── A 档：窄禁令扫整 trpg-gm/src ──────────────────────────────────────────
for crate in narrow_crates:
    src = os.path.join(repo, 'crates', crate, 'src')
    if not os.path.isdir(src):
        continue
    for f in sorted(glob.glob(src + '/**/*.rs', recursive=True)):
        if f.endswith('_tests.rs'):
            continue
        kept = strip_test_modules(read_lines(f))
        scan(os.path.relpath(f, repo), kept, narrow, 'BOUNDARY')

# ── B 档：Director 整 crate（*_tests.rs 除外）= 表示层完整禁令 ────────────
dsrc = os.path.join(repo, director_src)
if os.path.isdir(dsrc):
    for f in sorted(glob.glob(dsrc + '/**/*.rs', recursive=True)):
        if f.endswith('_tests.rs'):
            continue
        kept = strip_test_modules(read_lines(f))
        scan(os.path.relpath(f, repo), kept, wide, 'DIRECTOR')

# ── B 档：Narrator packet.rs 整文件 = 表示层完整禁令 ──────────────────────
pf = os.path.join(repo, narr_packet)
if os.path.isfile(pf):
    kept = strip_test_modules(read_lines(pf))
    scan(narr_packet, kept, wide, 'NARRATOR')

# ── B 档：Narrator 函数体（turn_loop.rs 内 run_narrator[_phase]）──────────
tl = os.path.join(repo, narr_turnloop)
if os.path.isfile(tl):
    lines = read_lines(tl)
    body_lines = extract_fn_bodies(lines, narr_fns_re)
    kept = strip_test_modules(lines)
    # 只保留落在 narrator 函数体内的行。
    kept = [(ln, txt) for (ln, txt) in kept if ln in body_lines]
    scan(narr_turnloop, kept, wide, 'NARRATOR')

print('\n'.join(hits))
PY
)"

if [[ -n "${REPORT//[$'\n\t ']/}" ]]; then
  echo "===== no-layer-boundary-violation: FAIL ====="
  printf '%s\n' "$REPORT"
  echo "表示层（Narrator / Director）出现状态变更 / 直写持久层裸调用。"
  echo "改经 runtime-owned typed service / 事件账本 / WorldPort（设计4 §9.5/§14/§18）。"
  HITS=$(printf '%s\n' "$REPORT" | grep -c '^\[' || true)
  echo "no-layer-boundary-violation: FAIL (${HITS} hits)"
  exit 1
fi

echo "no-layer-boundary-violation: OK (0 hits)"
exit 0
