# 规则感知 GM 设计（GM Agent Loop 二期：机制目录 + watcher/债务 + 场景机制意图）

日期：2026-06-10
状态：设计已获用户确认（范围三柱全上 / 债务出口 waive+记账）
前置：一期 `2026-06-10-gm-agent-loop-design.md`（trpg-gm loop + 10 工具，本 spec 全部为加法、一期代码零返工）
参考：用户设计稿 `/Users/haoli/leehow/code/chatrpgv2/agent设计.md`

## 1. 需求与背景

用户核心诉求（原话要点）：agent 应配合规则解析结果，**知道这套规则里有什么、遇到什么剧情该触发什么**——不是故事机器或简单 loop agent。CoC 就该用 d100（来自解析）；玩家想跳过一个坑，agent 应感知"可能需要检定"并找到跳跃检定，而不是把行为当背景板；SAN check 该在玩家受理智冲击时按规则放出；单次掉太多该触发疯狂检定及疯狂效果——全部来自规则解析、零硬编码。解析侧应给参数加描述（"san check 是什么"），让 agent 知道何时触发什么。

### 1.1 数据勘查实证（2026-06-10，真 :54347 库 CoC kernel）

- `check_model`：d100 roll-under、难度减半/五分之一、大失败、"Sanity 用同样结构"——**全在一段 prose summary 里**，无结构化视图。
- `resource_tracks.thresholds`：**`loss_in_one_go: 5 → may trigger temporary insanity` 已被解析出来**，但 consequence 是 prose 字符串，运行时零消费——"背景板"问题的实锤根源。
- `character_sheet_schema.fields` 有 `notes` 描述（STR 有），但**可检定技能目录整个缺失**（skills 区只有 credit_rating/cthulhu_mythos，Jump/Spot Hidden/Listen 等无结构化条目、无"何时用"语义）。

### 1.2 对 `agent设计.md` 的吸收与排除

- 已被一期覆盖（不重做）：结构化行动协议（tool-calling 即 AgentCommand/observation）、Rust 工具/事务层、统一结算入口（roll_check → execute_system_roll_bundle → after_check_resolved）、玩家门、SSE 分步事件。
- 本期吸收：**Mechanical Obligation（机械债务）**、**EffectPolicy/场景机制意图**（"切线缆 Brawling DV13 → athena_cable.power=cut + 4 轮倒计时"编译为可执行补丁）、叙事仅渲染已落账事实的原则（以债务门控"何时进叙事轮"实现，兼容一期 D2 流式哲学——不审查叙事文本，只控制叙事开始时机）。
- 明确排除：阻断式叙事校验/重写（与用户拍板的 D2 错误哲学冲突）；推倒重写 play_turn_sse（一期已以新路径并存解决）。

## 2. 已拍板决策

| 决策点 | 结论 |
|---|---|
| 范围 | 三柱全上一个 spec：①机制目录编译 ②运行时注入+watcher+轻量债务 ③场景机制意图。spec 内部按可独立交付切片 |
| 债务出口 | **可显式 waive + 记账**：债务不清不进叙事轮，但 agent 可调 `waive_obligation(id, reason)`（必须带理由）；waive 记账本+勘误记忆，下回合债务仍在清单提醒 |
| 目录位置 | kernel 新区 `mechanics_catalog`（复用 rule_kernel_patches override 机制），不另起表 |
| watcher 位置 | 阈值检测在 trpg-mechanics 效果落账单点（旧路径免费受益），债务消费在 trpg-gm loop |
| 场景意图抽取 | 模组到场深抽顺路产出（零新增管线） |
| 编译模型 | gpt-5.4（沿用模组抽取既有拍板：后台跑、质量优先） |

## 3. 目标 / 非目标

**目标**
- agent 在回合中**带着本规则集的机制知识**：技能/检定目录（是什么、何时用、绑哪个参数）、子系统程序（SAN check 全链、疯狂、Luck 花费等）。
- 阈值穿越（SAN 单次掉 ≥5、HP 至 0 等）由引擎 watcher 检测并以观察回填 loop——agent 漏了引擎也不漏（语义+机器双保险）。
- 机械债务清单门控叙事轮：开着的检定必须结算或开门，watcher 的 due 必须处理或显式 waive。
- 模组场景写明的检定（DV/技能/后果）编译为带 effect_policy 的场景机制意图，结算后由 Rust 强制执行效果补丁。
- 全部 data-driven：六套规则通用、零 per-ruleset 硬编码、fail-closed（解析不出→该规则集退化为现状，agent 靠 retrieve_rules）。
- 一期缓存稳定原则不破坏：目录索引按 ruleset 固定进 BP1。

**非目标（本期）**
- 不做阻断式叙事文本校验（D2 维持）。
- 不做疯狂"发作内容"的穷尽执行：程序链支持 table_roll 步骤，解析出几步执行几步（fail-closed），发作症状的演绎仍归 agent 叙事。
- 不动旧路径（advice/编排器照旧服务旧路径）。
- 不做玩家侧 UI。

