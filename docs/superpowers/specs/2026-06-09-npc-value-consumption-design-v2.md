# NPC 合成值消费层设计 v2.1 (contest 结算真正消费 NPC 值)

> 日期: 2026-06-09 · 状态: v2.1 落地稿 (v2 经第二轮对抗评审,折叠 2 BLOCKER + 4 MAJOR + 4 MINOR) · 主线: v1.20-formula
> 取代链: v1 (`2026-06-09-npc-value-consumption-design.md`,诊断 + §10 首轮评审) → v2 (写死 §10 八条) →
> **v2.1 (本稿,落地按此)**。无 git → checkpoint = `cargo test`。代码事实逐条核对(见 §12 锚点)。

## 0. 一句话目标
Phase 1 把 NPC 参数现搓写到了 `sheet_json`,但**结算吃不到**。本层补齐一条**端到端通道**:
**现搓 → 投影到 mechanical_profile → 盖章进契约(target_actor + 双方 tested key)→ contest 建对抗结算 → 比 tier 出胜负**。
全程数据驱动、零 per-ruleset 硬编码、fail-closed。Live 验在 CoC(唯一既有模组又 roll_under 的规则)。

## 1. v2.1 相对 v2 改了什么(第二轮评审折叠)

第二轮对抗评审(general-purpose agent,代码实据见 §12)在 v2 上揪出 2 个真 BLOCKER + 4 MAJOR + 4 MINOR。
**v2 的方向对(第二骰方案 A、trigger:always 保 SAN、平局诚实标注),但"消费链最后一寸断了两处"**:

| 评审条 | 问题(已核实) | v2.1 折叠 |
|---|---|---|
| **B1 BLOCKER** | 合成值写在 `sheet_json`,但 contest 只读 `mechanical_profile`(`load_actor_parameters` 返回存库视图不重投影,`ensure_npc_parameter` 写完不刷新)→ 读不到 | **§3.2**:`ensure_npc_parameter` 写 sheet 后 `refresh_mechanical_profile(&mut mech, &sheet)` 再 upsert(已验对稀疏 NPC sheet 安全、不碰 provenance、合"mech 是 sheet 物化视图"不变量) |
| **B2 BLOCKER** | 防御方该测哪个键,合成端(`map_check_param_need` 选 skills.dodge)与 contest 端(`derive_tested_source` 拿 attack 文本)**无共享通道**;且攻击方同样缺(combat 契约 `tested_parameter=None`) | **§3.3 新通道**:契约加字段 `opponent_tested_parameter`;盖章 hook 同时设 target_actor + 攻击方 tested_parameter + 防御方 opponent_tested_parameter |
| **M1 MAJOR** | 漏列 3 个 `resolve_outcome` 调用点(`live_percentile.rs:82/90/114`,§8 列为必绿)→ 签名改后编译挂 | **§3.1/§11**:全 8 调用点(runtime 4 + forced_tech 1 + live_percentile 3)入清单 |
| **M2 MAJOR** | trigger:always 信号被质疑非通用(Cyberpunk HP 有 on_failure 条目) | **已核实 moot 但诚实标注**:`derive_tested_source` **只 roll_under 走**(contest:145 唯一调用);现行 roll_under 规则仅 CoC 有 tracks(brp_orc tracks 空),Cyberpunk=meet_or_beat 永不 derive。信号对**所有现行 roll_under 规则充分**;§4 明标其作用域,不谎称覆盖 meet_or_beat 的轨 |
| **M3 MAJOR** | v2 把 Opposed 构造放 hydrate,会落进 formula-pack 门(CoC 无 pack 时不触发) | **§3.4 重设计**:Opposed 构造**移到 contest** `kernel_resolution_model`(centralizes,不受 combat pack 门控,combat/named 两路统一)→ M3 消解 |
| **M4 MAJOR** | 叙述守卫 `outcome_is_provisional[_api]`(cli:2165/api:1754)串匹配 "OpposedRoll"(PascalCase,因 serde snake_case 实为死代码);改 success 后行为翻转 | **§3.6**:守卫改判 `success`/`degree` 非空,删/修死分支 + 叙述断言 |
| **m1 MINOR** | 防御骰不落库 → 不可复算 | **§3.5**:防御骰落库(GmOnly)随对抗结算 |
| **m2 MINOR** | model 按 check_id 缓存,`defender_value` 进缓存,若首掷前卡空则焊死 None | **§3.7**:synthesis 失败/缺值时**不过早缓存 profile**;时序护栏写明 |
| **m3 MINOR** | 真实 CoC bands 无 otherwise/critical → 失败 roll `success_tier_for` 返 None;单测伪 bands 含之 | **§3.5+§4.4**:`resolve_opposed` 先用 compare 算成败布尔(不靠 tier),tier 仅"双方成功比质量";新单测覆盖真实 bands 形状(失败侧 tier=None) |
| **m4 MINOR** | id 分叉时勿被 env 默认 DV 兜底成编造值 | **§6.2**:断言 id 分叉 → defender_value=None → Provisional,不走 `TRPG_CONTEST_DEFAULT_*` |

