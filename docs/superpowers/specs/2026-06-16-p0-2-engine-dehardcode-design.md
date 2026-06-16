# P0-2 设计：引擎去规则集/模组硬编码（迁策略到 kernel/module 数据层 + 通用兜底 + CI 守卫）

日期：2026-06-16
状态：设计（自主执行；GPT Pro P0-2 + 项目头号理念"零规则集硬编码"）
前置：R1（execute_turn）+R2（Need bus）+R5（postprocess 分层）已合 main（ae3b05f）。grounding 见 `docs/GPT_Pro_审查_triage_2026-06-16.md` + Explore 审计（18 MIGRATE 点）。

## 1. 背景：引擎仍按规则集/模组名分支（违头号理念）
头号理念：引擎按**通用检定模型**（roll_under/meet_or_beat/count_faces…）分支，**绝不按规则集/模组名**；差异来自 parsed kernel/module 数据。但 grounding 查出 **18 处 MIGRATE 硬编码**（5 crate）：
- **trpg-combat（8）**：`infer_combat_mode_from_intent`（ruleset 分支选 Firefight/Netrun/HorrorEncounter…）、`{cyberpunk,dnd,coc,triangle,sword_world}_profile()`（动作经济/先攻/反应窗硬编码）、`inferred_homecoming_tech_dv`(→14/12)、`target_actor_for_combat_input`(关键词→npc.scav_boss/athena_drone)、bare dice 限定（contains 多分支）、check label（contains cyberpunk）。
- **trpg-referee（5）**：`ruleset_damage_family`/`common_damage_band`/`damage_plausible_for_ruleset`/`common_difficulty_band_json`/`difficulty_plausible_for_ruleset`（伤害/难度 family+band+合理性按 ruleset 分支）。
- **trpg-director（4）**：`is_homecoming()` + 据它生成 Homecoming 专属 scene facts/pressure/NPC advice/place summary。
- **trpg-material（2）**：`ruleset_aliases_for`（搜索 section/字段别名按 ruleset）、`module_preferences_for`（按 homecoming/masks/vault）。
- **trpg-object（1）**：check label（contains cyberpunk）。
legit-keep（2）：trpg-mechanics:854（env config 非硬编码）、trpg-gm opposed_prepass 测试 fixture。

根因：RuleKernel（trpg-model:1425）有 dice_core/resource_tracks 但**缺 combat/referee/search/mode 策略字段**；无 module 配置结构 → 引擎只能就地硬编码。

## 2. 已拍板决策（自主，按理念）

| 决策点 | 结论 |
|---|---|
| 迁移目标 | 扩 **RuleKernel** 加策略字段 + 新 **ModuleConfig**（或扩 ModuleGraph）；引擎改读数据、删 ruleset/module 名分支 |
| 兜底 | 所有新策略字段 `Option`/`#[serde(default)]`，引擎 `kernel.<p>.as_ref().unwrap_or(&GENERIC)` —— **未填→通用兜底（fail-soft，非规则集分支）**，不要求一次性全 parser 抽取 |
| 特例数据来源 | 现有硬编码的规则集/模组特例值迁进 **override 数据文件**（既有 kernel-override / ruleset_advice 模式，data/ 下），**非引擎码**；可后续由 parser/reader 自动抽取替代 override |
| CI 守卫 | 新增 `xtask`/脚本 grep：引擎 crate（runtime/gm/combat/referee/director/object/mechanics/material/contest/orchestrator/semantics/interaction）的 src **不得出现规则集/模组名字面量**（白名单：parser source-id 推断、`#[cfg(test)]`、data override 加载键） |
| 落地 | 分阶段按 crate（combat→referee→director→material→object），每阶段加字段+通用默认+迁 helper+override 数据+等价验证 |

## 3. 目标 / 非目标
**目标**：18 MIGRATE 点全部从引擎码迁到 kernel/module 数据 + 通用兜底；引擎 grep 零规则集/模组名（除白名单）；现有规则集/模组行为经 override 数据保持等价（live 验证）；CI 守卫防回潮；零硬编码、fail-soft、文件 ≤400 行。

**非目标（本期）**：parser/reader 自动抽取这些策略进 kernel（本期用 override 数据 + 通用默认；自动抽取留后续，与 reader 联动）；R3 字段级 verifier；R4 ModuleEntity 类型化（本期 module 配置可先用轻量结构，与 R4 不冲突）；改通用检定模型本身。

## 4. 设计

### 4.1 RuleKernel 策略字段（trpg-model，serde default→通用）
新增（均 `Option`/`#[serde(default)]`，向后兼容旧 kernel）：
- `combat_profile: Option<CombatProfile>`（动作经济/先攻/反应窗/搜索配方——替 *_profile() 五函数）。
- `combat_mode_policy: Option<CombatModePolicy>`（意图→CombatMode 的数据映射——替 infer_combat_mode_from_intent 的 ruleset 分支；CombatMode 枚举保留，选择数据化）。
- `check_label_policy: Option<CheckLabelPolicy>`（检定 label 模板——替 combat/object 的 contains cyberpunk label）。
- `referee_value_bands: Option<RefereeValueBands>`（damage_family/damage_band/difficulty_band + 合理性范围——替 referee 五函数）。
- `dice_qualification: Option<DiceQualification>`（bare dice 限定规则，如 1d10→1d10+0——替 combat contains 分支；或并入 dice_core）。
通用 `GENERIC_*` 常量（中性默认）：未填字段时引擎用它，**不按 ruleset 分支**。

