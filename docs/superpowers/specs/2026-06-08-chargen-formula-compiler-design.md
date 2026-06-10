# 建卡公式编译器设计 (chargen formula compiler)

> 把 2026-06-08-chargen-formula-eval-design.md §10.2 / §11 列为"下期"的
> "rule-steward 从规则书自动抽建卡公式"提前实现。**前置 spec 是它**，本文只补"谁产出 §4 spec"。

## 1. 背景与问题

`trpg-formula`(求值器)吃的是机器可求值的 §4 值记录(`expr` + 内联 `lookup_tables` +
hybrid 分段)。但规则 reader 的"深读那一遍"只能把建卡公式抽成**大白话**，而且漏掉两类难的:

CoC 实测(lean reader,duotext/mineru 任一)产出的 `template.derived_values`:

| reader 抽到的(prose) | 漏掉的 |
|---|---|
| `hit_points: "floor((CON+SIZ)/10)"` | ❌ `damage_bonus` / `build`(**查表**) |
| `magic_points: "POW/5"`、`sanity_points: "POW"` | ❌ `Dodge`(**属性派生的技能**,=DEX 一半) |

今天靠人手写 `{ruleset}.chargen.json` 补这个缺口(本期 spec 的做法)。**编译器把"人手写"
这步自动化、做成通用**。上次的失败教训:把机器格式的要求塞进深读那一遍的单个 submit →
mini 过载、整张模板回归(24KB→8KB,derived_values 10→0)。所以**必须拆成独立一遍**。

## 2. 理念对齐与护栏 (对照 chatrpgv2-design-philosophy)

- **它就是规则 agent 在解析,不是外挂工具**(回应"我们难道不是规则 agent 解析的吗")。
  实现成规则 agent 内部一个**专注的第二 slice**(`trpg-rule-agent`),复用 reader 现成的
  `run_loop` / `nav_tools` / 已定位的公式页,只把"编译成机器公式"单独跑——而不是再塞进深读。
- **单一数据源**(原则 1 数据驱动):编译结果**就地升级** `template.derived_values`
  (大白话 → §4 机器字段),全程只留一份;手写文件降级成可选 `override`。
- **语义优先、不许关键词启发式**(原则 2):"认属性派生技能 / 找对的表"由 LLM 读懂源文,
  **绝不**写成 `if formula.contains("half")` 之类。确定性部分只限安全机械操作(属性名小写、
  round-trip 求值校验)。
- **fail-closed、不捏造**(原则 1):每条编出来回灌求值器跑一遍;跑不通 / 源头核不准 →
  `status=provisional`,不丢不编。
- **复用已有工具**:`trpg-formula`(校验)、reader `run_loop`+`nav_tools`、duotext 的
  `.layout.md` sidecar(干净表格)、`DerivedValue` 的 §4 字段(已扩展)。

## 3. 范围

本期做:
- 规则 agent 内的编译 slice(prose→`expr`;表内联;属性派生技能补全)。
- 确定性 round-trip 校验 + provisional 降级。
- `template.derived_values` 就地升级(单一数据源)。
- 运行时 `load_chargen_spec` 改读 template + 可选 override。
- 读 duotext `.layout.md` sidecar 的能力(按页),供编译器取表行。
- 三套规则验收(见 §9)。

本期不做(YAGNI,见 §10)。

## 4. 架构与数据流

编译器挂在解析流水线里,**reader 之后、写盘之前**:

```
[duotext 解析] → <id>.md(prose,已索引) + <id>.layout.md(对齐表格,page-anchored)
                                  │
parse_rulebook:                   ▼
  run_reader_parallel ──► char 模板{ fields(含技能), derived_values(prose), source_refs(定位页) }
                                  │
  ★ compile_chargen_formulas(新) ─┤  输入: prose derived_values + 技能 fields + 定位页
                                  │        + layout_sidecar_path + nav_tools + read_layout
                                  │  mini agent(小 submit schema = §4 derived_values 数组):
                                  │    ① prose→expr,属性名小写
                                  │    ② "查表"类 → read_layout(定位页) 取表行 → 内联 lookup_tables
                                  │    ③ 扫技能找"=某属性一半" → hybrid 技能记录
                                  │  确定性后处理: evaluate_chargen round-trip;核不准→provisional
                                  ▼
  template.derived_values ◄── 就地升级为 §4(field_id 保持;填 expr/lookup_tables/role/...)
                                  │
  normalize → onboarding pack → DB / 文件(原样,不开新文件)
                                  ▼
运行时 load_chargen_spec: 读 template.derived_values(field_id→id) ⊕ {ruleset}.chargen.override.json(按 id 覆盖)
                                  ▼
                          trpg-formula 求值(不变)
```