## 2. 三个定论(v1 已确认,代码实据见 §12)
1. **OpposedRoll 整个 unbuilt**:`resolve_against_model`(contest:295-306)对 OpposedRoll 返回 `(None,None,None)`。
   攻击方掷一次,防御方从不掷,两骰从不比。
2. **唯一能消费防御方值的 = `AttackVsDefense.defense_value`,只从契约 `StaticNumber.value` 读**,不读 NPC runtime 参数。
3. **tested-source 真 bug**:`derive_tested_source` 先匹配 track;CoC `hit_points` 轨 `on_outcome.check_match:"damage|attack"`
   把"attack"赋给 HP 轨 → 攻击检定先命中 HP 轨(value 0)。

compare 现实(真实 kernel 已查,§12.6):CoC=roll_under(有模组、可 live 验);brp_orc=roll_under(**tracks 空**);
剑世界/Cyberpunk=meet_or_beat(无模组,单测);Triangle=count_faces;D&D 无 compare;Fate 无 kernel。

## 3. 设计(端到端通道,六段)

### 3.1 类型 + 签名改动
**`trpg-model`**:
- `CheckResolutionModel::OpposedRoll` 加 `attacker_value: Option<i32>` + `defender_value: Option<i32>`(确定值,可随 model 缓存;`#[serde(default)]` 向后兼容)。**defender_total 不进 model**(每次现掷的随机值,进 model 会被 check_id 缓存复用旧骰)。
- `CheckContract` 加 `opponent_tested_parameter: Option<TestedParameter>`(`#[serde(default)]`)= **防御方该测哪个键的通道**(B2)。

**`trpg-contest`**:`resolve_outcome` 加第三参 `defender_roll: Option<&DiceRollRecord>`:
```rust
pub async fn resolve_outcome(&self, contract: &CheckContract, roll: &DiceRollRecord,
    defender_roll: Option<&DiceRollRecord>) -> Result<Value>
```
**全 8 调用点**:runtime 394/477/929/983(改走 §3.4 包装器)、`forced_tech_assessment_data_driven.rs:160`、
`live_percentile.rs:82/90/114`(后 4 个传 `None`)。contest 仍**零 RNG**(只读传入值)。

### 3.2 现搓 → 投影(B1)(`trpg-runtime::ensure_npc_parameter`, 1551)
写 sheet 后立刻投影,再 upsert:
```text
npc_synth::write_synthesized_param(&mut p.sheet_json, bucket, param, &synth);
chargen::refresh_mechanical_profile(&mut p.mechanical_profile, &p.sheet_json);  // 新:把 skills/stats 投到视图
service.upsert_actor_parameters(&p).await?;
```
`refresh_mechanical_profile`(chargen.rs)只投 stats/skills/其余→fields,**不碰 `npc_param_provenance`**(它在 sheet 顶层,
会被归进 fields 但 provenance 数据保留;upgrade-only 逻辑读 sheet 的 provenance 不受影响)。已验对稀疏 NPC sheet 安全。