## 3.5 理念护栏（MUST-FOLLOW：通用系统 + 语义优先于字符匹配）

本期所有设计受两条项目根本原则约束，并以本期为契机**清算存量字符匹配**：

1. **结构化 id 绑定取代 check_match 字符匹配**。现有 `resource_tracks.on_outcome.check_match` 是对 check label 的 regex 子串匹配（如 `"sanity|san roll"`）——历史 bug 之源（"attack" 别名误绑 HP 轨、中文输入撞不中英文键名）。本期起：`roll_check(mechanic_id)` 时 CheckContract 携带 `mechanic_id`；on_outcome 触发与 followup 链**优先按 mechanic_id / procedure_id / tested_parameter 结构绑定**；`check_match` regex 降级为"契约无 mechanic_id 时的兼容回退"，watcher/结算依赖回退路径时在账本与 validation_report 中显式标注（可观测的技术债，不静默）。
2. **触发判定一律语义**：玩家行动→检定的映射靠目录 `when_to_use` 语义 + agent 判断，禁止关键词表路由（一期已退役 advice 关键词路由，本期不得回潮）；场景机制意图按 `description` 语义对应玩家行动，按 `intent_id` 结构引用。
3. **通用系统**：所有新结构零 per-ruleset 代码分支；分级/阈值按数据特征（条目数、kind）不按规则集名；六套规则同一套管线跑批验证。
4. **合法的字面匹配边界**（语义原则不适用处，明示以免误清）：私骰 token 防漏缓冲（过滤的是字面数值/ID 本身）、骰式表达式解析、id 相等比较。
5. **一期遗留清算并入 Slice B**：NarrationVerifier 的子串 token 对账升级为结构化引用优先——叙事提交带 referenced_ledger_ids 时按账本 id 结构核对，子串扫描仅作无引用时的回退。
6. **背景板防治三件套（通用规律）**：任何机械事实（成功度 band、资源池、阈值、倒计时）必须三件套齐活才不退化为背景板——**结算落账**（机械判定真实发生并入库）、**语义可见**（该事实在本规则集"意味着什么"作为知识进投影/工具结果）、**联动可触发**（band/阈值的机械后果走 on_outcome/watcher 而非叙事自觉）。chaos 累积、CoC 成功度、SAN 阈值疯狂均为此模式实例；新增机械概念时按三件套自检。

## 4. 支柱 1：机制目录编译（解析侧第三遍）

对标 `chargen_compile` 先例（prose→机器、template 就地升级、round-trip 护栏），在 trpg-rule-agent 新增 `mechanics_compile` 遍，产出写回 kernel：

```rust
// kernel content_json 新区
mechanics_catalog: Vec<MechanicEntry>

MechanicEntry {
  id: String,                  // "coc.skill.jump" / "coc.sanity_check" / "coc.temporary_insanity"
  name: String,
  kind: MechanicKind,          // skill_check | subsystem_procedure | reaction | spend | other(String)
  description: String,         // 是什么（给 agent 的语义知识）
  when_to_use: String,         // 触发语义（"跳越沟壑/躲落石"；"目睹恐怖/超自然"）
  tested_parameter: Option<String>, // 绑定 sheet schema 键，编译期校验
  procedure: Vec<ProcedureStep>,    // roll{dice,vs} | apply{track,op,amount} | table_roll{table_ref} | gate{...}；可为空（见表达力分层）
  hooks: Vec<EngineHook>,           // 事件钩子型触发：scene_enter | turn_start | time_advance | rest | combat_start | combat_end
                                    //   | calendar{granularity}（WorldTimeService 跨边界触发；粒度由 ruleset 数据定义——日/周/月/自定义时间段（Fate 一天 8 段实证），非固定枚举）
                                    //   | session_end | development_phase（session/章节结算节拍——七书勘查证实五书均有周期节拍器机制）
                                    //   （引擎真实事件点词表；哪个机制挂哪个钩子由解析决定）
  passive_projection: Option<String>, // 第四表达档 passive_modifier 专用：持续投影模板（如"信用评级 {value}：{band 语义}"），渲染进 BP 常驻
  followup_links: Vec<FollowupLink>, // {condition: threshold/outcome 引用, procedure_id}
  source_refs: Vec<SourceRef>,
}
```

**发现的开放性与表达力分层（MUST，勘查后定稿为四档）**：编译遍的任务是"遍历规则书，找出一切在游戏进行中会被触发、需要 GM 在叙事中配合执行的机制"——**开放枚举，不按预设类别清单抓取**；kind 是表达形态分类不是发现过滤器，归不进既有 kind 的用 other 收录。表达力四档（2026-06-10 七书勘查实证定稿，见附录 A）：
1. **procedure**——可结构化程序链，机械执行；
2. **hook**——事件钩子触发（含日历周期与 session/章节节拍）；
3. **passive_modifier**——持续被动场（信用评级/声望/态度场/生存档位类，五书全票存在）：无触发瞬间，靠 `passive_projection` 模板**常驻投影**进 BP（按 owner 与当前值渲染语义行），agent 在每个判定与叙事中持续感知；其数值变化仍走 track/watcher 体系；
4. **semantic**——纯语义条目（含"叙事授权转移"类：疯狂发作的控制权移交、推骰的失败加重授权等——LLM-GM 天然以叙事执行，无需机械层）。
结构化不了的降档保留，agent 仍能感知并以叙事配合 + apply_effect 手动落账。**fail-closed 的语义是"不编造"，绝不是"不认识就丢"**——丢弃仅发生在无 source_refs 支撑时。