## 5. 组件

### 5.1 编译 slice — `crates/trpg-rule-agent/src/reader/chargen_compile.rs`(新)
`pub async fn compile_chargen_formulas(client, template: &mut CharacterTemplate, ctx: CompileCtx)`
- `CompileCtx { units: &[Unit], layout_sidecar_path: Option<PathBuf>, located_pages: Vec<...> }`。
- 工具 = `nav_tools()`(get_toc/search/read prose)+ 新 `read_layout(pages)`(读 sidecar 对齐表行)
  + 一个小 `submit_chargen`(properties 只有 `derived_values` §4 数组,字段同 §4)。
- 系统提示词聚焦"把这些已定位的建卡公式编译成机器可求值的 §4"——通用例子(body/will/ref),
  **不出现 CoC 专属命中词**。

### 5.2 确定性护栏 — round-trip
- 编译后用 `trpg_formula::evaluate_chargen(records, sample_inputs)` 跑一遍(sample = 模板里
  characteristic 字段的中位/示例值)。
- 任一记录 `unresolved` 非空 / eval 报错 / 没有可核的 `source_ref` → `status=provisional`。
- `trpg-rule-agent` 加 `trpg-formula` dep。

### 5.3 就地升级 — `crates/trpg-parser/src/lib.rs` `parse_rulebook`(~520)
- `run_reader_parallel` 返回后、`normalize_character_sheet_template_schema` 之前,调
  `compile_chargen_formulas(&mut template, ...)`,把 prose `derived_values` 覆盖为 §4。
- `source_document.metadata.layout_sidecar_path` 在此可得 → 传入 `CompileCtx`。

### 5.4 sidecar 读取工具 — `read_layout(pages)`
- 复用 `render_page_anchored_markdown(... "duotext-layout")` 的 page anchor:按页从
  `.layout.md` 截出对齐文本(和 `read` 对 prose 对称)。sidecar 缺失(如旧 mineru-only 解析)→
  工具返回"无 layout 视图",编译器对该表标 provisional,不崩。

### 5.5 运行时 — `crates/trpg-runtime/src/chargen.rs` `load_chargen_spec`
- 改签名为 `load_chargen_spec(ruleset_id, template) -> Vec<Value>`:
  - 默认:`template.derived_values` 里**机器可求值**(有 `expr` 或 `attr_derived`)的记录,
    `field_id`→`id` 映射成 §4 record(纯大白话、无 expr 的跳过,免得求值成 0)。
  - `{ruleset}.chargen.override.json` 存在 → 按 `id` 逐条覆盖。
- `create_and_bind_character` 已有 `template`,改调新签名。手写 `chargen.json` 退休为 override。

### 5.6 缓存
- 编译器版本号并入 `parse_config_hash`(`chargen_compiler=v1`)。逻辑变 → bundle 缓存 miss →
  重解析,但 duotext markdown 有缓存(reader 仅 ~分钟级),可接受。更细的"只重编译"缓存 = YAGNI。

## 6. §4 输出格式

完全沿用前置 spec §4 值记录(`id/role/input_kind/recompute/result_type/expr/attr_derived/base/
allocations/lookup_tables/min/max/clamp_max/depends_on/source_ref/status`)。编译器在 template 里
以 `DerivedValue` 的对应字段承载(`field_id` 即 `id`)。

## 7. 护栏汇总

| 护栏 | 机制 |
|---|---|
| 不捏造 | round-trip 跑不通/核不准 → provisional,不编数字 |
| 不硬编码 | 编译 slice 通用提示词,无 ruleset 分支,无关键词启发式 |
| 单一数据源 | 就地升级 template.derived_values,手写文件降级 override |
| sidecar 缺失 | read_layout 优雅返回空,相关表标 provisional,不崩 |
| 过载回归防复发 | 独立小 submit schema,只产 derived_values 数组 |

## 8. crate / 文件改动清单

- `trpg-rule-agent`: 新 `reader/chargen_compile.rs`;`reader/tools.rs` 加 `read_layout`;
  `reader/mod.rs` 导出;`Cargo.toml` 加 `trpg-formula` dep。
- `trpg-parser`: `parse_rulebook` 接编译调用 + 传 sidecar 路径。
- `trpg-runtime`: `chargen.rs` `load_chargen_spec` 改签名 + override 合并;调用点更新。
- `trpg-model`: 不动(§4 字段已在 `DerivedValue`)。

