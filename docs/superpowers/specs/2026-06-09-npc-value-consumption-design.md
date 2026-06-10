# NPC 合成值消费层设计 (contest 结算真正消费 NPC 值)

> 日期: 2026-06-09 · 状态: 设计中, 经 4-reader 架构探查 · 主线: v1.20-formula
> 前置: NPC 卡懒生成 Phase 1 (合成值已写到 NPC 卡, 但**没被结算消费** —— live 验证确认)
> 目标: 让 Phase 1 现搓的 flagged-provisional NPC 值**真正驱动 check 结算**。

## 1. 背景与三个定论 (代码实据, 见 §9 锚点)

Phase 1 的 live 验证 + 架构探查确认: 合成值写到了 `npc.opposition` 卡, 但**结算层吃不到它**, 原因有三:

1. **对抗掷骰 (OpposedRoll) 整个 unbuilt** (typed stub): `CheckResolutionModel::OpposedRoll` 携带
   `attacker_expression / defender_expression / defender_actor_id` 但**零读取者**; `resolve_against_model`
   对它返回 `(None,None,None)` (contest:305), 被 narrator 当"无判定, 只叙述动作"。攻击方只掷一次,
   **防御方从不掷, 两骰从不比较**。→ roll_under 规则 (CoC/ORC) 无任何消费防御方值的机制。
2. **唯一能消费防御方值的 = `AttackVsDefense.defense_value`, 且只从契约 `CheckTargetModel::StaticNumber.value`
   读** (contest:209-211, 298), **不读 NPC 的 runtime_actor_parameters**; `defender_actor_id` 仅记账。
3. **pre-existing 真 bug**: `derive_tested_source` (contest:472-519) **先匹配 resource track**, 而 CoC `hit_points`
   轨的 `on_outcome.check_match:"damage|attack"` 把别名 **"attack"** 赋给该轨 → **任何攻击检定先命中 HP 轨**
   (value 0), 在匹配到真战斗技能前就 return。→ CoC 攻击方 roll_under 本身就坏 (测成 HP=0)。

各规则 compare 现实: CoC/ORC=`roll_under`(有模组); 剑世界=`meet_or_beat`(target_number=0); Cyberpunk=
`meet_or_beat`(target_number=null→Provisional); Triangle=`count_faces`(target_face=null, 解析坏); D&D=无 compare;
Fate=无 kernel。**唯一既有模组、又能 live 验的 = CoC/ORC (roll_under) → 必须建对抗结算才谈得上消费。**

## 2. 设计决策

让"NPC 值驱动结算"成立 = 三件套, 全**数据驱动、零 per-ruleset 硬编码**:
1. **建通用对抗掷骰结算** (roll_under / 显式 opposed): 防御方真掷其 (合成) 值, 两侧按 kernel `success_bands` 比
   degree → 胜负。
2. **修 `derive_tested_source` 的 HP-轨误绑**: `on_outcome.check_match` 是**出场结算路由**别名 (命中后扣谁的 HP),
   **不是 tested-param 选择**别名; 从 tested-source 匹配里剔除它。
3. **绑防御方合成值进契约** (meet_or_beat → `StaticNumber`; opposed → defender tested value): 在
   `hydrate_combat_check_contract_from_rule_steward` (combat:332-355) 这个已有 `target_actor` 在手的定型点绑。

## 3. 核心抽象

### 3.1 对抗掷骰结算 (新机制, 通用)
当 check 目标是 NPC 且解析模型是对抗 (roll_under 规则下攻防同测, 或显式 `CheckTargetModel::Opposed`):
- **攻击方**: 现有单骰 (`resolve_roll_input`) + 其 tested value (修复后正确, §3.2)。
- **防御方 (新)**: 用防御方 `defender_expression` 掷第二骰 + 解析**防御方 actor** 的 tested value
  (Phase 1 合成写在 NPC 卡上的那个技能/属性) —— 把 `resolve_percentile_target` 泛化成**按 actor_id 取**
  (现在写死 `contract.initiator.actor_id`, contest:170-172), 让它也能取 `defender_actor_id`。
- **比较 (通用, 复用 `success_bands`)**: 两侧各算 `success_tier_for` (已存在, contest:47-52), **比 tier 秩**:
  高 tier 胜; 同 tier → 比 margin (roll 距其值的差); 平 → 防御方胜 (data-configurable 默认, 不硬编码具体规则)。
  输出 `(target=None, success=Some(attacker_wins), degree=Some(对抗 degree))` —— 替掉现在的 `(None,None,None)`。
- **fail-closed**: 防御方 tested value 缺失 (NPC 卡没有该参数且合成关) → 仍回 Provisional (不编造)。
- **数据驱动**: 比较逻辑只读 kernel `success_bands` + compare; 无 `if ruleset`。