**事件钩子触发通路（EngineHook）**：玩家行动语义（agent）与数值穿越（watcher）之外的第三条触发通路——"每场景开始 X / 休息时回复 Y / 时间推进 Z 衰减"类机制由引擎在对应事件点（场景切换、回合开始、时间推进等引擎真实存在的事件）按 hooks 产出 MechanicDue 进债务通道，与 watcher 同路消费。解析出钩子语义但引擎无对应事件点的 → 降级为语义条目并记 validation_report（缺口可观测）。

配套升级（同遍完成）：
- `resource_tracks.thresholds[*]` 增 `followup_procedure_id: Option<String>`（prose consequence 保留；如 sanity 的 loss_in_one_go:5 → "coc.temporary_insanity"）。
- **`dice_core.success_bands[*]` 语义化与补全**（实证：现 active CoC kernel 的 band 零语义零后果，且缺 critical/failure band——历史 _fix_coc_success_bands.sql 为手工修未普及）：每个 band 增 `semantics: Option<String>`（该成功度在剧情/机制上意味着什么——extreme=贯穿/卓越效果、fumble=灾难并发）与可选 `mechanical_effects`（链接 on_outcome band 触发或目录程序）；编译护栏检测 band 集完整性（缺 critical/failure 记 validation_report 并补全）。
- **`on_outcome` 触发结构化扩展（per-band 机械触发）**：trigger 现词表全库仅 `always`/`on_failure`（实证查询）——扩展为兼容结构化形态 `{kind:"band", band_id:"fumble"}`（字符串 trigger 向后兼容），mechanics 结算按 outcome.band 选触发；amount 迷你语言扩展支持取最大形态（如 fumble→sanity_loss 取最大值），具体语法留计划阶段。**"SAN 大失败掉最大值"由 prose 变机械事实**。fail-closed：未解析出 band 触发的规则集行为不变。
- `character_sheet_schema.fields[*].notes` 描述补齐（含派生值：sanity/luck/mp 的"是什么/怎么用"）。
- **护栏（fail-closed）**：目录条目引用的 tested_parameter 必须存在于 sheet schema（含 skills 桶约定），followup_procedure_id 必须指向目录内真实条目；不满足则丢弃该条目并记 validation_report。`#[serde(default)]` 全量向后兼容，老 kernel 无此区照常工作。
- 六套规则跑批（CoC/Cyberpunk/Triangle/ORC/D&D中文/剑世界中文）验证零硬编码。

## 5. 支柱 2：运行时注入 + watcher + 机械债务

### 5.1 目录注入
- **BP1 紧凑索引**：每条一行 `id | name | when_to_use`（CoC 约 50-70 条 ≈ 2-3k token），按 ruleset 固定 → 不破坏缓存稳定。超 BP1 预算时分级：kind=subsystem_procedure 与高频 skill_check 进 BP1，长尾仅靠 lookup（分级规则 data-driven，按条目数阈值非按规则集名）。
- **track 投影语义化（owner_kind 通用）**：资源/池 current 值投影到 BP3 时不给裸数字，附带语义状态行——用已解析的 `thresholds`（当前值所处区间的 consequence）与 `zero_means` 渲染，如 `chaos_pool: 7（scene 级）—— 异常体可花用充足` / `sanity: 38 —— 距临时疯狂阈值正常`。投影与 watcher 必须按 `owner_kind`（actor/scene/party/world…开放枚举）通用处理，**不得只投 actor 级**（实证：Triangle chaos_pool 是 scene 级；Fate 环境魔力 scene 级、圣杯战争阶段计数 world 级）。fail-closed：无 thresholds/zero_means 语义可渲染时退回裸数值。
- **新工具 `lookup_mechanic(id)`**：拉取完整条目（procedure/联动/source_refs）。
- **`roll_check` 升级**：可选 `mechanic_id` 参数——给定时从目录继承 tested_parameter/骰程序（agent 仍可自由发挥不给 id，目录是知识不是枷锁）。
- **工具结果 band 语义投影**：roll_check/结算结果中 band 不给裸 id，附 `success_bands[*].semantics` 渲染的语义行（"extreme——极难成功：贯穿/卓越效果"），agent 直接拿到"这个成功度意味着什么"来叙事分档。fail-closed：无 semantics 退回裸 band id+label。

