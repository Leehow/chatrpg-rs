# 角色成长 / Track 层设计 (Character Growth — generic Track layer)

> 日期: 2026-06-09 · 状态: 设计已批准, 经对抗评审修订, 待 writing-plans · 主线: v1.20-formula
> 前置: §4 chargen-spec (`2026-06-08-chargen-formula-compiler-design.md`) +
> live 联动 (`2026-06-08-chargen-formula-eval-design.md`, §10.1, 已建)
> 本稿已据一轮对抗评审修订: 解决了"存储分叉"与"求值器上下文形状"两个 BLOCKER (见 §4.2 / §4.3 / §11)。

## 1. 背景与缺口

不同规则的"角色成长 / 进阶"差异极大 (D&D 职业等级且可兼职; CoC 无等级、技能用进废退;
Cyberpunk 点数购买; Fate 字母评级 + 存活换点)。当前引擎**完全没有** level / class-level / XP /
advancement / 多职业的概念 (数据模型勘查确认: 全 `crates/` 仅一个无关的 `GuidanceLevel`, sheet 是
扁平当前值快照, "class" 只作 categorical 输入字段/lookup 键)。

目标: **模块化、数据驱动、零 per-ruleset 硬编码**的成长表示层, 跨异构规则统一, 并补上"在 play 里改
基础值"这半边 (现状: live 重算这侧已建且正确, 但**没有任何流程能在跑团中改基础值** —— live 联动勘查
原话 "the base-mutation side is the missing half")。

## 2. 成长原型分类 (跨 7 套规则)

| 原型 | 规则 | 成长单位 | 触发 | 什么按它算 |
|---|---|---|---|---|
| A 多轨·等级制 | D&D 5e, 剑世界2.5 | 每职业一条等级 (向量, 多职业=多条) | D&D: XP 共享阈值表→**总和**升级; SW: EXP **预算**按职业逐级买 | D&D 熟练读总和、HP 累加、法术位读加权混合; SW 读**最高**(冒险者级)或对应职业级 |
| B 单技能·用进废退 | CoC 7e, ORC | 单技能/属性 +几点 | 成功**使用**→打勾→休整 roll-over-current (+1d10/+1d6) | 无任何按等级的值 |
| C 点数购买·职阶 | Cyberpunk RED | 单技能级 / 单一职阶(1-10) | 每场给 IP→花费买, 成本随下一级×系数 | 先攻 += 职阶; 技能点买 |
| D 叙事·解锁 | Triangle Agency | 勾轨道格→解锁能力/头衔 | 有限 30 Time, 零和 | 无数字等级; QA(≤9) 门控 |
| E 存活门控点池 + 序数评级 | Fate Atrous Grail | 御主: FP 点池; 英灵: 字母评级(E~A++/EX) | 御主: 存活+5FP, 幕间花 10FP 重置属性; 英灵: 点买建卡(召唤的固定传说, 无持久成长) | 御主衍生属性=§4 公式; 英灵衍生**按职阶查表** + 属性级**查表成数** + 技能效果**按级查表** |

关键观察: 抽干净后**全是同一件事** —— *有若干会变的"量", 某些派生值是其函数*。引擎**派生重算这半边
已建** (§4 `expr`+`lookup`+`recompute:"live"` + live 联动); 缺的是 (a) "可成长量"一等概念, (b) 多轨/
嵌套键打通, (c) "在 play 里改值"这半边, (d) 序数(字母)评级这种非数值的值类型。

## 3. 设计决策 (已与用户逐条敲定)

1. **A0 — 引擎只"表示 + 自动重算", 不"执行"进阶规则。** track 表示可成长量、派生随其变化自动重算;
   "怎么涨"(XP→级、IP 成本、tick 掷骰、多职业前提) 由 GM/玩家驱动或叙事, 引擎不强制。理由: 对抗
   已验证两次的 reader/compiler 跑批变异; track 派生本身仍数据驱动, 未来可平滑升级到 A1。是 scoping, 非违背。
2. **通用多轨 from day one。** 角色带一组通用 track, 多职业天生=多条等级 track, 无任何特判。
3. **通用"改值"动作。** 一个通用 runtime 原语 (set/add 某 track 或 base), 玩家命令 + GM-agent
   **类型化工具**都走它; 写存储后触发已建 live 重算; 引擎只搬值不判规则。