### 3.3 盖章进契约(B2 新通道)
对抗动作的契约必须同时带齐三样,contest 才建得出对抗:
- `target_actor` = 当前场景 NPC 的 actor_ref(用 `current_check_npc_persona` 解析出的**同一 actor_id**,与合成写入对齐 §6.2)。
- `tested_parameter` = **攻击方**该测的键(named/技能检定路径已设 = 玩家点名的技能;见 §9 范围)。
- `opponent_tested_parameter` = **防御方**该测的键 = `map_check_param_need(action_kind, compare).param`(§7)。

**盖章 hook**:与 Phase 1 的 NPC pre-pass 同一处触发(CLI:1082-1091:`check_param_need` 返 Some + `current_check_npc_persona` 有 NPC),
把 persona 的 actor_id + (bucket,param) 线进**该回合 check 契约**的构造点(named/situation check builder 接收这两项并落到上述字段)。
单一信号(`check_param_need` 返 Some)同时驱动:① 合成(§3.2)② 盖章(本节)。

### 3.4 contest 建 OpposedRoll(centralized,消解 M3)(`trpg-contest::kernel_resolution_model`, roll_under 分支 140-157)
roll_under 分支改为:
```text
if contract.target_actor.is_some() && contract.opponent_tested_parameter.is_some() {
    let attacker_value = resolve_percentile_target_for(initiator.actor_id, contract.tested_parameter, kernel);
    let defender_value = resolve_percentile_target_for(target_actor.actor_id, contract.opponent_tested_parameter, kernel);
    OpposedRoll { attacker_expression: contract.dice_expression, attacker_value,
                  defender_actor_id: Some(target_actor.actor_id), defender_expression: <kernel 核心掷式 d100>,
                  defender_value, defender_roll_visibility: PrivateGmRoll }
} else {
    // 现状:自测 percentile（Spot Hidden 等无对手的检定）
    PercentileRollUnder { ... }
}
```
- 泛化 `resolve_percentile_target(contract, kernel)` → `resolve_percentile_target_for(actor_id, hint: Option<&TestedParameter>, kernel)`
  (现写死 initiator + contract.tested_parameter, contest:169-186);原签名做薄包装。
- **不再**在 combat hydrate 构造 `CheckTargetModel::Opposed`(M3 消解);combat/named 两路只负责 §3.3 盖章。
- `defender_expression` 取 kernel 核心掷骰(CoC=d100);kernel 无清晰核心掷式 → 不建 Opposed,退回现状(fail-closed)。
- 任一 value=None → 字段填 None → §3.5 Provisional。

### 3.5 防御骰预掷 + 对抗结算
**预掷(`trpg-runtime` 包装器 `resolve_outcome_with_opposition`,4 处调用点统一改走)**:
```text
let opposed = contract.target_actor.is_some() && contract.opponent_tested_parameter.is_some();
let def_roll = if opposed { Some(roll_defender_die(<kernel 核心掷式>)) } else { None };
// roll_defender_die: 复用纯 roll_dice(expr) → 包成 DiceRollRecord{roller_kind:Npc, visibility:PrivateGmRoll};
// 落库(insert_dice_roll, GmOnly)供复算(m1)
ContestService::new(db).resolve_outcome(contract, roll, def_roll.as_ref()).await
```
**结算(`trpg-contest::opposed.rs` 新文件 ≤400,纯函数 + 单测)**:`resolve_outcome` 内检测 model=OpposedRoll 时,
**显式加载一次 kernel**(现有加载 contest:51 被 `if let Some(t)=target` 门控,对抗 target=None 会跳过)取 `compare`+`success_bands`,
调 `resolve_opposed(compare, bands, atk_total, atk_val, def_total, def_val) -> (Option<i64>,Option<bool>,Option<String>)`:
1. **缺值 fail-closed**:atk_val/def_val/def_total 任一 None → `(None,None,None)`,profile 标 Provisional(不编造)。
2. **成败布尔靠 compare 方向**(roll_under: total≤value 成功、越低越好;meet_or_beat: total≥value、越高越好)——
   **不靠 tier**(真实 CoC bands 无 otherwise,失败 roll `success_tier_for` 返 None,m3)。
