# 行为层 ↔ 公式层 按 id 对齐设计（§10.5 参数语义/行为层）

- 日期：2026-06-17
- 分支：`worktree-claude+behavior-track-alignment`（基于 `main` = `24b8b40`）
- 参考：`docs/超夜作业报告_2026-06-09.md:79`（§10.5 下一步定义）
- 状态：设计已与架构师口头认可，待 spec review

## 1. 背景与问题

建卡公式编译器（`chargen_compile.rs` 第二遍）已经把 prose 公式语义编译成机器
`derived_values`（**算术/公式层**，带 round-trip 校验）。但"一个参数*是什么*、play
里*怎么变*"的**行为层**（伤害落账、疯狂/重伤阈值、恢复）目前没有与公式层成对治理：

| 层 | 产出者 | 现状 |
|---|---|---|
| 公式层 `derived_values` | `chargen_compile.rs`（聚焦第二遍 + round-trip 校验） | 健康。CoC 有 `hp_max`/`mp_max`/`sanity` 的 `expr` |
| 行为层 `resource_tracks.on_outcome` / `.thresholds` | **首遍 reader**（抽取质量不稳） | 缺口。`data/parsed/rules/*.override.json` 里**根本没有 `resource_tracks`**；行为只活在 DB，靠一堆 `_fix_*.sql` 事后补，**重 parse 会丢** |
| 消费层 runtime | `apply_outcome_resource_tracks` + `detect_crossings` | 已通用：任何良构 track 的 on_outcome 落账与阈值穿越都能消费 |

### 两个具体 bug

1. **id 错位（播种落空）**：chargen 把 HP 上限存进 `sheet_json.resources["hp_max"]`
   （由 derived value 的 `id` 决定，见 `trpg-runtime/src/chargen.rs:241` 的
   `resource_max -> "resources"` 路由）。但 CoC kernel 的 HP track `id` 是
   `hit_points`。`trpg_model::match_seed`（`trpg-model/src/lib.rs:1516`）按 **track id**
   去 `resources[track_id]` 查种子 → 查 `resources["hit_points"]` 落空 → 回退 kernel
   静态 `initial`/`max` → **角色派生的 HP 上限没生效**。`sanity` 因两边同名才碰巧对上。
   `_max` 后缀约定（`hp_max` 公式 ↔ `hp`/`hit_points` track）从未被系统化。

2. **孤儿无人守**：有公式 max 却没有带行为的 track（或反过来，有 track 没有公式 max）
   没有任何校验、没有 fail-closed。

### 结论

真正缺的是**"对齐"这一环**：一个以公式层为脊柱、把行为层按**同 id**对齐 + 校验 +
fail-closed 的环节。消费层基本现成，只需修复种子桥并验证新对齐 track 走得通。

## 2. 目标与非目标

### 目标
- 每个资源派生值（`role` ∈ {`resource`, `resource_max`}）都有一条按 id 对齐的行为 track。
- 每条行为 track 都有对应的公式 max（无孤儿双向）。
- 错位/缺口被**检测、fail-closed、可测**（绝不臆造行为）。
- 行为配置**可复现**（落进 override 数据，clean checkout 重 parse 能重现），不只在 DB。
- runtime 消费对齐后的 track（种子桥修复 + 阈值穿越验证）。
- **零 per-ruleset 硬编码**；现有 **CoC SAN / D&D HP 行为不变**（回归基线）。

### 非目标（本期不做）
- 不重写首遍 reader 的 resource_tracks 抽取。
- 不把所有规则集都跑对齐（仅 CoC + D&D，见 §7 覆盖范围）；其余规则集对齐留后续。
- 不做 wound_state 之类"派生自 HP 的状态"重构。
- 不动成长层（XP/level，`sheet_json.tracks`）。

## 3. 架构总览（4 个单元，各 ≤400 行）

```
chargen_compile（公式层，已存在）
        │  derived_values（含 role=resource/resource_max 的 id）
        ▼
behavior_align（本期新增，聚焦第二遍之后跑）
   ① 对齐核心 audit_alignment（纯函数，脊柱）
   ② 行为补缺 fill（override > 现有抽取 > LLM best-effort）
   ③ 校验 + fail-closed（复用 apply_on_outcome_ref_guard）
        │  对齐 + 补好的 resource_tracks（带 derived_from 链接）
        ▼
merge_resource_tracks（trpg-db，已存在；按 id 增量、override 永远赢）
        ▼
runtime 消费（apply_outcome_resource_tracks + detect_crossings，已通用）
   ④ match_seed 增量读 derived_from（修种子桥）
```

## 4. 详细设计

### 4.1 单元① 确定性对齐核心（纯函数，脊柱）

新文件 `crates/trpg-rule-agent/src/reader/behavior_align.rs`。