4. **派生值来源 = 混合, 永不回退 + 人工 override 优先。** track-keyed 派生是 §4 记录; 编译器**可**抽,
   但重解析**永不覆盖/丢弃**已解析(source_backed)的记录; per-ruleset 声明式 override 优先。
   确定性优先级: **override > 已解析编译 > prose**。是对编译器变异的直接防御。

## 4. 核心抽象

### 4.1 Track = 一个命名的可成长量
- 字段: `{ value, kind, category?, ladder?, source_ref? }`。
  - `value`: **数字** 或 **序数 label** (见 4.4)。
  - `kind`: 自由语义标签 (class_level / magical_class_level / skill / rank / pool / counter / narrative…)
    —— **数据**, 非 Rust 枚举。**必须够细**: 聚合按 kind 过滤 (如剑世界 MP 只 sum *魔法*职业级 → 魔法职业
    track 带 `kind:"magical_class_level"`, 普通职业 `kind:"class_level"`)。
  - `category`: 次级分类 (major/minor 等), 聚合亦可按它过滤。
  - `ladder`: 序数 label 时指向其有序阶梯 (见 4.4)。
- 多职业: 多条 class_level track, 零特判。

### 4.2 存储: 落在 live 重算读得到的 sheet_json 侧, 不新增物理存储 (BLOCKER 已解)
**评审纠正**: track ≠ 现有 resource track 的复用 —— 因为 `apply_outcome_resource_tracks` 写的是
`generic_parameter_states.resources.{id}.current`, 而 live 重算 (`recompute_live_derived` /
`build_chargen_inputs`) 只读 `runtime_actor_parameters.sheet_json` 的 `stats/skills` 桶 + 顶层标量,
**两者是两个物理存储, 不重叠**。若 track 落在 `generic_parameter_states`, live 重算**永远看不到它** →
"改 track → 派生重算"这个核心承诺**静默失效**。

**决策 (本期切定, 非留待实现)**: track 当前值落 **`sheet_json`** (live 重算所读的同一存储), 作为一个
`tracks` 子桶 (与 stats/skills 并列)。改值原语写 `sheet_json.tracks` 并触发 `refresh_actor_live_derived`,
**不经** `generic_parameter_states` / `apply_outcome_resource_tracks`。
- **reuse-before-add 如何满足**: 复用 **sheet_json 存储 + live 重算路径** (无新物理存储、不碰 generic
  parameter 那套), 只在 sheet_json 内**新增一个 `tracks` 子桶** —— 因为现有 stats/skills/resources 桶
  语义都不容"职业等级/点池" (硬塞是语义污染)。这是必要的最小新增, 不是第三套并行存储。
- **已知债不恶化**: "两套资源存储分叉" (sheet_json ↔ generic_parameter_states) 是既有债; 成长层**明确选
  sheet_json 侧**、不新开第三套; 两套存储的收敛是更大的独立工作, 本期范围外, 但本期不加剧它。

### 4.3 聚合原语 (多职业关键; 求值器上下文形状已解, BLOCKER 已解)
trpg-formula 新增**通用** `sum_tracks(kind)` / `max_tracks(kind)` —— kind 由数据传入, 对一组 track 聚合
(事先不知角色有几个职业, 静态 `{{a}}+{{b}}` 枚举不出)。`count_tracks` **不做** (无 in-scope 消费者, YAGNI)。
- D&D 总等级 = `sum_tracks("class_level")`; 熟练 = `lookup(prof_by_level, {{total_level}})`。
- 剑世界 冒险者级 = `max_tracks("class_level")`; HP = `{{vitality}} + max_tracks("class_level")*3`;
  MP = `{{spirit}} + sum_tracks("magical_class_level")*3` (按细 kind 过滤的子集和)。
- 全是 `recompute:"live"` 派生, track 一变即自动重算。

**评审纠正的实现要点** (§8 不再用一行带过):
1. **`build_chargen_inputs` 现在不转发 track 子树** (只转发 stats/skills/player/顶层标量), 故 `{{track.*}}`
   今天到达即 unresolved→provisional。本期改: **转发 `tracks` 子树**。
