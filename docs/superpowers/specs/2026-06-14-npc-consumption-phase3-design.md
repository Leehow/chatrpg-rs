# NPC 消费层 Phase 3 设计：比大小战斗对抗 + 引擎对抗语义预 pass

日期：2026-06-14
状态：设计已获用户逐项确认（引擎语义预 pass 兜底 + 双保险 / 下游通用模型支路 / fail-closed）
前置：NPC 消费层 Phase 1（lazy 合成，`2026-06-09-npc-card-lazy-generation-design.md`）+ Phase 2（消费层 CoC opposed live 验，`2026-06-09-npc-value-consumption-design-v2.md`，§9 明确把 combat-attack 对抗、meet_or_beat live 验列为范围外）。本期 = Phase 3，把 Phase 2 验证过的 roll_under opposed 通路扩展到 meet_or_beat 模型 + agent 路径战斗攻击。

## 1. 背景：产品评测坐实的双层缺口

2026 产品评测（eval_v3）+ 真 agent 路径坐实（session_5fa841db，:54346，path_confirmed=agent_path）：玩家在赛博朋克战斗中攻击敌人，**dice/frame/不索骰都正常，但 NPC HP 零变化**——`damage_packets=0 / parameter_impacts=0 / NPC hp_current=NULL`。根因是**两层缺口叠加**：

- **上游（AI 决策）**：GM 调 `roll_check` 5 次全没传 opposed 参数（`target_actor_id`/`opponent_tested_parameter`），只传攻击方 `pc.current`——把对抗攻击当成了单方检定，防御方 NPC 从未绑进契约。
- **下游（引擎执行）**：即便填了，contest 的 meet_or_beat 分支（`trpg-contest/src/lib.rs:220-222`）只有一行 `tnum.map()`，**结构上没有"读对手防御值"的代码**；roll_under 分支（:173-219）有（百分比检定目标值=卡上数值，被迫读卡）。

### 1.1 关键架构澄清（理念对齐）
引擎按**通用检定模型**（roll_under / meet_or_beat / count_faces…）分支，**不按规则集名**分支——规则集用哪个模型是解析出的 kernel `dice_core.compare` 数据。缺口在"比大小（meet_or_beat）"这个**通用模型层**，受影响的是**所有用比大小的规则集**（赛博朋克、剑世界），修复也落在通用层。零规则集硬编码，符合项目根本理念。

### 1.2 架构边界（为何 AI 知道却办不到）
一期铁律：AI 只"知道+下指令"（语义决策、填工具参数），引擎才"取数据+算+落库"（防 AI 编造机械结果）。所以 AI 即便语义上知道"攻击要比对手防御 DV"，也只能经 roll_check 把意图传给引擎；引擎对 meet_or_beat 的"取对手防御值"执行支路是空的 → AI 替代不了，必须补引擎执行端。本期修复正落在这条边界上。

## 2. 已拍板决策

| 决策点 | 结论 |
|---|---|
| 上游 opposed 兜底 | **引擎对抗语义预 pass + gm_skill 提示双保险**（呼应 SAN stimulus 预 pass 已验证模式：0/5→4/6）。预 pass 内部是 AI 语义判断（非关键词），通用、兜底不抢主 GM 的活 |
| 下游读卡 | **给 meet_or_beat 通用模型补"读对手防御值"支路**，对称 roll_under 分支；所有比大小规则集共享，零硬编码 |
| target 解析 | **语义解析**（场景 referenced_npc_ids / current_check_npc_persona），**非关键词扫**——治掉 Phase 诊断指出的 `npc.opposition` 占位符坑 |
| fail-closed | 现搓不成/查不到防御值 → 诚实保留 Provisional，**绝不乱绑平衡值**；且 provisional **不再静默 miss**，给 AI `awaiting_binding` 信号改道（找 DV / 叙事降级 / request_player_roll） |

## 3. 目标 / 非目标

**目标**：agent 路径下，比大小规则集（CPR/剑世界）的战斗攻击命中后伤害真落 NPC HP；NPC 防御值经 Phase 1 现搓供消费；全程零规则集硬编码、语义优先、fail-closed 不乱绑；CoC roll_under 回归零影响。

**非目标（本期）**：多人 PVP；count_faces（Triangle）的对抗（另期）；退役 legacy conflict kernel（旧路径继续 A/B）；战斗 AI 的战术深度。

## 4. 设计（三件套）