### 3.2 tested-source 修复 (真 bug)
`derive_tested_source` 的 track 匹配循环 (contest:490-509) **不再把 `on_outcome[].check_match` 拆进 track 别名**
(那是出场效果路由, 非 tested-param)。track 仅当 tested_parameter / check 显式命名其 `id`/`name` 时才作 tested
source; 否则继续匹配 skills/stats (contest:512-517)。→ CoC 攻击测真战斗技能, 不再 HP=0。
(独立真 bug, 但也是对抗结算消费攻击方/防御方技能的前置。)

### 3.3 防御方值绑定 (consumption hook)
`hydrate_combat_check_contract_from_rule_steward` (combat:332-355) 已有 `check.target_actor` (NPC, combat:1080):
- **meet_or_beat 规则**: 读 NPC 卡的合成 defense → `check.target = StaticNumber{value, label:"npc synthesized defense"}`
  → 现有 `AttackVsDefense` 消费它。
- **roll_under / opposed 规则**: 读 NPC 卡的合成对抗技能 → `check.target = Opposed{opponent_id, opponent_check}`
  → §3.1 对抗结算消费它。
- NPC 值经 Phase 1 的 `ensure_npc_parameter` 现搓 (这里**触发/读取**, 与 Phase 1 同一张卡)。**合成的参数名必须
  对齐消费模型** (roll_under→对抗技能如 Dodge/闪避; meet_or_beat→defense DV) —— 回头收紧 Phase 1 的
  `check_param_need` 按 compare 模型选参数。

## 4. 与 Phase 1 的衔接
Phase 1 `check_param_need` 现在 attack→`(stats,defense)`。本层让它**按 ruleset compare 模型**选: meet_or_beat→
defense DV; roll_under→防御方对抗技能 (闪避/招架/察觉)。`ensure_npc_parameter` 仍负责现搓+写卡 (T1>T2>T3);
本层负责**绑进契约 + 对抗结算消费**。

## 5. 不变量 / 护栏
1. **零 per-ruleset 硬编码**: 对抗比较只读 kernel `compare`/`success_bands`; 绑定只读 NPC 卡 + kernel; 无 `if ruleset`。
2. **fail-closed**: 防御方值缺失 → Provisional (不编造); 合成值仍 flagged-provisional (Phase 1 不变量延续)。
3. **退役关键词**: `target_actor_for_combat_input`/`infer_target_actor` 的 NPC-id 关键词扫 (combat:1027, object:749)
   应改用语义 target 解析 (与 NPC Phase 2 重叠) —— **本层标注依赖, 不强求同期** (保留作 fallback)。
4. **单一真相**: NPC 值在 `sheet_json` (Phase 1); 本层只读不另存。
5. **tested-source 修复不回归**: 现有 percentile 测试 (`percentile_reads_real_actor_value_not_fifty` 等) 仍绿。
6. **文件 ≤ ~400 行**: 对抗结算切片独立。

## 6. 范围
**内**: 通用对抗掷骰结算 (§3.1) + tested-source 修复 (§3.2) + 防御值绑定 (§3.3) + Phase 1 `check_param_need`
按 compare 模型选参数。**Live 验证在 CoC 模组** (有 NPC 场景)。
**外**: 退役 combat/object 的 target_actor 关键词扫 (NPC Phase 2); Triangle `count_faces` 解析修复 (`target_face`
未抽, 另案); D&D/Fate 无 kernel compare (另案); SavingThrow 模型 (从未构造, dead, 不动)。

## 7. 组件落点
| 模块 | 改动 |
|---|---|
| `trpg-contest/src/lib.rs` | `resolve_against_model`/`resolve_outcome` 加 OpposedRoll 真结算 (掷防御方+比 tier); `resolve_percentile_target` 泛化按 actor_id 取; `derive_tested_source` 剔除 on_outcome 别名 |
| `trpg-combat/src/lib.rs` | `hydrate_combat_check_contract_from_rule_steward` 绑防御方合成值 (meet_or_beat→StaticNumber / roll_under→Opposed) |
| `trpg-runtime/src/npc_synth.rs` 或 lib.rs | `check_param_need` 按 ruleset compare 选参数 (defense DV vs 对抗技能) |
| (防御方掷骰) | 对抗结算需要第二次掷骰 —— 复用现有掷骰原语 (resolve_roll_input 在 runtime; 若 contest 无掷骰能力, 防御骰由调用方/runtime 预掷并随 contract 传入, 保 contest 不引随机源) |

## 8. 风险 / 开放问题
1. **掷骰归属**: contest 当前消费单骰 (调用方掷)。对抗需要第二骰 —— 决定: 防御骰在 contest 内掷 (引随机源) 还是
   由 runtime 预掷传入 (保 contest 纯)? 倾向**后者** (runtime 预掷防御骰 + 防御方值, 随 contract/解析传入,
   contest 只比较) —— 与 contest 现有"纯 + 单骰"架构一致。**实现期定**。
2. **roll_under 对抗的胜负规则**: 同 tier 平局归属、margin 比较 —— 默认数据可配 (success_bands 的 rank), 但
   具体规则书可能有细则 (CoC: 对抗时高成功等级胜)。本层做**通用 tier 比较**, 细则留 override。