2. **求值器上下文是 `HashMap<String,Num>` 扁平标量**, `ctx_from_inputs` 递归展平会把 `track.fighter =
   {value:4,kind:"class_level"}` 拆成两条无关项 `track.fighter.value→4` / `track.fighter.kind→Str(...)`,
   **kind 分组被打散**, `sum_tracks` 无物可遍历。解法: **把结构化 tracks 与 ladder 数据作为一个
   `&Value` 线进 `Parser`** (与现有 `tables: &Value` 同法多加一个 `tracks: &Value`); 聚合函数读它按 kind
   过滤求和/取最值。`expr::eval` 签名相应加一参。
3. **函数调用派发把非 `lookup` 实参一律 coerce 成 f64, 遇字符串报错** (expr.rs `call()`)。`sum_tracks/
   max_tracks` 需像 `lookup` 一样**单独特例分支** (接受 bare-ident/字符串 kind 实参 + 访问结构化 tracks),
   不是 `min/max` 那种数值 reduce。
- **加权混合** (D&D caster level) A0 下逐规则 authored; 注意 `lookup` 表名必须是**字面 ident** (不能用
  `{{class}}` 选表), 故"按职业选法术位表"需**每职业一条 authored 记录**, 非一条参数化公式 (见 §7 范围外)。

### 4.4 序数刻度 (ordinal ladder) —— Fate 字母评级的统一抽象
字母级 E/D/C/B/A/EX (+ 中间级 D-/D+/…/A++) **不是纯文本, 有三层说法**, 但 100% 可抽象:
1. **级→数值**: 评级表 (A→14-15、EX→18…), 进检定/衍生。
2. **级当按级查表、每效果各自一张表**: 技能效果 `[1/1/2/2/3]` 按级取, 同一级对不同效果**不同数**
   (狂化 受伤 `[5/4/4/3/3]` 随级**递减**) —— 级是**位置**, 不是全局数字。
3. **+/- 是有序阶梯上升降一级的真机制** (24h CD): "升一级"=在 E<E+<D<…<A++<EX 上挪一格。

落地 = **label + 声明的有序阶梯(data) + 按上下文的 label→值查表**, 不是扁平 label→单一数字:

| 作用 | 落地 | 本期是否做 |
|---|---|---|
| 级→数值 (检定/衍生) | `lookup(rank_value, {{力量级}})` → 数 → 再算。**复用已建 `lookup_cat`/`Num::Str`** | ✅ 做 |
| 每效果按级缩放 | 每效果一张**按 label 键**的表 `lookup(本效果_scale, {{技能级}})`, **枚举相关 rung** (递减亦可); 同一原语不同数据表 → "A 对不同效果不同"天然成立 | ✅ 做 (数据面大, 受 §3.4 永不回退/override 护) |
| 级间比较 / +/- 升降级 | 阶梯声明为有序 list (data); "升一级"=取下一个元素; 需引擎"阶梯挪格/比较" op | ❌ **本期不做** (见下) |

- **评审纠正**: `success_bands` (`success_tier_for`) 是**掷骰结果分类器**, 有 `rank:i64` 但**无"下一级
  label"、无 step、无 label↔label 比较** —— 序数**值查表**复用 `lookup_cat` (真); 序数**排序(step/比较)**
  是全新、`success_bands` **不提供**。故 §6 不再宣称"reuse success_bands 做排序"; 只说它是"有序带作数据"的
  相关先例。
- **本期不建 step/compare op** (YAGNI/反投机泛化): 其唯一真实消费者 Fate `+/-` 临时 bump 属 **play-time
  能力效果** (24h CD), 本就 defer 到能力层 (§9)。**有序阶梯仍声明为 data** (供未来 +/- 与比较), 但 op 与
  其消费者一起 defer。本期序数刻度只做"label→值查表"(Fate 御主/英灵衍生与效果都用得上)。
- **fail-closed**: 字母 label (`Num::Str`) **不能直接算术**, 必先 lookup 成数 —— 正是引擎现行为。