3. **比较**(高→低):一胜一负→胜方;双胜→比 `success_tier_for` rank(高胜),tier 平→比 margin,margin 平→§3.6 约定;双负→主动方未达成→防御方胜(mutual_failure)。
4. **输出**:`success=Some(attacker_wins)`、`degree=Some(对抗标签)`、`target=None`;outcome JSON 富化
   `opposed:{attacker_total,attacker_value,attacker_tier,defender_total,defender_value,defender_tier,winner}`。

### 3.6 平局归属(§10.7)+ 叙述守卫(M4)
- **平局**(双胜、同 tier、同 margin)→ **防御方胜**:**可 override 的引擎约定**(6 套 kernel 均无字段编码对抗平局归属);
  代码注释明写"engine convention: ties favor the defender (status quo); a future kernel field may override"。**不得**称 data-driven。
- **叙述守卫**:`outcome_is_provisional`(cli:2165)/`outcome_is_provisional_api`(api:1754)现靠 `s.contains("OpposedRoll")`
  判"无判定"(PascalCase 串,因 `CheckResolutionModel` 是 `serde(rename_all="snake_case")` 序列化成 `"opposed_roll"`,**该分支是死代码**)。
  对抗产出 `success=Some` 后,现状碰巧走 no_verdict→provisional 翻转为"已判定"。**改判据为 `success`/`degree` 字段非空**,删死分支,加叙述断言,避免后人"修好"串匹配反噬。

### 3.7 缓存时序护栏(m2)
`ensure_contest_profile`(contest:78-81)按 check_id 缓存 profile(含 defender_value)。护栏:
- defender_value=None 时(synthesis 失败/卡空)**profile 仍可建但标 Provisional**;一旦缓存 None 不自愈 → 依赖 §3.3 盖章 + §3.2 合成
  在**首次 resolve 之前**完成(CLI:1086 prepare_npc_for_check await 早于契约构造与结算,时序成立)。
- defender_total **不进 model**(§3.1)→ 每次结算现掷,无"复用旧骰"隐患。

## 4. tested-source 外科修复(§10.2/§10.3 BLOCKER)
### 4.1 数据信号(真实 CoC kernel 已验,§12.6)
- `sanity` 轨:`on_outcome[0].trigger="on_failure"`(结果条件型)→ check_match 是"被测对象"别名 → **保留**。
- `hit_points` 轨:`on_outcome[0].trigger="always"`(无条件效果路由)→ check_match `"damage|attack"` 是"出场路由"别名 → **剔除**。
### 4.2 改动(`derive_tested_source`, 490-502)
收集 track check_match 别名时**跳过 `trigger=="always"` 的 on_outcome 条目**;轨 id/name 永远合法,非 always 条目别名照收。
### 4.3 作用域诚实标注(M2)
`derive_tested_source` **仅 roll_under 路径调用**(contest:145 唯一)。现行 roll_under 规则:CoC(有 tracks,信号生效)、
brp_orc(tracks 空,循环 no-op)。meet_or_beat(Cyberpunk 等)**永不 derive**,故其 HP 轨形状与本修复无关。
spec **不**声称该信号覆盖 meet_or_beat 的轨——它解决的是"roll_under 规则下攻击误绑 HP 轨",对现行所有 roll_under 规则充分。
### 4.4 不变量 + 新测试
- `sanity_check_resolves_to_the_sanity_track`(contest:583)、`no_match_fails_closed_not_fifty`(604)**仍绿**。
- **新回归测试**:含 `hit_points{trigger:always,check_match:"damage|attack"}` + `Fighting` 技能的 kernel/mech,attack 检定**不**解析到 HP 轨。

