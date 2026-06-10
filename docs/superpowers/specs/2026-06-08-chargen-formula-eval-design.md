# 建卡期 Source-backed 公式求值设计 (chargen formula eval)

- 日期: 2026-06-08
- 状态: 设计待实现 (brainstorming 已通过, 下一步 writing-plans)
- 范围: 本期只做"消费层"(数据模型 + 求值器 + 建卡流程 + 校验); 公式来源喂 source-backed JSON, 抽取层与运行时联动触发为后续 spec

## 1. 背景与问题

现状(已核实, 见 file:line):

- 建卡 `crates/trpg-runtime/src/chargen.rs:generate_starter_character` 把模板字段+衍生公式当**文本提示丢给 LLM**, 让 LLM 自行填写所有值(含 HP/SAN/闪避), 无确定性计算。
- `crates/trpg-model/src/lib.rs:925 CharacterField` 没有 `source/formula/depends_on`, 无法表达"字段是 input / derived / hybrid", 更无法表达"基础值 + 玩家加点"。
- `compile_formula`(`crates/trpg-combat/src/formula.rs:32`)只处理加法掷骰式, 对 `/`、`max/min`、无骰阈值一律返回 `None` —— 正是 CoC 建卡派生值的形态(HP=(CON+SIZ)/10、SAN=POW、闪避=DEX/2), 因此**建卡期不被任何引擎消费**。
- `validate_character_template_sheet`(`crates/trpg-runtime/src/lib.rs:2344`)只查必填字段是否存在, 不重算、不核对。

定位: 系统主线是"解析规则书 → 数值化 JSON → 通用引擎消费"; 判定 kernel(`trpg-contest`)与资源轨 `on_outcome`(`trpg-mechanics:62`)已是范本级 data-driven。**唯独"建卡期派生/混合计算"这一节完全缺失**。本设计补上它, 且复用同一"公式即数据 + 通用求值器"风格。

参考: 用户既有项目 `deepwood_coc`(CoC 跑团 web app)已验证此方向 —— 规则模板(`rules`)与角色卡(`users_roles`)分离; 用 `tag`(字母标识、不重名)跨参数引用; 技能项拆 `attr_tag/attr_val`(派生段) + `val_job/val_hob`(玩家段) + `val`(最终); 伤害加值=区间表→1d6/1d10。本设计借鉴其结构, 并改进其两处局限(用 `eval()` 跑数据 → 改自写 AST 求值器; db 算法硬编码 → 改 lookup 表数据)。

## 2. 设计理念对齐与护栏

遵循的理念:

1. Onboard-and-Play, 不 Parse-all (产品设计 §3.1)
2. Source-backed: 数值/机制/**derived formula** 必须有来源, 找不到留 `unresolved gap`, 绝不写合成值 (`docs/design/rule_steward_agent_v1_16.md` §Source policy); 裁定档位 `source_backed / provisional`
3. Data-driven / JSON-first, 不写 per-ruleset 引擎 (`docs/design/parameter_facet_executor_v1_13.md` §Goal)
4. LLM 是 skill/tool, 不是 controller

由理念派生、必须落实的**护栏**(实现期为不变量):

- **护栏 1 (优雅降级)**: 公式缺失/不全/token 取不到值时, 该派生值标 `provisional` 或记 `unresolved gap`, **建卡照常完成**(该值待补), 求值器**返回 gap 而非报错/伪造**。不得因"缺 source-backed 公式"硬卡建卡(否则违反理念 ①)。
- **护栏 2 (不留只产不消费字段)**: 新增的 `role`(语义 tag)**本期即有消费** —— 用于回填路由(写 `stats` 还是 `skills`)+ 资源类 clamp 语义; 下期再扩展为运行时资源指向。不做纯占位字段(避免重蹈现有 `evaluator`/`check_model` 空转)。
- **护栏 3 (求值器只加通用算子)**: 求值器只支持通用算子(算术/floor/min/max/lookup/if); 规则专属逻辑(如 MOV 三分支)用 JSON 表达或标 provisional, **绝不在求值器里写 per-ruleset 分支**。
- **护栏 4 (喂入数据真 source-backed)**: 本期手备的 CoC spec 必须带真实书页引用; 核不准的页码 → 该条 `status=provisional`, 不得把凭记忆写的公式标成 `source_backed`。
- **护栏 5 (不新增存储分叉)**: 本期初始资源值回填**不解决**亦**不新增**现有 `actor_mechanical_states` / `generic_parameter_states` 分叉(统一留下期)。

## 3. 范围

本期做:

- 数据模型: 建卡值记录(单字段表达式 + `input_kind` + 多段 hybrid), 扩展 `DerivedValue`, 不动 `CharacterField`
- 通用求值器: 新 crate `trpg-formula`
- 建卡流程改造: `generate_starter_character` 改为 LLM 只填 input + 新增求值回填步
- 升级版校验器: 重算核对 + hybrid 约束 + 结构化报告; 复用到 import
- 一份 CoC source-backed 建卡 spec(JSON)作为喂入样例与测试基线

本期不做(YAGNI, 见 §10)。

## 4. 数据模型

一份 ruleset 的"建卡求值规格" = `phase: chargen` 的一组**值记录**。值记录扩展自 `crates/trpg-model/src/lib.rs:938 DerivedValue`:

```jsonc
{
  "id": "dodge",                 // = 引用用的 tag/canonical id (不重名)
  "role": "skill",               // attribute | skill | resource | resource_max | background
  "input_kind": "hybrid",        // player | derived | hybrid  —— 显式标识谁填/谁算
  "recompute": "live",           // once(快照) | live(基础值变则联动)
  "max": 90, "min": 0,           // 约束(放数据里)
  "result_type": "int",          // int | dice_or_int(lookup 命中骰子串时)

  // derived 用:
  "expr": "floor(({{con}}+{{siz}})/10)",
  "depends_on": ["con", "siz"],  // 可由 expr 自动解析, 显式列用于拓扑

  // hybrid 用(多段, 借鉴 deepwood):
  "attr_derived": "floor({{dex}}/2)",     // 派生段(随属性联动)
  "base": 0,                              // 固有基础值
  "allocations": [                        // 玩家分配段(命名来源 + 预算)
    {"source": "occupation", "input": "{{player.dodge.occ_pts}}", "budget": "occupation_points"},
    {"source": "interest",   "input": "{{player.dodge.int_pts}}", "budget": "interest_points"}
  ],
  "combine": "attr_derived + base + sum(allocations)",

  // lookup 用:
  "lookup_tables": { "t": { "ranges": [ {"max":64,"value":"-2"}, {"min":65,"max":84,"value":"-1"} ] } },

  // 资源用:
  "clamp_max": "hp_max",         // 当前值不超上限(可为常量或另一 id)

  // 来源/裁定(护栏 4):
  "source_ref": { "book": "coc7e", "page": 33 },  // 页码为示意; 实现期须对照源书核实(护栏4), 核不准则降 provisional
  "status": "source_backed"      // source_backed | provisional
}
```

`input_kind` 三态:

| input_kind | 含义 | CoC 例 |
|---|---|---|
| `player` | 玩家填, 引擎不算 (`expr` 省略) | `dex`(属性) |
| `derived` | 引擎算, 不需填 | `hp_max=floor((con+siz)/10)` |
| `hybrid` | 一部分填、一部分算 | `dodge = floor(dex/2) + 职业点 + 兴趣点` |

`recompute` 与"上限 vs 当前值":

| recompute | 含义 | 例 |
|---|---|---|
| `live` | 永远 = f(基础值), 基础值变则联动重算 | `hp_max`、`mp_max`、`dodge.attr_derived`、`db` |
| `once` | 建卡算一次成"初始值", 之后独立涨落 | `hp_cur` 初值=hp_max、`san_cur` 初值=pow |

- `hp_max` = live 派生; `hp_cur` = 资源状态(once 初始化 + 之后受伤/治疗独立涨落, 约束 `cur ≤ max`, 涨落走现有 `resource_tracks.on_outcome`/facet executor, 不归本求值器)。

## 5. 占位符与求值器 (`trpg-formula`)

### 占位符

`{{namespace.id}}`, 带分隔符防子串误替, 按 id/tag 引用(非显示名):

- `{{con}}` / `{{dex}}` —— 引用属性(从 sheet 输入取)
- `{{player.dodge.occ_pts}}` —— 玩家加点段(hybrid)
- `{{derived.hp_max}}` 或直接 `{{hp_max}}` —— 引用另一派生值(允许派生依赖派生)

sheet 里实际 key 若为 `DEX/敏捷`, 求值器做一层 id 规范化(大小写不敏感 + 可选别名表)匹配到 `dex`。

### 求值五步

对每条 derived/hybrid 记录:

1. 解析 `expr`(及 hybrid 的 `attr_derived`/各 `allocations.input`), 提取所有 `{{token}}`
2. 逐 token 取值; 取不到 → 记 `unresolved`(不伪造, 护栏 1)
3. 正则按边界把 `{{token}}` 代入为数值
4. 自写 recursive-descent parser → AST → 求值(**不用 `eval`**, 防注入)
5. 返回 `{ value, result_type, breakdown, unresolved[] }`

### 能力(护栏 3: 仅通用算子)

- 二元 `+ - * /`、括号、负数
- 函数 `floor(x) ceil(x) round(x) min(a,b,..) max(a,b,..)`
- `lookup(table, key)`: 在记录 `lookup_tables` 内按 `ranges` 区间匹配; 命中值可为数字或**骰子串**(如 `1d4`), 后者令 `result_type=dice_or_int`, 原样保留为"建卡产出、待战斗投的公式", 不再继续算术
- `if(cond, a, b)` + 比较 `< <= > >= ==`: 供 MOV 这类条件派生(若实现; 否则该条标 provisional)

### 依赖拓扑与降级

- 按各记录 `depends_on`(含 `{{derived.*}}`)建 DAG, 拓扑排序后依次求值; 检测到环 → 报 gap, 不死循环。
- 任一段 unresolved → 整条记录 `status` 降为 `provisional`/gap, 建卡不中断(护栏 1)。

### hybrid 多段拆分

- `final = attr_derived + base + Σ allocations`(由 `combine` 描述; **本期 `combine` 仅支持加法叠加**, 字段保留供未来非加法形态)。
- `breakdown = { attr_derived, base, allocations:[{source,value}], final }` 供 UI 展示与校验。
- 玩家段缺失按 0 计; `attr_derived` 段缺 token → 整条 provisional。

## 6. 建卡流程改造 (`chargen.rs`)

```
create_and_bind_character:
 1. generate_starter_character  [改]
    prompt 只让 LLM 产出 input_kind=player 字段(属性/职业/背景)
    + hybrid 的玩家加点 {{player.*}}(预算内); 明确告知 derived/hybrid 算的部分别填
 2. apply_chargen_formulas      [新增]  ← 调 trpg-formula
    load ruleset 的 chargen spec(phase=chargen)
    → 拓扑序算所有 derived + hybrid → 回填 sheet(按 role 路由到 stats/skills/resources)
    → 每个值记 { value, breakdown, source_ref, status, recompute }
 3. validate (升级版)            [改]  (见 §7)
 4. materialize_actor_params    [改]
    存最终值 + 把 live 派生的"公式 + recompute 标记"随 actor 存(mechanical_profile.derived_spec)
    → live 不冻死, 为下期运行时联动复用; 不新增存储分叉(护栏 5)
```

## 7. 校验器升级 (`validate_character_template_sheet`)

1. 必填检查(保留)
2. **重算核对**: 对每个 derived/hybrid 用求值器重算, 与 sheet 值比对(挡篡改/脏 import)
3. **hybrid 约束**: `final = attr_derived + base + Σalloc`; `final ≤ max`; 各 alloc ≤ 对应 `budget`(预算缺省则只记录)
4. **input 完整性**: `input_kind=player` 必填项有值; hybrid 的玩家加点 token 有给
5. **来源/状态**: unresolved → gap; `status=provisional` 标注(不阻断, 进报告)
6. 产出结构化 `ValidationReport`: 逐值 `{value, breakdown, source_ref, status, recompute}` + gaps

复用: 同一 validate 用于"import 玩家上传的角色卡"—— 重算核对、标出不符/缺来源, 而非照单全收(回应最初"解析角色卡"语境)。

## 8. crate 归属

| 单元 | 放哪 | 理由 |
|---|---|---|
| 求值器 `ChargenEvaluator` | **新建 `trpg-formula`** | 纯函数、无 DB、无副作用; 易单测; 将来可统一替换 combat `compile_formula` |
| 值记录类型(扩展 `DerivedValue`) | `trpg-model` | 与现有类型同处 |
| `apply_chargen_formulas` + 升级 `validate` | `trpg-runtime`(`chargen.rs` 旁) | 现有建卡入口 |
| CoC spec JSON | `data/parsed/characters/` | 本期手备的 source-backed 样例 |

## 9. 测试策略

- `trpg-formula` 单测: 算术/floor/min/max; lookup 区间(含骰子结果); 占位符代入防子串误替; 依赖拓扑; 循环检测报 gap; unresolved 不伪造; 多段 hybrid 求值 + breakdown; 优雅降级。
- CoC 黄金用例: CON60+SIZ60→hp_max=12; POW50→san_cur=50, mp_max=10; DEX70→dodge.attr_derived=35; STR60+SIZ60→db=0。
- 校验测: `final=attr_derived+base+Σalloc`; `final≤90`; alloc 超预算报 gap; 改 CON 重调求值器→hp_max 变(验证"可重算不冻死")。
- import 测: 喂被篡改 hp_max 的脏卡 → validate 重算发现不符并标记。

## 10. 本期不做 (YAGNI) 与已知边界

1. 运行时"基础值变→live 自动联动"的触发入口 → **下期 spec**(连带收拾两套资源存储分叉)。本期只保证"可重算、存公式不冻死"。
2. rule-steward 从规则书自动抽建卡公式 → 下期; 本期喂 source-backed JSON。
3. 建卡掷骰/随机生成属性。
4. 完整技能点预算系统(本期 final≥base/≤max + alloc 记录; 预算上限 spec 给了就校验, 没给只记录)。
5. **MOV** 等条件比较派生: 用通用 `if()` 表达或标 provisional(护栏 3), 不写 CoC 专属分支。
6. 战斗结算期公式(已有 `compile_formula`, 本期不动; `trpg-formula` 设计成将来可统一替换它)。

## 11. 数据流图

```
[规则书] --(下期 rule-steward 抽取)--> [chargen spec JSON(本期手备, source-backed)]
                                              |
玩家/LLM 填 input(属性/职业/加点) --> [sheet] |
                                              v
                              [trpg-formula 求值器]  (通用, 读 JSON, 拓扑求值, 缺则降级)
                                              |
                       回填 derived/hybrid + breakdown + status
                                              v
                            [升级版 validate] --重算核对/约束--> [ValidationReport]
                                              v
                       [materialize: 值 + 公式 + recompute(不冻死)]
                                              v
                  (下期: 运行时基础值变更 → 同一求值器 live 联动重算)
```