## 9. 测试策略

- 确定性单测(`trpg-rule-agent` / `trpg-runtime`):
  - `field_id`→`id` 转换 + 纯 prose 记录被跳过。
  - override 按 id 覆盖、未覆盖保留。
  - round-trip:伪造一条 unresolved 记录 → 标 provisional 不丢。
- 三套规则 live 验收(都用 **duotext** 默认后端重解析一遍,确保 `.md` 与 `.layout.md` 同源;
  CoC 当前是 mineru `.md` + duotext `.layout.md` 混源,需用 duotext 重解析对齐):
  - **CoC**:编出的 §4 对齐金标准(hp/mp/san/dodge/db/build 六项)。
  - **Cyberpunk RED**:编出可用派生(HP from BODY+WILL、Humanity 等),round-trip 通过。
  - **Triangle**:能编译、不崩、不捏造(数值公式少,接近空 = 合格)。
  - 只有 CoC 有金标准;另两套及格线 = 编译通过 + 不捏造(不为对答案而调 ruleset 专属查询)。

## 10. 本期不做 (YAGNI)

1. 运行时"基础值变→live 联动重算"触发(仍是更下期,前置 spec §10.1)。
2. 只重编译不重解析的细粒度缓存(§5.6)。
3. 编译器对 prose 二义公式的多解消歧 UI——直接标 provisional 交人工 override。
4. override 文件的可视化编辑/校验工具。
5. **参数语义/行为层(下一步候选,用户 2026-06-08 提出)**:本期编译器只产"怎么算"(chargen 公式
   → trpg-formula)。但 GM 跑团还需要"这是什么 + play 里怎么变":即每个参数的语义 + 行为应进
   `rule_kernel.resource_tracks`(`zero_means`/`on_outcome`/`thresholds`,被 `apply_outcome_resource_tracks`
   通用消费)。现状缺口:① resource_tracks 只覆盖招牌轨道(CoC 只有 HP/Sanity),MP/Build/DamageBonus/
   Dodge 等有公式无语义条目;② `on_outcome`(如"SAN 检定失败掉 1d6")常被 mini reader 漏;③ 公式层
   (compiler)与语义层(reader resolution slice)是两道 pass、仅靠 id 松散关联。**下一步**:给编译器配
   姊妹动作,抽到参数时按 id 对齐/补齐 kernel 里的语义+行为条目(formula↔track↔含义统一),漏的 on_outcome
   也补——同样强模型、fail-closed、不捏造。

## 11. 一句话

给规则 agent 补一个**专注的第二遍**:把它已经读到的建卡公式(prose)+ duotext 干净表格,
**语义编译**成机器可求值的 §4,**就地升级**进 template(单一数据源),**round-trip 护栏**保证
不捏造——三套规则都跑通,CoC 对齐金标准。

## 12. 实现期修订 (2026-06-08, 与 §4/§5 的差异)

落地时设计有三处演进,以此为准:

1. **duotext 变单产物**:duotext 后端更新为**单个 layout-preserving `.md`**(对齐表格直接 inline 在主产物里),
   `.layout.md` sidecar 退休。因此编译器用普通 `read(pages)` 就能拿到表格行;`read_layout` 降级为可选 fallback
   (sidecar 不存在时优雅返回空)。§4 数据流里"读 sidecar 取表"改为"读主 `.md` 取 inline 表"。

2. **编译器 = augment + 1 次定向 retry**(取代单遍):mini agent 单遍产 §4 数组**run-to-run 不稳**(同一本书,
   一次出 hp/mp/san/db/build、另一次丢 mp/san)。改为:reader 的 prose `derived_values` 是**必需清单**,
   ①第一轮编译;②对仍缺机器 `expr` 的必需字段做第二轮定向补;③**augment**——编译器没产出的字段保留 reader 的
   prose 记录,coverage 永远 ≥ reader 集(绝不回退)。技能列表(option_catalogs)喂给 agent 以识别属性派生技能(Dodge)。

3. **finalize 确定性归一化**(护栏内):查表值 `+1d4`→`1d4`、`None`/`-`→`0`;非字符串落入字符串字段时丢弃该字段
   (不让一条坏字段毁掉整记录);hybrid 技能被写成 `expr` 里塞假 `{{allocations}}` token 时,拆出属性段进
   `attr_derived`、加点进 allocations 数组,使其可解析(否则永远 provisional)。

4. **缓存 token**:`parse_config_hash` 里是 `chargen_compiler=v4`(每次编译器逻辑变更 bump,使 bundle 缓存失效重编)。