### 4.5 改值原语 (缺失的另一半)
通用 `apply_track_change(actor, target, op: set|add, amount, kind?, category?)`:
- 写 §4.2 的 `sheet_json.tracks` (首次 set 即创建该 track), 然后调 `refresh_actor_live_derived` 触发重算。
- `target` 可为 track, 也可为 base 属性 (剑世界每场掷骰涨属性走同一原语, op 作用于 `stats`)。
- 两入口: **CLI 命令** + **GM-agent 类型化工具** (GM 语义决定后调用)。
- **评审护栏**: 此原语是**纯值操作**, 由类型化工具/命令显式驱动, **不继承** `apply_outcome_resource_tracks`
  的 `check_match` 子串关键词门 (那是理念要退役的反模式)。
- 引擎只搬值, 不判 XP 够不够 / IP 成本 / 多职业前提 (A0)。

## 5. 各规则映射

| 规则 | track | 关键派生 / 动作 |
|---|---|---|
| D&D 5e | 每职业 class_level | 总级=`sum_tracks`; 熟练=`lookup(by_level)`; **HP=base/`once` 累加器** (升级时 GM 改值原语累加; **绝不作 live 派生**, 见 §6.7) |
| 剑世界2.5 | class_level(major/minor) + magical_class_level | 冒险者级=`max_tracks`; HP=活力+级×3; MP=精神+`sum_tracks(magical)`×3; 属性=每场改值原语涨 |
| CoC / ORC | 技能即 track | 无等级派生; 成长=dev 阶段 GM 改值原语 +1d10/+1d6 |
| Cyberpunk | ip_pool + role_rank | 先攻=base+`{{track.role.value}}` (live 派生); 技能/职阶=改值原语花点买; 成本/多职业 Rank-4 门=GM |
| Triangle | time + competency… | 叙事解锁=GM 改值原语 + 叙事; 无数字等级 |
| Fate 御主 | FP 点池 + 令咒 counter | 衍生属性=§4 公式; 存活+5FP/幕间-10FP 重置属性=改值原语 |
| Fate 英灵 | 职阶(choice) + 字母级属性 | 衍生**按职阶查表**(同 D&D class-keyed); 属性级=序数刻度→查表成数; 技能效果=按 label 键的 per-效果表 |

## 6. 不变量 / 护栏 (理念对照, 写进实现验收)

1. **零 per-ruleset 硬编码**: 无 `if ruleset_id==…`; 阶梯/kind/评级表/效果表/阈值全是**数据**; 引擎只有
   `sum/max_tracks(kind)`、"按 label 查值"等**通用**操作。绝不在 Rust 写死 `["E",…,"EX"]` 或 `if kind=="class_level"`。
2. **语义而非关键词**: GM 经**类型化工具**触发成长; `sum_tracks` 读的 `kind` 是内部**类型化标签** (理念允许)。
   改值原语**不**走 `check_match` 子串门 (§4.5)。
3. **fail-closed / 不捏造**: 派生引用缺失 track→unresolved→provisional (复用现有护栏), 绝不造值; 序数 label
   未 lookup 不得入算术。
4. **单一真相 (派生公式)**: 来源优先级 **override > 已解析编译 > prose**; 永不回退 source-backed 记录。
5. **单一真相 (track 值)**: 建卡 seed 的起始值 vs 改值原语 vs 编译抽取 —— 起始值由建卡输入提供; 改值原语
   `set` 覆写、`add` 增量; 编译器**不**写 track 当前值 (只写派生公式)。precedence 写进实现。
6. **reuse before add**: 复用 §4 / live 联动 / `lookup_cat` / sheet_json 存储 + 重算路径; **无新物理存储**,
   仅 sheet_json 内加 `tracks` 子桶 (§4.2)。
7. **路径依赖累加器 ≠ live 派生**: HP-by-level 这类**累加**值必须建为 **base/`once`**; **级-keyed 的 live
   HP 公式绝不可被接纳** (默认 `recompute=live` + 编译器已知不稳 → 否则每回合静默覆盖手累加的 HP)。
8. **文件 ≤ ~400 行**: 拆小模块 (聚合原语 / 序数查表 / 改值原语 / 派生 merge / 输入打通 各独立)。

## 7. 范围