### 5.2 引擎 watcher（MechanicDue）
trpg-mechanics 在效果落账单点（after_check_resolved / apply_direct_effect 之后）检测阈值穿越：
- `loss_in_one_go`（本次 delta 绝对值 ≥ 阈）与 `at_or_below`（穿越临界值）；
- 命中产出 `MechanicDue { due_id, source_track, threshold_desc, followup_procedure_id?, evidence(数值前后), turn_id }`，落账（账本+DB 事实）并作为工具结果/观察回填 loop。
- 无 followup_procedure_id 的阈值（目录缺失）仍发 due（带 prose consequence），agent 凭语义处理——fail-closed 但不静默。
- **EngineHook 事件点发 due**（§4 第三触发通路的运行时侧）：回合头部（turn_start）、navigate_scene 成功后（scene_enter）、advance_time 后（time_advance）等引擎事件点查询目录中挂接该钩子的条目，产出 MechanicDue 进同一债务通道。事件点实现分布在 trpg-gm（回合头部）与相应工具内（navigate/advance），检测逻辑统一在一个 hook 查询原语里（单点、按 ruleset 目录数据驱动）。

### 5.3 机械债务清单
trpg-gm loop 维护回合债务：`未结算的 CheckContract`（一期已有，升格为债务）+ `未处理的 MechanicDue` + **追溯债务（RetroactiveEffectDebt）**——流后 verifier 抓到 InventedEffect（叙事声称了伤害/资源/状态变化但账本无证据）时，除写勘误记忆外，同时生成追溯债务进下回合清单：补 `apply_effect` 落账，或 `waive_obligation` 带理由（如"叙事中已收回该说法"）。**落库保证由此从"靠 agent 自觉"闭环为"债务必清"**。规则：
- **债务非空 → 不进叙事轮**（loop 把债务清单作为 system 观察回填，要求处理）。
- 处理方式：roll_check / request_player_roll（结算）或 **`waive_obligation(due_id|check_id, reason, scope)`**（必须带理由；scope=turn（默认，下回合仍提醒）或 scene（本场景内不再提醒，场景切换时清除豁免）；记账本 + 勘误记忆 tags=["gm_waive"]）。
- 轮数耗尽仍有债务 → 强制叙事（一期 tool_choice none 语义不变），全部债务进下回合 BP3 + 勘误记忆——绝不静默丢失。

## 6. 支柱 3：场景机制意图（模组侧）

深抽 prompt 扩展（module_reader Pass B / deep_extract_scene_in_place）：场景文本**明确写出**的检定编译为：

```rust
// ScenarioNode.deep 新增
scene_mechanics: Vec<SceneMechanicIntent>

SceneMechanicIntent {
  intent_id: String,           // "homecoming.lawmen.cut_cable_force"
  description: String,         // 何种行动触发（语义）
  tested_parameter: String,    // "brawling"
  difficulty: Option<Json>,    // {kind:"dv", value:13} / CoC 难度档
  effect_policy: EffectPolicy, // on_success / on_failure → Vec<EffectPatchIntent>
  source_anchor: String,       // fail-closed：无页锚不编造
}
// EffectPatchIntent 映射到既有 StatePatch 词表：
// set_object_state / modify_track / create_fact / start_countdown(track 倒计时)
```

运行时：随 P4 场景投影进 BP2（当前场景的 intents 索引）；agent 开检定可带 `scene_mechanic_id`，结算后 **effect_policy 由 Rust 在 after_check_resolved 链路强制执行**（效果不留给叙事）；effect 落账自然进入 watcher/债务体系。fail-closed：模组没写明的不编造，agent 退回目录/裁量；`#[serde(default)]` 向后兼容旧图谱。

## 7. 与一期接缝

- 新工具 `lookup_mechanic` / `waive_obligation` 注册进 ToolRegistry（10→12）。
- MechanicDue 与债务清单进 BP3 动态块；waive 记账复用一期 errata 通道。
- gm_skill 裁量准则文件增两条（data 文件，非代码）：目录优先于自由发挥；due 必须回应（处理或 waive 带理由）。
- 缓存稳定回归测试扩展：BP1 含目录索引后 hash 仍按 ruleset 固定。
- 实施切片建议（计划阶段细化）：Slice A=支柱1 编译+跑批；Slice B=支柱2 注入+watcher+债务；Slice C=支柱3 模组侧。A 无运行时依赖可先行；B 依赖 A 的目录；C 不依赖 A/B（effect_policy 执行复用一期既有的 after_check_resolved/StatePatch 落账链），但与 B 同装时效果落账自然被 watcher 覆盖。

## 8. 测试与验收

**编译跑批**（Slice A）
1. 六套规则 mechanics_catalog 生成成功，validation_report 记录丢弃条目及原因。
2. CoC 断言：必含 sanity_check 与 temporary_insanity 条目且 followup 链通（sanity.thresholds.loss_in_one_go → temporary_insanity）；必含 jump（或语义等价跳跃技能）条目带 when_to_use。
3. 护栏：人为注入坏条目（tested_parameter 不存在）→ 被丢弃且报告。
3a. **覆盖率审计（发现开放性）**：编译后审计 agent 以规则书章节/目录清单对照 mechanics_catalog，找出"书中存在但目录遗漏的剧情配合机制"记入 validation_report；六套规则各抽查 ≥1 个**非预设类别**机制被收录（荣誉/令咒/堕落类——具体条目不写死，以审计报告为准），且至少一例纯语义条目（procedure 空）与一例 EngineHook 条目存在并可被运行时感知/触发。