## 5. 不变量 / 护栏(铁律)
1. **零 per-ruleset 硬编码**:对抗比较只读 kernel compare/success_bands;tested-source 只读 track trigger/id/name/check_match;
   盖章只读 compare + typed action_kind。无 `if ruleset`。
2. **fail-closed**:防御值缺 / id 分叉 / 防御骰缺 / kernel 无核心掷式 → Provisional(不编造);合成值仍 flagged-provisional。
3. **单一真相**:NPC 值在 `sheet_json`(Phase 1 写),`mechanical_profile` 是其物化视图(§3.2 投影);本层只读不另存。
4. **既有测试必绿**:`sanity_check_resolves_to_the_sanity_track`、`no_match_fails_closed_not_fifty`、
   `live_percentile.rs`(percentile_reads_real_actor_value_not_fifty + percentile_emits_coc_success_tiers)、CoC tier 系列、forced-tech 集成测试。
5. **contest 纯**:零 RNG;随机值(defender_total)由 runtime 预掷传入。
6. **文件 ≤ ~400 行**:对抗结算 `resolve_opposed` 切到 `contest/src/opposed.rs`,主文件不臌胀(已 640 行)。

## 6. NPC 卡读取分层 + actor-id 对齐(§10.4/§10.5)
### 6.1 读取分层
对抗路径双方 tested value 都由 **contest** `resolve_percentile_target_for`(§3.4)读卡——攻击方读 PC 卡、防御方读 NPC 卡,
同一套代码、单一来源(B1 保证合成值已在 mechanical_profile)。**hydrate 不再读卡/不构造 Opposed**。
时序安全:合成+投影(§3.2)与盖章(§3.3)在 CLI:1086 早段完成,远早于 contest resolve。
### 6.2 actor-id 对齐
- 合成写 `current_check_npc_persona.actor_id`(现硬编码 `npc.opposition`, runtime:1600);盖章用**同一 actor_id**填 target_actor → 对齐。
- **CoC**:场景 NPC 拉斯 → `npc.opposition`,两路一致 ✓。
- **id 分叉**(如 combat 关键词扫给 `npc.scav_boss`,combat:1038):若盖章 target_actor 与合成 actor_id 不等,
  contest 在该卡读不到合成值 → `defender_value=None` → **Provisional**(不编造)。**断言**:此路径**不**被 `TRPG_CONTEST_DEFAULT_ATTACK_DV`
  等 env 默认兜底成假值(m4)。关键词扫退役(语义 target 解析)= NPC Phase 2,本期标依赖。

## 7. check_param_need 按 compare 选参数(§10.6)(`trpg-runtime`)
- `map_check_param_need(action_kind)` → `map_check_param_need(action_kind, compare: &str)` 纯函数:
  - meet_or_beat:攻击/受击族 → `("stats","defense")`(被动 DV)。
  - roll_under(或对抗):攻击/受击/施法族 → `("skills","dodge")`;潜行/盗窃/对抗社交族(Hide/Hack/...)→ `("skills","perception")`。
  - 不需 NPC 参数的 kind → None。
- `RuntimeEngine::check_param_need` 改 **async** + 加载 ruleset kernel 取 compare:
  `pub async fn check_param_need(&self, ruleset_id: &str, action_kind: &SituationActionKind) -> Option<(String,String)>`。
- **调用点**:CLI:1083 改 async + 传 ruleset(全仓仅此一处调 `runtime.check_param_need`;API 路径 §9 范围外)。
- **重写单测** `check_param_need_maps_typed_action_kind_to_bucket_param`(runtime:3024):对 roll_under 与 meet_or_beat 两 compare 分别断言。
- 与 §3.3/§3.4 对齐:roll_under 合成 `skills.dodge`/`skills.perception`,盖章把它填进 `opponent_tested_parameter`,contest 据此读防御方值。