3. **live 验证只在 CoC/ORC**: meet_or_beat 绑定 (剑世界/Cyberpunk) 无模组没法 live 验, 单测覆盖 + 标注。
4. **Phase 1 参数对齐**: `check_param_need` 改成按 compare 选参数后, 之前 live 验证写的 `stats.defense`(对 CoC 是
   错参数) 应改为对抗技能 —— 收紧后重验。

## 9. 探查锚点
opposed unbuilt: contest:216 (唯一构造), 295-307 (no-op), 377 (kind label); api:1755 / cli:2166 (当无判定叙述)。
defender 消费: contest:209-211/298 (AttackVsDefense 从 StaticNumber.value), 357-361 (defender_for_contract 仅记账)。
契约 target 定型: combat:332-355 (hydrate, 现绑攻击方 DV), 1080 (target_actor 设), 1027-1057 (关键词扫 NPC id);
runtime:940 (kernel 默认); model:2408-2426 (target_model_from_dice_core: meet_or_beat→StaticNumber, roll_under→None)。
compare 映射: contest:129-163 (kernel_resolution_model)。tested-source bug: contest:472-519, 490-499 (on_outcome
别名注入), 504-505 (匹配), kernel `hit_points.on_outcome.check_match:"damage|attack"`。
percentile 取攻击方: contest:170-172 (写死 initiator)。success tier: contest:47-52 (success_tier_for, 复用)。

## 10. v2 必改 (对抗评审, 落地前) —— 评审揭出 3 块被我误标"复用"实为净新增 + 1 处会破 SAN

1. **[BLOCKER] 对抗第二骰不可经现有类型穿过**: `resolve_outcome(contract, roll)` 只收一骰; `OpposedRoll`
   只带 `defender_expression`(骰式串)非 total。要么给 `OpposedRoll` 加 `defender_total`/`defender_roll` 字段,
   要么 `resolve_outcome` 加第二骰参数 + 改全部 5 处调用点 (runtime:394/477/929/983 + forced-tech test)。
   **这就是设计本体, 不能留"实现期定"。** (§3.1/§8.1 必须写死方案。) 取防御方"值"可行 (按 actor_id 泛化
   resolve_percentile_target); 难的是"第二骰"。
2. **[BLOCKER] §3.2 不能整删 on_outcome 别名 —— 会破 SAN**: `sanity_check_resolves_to_the_sanity_track`
   靠 `check_match` 把 `理智`→sanity 轨 (轨 id=sanity 不含"理智")。修法必须**外科**: 只剔除"动作名"出场动词
   (attack/damage/fire), 或"被该动作触发扣减(op:subtract)的轨不作该动作的 tested source", **保留** SAN 式
   "这条轨就是被测对象"的别名。§5.5 与现 §3.2 自相矛盾, 一并改。
3. **[BLOCKER] CoC→Opposed 是净新增**: 现无任何代码把 CoC 攻击建成 `Opposed` (roll_under→percentile of
   initiator)。hydrate 必须**新构造** `CheckTargetModel::Opposed{opponent_id, opponent_check}`; live-verify
   依赖三件 (§3.1 对抗结算 + §3.2 修复 + 此新绑) 同落, 非两件。
4. **[MAJOR] §3.3 hook 现在不读 NPC 卡**: `hydrate_combat_check_contract` 读的是**攻击方** chargen pack
   (`resolve_attack_dv`)。绑防御值 = 新增 `load_actor_parameters(session_id, target_actor.actor_id)`; 且要解决
   与 fire-and-forget `prepare_npc_for_check` 的**先写后读时序**。
5. **[MAJOR] actor-id 分叉**: Phase 1 写 `npc.opposition`; combat 关键词扫可能给 `npc.scav_boss` → 取防御值
   miss → fail-closed。本期就要让两路 id 对齐 (或 id 不等时显式 Provisional), 不能推 Phase 2。
6. **[MAJOR] `check_param_need` 看不到 compare 模型**: 现仅收 `action_kind`。要按 compare 选参数得把 kernel
   线进它 + 改 CLI 调用点 + 重写 `check_param_need_maps_typed_action_kind_to_bucket_param` 测试。
7. **[MINOR] 平局"防御方胜"是潜在硬编码**: 无 kernel 字段编码对抗平局归属。要么从数据出, 要么诚实标成
   "可 override 的引擎约定", 别称"data-configurable"。
8. **[MINOR] 新模型过 pre-contest 门 + profile 缓存语义**待核 (`contract_missing_source_backed_parameters`;
   `ensure_contest_profile` 按 check_id 缓存早返)。
评审确认对的: HP-误绑诊断、`success_tier_for` 复用做跨side 比较、fail-closed 姿态、hook 位置选对。
**结论: 这是一个比 spec 所述更大、更精细的 contest 结算子项目 (净新增: 第二骰模型+签名改、roll_under→Opposed
构造、hydrate 读 NPC 卡), 且 contest 正确性高风险 —— 值得专门、清醒地做一遍 (spec v2 → plan → build → live验)。**