**运行时单测**（Slice B）
4. watcher：合成结算使 SAN 单次 -6 → MechanicDue 产出（evidence 含前后值）；-4 → 无 due；HP 穿 0 → due。
5. 债务门控：有未处理 due 时 loop 不进叙事轮（MockLlm 断言回填了债务观察）；waive 带理由后放行且勘误记忆落账；轮耗尽强制叙事时债务进下回合 BP3。
6. lookup_mechanic / roll_check(mechanic_id) 继承绑定正确。

**e2e 黄金链**（真 DB 真 LLM）
7. **CoC SAN→疯狂全链**：目睹恐怖场景 → agent 放 SAN check（目录驱动，非玩家明示）→ 失败掉 SAN ≥5 → watcher due → agent 放临时疯狂检定**或** waive 带理由 → 全链 SQL 可查（check_contracts / generic_parameter_states / due 事实 / waive 记账）。
8. **跳坑感知**：玩家"我跳过裂隙"（不提检定）→ agent 调 roll_check 且其 tested_parameter 与**目录中跳跃语义条目所绑参数一致**（断言通过目录解析得出，不在测试里写死技能名单——理念护栏 §3.5）。
9. **Homecoming 场景意图**：切线缆 → DV13 检定 → 成功 → `athena_cable` 对象状态 + 倒计时 track 真实落库（effect_policy 强制执行，非叙事声明）。
10. **Triangle chaos 累积与感知**：连跑 ≥3 回合，`chaos_pool`（scene 级）current 在库中递增可查（SQL），且 BP3 投影含语义状态行；目录含 chaos 花费/异常体条目（kind=spend/subsystem_procedure），agent 在池子高位时的行动或叙事反映池子状态（花池制造异常或基调变化，凭账本/叙事证据判定）——验证"累计系统真的被记得并加强影响"。
11. **追溯债务闭环**：构造 agent 叙事声称伤害但未调工具的回合（MockLlm 脚本）→ verifier 抓 InventedEffect → 下回合债务清单含追溯债务 → 补 apply_effect 后库中状态真实变化。
12. **成功度三件套（CoC）**：① band 补全护栏——编译后 CoC success_bands 含 critical/failure（validation_report 记录补全动作）；② 机械触发——SAN 检定 fumble → 损失取最大值真实落库（解析出 band 触发则验机械路径；未解析出则该项记为目录缺口而非静默通过）；③ 语义投影——extreme 与 regular 成功的工具结果含不同语义行，e2e 叙事凭 band 语义证据分档（极难成功的叙事明显区别于常规成功）。
13. 缓存回归：目录注入后跨回合 prefix/pinned hash 不变（场景不变时）。

**工程约束**：文件 ≤400 行；零硬编码；全部新结构 `#[serde(default)]` 向后兼容。

## 9. 风险

1. **目录编译质量随规则书排版浮动**——fail-closed 兜底 + rule_kernel_patches 手修通道；validation_report 给出可观测性。
2. **BP1 体积增长**——索引分级（§5.1）；超预算视为配置错误不静默裁剪（一期 §6.1 原则延续）。
3. **场景意图依赖深抽质量**——模组管线既知波动；source_anchor fail-closed 防编造；旧模组不重抽也不受损（default 空）。
4. **债务门控可能拖长回合**（多一轮处理 due）——waive 出口保节奏；e2e 记录回合时长对比。

## 附录 A：七规则剧情配合机制勘查清单（2026-06-10，Slice A 覆盖率审计基线）

> 本清单由 7 本规则书并行勘查产出（CoC 7e / Cyberpunk RED / BRP-ORC / Sword World 2.5 / D&D 5e 中文各 17-22 条；Triangle Agency 与 Fate 圣杯战争补勘中，到货后追加）。**用途：§8 验收 3a 覆盖率审计的基线**——mechanics_catalog 编译结果应覆盖本清单条目，或在 validation_report 给出缺失理由。表达档：P=procedure，H=hook，PM=passive_modifier，S=semantic。

### A1. 社会评价 / 被动持续场（PM 为主——第四档的实证主体）

| 规则集 | 机制 | 触发 | 档 |
|---|---|---|---|
| CoC | Credit Rating 信用评级（生活水准/NPC 初见态度/可调动资源） | 被动持续 | PM |
| CPR | Reputation 声望（初遇 NPC 掷骰对比声望级）/ Facedown 压场加值 | GM 裁量授级 | PM |
| CPR | Operator（Fixer 黑市通路：Contacts/Reach/Grease） | 被动持续 | PM |
| CPR | Moto（Nomad 家族车库与信任） / Teamwork（Exec 公司资源包） | 被动持续 | PM |
| ORC | Reputation 声望 / Status 地位技能 / Wealth 财富五档 | 被动持续 | PM |
| SW2.5 | Reputation 名誉点数与冒险者 Rank（公会态度/委托档次） | session 结算累积 | PM |
| SW2.5 | 怪物 Disposition 初始态度场 / 语言系统沟通门槛 | 遭遇即生效 | PM |
| D&D | 背景特性 Background Features（神庙庇护/罪犯接头人——纯文本社会特权） | 场景条件 | PM |
| D&D | 生活方式 Lifestyle / 旅行步调 / 被动检定 / 游侠宿敌与地形 | 被动持续 | PM |