### 4.1 上游：对抗语义预 pass（双保险）
- **新预 pass**（呼应 `trpg-gm/src/stimulus.rs` 模式，新建或扩展）：回合头部一次小 LLM 语义判断——「玩家这回合是否在攻击一个场景中的对手」。命中 → ㈠按 `check_param_need` 的 **meet_or_beat→defense/evasion/DV 映射**经 `ensure_npc_parameter`（Phase 1 通路）现搓防御方 NPC 防御参数落卡；㈡为本回合 roll_check 契约预备 opposed 参数（`target_actor_id`=语义解析的真实场景 NPC id、`opponent_tested_parameter`=防御键）。内部 AI 语义判断，零关键词表；通用（任何"攻击对手"场景），fail-closed（模糊不判，MAX 封顶，呼应 stimulus）。
- **gm_skill 提示**（纯 AI 那道）：40_mechanics_catalog.md 或战斗 mode 准则加一段——"攻击一个有目标的对手时，以 opposed 形态调 roll_check（填 target_actor + 对手该测的防御键）"。GM 自己填对走 GM 的，漏了预 pass 补。
- **接入点**：roll_check 工具构造契约时，若 args.opposed 缺失但预 pass 备好了对抗参数 → 注入（与 GM 显式 opposed 同走 `stamp_opposed_check`）。语义 target 解析替换 `trpg-combat:1028-1058` 的关键词扫（或在 agent 路径绕过它）。

### 4.2 下游：meet_or_beat 模型补读卡
- `trpg-contest/src/lib.rs:220-222` meet_or_beat 分支照 roll_under 分支（:173-219）补 opposed 判定：`target_actor + opponent_tested_parameter` 都在时，读防御方 NPC 卡防御值（`resolve_percentile_target_for` 的 meet_or_beat 等价），建 `OpposedRoll`（compare 方向交 `opposed.rs`，它已支持 meet_or_beat=`total>=value`，现因没人构造 OpposedRoll 而是死代码）。无 target_number 时不再直接 None。
- 激活 `opposed.rs` 对 meet_or_beat 的既有支持（side_succeeds: total>=value），让防御方 NPC 值真参与命中判定。

### 4.3 fail-closed 不变量
现搓不成 / 无目标 / 防御值仍缺 → 维持诚实 Provisional（不乱绑），但 `resolve_against_model` 对 attack Provisional 返回显式 `awaiting_binding` 信号（而非默默 success=null→miss）→ GM agent 收到后改道（retrieve_rules 找 DV / request_player_roll / 叙事降级）。把"沉默 miss"升级为"显式待绑"。

## 5. 理念守卫（MUST）
1. **语义优先**：预 pass 内部 AI 语义判断 + target 语义解析，零关键词路由；
2. **零规则集硬编码**：修复落在 meet_or_beat 通用模型层 + check_param_need 映射数据，CPR/剑世界共享，grep 不得出现规则集名于逻辑；
3. **fail-closed 不编造**：查不到防御值不乱绑平衡值，诚实 Provisional + 改道；
4. **AI 决策/Rust 执行边界**：AI 经工具下指令，引擎落账，绝不让 AI 绕过引擎改库。

## 6. 测试与验收
**单测**：① contest meet_or_beat opposed 分支真结算（合成 CPR kernel + NPC 防御卡 → 非 provisional、读到防御值）；② 对抗预 pass 攻击意图检测 + opposed 补参（合成场景 NPC，语义判定）；③ actor-id 语义对齐（不落 npc.opposition 占位符）；④ check_param_need meet_or_beat→defense 映射；⑤ CoC roll_under 回归零影响；⑥ fail-closed（无防御值 → awaiting_binding 非静默 miss）。
**e2e 黄金链**（真 :54346 agent 路径）：CPR Homecoming 真战斗 → 预 pass 补 opposed → contest meet_or_beat 读 NPC 防御 → 命中判定 → **伤害落 NPC HP**（damage_packets/actor_mechanical_states 真变化），对标 Phase 2 `live_npc_opposed` 的 meet_or_beat 版。剑世界（meet_or_beat 第二规则集）单测验通用性。
**工程**：文件 ≤400 行；零 per-ruleset 硬编码；新结构 #[serde(default)]。

## 7. 风险
1. **预 pass 误判**（小 LLM 语义幻觉）——fail-closed 兜底（现搓不成不乱绑 + waive + MAX 封顶），最坏退回主 GM 自填，不造成乱绑值；
2. **CPR 无模组真实 DV**（kernel target_number=null）——靠 NPC 卡现搓防御值消费，单测兜底 live 受限场景；
3. **meet_or_beat 防御键的语义映射**（哪个参数是"防御值"因规则集而异）——经 check_param_need 数据映射 + 现搓，不写死；
4. **预 pass 成本**（多一次小 LLM/回合）——与 SAN stimulus 同款增量，可控；只在检测到攻击意图时触发现搓。

## 8. 工作量
约 2-3 天：对抗预 pass（trpg-gm）+ check_param_need 映射 + roll_check opposed 注入 + contest meet_or_beat 补读卡（trpg-contest）+ ensure_npc_parameter 防御现搓 + 单测 + CPR e2e 黄金链。建议拆两步：先下游 contest 补读卡（手工 opposed 契约单测验）→ 再上游预 pass 接通（e2e 验全链）。