## 8. 组件落点
| 模块 | 改动 |
|---|---|
| `trpg-model/src/lib.rs` | `OpposedRoll` 加 `attacker_value`/`defender_value: Option<i32>`;`CheckContract` 加 `opponent_tested_parameter: Option<TestedParameter>`(均 serde default) |
| `trpg-contest/src/lib.rs` | `resolve_outcome` 加 `defender_roll` 参;`resolve_percentile_target_for(actor_id, hint, kernel)` 泛化;`kernel_resolution_model` roll_under 分支建 OpposedRoll;`derive_tested_source` 跳 trigger:always;对抗分支显式加载 kernel |
| `trpg-contest/src/opposed.rs`(新, ≤400) | `resolve_opposed(...)` 纯函数 + 单测(含真实 bands 无 otherwise 形状) |
| `trpg-runtime/src/lib.rs` | `ensure_npc_parameter` 加 refresh_mechanical_profile(B1);`resolve_outcome_with_opposition` 包装器(预掷+落库,4 处改走);`check_param_need` async+compare;`map_check_param_need(ak, compare)`;盖章 hook(target_actor + opponent_tested_parameter) |
| `trpg-runtime/src/<check builder>` | named/situation check builder 接收 NPC actor_id + (bucket,param),落 target_actor + opponent_tested_parameter |
| `trpg-cli/src/main.rs` | check_param_need 调用点 async+ruleset;`outcome_is_provisional` 判据改 success/degree(M4) |
| `trpg-api/src/lib.rs` | `outcome_is_provisional_api` 判据改 success/degree(M4) |
| `trpg-runtime/tests/forced_tech_assessment_data_driven.rs`、`trpg-contest/tests/live_percentile.rs` | `resolve_outcome` 调用补 `None`(共 4 处) |

## 9. 范围
**内**:端到端通道(§3:投影 B1 + 盖章 B2 + contest 建 OpposedRoll + 预掷 + 对抗结算 + 叙述守卫)+ tested-source 外科修复(§4)
+ 卡读取分层/id 对齐(§6)+ check_param_need 按 compare 选参(§7)。**Live 验在 CoC 模组**。
**外**:① combat-attack 路径的**攻击方** tested_parameter 自动派生(combat 契约现 None;live 验走"玩家点名技能的对抗检定"路径,
攻击方键已设 —— 见 §11 风险;combat attack 对抗留跟进);② API 路径 `prepare_npc_for_check`+盖章奇偶补齐(跟进);
③ meet_or_beat 防御 DV 的 live 验(无模组,单测);④ combat/object target_actor 关键词扫退役(NPC Phase 2);
⑤ Triangle count_faces / D&D·Fate 无 compare(另案);⑥ SavingThrow 模型(dead,不动)。

## 10. Live 验证路径(已定:方案 A,2026-06-09 用户拍板)
**决定 = 玩家发起的对抗技能检定**(named/situation check 路径)。combat-attack 对抗(需自动派生攻击方战斗技能)留跟进。
**玩家发起的对抗技能检定**(named/situation check 路径)——该路径已设攻击方 `tested_parameter`(玩家点名的技能),
只需 §3.3 盖章补 target_actor + opponent_tested_parameter。CoC 场景:玩家"偷拉斯的东西/潜行过拉斯"→ 攻击方测妙手/潜行,
防御方(拉斯)现搓 Spot Hidden/perception → 对抗结算出胜负。**避开** combat-attack 路径需自动派生攻击方战斗技能的难题(范围外①)。
**待用户拍板**:接受"对抗技能检定"作 live 验场景,还是坚持 combat-attack 场景(则需额外把攻击方战斗技能派生纳入本期)。

## 11. 风险 / 开放问题
1. **攻击方 tested value 也可能缺**:对抗需双方值。named 路径攻击方键已设(✓);combat-attack 路径 `tested_parameter=None`
   → 攻击方派生不到 → fail-closed。故 live 验推荐 named 对抗路径(§10),combat attack 对抗留跟进。