### 4.2 ModuleConfig（轻量，与 R4 ModuleEntity 不冲突）
新增 module 级配置（存 module bundle，`#[serde(default)]`）：
- `npc_actor_bindings: Vec<NpcActorBinding>`（关键词/语义→actor_id——替 target_actor_for_combat_input 的 npc.scav_boss/athena_drone）。
- `technical_option_table: Option<Vec<TechOption>>`（tech DV 等——替 inferred_homecoming_tech_dv 14/12）。
- `scene_entity_aliases / module_search_profile`（替 director scene facts、material module_preferences）。
director 的 Homecoming 专属 scene facts/advice → 应来自 **module parse 的 scene/NPC deep 数据**（已有 ScenarioNode deep 字段），非 is_homecoming() 硬编码。

### 4.3 引擎改造模式（每 helper）
`fn x(ruleset_id) { if contains("cyberpunk") {A} else if contains("dnd") {B} ... }`
→ `fn x(kernel: &RuleKernel) { kernel.combat_profile.as_ref().unwrap_or(&GENERIC_COMBAT_PROFILE).<field> }`。
helper 签名从 `ruleset_id: &str` 改收 `&RuleKernel`（调用点已 load kernel——见 grounding 的 load_rule_kernel 路径）。6 个 shared helper（is_homecoming/inferred_homecoming_tech_dv/target_actor_for_combat_input/infer_combat_mode_from_intent/ruleset_damage_family/ruleset_aliases_for）全转数据查找。

### 4.4 override 数据（特例值的家）
现硬编码的特例（cyberpunk profile、Homecoming DV14、masks/vault search pref 等）迁进 data override 文件（既有 kernel override / ruleset_advice / module bundle 机制）。引擎 load kernel 时合并 override（已有 merge_override 模式）。**等价验证**：迁移后六规则集/三模组的相关行为与迁移前一致（live + 单测）。

### 4.5 CI 守卫
`xtask no-engine-ruleset-hardcode`（或 scripts/）：grep 引擎 crate src 的规则集/模组名字面量集合，命中即 fail；白名单=parser source-id 推断函数、`#[cfg(test)]` 块、data override 加载键常量。纳入 CI/pre-merge。

## 5. 理念守卫（MUST）
1. **零规则集硬编码**：迁移后引擎 grep 零规则集/模组名（白名单除外）——CI 守卫强制；
2. **fail-soft 通用兜底**：未填策略→GENERIC 默认（中性、非规则集分支），不 panic、不退化质量到不可玩；
3. **数据驱动**：特例进 override 数据/parsed kernel，不进引擎码；
4. **等价不回归**：override 数据让现规则集/模组行为保持（live + 单测对比）；
5. 文件 ≤400 行；新结构 `#[serde(default)]` 向后兼容旧 kernel/bundle。

## 6. 测试与验收
**单测**：每 helper 转数据查找后——给定 kernel.combat_profile=X → 选 X；缺字段 → GENERIC；override 合并正确。CI 守卫脚本自测（注入一个假硬编码→脚本红）。
**等价验证（核心闸）**：六规则集（CoC/Cyberpunk/D&D/SW/Triangle/Fate）+ 三模组（血色公路/Homecoming/Vault）迁移前后相关行为等价——combat mode 选择、damage/difficulty 合理性、search profile、NPC 绑定、tech DV、director scene brief。live（真库）+ 单测。
**e2e**：真库回合（CoC + Cyberpunk Homecoming）跑通，combat/referee/director/material 路径经数据驱动、行为不变、零 panic。
**守卫**：CI grep 引擎 src 规则集/模组名=0（白名单外）。
**工程**：文件 ≤400；零回归；override 数据齐。

## 7. 风险
1. **行为等价漂移**（通用默认 ≠ 原硬编码特例）——override 数据精确复刻原值 + 逐规则集/模组 live 等价验证为硬闸；
2. **跨 5 crate 回归面**——分阶段按 crate、每阶段等价验证 + 全套件零回归；
3. **helper 签名改（ruleset_id→&RuleKernel）触调用点**——调用点已 load kernel（grounding 证实），机械改 + 编译护；
4. **override 机制覆盖度**（module 级 override 是否已有加载路径）——实施先确认 module bundle override 加载，缺则轻量补；
5. **director Homecoming facts** 本应来自 module deep parse——若 deep 数据不全，过渡用 module override，标 reader 后续抽取。

## 8. 工作量与切分（分阶段，按 crate）
约 3-5 天。实施序（同一分支或逐 crate worktree，最后过等价闸）：
1. trpg-model：RuleKernel 策略字段 + ModuleConfig + GENERIC_* 默认 + serde default（+ 单测）。
2. trpg-combat（8 点，最大）：6 helper 转数据查找 + override 数据（cyberpunk/dnd/coc/triangle/sword_world profile、Homecoming DV、NPC 绑定）+ 等价验证。
3. trpg-referee（5 点）：damage/difficulty bands → referee_value_bands + override + 等价。
4. trpg-director（4 点）：is_homecoming 去除 → module deep 数据 / module override + 等价。
5. trpg-material（2 点）+ trpg-object（1 点）：search profile / check label → 数据 + 等价。
6. CI 守卫 xtask + 全等价闸（六规则集三模组 live + 零回归 + grep=0）→ 合并 main。
</content>