### A2. 精神 / 堕落 / 生存螺旋（数值穿越，watcher 主战场）

| 规则集 | 机制 | 触发 | 档 |
|---|---|---|---|
| CoC | Sanity 螺旋 + Bout of Madness + Underlying Insanity 两阶段 | 数值穿越（多级阈值） | P+S |
| CoC | Cthulhu Mythos 侵蚀（max SAN=99-CM）/ Mythos Hardened / 习惯恐怖 per-怪计数 / Believer 延迟理智债 | 数值穿越 | PM |
| CPR | Humanity Loss & Cyberpsychosis（EMP 实时联动，≤2 强制扮演） | 装件/创伤扣减穿越 | P+PM |
| ORC | Sanity（TIS 单位时间损失阈值 → 临时疯狂） | 数值穿越 | P |
| SW2.5 | Soulscars 魂之伤痕（1-5 级外观异变，≥5 角色归 GM） | 复活事件驱动穿越 | PM |
| D&D | 力竭六级螺旋 / 饮食需求计数 / 死亡豁免三成三败状态机 | 数值穿越+计数 | P+PM |

### A3. 累积池与元资源（花费换叙事/机制权）

| 规则集 | 机制 | 触发 | 档 |
|---|---|---|---|
| CoC | Luck 双形态（GM 命运裁定 + 玩家花费抵点） | GM 裁量+玩家宣告 | P |
| CPR | LUCK Pool（每 session 刷新，掷前宣告加值） | 玩家宣告 | P+H |
| SW2.5 | Sword's Grace 命运翻转（每日一次，骰后翻面） | 玩家宣告（掷后） | H |
| D&D | 激励 Inspiration（扮演四件套换优势，可转赠） | GM 裁量授予 | H |

### A4. 检定元规则（重掷 / 加注 / 改写——roll_check 流程的外环）

| 规则集 | 机制 | 触发 | 档 |
|---|---|---|---|
| CoC | Pushing the Roll 推骰（失败后叙述额外冒险换唯一重掷，失败代价加重） | 玩家行动语义 | H+S |
| CoC | Idea Roll 灵感骰（卡关保险丝：线索必给、roll 定代价） | 玩家请求/GM 裁量 | H |
| CPR | Trying Again 禁原样重掷 / Complementary Skills 互补检定 | 失败后挂条件 | H |
| ORC | Augment 辅助加注（成功升档、失败反噬复杂化） | 玩家宣告 | P |
| SW2.5 | Exceptional Skill Checks 叙事改写检定式 / 双1必败+50XP | 玩家提案/骰面事件 | S+H |
| D&D | 优势/劣势+GM 形势裁量（一切情境修正的单一开关）/ 合作与团体检定 | GM 裁量 | H+P |

### A5. 周期节拍器与结算点（EngineHook 日历/session 词表的实证依据）

| 规则集 | 机制 | 触发 | 档 |
|---|---|---|---|
| CoC | Investigator Development Phase（剧本末全局结算：成长勾/财务复查/理智恢复等 ≥9 项汇聚） | 章节结算 | H |
| CoC | 疯狂治疗月度结算 / 理智恢复四渠道 | calendar_monthly | H |
| CPR | 月初 Lifestyle/房租/Trauma Team 订阅 / 每周 Media 谣言×2 / 每 session LUCK+IP / 7 天 Hustle | calendar_monthly/weekly + session_end | H |
| SW2.5 | 每日 6:00 饮食睡眠结算 / session 终局目标判定→XP/报酬/欠片三岔 | calendar_daily + session_end | H |
| D&D | 短休/长休资源经济总闸 / 休整期活动 / 赶路递增豁免 | rest + 事件钩子 | H+P |
| ORC | Experience Checks 冒险末结算 / Allegiance 冒险末记点 / 衰老年度损耗 | development_phase + calendar | H |

### A6. 状态机与叙事授权转移（semantic 档承载——LLM-GM 天然可执行）

| 规则集 | 机制 | 触发 | 档 |
|---|---|---|---|
| CoC | Bout of Madness（玩家控制权移交 GM，表驱动失控行为+时间快进） | 阈值触发 | P+S |
| CoC | 重伤/濒死状态机（数周叙事停摆+照护场景） | 数值穿越 | P |
| CPR | Wound States + 致残（断肢→义肢→Humanity 回环）/ Facedown 压场二选一 | 阈值+GM 裁量 | P |
| ORC | Major Wounds 永久后遗症表 / Dying Blows 临死一击英雄时刻 | 阈值+玩家宣告 | P+H |
| SW2.5 | Death Check 濒死倒计时 / 复活仪式（记忆缺失+债务+禁忌观感） | 数值穿越+事件 | P |
| D&D | 死亡豁免 + 击晕选项（杀/俘虏瞬间抉择）/ 怪物死亡叙事分层（BOSS 例外规则） | 阈值+GM 裁量 | P+S |