2. **盖章 hook 落点**:契约在 turn 流程深处按路径(combat/named/ability)各自构造;统一盖章需在"已知本回合 check 契约 + 已知场景 NPC"
   的单点。CLI:1082-1091 现先于契约构造,需把 NPC actor_id + (bucket,param) 线进 check builder。实现期确认 named/situation builder 的接入点。
3. **live 验只 CoC**:meet_or_beat 绑定无模组,单测覆盖 + 标注。
4. **opponent_check 掷式**:取 kernel 核心掷骰;kernel 未声明 → 不构造 Opposed(退回现状,fail-closed)。

## 12. 探查锚点(代码实据,v2.1 已逐条核对)
- 12.1 OpposedRoll no-op:contest:295-306,216,377。 12.2 防御消费:contest:209-211/298,357-361。
- 12.3 **全 8 个 resolve_outcome 调用点**:runtime:394/477/929/983;`forced_tech_assessment_data_driven.rs:160`;
  `live_percentile.rs:82/90/114`。
- 12.4 percentile 写死 initiator + contract.tested_parameter:contest:170-177(泛化点)。 12.5 tested-source bug:contest:472-519,490-502。
- 12.6 真实 kernel(:54347 已查):CoC `compare="roll_under"`,`success_bands` ids=regular(1)/hard(2)/extreme(3)/fumble(0)
  (**无 otherwise/critical**,失败 roll → success_tier_for 返 None),`sanity{on_outcome:[{trigger:"on_failure",check_match:"sanity|san roll"}]}`、
  `hit_points{on_outcome:[{trigger:"always",check_match:"damage|attack"}]}`;brp_orc `compare="roll_under"` 但 `resource_tracks:[]`;
  cyberpunk_red=meet_or_beat,hit_points 两条 on_outcome(always + on_failure,均 check_match "damage|attack",但**永不 derive**)。
- 12.7 B1:`load_actor_parameters`(trpg-params:47-54)原样返 mech 列不重投影;`ensure_npc_parameter`(runtime:1534-1553)写 sheet 不刷新;
  `refresh_mechanical_profile`(chargen.rs)只投 stats/skills/fields,materialize_actor_params 明示"mech 是 sheet 物化视图"。
- 12.8 B2:`map_check_param_need`(runtime:2126)产物只喂 prepare_npc_for_check(CLI:1083-1088)不进契约;combat 契约 `tested_parameter:None`(combat:1088);
  `build_named_check_plan`(runtime:2463-2493)设 tested_parameter 但 target_actor:None、target:UnknownUntilLookup。
- 12.9 M4 叙述守卫:cli:2165-2172 / api:1754-1761 `outcome_is_provisional[_api]` 串匹配 "Provisional|RulesetProcedureLookup|OpposedRoll"(后者 PascalCase 死分支);
  `CheckResolutionModel` serde tag snake_case(model:6250-6251)。
- 12.10 hydrate:combat:257-358(读攻击方 pack,308-313 载 actor,333 resolve_attack_dv,不读 kernel/NPC 卡);
  target_actor:combat:1027-1057(关键词扫),1062-1080(契约设),1155-1183。
- 12.11 时序:CLI:1082-1091(check_param_need→current_check_npc_persona→prepare_npc_for_check await)早于 prepare_turn_context:1093;
  runtime:1534 ensure_npc_parameter、1564-1600 persona(硬编码 actor_id)、1607 check_param_need、2126 map_check_param_need、3024 单测。
- 12.12 pre-contest 门:runtime:1041(env 默认关;Opposed 经 kernel_resolution_model 建、target=Unknown 不触 missing_target)。
  掷骰原语:runtime:1075 resolve_roll_input 内 `roll_dice(expr)` 纯掷;`load_actor_parameters` 返 `{sheet_json, mechanical_profile}`。