```rust
pub struct AlignmentReport {
    pub aligned: Vec<Aligned>,          // (derived_id, track_id) 成对
    pub orphan_formulas: Vec<String>,   // 有公式 max 无行为 track
    pub orphan_tracks: Vec<String>,     // 有 track 无公式 max
    pub behavior_gaps: Vec<String>,     // track 在但缺 on_outcome/thresholds
}

/// 纯函数：既在 parse 时记 gap，也被测试直接断言。
pub fn audit_alignment(
    derived_values: &[Value],
    resource_tracks: &[Value],
) -> AlignmentReport;
```

**id 调和（零硬编码）**——对齐用**显式链接字段** `derived_from`（track 上写
`"derived_from": "hp_max"`），不靠脆弱的字符串猜测。匹配优先级：

1. track 已有 `derived_from` → 直接用。
2. HP 走已有的**通用语义解析** `trpg_model::hp_resource_track_id`（非 per-ruleset，
   按 `kind=="health"` / id 含 `hit_point` / id == `hp` 命中）→ 把对应的
   `*_max`（role=resource_max）派生值链接上去。
3. 其余：base-id 匹配——派生值 id 去掉尾部 `_max` 后，与 track id 大小写不敏感比较
   （`mp_max`→`mp`；`sanity`→`sanity`）。

匹配成功后在 track 上落 `derived_from`，让 `hp_max`↔`hit_points` 这种桥被**数据显式记录**。

### 4.2 单元② 行为补缺（按认可的优先级）

对"有资源公式但 track 缺 on_outcome/thresholds"的 track，按序补：

1. **override 数据**（`*.rule_kernel.override.json` 里手写的 track 行为）— 主路径、确定、可 review。
2. **现有首遍抽取**（kernel 里已有的 on_outcome/thresholds）— 原样保留，绝不覆盖。
3. **LLM best-effort 抽取**（兜底）— 仿 `mechanics_compile` 的聚焦 submit：仅对缺行为的
   资源参数、从 prose 抽
   - `on_outcome`：op（subtract/add）+ amount（`=damage` / `1d6` / `default_amount`）
     + trigger（always/on_failure）+ check_match（如 `sanity|san roll`、`damage|attack`）。
   - `thresholds`：`{at:0, direction:"at_or_below", consequence:"dying"}`（濒死/0 线）、
     `{loss_in_one_go:N, consequence:"..."}`（单次损失≥N → 疯狂/重伤）。
   经 `apply_on_outcome_ref_guard` 校验 `=field` 合法（`outcome_fields::AMOUNT_RESOLVABLE`）。

**fail-closed 铁律**：prose / override 都不支持时，**保留对齐 track 但 on_outcome/thresholds 留空**，
记 `behavior_gaps`，**绝不臆造**伤害公式或阈值数字。

### 4.3 单元③ 校验 + fail-closed

- `apply_on_outcome_ref_guard`（`mechanics_finalize.rs`，已存在）继续无条件审计 `=field` 引用，
  非法且无 `default_amount` 兜底则丢弃该规则。
- 对齐守卫断言（§7 T1）把 `AlignmentReport` 的 orphan/gap 变成可测不变量。

### 4.4 单元④ runtime 消费

- **`trpg_model::match_seed` 小改（加性、数据驱动）**：对每条 track，若有 `derived_from`，
  先按 `resources[derived_from]` 查种子（修 `hp_max`→`hit_points` 落空 bug）；查不到再回退
  到现有的 `resources[track_id]` 大小写不敏感查找，再回退 kernel 静态值。**纯加性**：没有
  `derived_from` 的 track 行为完全不变。
- `apply_outcome_resource_tracks` + `detect_crossings`（`trpg-mechanics`）已通用消费 on_outcome
  与阈值穿越 → 只加测试验证新对齐 track 走得通，**不改逻辑**。

### 4.5 接线

`behavior_align::align(...)` 在 chargen 第二遍**之后**调用：
- `crates/trpg-parser/src/staged.rs`（`compile_chargen_formulas` 调用点 :151 之后）。
- `crates/trpg-parser/src/lib.rs`（:582 之后）。
两处都传入已编译的 `template.derived_values` + reader 产出的 `kernel.resource_tracks`，
对齐结果写回 kernel.resource_tracks（落库 + override 合并时按 id 增量）。

## 5. id 对齐规则（`derived_from` 字段）

- **写入方**：`behavior_align`（parse 时在 track 上写 `derived_from`）。
- **读取方**：`match_seed`（runtime 播种）；测试断言（对齐校验）。
- 实现时若发现 kernel/track 已有等价链接字段，复用之，不新造。
- 该字段是**纯加性** serde 字段，向后兼容（老数据无此字段 → 走旧回退路径）。