**本期内 (A0)**:
- Track 落 sheet_json `tracks` 子桶; `value` 支持数字|序数 label。
- `build_chargen_inputs` 转发 `tracks` 子树; `expr::eval`/`Parser` 加 `tracks: &Value` 线。
- trpg-formula: `sum_tracks/max_tracks(kind)` (lookup 式特例派发) + 序数 **label→值查表** (复用 `lookup_cat`)。
- 通用改值原语 `apply_track_change` (CLI + GM 类型化工具), 复用 `refresh_actor_live_derived`。
- track-keyed §4 派生 + 永不回退/override 的 merge。

**本期外 (A0 明确不做; 门为 A1 留开)**:
- XP→升级 / IP 成本 / tick 掷骰**自动执行**; 多职业**前提门控**。
- 序数阶梯 **step/compare op** 及其消费者 Fate `+/-` 临时 bump (属 play-time 能力层)。
- 选职业/ASI/法术等**离散选择点**接职业选项数据层; **集合值进阶** (D&D 已知/备法术列表、英灵技能组) ——
  非标量, Track 装不下, 归选项数据层。
- 路径依赖值**自动累加** (D&D 逐级 HP, A0 下 GM 改值原语累加, 见 §6.7)。
- `lookup` 按 track **选表** (表名是字面 ident; 按职业选法术位表 = 每职业一条 authored 记录)。

## 8. 组件落点 (每文件小)

| 模块 | 改动 |
|---|---|
| `trpg-formula/src/expr.rs` | `call()` 加 `sum_tracks/max_tracks` 特例分支 (字符串 kind 实参); `Parser` 加 `tracks:&'a Value` (仿 `tables`); 复用 `lookup_cat`/`Num::Str`; **不**加 step/compare op |
| `trpg-formula/src/lib.rs` | `eval()` 签名加 `tracks:&Value` 并下传; 结构化 tracks(+ladder) 进 Parser (不靠展平) |
| `trpg-model/src/lib.rs` | Track 类型 (value: number\|label, kind, category, ladder ref); 有序阶梯声明类型 |
| `trpg-runtime/src/chargen.rs` | `build_chargen_inputs` 转发 `tracks` 子树; 派生 merge 永不回退/override; HP 类累加器标 base/`once` |
| `trpg-runtime/src/lib.rs` | `apply_track_change` 原语 (写 sheet_json.tracks + `refresh_actor_live_derived`); 无 `check_match` 门 |
| `trpg-cli/src/main.rs` | `trpg grow` / `track set\|add` 命令 |
| GM 工具 | 给 GM-agent 的 `apply_track_change` 类型化工具 |
| 数据 (per-ruleset) | 有序阶梯 + 评级/效果表 + track-keyed 派生的声明/override 层 |

## 9. 风险 / 开放问题 (存储分叉与上下文形状已在 §4.2/§4.3 解决)

1. **编译器变异**: track-keyed 派生靠 LLM 抽取仍不稳 (已验证两次)。缓解 = 永不回退 + 人工 override (决策 4);
   **关键 track 派生 (尤其 Fate per-效果表的大数据面) 建议直接 authored override**, 不赌每次重抽。
2. **加权混合派生** (D&D caster level): `sum_tracks` 只覆盖 sum/max(+按 kind 子集); 复杂加权 + 按职业选表
   逐规则 authored, 每职业可能多条记录, 可接受但数据面不小。
3. **两套存储收敛**: 本层选 sheet_json 侧、不加剧分叉; 但 sheet_json ↔ generic_parameter_states 的最终
   统一是更大独立工作, 仍欠 (本期不做)。
4. **集合值进阶** (法术/技能列表) 与序数 step/compare op 一并 defer (§7 范围外), 留作能力层/选项层后续。

## 10. 验证矩阵 (writing-plans 期细化; 前置 = §4.2 存储决策已落)