### A7. 关系 / 羁绊 / 阵营义务（semantic 为主——角色卡语义字段的机制化）

| 规则集 | 机制 | 触发 | 档 |
|---|---|---|---|
| CoC | Backstory 条目与 Key Connection（一等机制对象：6+ 机制对其读写、有锁定权限） | 玩家动用/GM 改写 | S |
| CoC | Contacts 人脉（凭技能凭空召唤 NPC 进剧情，暗骰失败=背叛者入口） | 玩家宣告 | S |
| ORC | Passions 激情（Hate 95% 放走仇敌有机制代价）/ Allegiance 阵营忠诚 / 人格特质（NPC 行为引擎，可治疗改写） | 玩家请求+结算 | H+S |
| ORC | 永久 POW 牺牲与神恩交易（力量换剧情义务） | 玩家发起仪式 | S |
| CPR | 粉丝感召（路人→粉丝→favor 经济）/ Media Credibility（每周谣言流+发稿改写世界） | 行动语义+周期 | S+H |
| SW2.5 | Fellows 跨桌同伴卡（TPK 剧情兜底内建） | 玩家请求 | P |
| D&D | 四件套（特点/理想/牵绊/缺点——GM 明示可作剧情杠杆）/ 圣武士誓言与背弃 / 邪术师宗主 / 阵营 | 被动持续 | S |

### A8. 信息经济（暗骰 / 知识发放 / 线索阀门）

| 规则集 | 机制 | 触发 | 档 |
|---|---|---|---|
| SW2.5 | Monster Knowledge（成功=获读官方数据块权限，知识跨 session 持久）/ GM 指令型暗骰三连（Notice/Danger Sense/Spot Trap 玩家不可主动要求） | 遭遇钩子+GM 裁量 | P+S |
| CoC | Luck roll（世界此刻偏不偏心）/ 灵感骰代价制给线索 | GM 裁量 | P+H |
| D&D | 被动检定（决定玩家"自动注意到什么"而不暴露有东西） | GM 幕后比对 | PM |
| CPR | Media 被动谣言流（GM 每周有义务投喂剧情钩子） | calendar_weekly | H |

### A9. 世界生成 / 内容注入（机制产出场景内容）

| 规则集 | 机制 | 触发 | 档 |
|---|---|---|---|
| CoC | Chases 追逐（每个 location GM 即兴生成环境挑战链）/ 神话典籍研读（推进数周+SAN 注入）/ 法术副作用表 | 场景成立/玩家投入 | P+S |
| CPR | Maker 发明（GM 即席撰写新物品规则+"会有人来偷"预告）/ Night Market（Fixer 凭空造限时市集） | 玩家提案/GM 裁量 | S |
| SW2.5 | 浅渊场景规则（场景内容由 PC 欲望语义生成+限时撤离）/ 欠片强化怪物（强度锚进世界观+预埋奖励） | 场景级 | S+P |
| D&D | 狂野魔法浪涌 d100 表 / 神圣干预（效果全开放给 DM 叙事裁定） | 施法后/玩家呼求 | H |

### A10. Triangle Agency 追加（18 条，按类归档）

| 类 | 机制 | 触发 | 档 |
|---|---|---|---|
| A3 | Chaos 池累积（每掷 6d4 非 3 骰面进公共池；Burnout 额外+1） | 数值穿越 | P |
| A3 | Chaos Effects——GM 替异常体花池触发现实扭曲（1扭曲/4造小异常/8幻境/10杀凡人；Domain 内折扣） | GM 裁量 | H |
| A3 | QA/Burnout/Burnout Release（改骰面资源；Release 是语义条件：处于 Reality 定义的情境） | 数值穿越+语义 | P+S |
| A3 | Triscendence 完美传导（恰好三个 3 → 零 Chaos+公司心灵共感"表演"高光） | 骰面事件 | P |
| A4 | Ask The Agency 现实改写（共建因果链+至少一个 3；失败现实反击+地点持续 Burnout） | 玩家行动语义 | S+P |
| A4 | Conflict Resolution PvP 仲裁（时间冻结、常规规则全部失效的真空期） | 事件钩子 | P |
| A1 | 嘉奖/记过经济 + Prime Directive 职务条款（**忠诚扮演经济：GM 逐回合语义识别言行命中条款**） | 行为语义监听 | S |
| A1 | Relationships/Connection/Network（关系 NPC 由其他玩家扮演；Bonus 是下任务叙事资源） | 被动持续 | PM+H |
| A2 | Loose Ends 计数 + Weather Events（11/22/…/77 阶梯扮演限制直至强制退休、区域抹除） | 数值穿越阶梯 | P |
| A2 | Reality Trigger 软肋 + 崩坏 track（标满强制更换整个 Reality） | GM 裁量+计数 | H |
| A5 | 任务报告结算 + Final Defense 申辩（AAA~F 评级；结案后事件不可再引用） | session_end | P |
| A5 | Mission Superlatives（试用期/MVP/参与奖）+ Optional Objectives 简报可选目标 | session_end + 简报钩子 | P+H |
| A5 | Work/Life Balance 三轨 Time 成长（生涯 30 格倒计时） | 任务间结算 | P |
| A6 | Harm 与 Life Insurance（1 QA 完全免伤但旁观凡人成 Loose Ends；复活可改记忆/外观） | 事件钩子 | P |
| A9 | 异常捕获语义条件（不靠掷骰靠叙事方案：满足/说服/夺源）+ Playwalled Documents（条件触发私读隐藏规则） | 语义+事件 | S+H |