## 6. 数据改动（CoC + D&D override）

主补缺路径是 override 数据（确定、可 review、可复现）：

- `call_of_cthulhu_7e.rule_kernel.override.json` 增 `resource_tracks`：
  - `sanity`：对齐 + on_outcome（SAN 检定失败减 SAN）+ thresholds（0 线疯狂、单次≥5 临时疯狂）。
    与现有 live 行为对齐，作回归基线之一。
  - `hit_points`：`derived_from:"hp_max"` + on_outcome（伤害落账）+ thresholds（0 濒死、
    单次≥半血重伤 `loss_in_one_go`）+ `magic_points` 同理对齐 `mp_max`。
- `dnd5e.rule_kernel.override.json`：确认 `hit_points` 对齐字段，**行为保持不变**（回归基线）。

所有数值来自 CoC/D&D 经典 prose；page unverified 沿用现有 `source_ref` 习惯。

## 7. 测试与验收

### 覆盖范围
**CoC（sanity + hp + mp）+ D&D（hp）**。硬约束：CoC SAN / D&D HP 行为不变。

### 测试
- **T1 对齐守卫（纯，`behavior_align` 单测）**：CoC + D&D fixture 断言——
  每个资源派生值有对齐行为 track；无孤儿公式 / 无孤儿 track；
  **fail-closed 用例**（构造一个无 prose/override 源的资源 → track 在、on_outcome/thresholds 空、
  进 `behavior_gaps`、不臆造）。
- **T2 阈值穿越 → on_outcome（`trpg-mechanics` 单测/live-ish）**：
  - SAN 单次掉 ≥5 → `detect_crossings` 命中 `loss_in_one_go` 阈值 + 落 due。
  - HP 跌破半血重伤 / 跌到 0 濒死 → 阈值穿越触发。
  复用 `watcher.rs` / `apply_outcome_resource_tracks` 既有 harness。
- **T3 回归**：现有 CoC SAN / D&D HP 测试全绿（`trpg-db/tests/live_kernel_override.rs` 等）。
- **T4 种子桥（`trpg-model` 单测）**：`match_seed` 带 `derived_from` 把 `hit_points` track
  从 `resources["hp_max"]` 播种成功；无 `derived_from` 时行为与现状字节等价。

### 验收清单（对应任务 ACCEPTANCE）
1. ✅ 2–3 规则集中编译参数有匹配 track id + thresholds/on_outcome；测试断言 id 对齐（无孤儿、fail-closed）。
2. ✅ 阈值穿越触发 on_outcome 的单测/live-ish。
3. ✅ 零 per-ruleset 硬编码；CoC SAN / D&D HP 不变。
4. ✅ `cargo build` + `cargo test` 通过；文件 ≤400 行。

## 8. 文件清单与行数预算

| 文件 | 操作 | 预算 |
|---|---|---|
| `crates/trpg-rule-agent/src/reader/behavior_align.rs` | 新增（对齐核心 + audit + 补缺编排 + LLM 兜底 + 单测） | ≤400 |
| `crates/trpg-rule-agent/src/reader/mod.rs` | 导出 | 数行 |
| `crates/trpg-parser/src/staged.rs` + `lib.rs` | 接线 behavior_align | 各数行 |
| `crates/trpg-model/src/lib.rs` | `match_seed` 加 `derived_from` 桥 + 单测 | 小改 |
| `crates/trpg-mechanics/.../*tests*` | T2 阈值穿越测试 | 新增 |
| `data/.../call_of_cthulhu_7e.rule_kernel.override.json` | 增 resource_tracks | 数据 |
| `data/.../dnd5e.rule_kernel.override.json` | 确认对齐字段 | 数据 |

> 若 `behavior_align.rs` 逼近 400 行，把 LLM best-effort 抽取拆到 `behavior_align_llm.rs` 兄弟文件。

## 9. 风险与回滚
- **回归 live 行为**：补缺以 override（确定）为主、LLM 兜底严格门控且只对缺行为 track 生效；
  T3 守住 CoC SAN / D&D HP。
- **种子桥改变 CoC HP 播种**：CoC HP 从静态回退改成派生 max——这是**修复**（且 CoC HP 不在
  "不变"硬约束内，只有 CoC SAN / D&D HP 是）。D&D HP 作基线，T4 验证无 `derived_from` 时字节等价。
- **回滚**：本分支独立，丢弃 worktree 即回滚；数据改动集中在两个 override 文件。

## 10. 范围外 / 后续
- 其余规则集（CPR humanity/luck、SW hp/mp、ORC、Triangle）对齐。
- LLM best-effort 抽取的批量跑批与质量评估。
- wound_state 等"派生状态"从独立 track 改为 HP 阈值计算属性。