每套规则一个端到端断言 (建卡只填输入→引擎算派生→改值原语改 track→**live 重算确实触发**):
- D&D: 两条 class_level → `sum_tracks` 总级 → 熟练 lookup; 改 fighter 级 → 熟练重算; HP 为 base/`once`, **不被** live pass 覆盖。
- 剑世界: 多职业 → `max_tracks` 冒险者级 → HP=活力+级×3; `sum_tracks(magical)` → MP; 改属性原语 → 重算。
- CoC/ORC: 技能 track 改值原语 +1d10 → 落值, 无等级派生。
- Cyberpunk: role_rank 改值 → 先攻 += 重算; ip_pool +/-。
- Fate 御主: FP 点池 +5/-10 改值; 衍生属性 §4。
- Fate 英灵: 职阶 choice → 衍生 class-keyed lookup; 字母级属性 → 序数查表成数 → 进衍生; per-效果表按 label 取值。
- 不变量: 无 `if ruleset`; 缺 track→provisional; 序数 label 未 lookup 不入算术; 派生来源优先级确定;
  HP 累加器不被 live 覆盖; 改值原语无关键词门。

## 11. 修订记录

- v1 (本日): 初稿 (A0 + 通用多轨 + 改值原语 + 混合来源, 7 套规则映射)。
- v2 (本日, 对抗评审后): 修复 2 个 BLOCKER —— (a) **存储**: 明确 track 落 sheet_json (live 重算所读),
  纠正"复用 resource-track 存储"的错误声明, 不加剧两套存储分叉 (§4.2); (b) **求值器上下文**: 结构化 tracks
  线进 Parser、`build_chargen_inputs` 转发子树、聚合函数 lookup 式特例派发 (§4.3)。并: 加路径依赖累加器
  不变量 (§6.7); 收回 success_bands 排序复用、defer step/compare op 及 Fate +/- (§4.4); 明确 Fate per-效果
  按 label 键表、`lookup` 不能按 track 选表、SW MP 子集和需细 kind、集合值进阶出范围; 删 `count_tracks`;
  改值原语不走关键词门 (§4.5)。
- v3 (2026-06-09, 实测 D&D 后增补): 改值原语 `apply_track_change` 新增 **字符串(分类)值 + `field` 桶**支持 ——
  `text: Option<&str>` 参数 + `--value` CLI 选项 + GM 工具 `value` 属性。`bucket="field"` 设顶层分类**输入**
  (种族/血统/职业**选择**, 如 draconic_ancestry=red、class=wizard), 使 play 期能改"分类选择",race/class-keyed
  派生随之 live 重算(端到端实测: `grow --bucket field --id draconic_ancestry --value blue` → breath_damage_type
  fire→lightning)。补齐了原"grow 只能设数字、设不了字符串分类值"的限制。测试 `categorical_field_set_drives_race_keyed_derived`。
- v4 (2026-06-09, 投影债收尾): 把 `sheet_json` 派生值的两个**展示/消费视图**做成**物化视图**(成长后即时刷新, sheet_json 是唯一真相):
  (a) `mechanical_profile.stats/skills/fields` —— 新增 `refresh_mechanical_profile(profile, sheet)`, 接进 `refresh_actor_live_derived`+`apply_track_change`; **检定(contest 读 mechanical_profile)与战斗公式(compile_formula 绑 mech.stats/skills)现在读到升级后的新值**(真实 Daria 实测 mech.combat_awareness_bonus absent→7); DRY 进 `materialize_actor_params`。测试 `refresh_mechanical_profile_reprojects_from_sheet_preserving_metadata`。
  (b) `derived_spec` 展示数组 —— `recompute_live_derived` 现在刷新 live 记录条目(once 保快照), 不再停留建卡快照; 测试 `live_recompute_refreshes_derived_spec_view`。
  **仍未收(真·架构债, 需专门设计+测试, 勿盲改)**: 资源三处脱节 —— `sheet_json.resources`(chargen 派生 MAX) / `generic_parameter_states.resources.{id}.current`(play 当前值, mechanics 写 contest 读) / 战斗读 `status_json.hp_max`(materialize 没填→PC 战斗 HP=None) + 资源命名 ruleset 各异(hp_max vs hit_points_max, 战斗硬读 "hp_max")。需一次定调: MAX(派生视图)留 sheet、CURRENT 留 generic_parameter_states、按 kernel.resource_tracks 的 id **语义**桥接进战斗(非硬读字面名), 并处理 "MAX 涨了 CURRENT 上限"。触及 combat/mechanics/kernel 语义, 是 deliberate 任务。