> Triangle 勘查印证与新增：① **行为语义监听**（嘉奖/记过经济）是 semantic 档的高强度用例——LLM-GM 的天然主场，gm_skill 准则须含"按 ruleset 条款持续评估玩家言行"；② "规则真空期"（Conflict Resolution、Ask The Agency 改写期）要求 loop 能按目录条目临时切换裁定模式（数据驱动，非硬编码）；③ Playwalled 内容提示目录须支持"条件解锁"条目（解析期收录但标记 locked_until 语义）。
### A11. Fate 圣杯战争同人追加（18 条，按类归档）

| 类 | 机制 | 触发 | 档 |
|---|---|---|---|
| A3 | 令咒系统（3 划一次性绝对命令；归零即出局）/ 魔力残余战利品 | 行动语义+数值穿越 | S+H |
| A1 | 魔力供给与现界维持（每时间段结算耗 MP，枯竭驱动龟缩/补魔剧情） | 被动持续 | PM |
| A1 | 环境魔力与灵脉（**scene 级被动场实证**：浓度决定回复上限；灵脉是可破坏的战略据点） | 被动持续 | PM |
| A9 | 宝具真名解放（准备一回合+宣称真名→**信息暴露账本更新**，弱点/情报连锁泄露） | 行动语义 | P |
| A8 | 信息暴露三层与猜测元规则（职阶/属性/真名分层；每次见面可猜）/ 英灵弱点（语义判定"针对弱点"） | 事件钩子+语义 | P+S |
| A2 | 圣杯战争阶段推进（**全局计数器穿越实证**：按从者存活数切阶段，5 骑退场圣杯显现+7 天限期） | 数值穿越（world 级） | H |
| A5 | 时间段日程（每天 8 段、每段一区域、多组同区碰面——GM 是日程调度器） | calendar 段级 | P |
| A7 | 隐秘规则与联合通缉（违规→悬赏令咒→敌我关系改写）/ 教会庇护与脱离程序 | 事件钩子+GM 裁量 | H+P |
| A7 | 阵营约束骰（OOC 行为语义判定→D6 决定执行/拒绝/反向）/ 狂化扮演约束（按等级限制对话能力） | GM 裁量 | S |
| A3 | 专注值 FP 扮演经济（贴合人设+1/天、OOC 扣 1；FP=检定加值——**行为语义监听第二实例**） | GM 裁量 | S |
| A1 | 序数字母评级剧情语义（评级=对外口径与知名度；带+可临时升级造叙事爆发） | 被动持续 | PM+S |
| A6 | 补魔与魔力让渡（体液补魔须扮演+耗时间段；MP 过载演出） | 行动语义 | P |
| A9 | Ruler 调停者特权（战争失衡时圣杯加召、神明裁决强制改写）/ 叙事型固有技能（黄金律=无限金钱、人类观察=直取 NPC 情报） | 事件钩子+GM 裁量 | H+S |

> Fate 勘查印证与新增：① 环境魔力（scene 级）与战争阶段（world 级）确证 **owner_kind 必须开放枚举**（actor/scene/party/world）；② FP 与 Triangle 嘉奖/记过构成"行为语义监听"双实证——gm_skill 准则必含"按 ruleset 条款持续评估玩家言行并发放/扣减元资源"；③ 信息差账本（PVP 多方知识集）超出当前单人产品形态，记为观察不入本期 scope；④ 时间段（一天 8 段）确证 calendar 钩子需支持 ruleset 自定义粒度（非固定日/周/月——粒度定义也是数据）。

**附录 A 终态**：7/7 书勘查完成，共 137 条机制归 11 节。覆盖率审计（§8 验收 3a）以本清单为基线。
> **勘查对设计的三条直接修正**（已反映在 §3.5/§4/§5）：① passive_modifier 立为第四表达档+常驻投影（五书全票实证）；② EngineHook 词表扩展 calendar_daily/weekly/monthly + session_end/development_phase（五书均有周期节拍器）；③ 叙事授权转移类归 semantic 档（LLM-GM 天然可执行，无需新机械层）。
