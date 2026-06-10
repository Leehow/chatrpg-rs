# 规则感知 GM 二期 Implementation Plan（机制目录 + watcher/债务 + 场景机制意图）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** GM agent 带着本规则集的机制知识进回合：①解析侧第三遍 `mechanics_compile` 把规则书"剧情配合机制"编译为 kernel `mechanics_catalog`（开放枚举、表达力四档 P/H/PM/S、137 条附录 A 审计基线）；②运行时把目录注入 BP1 索引 + lookup_mechanic 工具，阈值穿越/事件钩子由引擎 watcher 产 `MechanicDue`，机械债务清单（未结算检定 + 未处理 due + 追溯债务）门控叙事轮，waive 出口带理由记账；③模组深抽顺路产出 `SceneMechanicIntent`，结算后 `effect_policy` 由 Rust 强制执行。全程零 per-ruleset 硬编码、fail-closed、`#[serde(default)]` 向后兼容、一期缓存稳定不破坏（理念护栏 spec §3.5 六条 MUST）。

**Architecture:** 三柱三切片。Slice A（解析侧，无运行时依赖可先行）：对标 `chargen_compile` 先例在 trpg-rule-agent 新增编译遍，产物就地升级 RuleKernel（复用 `upsert_rule_kernel` / `rule_kernel_patches` override 通道），同遍升级 thresholds/success_bands/on_outcome/fields.notes，确定性护栏 + validation_report。Slice B（运行时，依赖 A 的目录）：BP1 紧凑索引块（kernel BP1 JSON 投影同步瘦身防爆预算）、watcher 在 `after_check_resolved`/`apply_direct_effect` 落账单点检测阈值穿越产 due（持久化 `mechanic_dues` 表）、EngineHook 在 turn_start/navigate/advance_time 事件点发 due、trpg-gm 新增 `obligations.rs` 债务清单门控 + `lookup_mechanic`/`waive_obligation` 两工具（10→12）。Slice C（模组侧，不依赖 A/B）：module_reader 深抽 submit_deep 契约扩展 scene_mechanics、当前场景 intents 投影 BP2、`roll_check(scene_mechanic_id)` 结算后 effect_policy 映射到既有 StatePatch/schedule_in 原语强制执行。

**Tech Stack:** Rust workspace（既有 crate：trpg-model/db/mechanics/runtime/time/agent/llm/rule-agent/parser/gm/cli）。编译遍模型 gpt-5.4（沿用模组抽取拍板：后台跑、质量优先，env `TRPG_MECHANICS_COMPILE_MODEL`）；运行时 loop 不换模型。PostgreSQL :54347（迁移 0027 新表 `mechanic_dues`）。数据文件 `data/agent/gm_skill/global/` 增两条准则。

权威设计 spec：`docs/superpowers/specs/2026-06-10-rule-aware-gm-design.md`（理念护栏 §3.5、支柱1 §4、支柱2 §5、支柱3 §6、接缝 §7、测试验收 §8 共 14 条、附录 A 137 条审计基线）。

**本工作区无 git**：所有任务直接落工作副本，绝不写任何 git/commit 步骤；每任务以验证收尾（`cargo test -p <crate>`，注意 cwd 须在 crate 目录而非 workspace 根 + 新文件 `wc -l` ≤400）。

**LEAN 格式声明**：本计划是骨架——每任务一段精确范围（改哪些文件、调用哪些真实签名、错误码、对应验收编号），不展开 checkbox 步骤；执行 worker 按 TDD 自行展开 RED→GREEN。所有"调用现有代码"的签名均已对照真实源文件核实（核实记录见各契约的 `// grounded:` 注释）。

---

## File Structure

### Create（新建，全部 ≤400 行）

```
crates/trpg-model/src/mechanics.rs                      # 二期共享类型单一新模块：MechanicEntry/MechanicKind/ProcedureStep/EngineHook/CalendarGranularity/FollowupLink/SceneMechanicIntent/EffectPolicy/EffectPatchIntent/MechanicDue + 纯函数（expressiveness_tier/track_semantic_line/granularity_seconds）——lib.rs 已 6620 行，绝不往里塞类型
crates/trpg-rule-agent/src/reader/mechanics_compile.rs  # 第三编译遍 loop：MECH_SYS 开放枚举 prompt + submit_mechanics 子循环（对标 chargen_compile::run_compile_loop 的 complete_with_tools 样板，复用 tools::nav_tools/read_layout，两轮 merge-by-id）
crates/trpg-rule-agent/src/reader/mechanics_finalize.rs # 确定性护栏 + kernel 就地升级写回：tested_parameter 存在性校验、followup_procedure_id 引用闭合、band 补全审计、降档保留（绝不"不认识就丢"）、validation_report 条目
crates/trpg-rule-agent/src/bin/mechanics_proto.rs       # 跑批/审计 bin（对标 reader_proto/module_reader_proto 先例）：单规则直打 units 编译 + 六规则批量 + 附录 A 覆盖率审计输出
crates/trpg-mechanics/src/watcher.rs                    # MechanicDue 产出单点：detect_threshold_dues（阈值穿越，从 after_check_resolved/apply_direct_effect 收口调）+ dues_for_hook（EngineHook 事件点查询原语）+ db 持久化包装
crates/trpg-gm/src/obligations.rs                       # ObligationLedger：开着的 CheckContract + 未处理 MechanicDue + RetroactiveEffectDebt + WaiverRecord；blocking()/block_text() 债务门控渲染；waive 语义（scope=turn/scene）
crates/trpg-gm/src/scene_policy.rs                      # effect_policy 强制执行：EffectPatchIntent → 既有原语映射（ObjectPatch/apply_direct_effect/CreateFact/WorldTimeService::schedule_in），roll_check(scene_mechanic_id) 结算后调用
crates/trpg-gm/src/tools/mechanic.rs                    # LookupMechanicTool（目录全文拉取）+ WaiveObligationTool（必须带 reason，scope 默认 turn）
migrations/0027_mechanic_dues_v120.sql                  # mechanic_dues 表（due 跨回合"债务必清"持久化 + e2e SQL 可查；status open|resolved|waived）
data/agent/gm_skill/global/40_mechanics_catalog.md      # 准则一：目录优先于自由发挥 + 按 ruleset 条款持续评估玩家言行（行为语义监听，Triangle 嘉奖/Fate FP 双实证）
data/agent/gm_skill/global/50_obligation_policy.md      # 准则二：due 必须回应——处理（roll_check/request_player_roll/apply_effect）或 waive_obligation 带理由，绝不无视
```

### Modify（修改，标注真实锚点）

```
crates/trpg-model/src/lib.rs                            # 仅 3 处小改：`pub mod mechanics; pub use mechanics::*;` + RuleKernel 增 `#[serde(default)] pub mechanics_catalog: Vec<MechanicEntry>`（L1370 结构体）+ ScenarioNode 增 `#[serde(default)] pub scene_mechanics: Vec<SceneMechanicIntent>`（L1602 结构体）
crates/trpg-rule-agent/src/reader/mod.rs                # pub mod mechanics_compile/mechanics_finalize + re-export compile_mechanics_catalog
crates/trpg-parser/src/staged.rs                        # 接线第三遍：kernel 产出/upsert 后调 compile_mechanics_catalog（对标 L151 `reader::compile_chargen_formulas` 接线样板；门 TRPG_MECHANICS_COMPILE 默认开，失败 fail-closed 保留原 kernel）
crates/trpg-mechanics/src/lib.rs                        # apply_outcome_resource_tracks（L65）trigger 解析升级：接受结构化 {kind:"band",band_id}（现仅 .as_str()，保留 always/on_success/on_failure/on_tier 字符串兼容）+ amount 迷你语言 max_of 形态；after_check_resolved（L196）收口调 watcher::detect_threshold_dues；mechanical_ledger_context_block（L271）资源行附语义状态
crates/trpg-runtime/src/lib.rs                          # rule_steward_prefix_blocks_for_turn（L1432）：kernel BP1 JSON 投影剔除 mechanics_catalog 全文 + 新增紧凑索引块（id|name|when_to_use，CacheZone::Prefix 按 ruleset 固定）；scene_node_to_blocks（L2233）：scene_mechanics intents 投影块（PinnedMiddle/SceneStable/expires_at_scene）——新逻辑做薄，渲染纯函数放 trpg-model/mechanics.rs
crates/trpg-gm/src/turn_loop.rs                         # 六步状态机插债务门：确定性头部加 turn_start hook dues 装载；工具轮 `if !saw_tool`（L116）改为先查 ObligationLedger.blocking()——非空则回填债务观察继续轮而非 break；轮耗尽带债强制叙事后债务落 BP3+勘误记忆；verify_after_stream（L153）InventedEffect→RetroactiveEffectDebt + referenced_ledger_ids 传账本 id 全集
crates/trpg-gm/src/prompts.rs                           # DynamicTailInput（L10）增 `obligations_block: Option<&'a str>`（上回合遗留债务进下回合 BP3 尾段）
crates/trpg-gm/src/tools/mod.rs                         # ToolRegistry::standard()（L133）追加 LookupMechanicTool/WaiveObligationTool（尾部追加保持既有 10 个顺序字节不变）；错误码全集登记 mechanic_not_found/scene_mechanic_not_found/obligation_not_found；ToolCtx 增 `obligations: Option<&'a mut ...>` 形态由任务 B6 定（dispatch 借用冲突时改 dues 经 db 读写）
crates/trpg-gm/src/tools/check.rs                       # RollCheckArgs（L23）增可选 mechanic_id/scene_mechanic_id；roll_check 结果 JSON 附 band 语义行（success_bands[*].semantics 渲染，fail-closed 退裸 band id）
crates/trpg-gm/src/tools/world.rs                       # NavigateSceneTool::call（L86）成功后发 scene_enter dues；AdvanceTimeTool::call（L54）advance 后按 calendar 粒度跨界发 time_advance/calendar dues（用 result.from/to 的 world_tick）
crates/trpg-gm/src/lib.rs                               # pub mod obligations/scene_policy + tools/mechanic 接线
crates/trpg-agent/src/gm_loop.rs                        # NarrationVerifier::verify（L176）：referenced_ledger_ids 非空时按账本 id 结构核对优先，子串扫描降回退路径且 finding detail 标注 "fallback:substring_scan"（理念护栏 §3.5.5 可观测技术债）
crates/trpg-rule-agent/src/reader/module_reader.rs      # DEEP_SYS prompt + apply_deep_to_node（L117）：解析 scene.scene_mechanics（source_anchor 空 → fail-closed 丢弃该条）
crates/trpg-rule-agent/src/reader/module_reader_loop.rs # submit_deep_tool schema（L102）+ oneshot_deep_extract user prompt（L201）：scene_mechanics 字段加入提交契约与指引
crates/trpg-db/src/lib.rs                               # migrate() include_str! 数组手动加 0027 行（L50 之后）；新原语 insert_mechanic_due/list_open_mechanic_dues/update_mechanic_due_status
```

---

## 共享类型契约

> 全部落 `crates/trpg-model/src/mechanics.rs`（除注明），全部 `#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]` + 字段级 `#[serde(default)]`——老 kernel/老图谱 JSON 反序列化必须照常工作（A1/C1 各有专门回归测试）。`// grounded:` 注释 = 已对照真实源码核实的依赖签名。

### 1. 机制目录（kernel 新区）

```rust
// RuleKernel 扩展（trpg-model/src/lib.rs L1370，唯一字段级改动）：
//   #[serde(default)] pub mechanics_catalog: Vec<MechanicEntry>,
// grounded: RuleKernel 整体 serde 进 rule_kernels.content_json（trpg-db L682
//   upsert_rule_kernel bind serde_json::to_value(kernel)），load_rule_kernel（L691）
//   原样反序列化 → 新区随 kernel 自然落库/装载，无新表。
// 注意：kernel 整体 JSON 同时被投进 BP1（runtime L1440 RuleStewardKernel 块）——
//   任务 B1 必须同步在该投影处剔除 mechanics_catalog 全文，否则 BP1 直接爆预算。

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct MechanicEntry {
    pub id: String,                            // "coc.skill.jump" / "coc.sanity_check" / "coc.temporary_insanity"
    pub name: String,
    #[serde(default)] pub kind: MechanicKind,
    #[serde(default)] pub description: String, // 是什么（给 agent 的语义知识）
    #[serde(default)] pub when_to_use: String, // 触发语义（语义路由的唯一依据，禁止关键词表）
    #[serde(default)] pub tested_parameter: Option<String>, // sheet schema 键，finalize 护栏校验存在性
    #[serde(default)] pub procedure: Vec<ProcedureStep>,    // 可为空（semantic 档）
    #[serde(default)] pub hooks: Vec<EngineHook>,
    #[serde(default)] pub passive_projection: Option<String>, // PM 档常驻投影模板，如 "信用评级 {value}：{band}"
    #[serde(default)] pub followup_links: Vec<FollowupLink>,
    #[serde(default)] pub locked_until: Option<String>,     // Playwalled 条件解锁语义（Triangle 实证），纯语义字段
    #[serde(default)] pub source_refs: Vec<SourceRef>,      // 空 → finalize 丢弃（fail-closed="不编造"的唯一丢弃条件）
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MechanicKind {
    SkillCheck,
    SubsystemProcedure,
    Reaction,
    Spend,
    #[default]
    #[serde(untagged)]
    Other(String),   // kind 是表达形态分类不是发现过滤器：归不进既有 kind 一律收录为 Other
}
// 实现注记：unit 变体 + 末位 #[serde(untagged)] Other(String) 是 serde ≥1.0.171 支持的
// 兜底模式；若 workspace serde 版本不支持 variant-level untagged，降级为手写 Deserialize
// （from string，匹配不中归 Other）——执行任务时先 `cargo tree -p serde` 核版本。
// Default 需要落在某变体上：用 Other(String::new())，serde(default) 反序列化缺字段时取此值。

/// 程序步（表达力第 1 档）。step 标签判别；结构化不了的整步降档进 Other 保留原 JSON。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum ProcedureStep {
    Roll      { dice: String, #[serde(default)] vs: Option<String>, #[serde(default)] note: Option<String> },
    Apply     { track: String, op: String, amount: String, #[serde(default)] note: Option<String> },
    TableRoll { table_ref: String, #[serde(default)] note: Option<String> },
    Gate      { condition: String, #[serde(default)] note: Option<String> },
    #[serde(untagged)]
    Other(serde_json::Value),
}

/// 事件钩子（表达力第 2 档；第三触发通路）。词表 = 引擎真实事件点；
/// 解析出钩子但引擎无对应事件点 → finalize 降级为语义条目并记 validation_report。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum EngineHook {
    SceneEnter,                                  // 事件点：NavigateSceneTool 成功后（tools/world.rs L94 set_session_scene 之后）
    TurnStart,                                   // 事件点：GmLoop::run_gm_turn 确定性头部（turn_loop.rs 步骤 1）
    TimeAdvance,                                 // 事件点：AdvanceTimeTool advance_world_time 之后
    Rest,                                        // 事件点：advance_time scale=downtime（数据映射，非新工具）
    CombatStart, CombatEnd,                      // 事件点：state_frames FrameKind 开/收（二期仅查询原语支持，接线视 frame 钩子现状 fail-closed）
    Calendar { granularity: CalendarGranularity }, // 跨界检测用 TimeAdvanceResult.from/to 的 world_tick
    SessionEnd,                                  // 事件点：CLI 会话收尾命令（无则降级语义，记 validation_report）
    DevelopmentPhase,                            // 同上：章节结算节拍（七书勘查实证），无引擎事件点时降级语义
}

/// 日历粒度数据化（Fate 一天 8 段实证：粒度由 ruleset 数据定义，非固定枚举）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CalendarGranularity {
    pub unit: String,                                  // "day" | "week" | "month" | "segment" | 自定义
    #[serde(default)] pub seconds_per_unit: Option<i64>,  // 自定义周期秒数（优先生效）
    #[serde(default)] pub segments_per_day: Option<u32>,  // unit="segment" 时：86400/segments
    #[serde(default)] pub label: Option<String>,
}
/// 纯函数：粒度→秒；认不出 → None（fail-closed：该 hook 降级语义，不猜）。
pub fn granularity_seconds(g: &CalendarGranularity) -> Option<i64>;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FollowupLink {
    pub condition: FollowupCondition,
    pub procedure_id: String,    // finalize 护栏：必须指向目录内真实条目 id，否则丢弃该 link 并记 validation_report
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FollowupCondition {
    Threshold   { track_id: String, threshold_ref: String }, // 如 {track_id:"sanity", threshold_ref:"loss_in_one_go:5"}
    OutcomeBand { band_id: String },                          // 如 fumble → 取最大损失程序
    Outcome     { value: String },                            // "success"|"failure"
    #[serde(untagged)]
    Other(serde_json::Value),                                 // 降档保留
}

/// 纯函数：表达力四档判定（不存冗余字段，按数据特征算——理念护栏 §3.5.3）：
/// procedure 非空→Procedure；hooks 非空→Hook；passive_projection 有→PassiveModifier；其余→Semantic。
pub enum ExpressivenessTier { Procedure, Hook, PassiveModifier, Semantic }
pub fn expressiveness_tier(e: &MechanicEntry) -> ExpressivenessTier;
```

### 2. kernel 既有区的数据形状升级（无 Rust 结构体——resource_tracks/dice_core 是 `Vec<serde_json::Value>`/`Value`，升级是 JSON 契约 + 读取点适配）

```jsonc
// resource_tracks[*].thresholds[*] 增（mechanics_finalize 写回；watcher 读取）：
{ "at": 0, "direction": "at_or_below", "consequence": "...",   // 既有字段（mechanics lib.rs L168-189 已消费）
  "followup_procedure_id": "coc.temporary_insanity" }           // 新增；可缺省（无→due 仍发，带 prose consequence）

// dice_core.success_bands[*] 增：
{ "id": "extreme", "label": "极难成功", /* 既有 */
  "semantics": "贯穿/卓越效果……",                                // 新增：该成功度在剧情/机制上意味着什么
  "mechanical_effects": [ {"kind":"band_trigger_ref","track_id":"sanity"} ] } // 新增可选：链接 on_outcome/目录程序

// resource_tracks[*].on_outcome[*].trigger 升级为双形态（mechanics lib.rs L98 现仅 .as_str()）：
"trigger": "always" | "on_success" | "on_failure" | "on_tier"   // 既有字符串全部兼容
"trigger": { "kind": "band", "band_id": "fumble" }              // 新增结构化形态；按 result.outcome 的 band 选触发
// grounded: 代码已支持 "on_tier"+tier/min_rank 字段（lib.rs L104-113，对账 outcome.success_tier/_rank）；
// 结构化 band 形态与其并存——读取点先试 as_str()，再试 as_object()，认不出 → 该 rule 跳过（fail-closed）。

// on_outcome[*].amount 迷你语言扩展（resolve_track_amount 既有：骰式/"=field"/"=value"/int/default_amount）：
"amount": "max_of:1d10"     // 新形态：取该骰式的最大值（fumble→SAN 掉最大值"由 prose 变机械事实"）
```

### 3. 场景机制意图（模组侧）

```rust
// ScenarioNode 扩展（trpg-model/src/lib.rs L1602，唯一字段级改动）：
//   #[serde(default)] pub scene_mechanics: Vec<SceneMechanicIntent>,
// grounded: 深抽提交经 submit_deep（module_reader_loop.rs L102 schema {scene, entities}），
//   apply_deep_to_node（module_reader.rs L117）从 deep.scene 取字段——scene_mechanics 同路。

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SceneMechanicIntent {
    pub intent_id: String,         // "homecoming.lawmen.cut_cable_force"
    pub description: String,       // 何种行动触发（语义对应玩家行动；结构引用按 intent_id——护栏 §3.5.2）
    pub tested_parameter: String,  // "brawling"
    #[serde(default)] pub difficulty: Option<serde_json::Value>, // {kind:"dv",value:13} / CoC 难度档——保持 Value 开放
    #[serde(default)] pub effect_policy: EffectPolicy,
    pub source_anchor: String,     // 空 → apply_deep_to_node 丢弃该条（fail-closed 不编造）
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct EffectPolicy {
    #[serde(default)] pub on_success: Vec<EffectPatchIntent>,
    #[serde(default)] pub on_failure: Vec<EffectPatchIntent>,
}

/// 映射到既有原语（spec §6 的 set_object_state/modify_track/create_fact/start_countdown
/// 词表按真实代码落位——StatePatch 真实变体见 trpg-model L2315：没有 SetObjectState/
/// StartCountdown 变体，对象态走 ObjectPatch，倒计时走 trpg-time 既有 schedule_in）：
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EffectPatchIntent {
    SetObjectState { object_id: String, patch: serde_json::Value },
        // → StatePatch::ObjectPatch { patch_id, object_id: Some, patch_json, reason }
    ModifyTrack    { owner_kind: String, owner_id: Option<String>, track_id: String, op: String, amount: i64 },
        // → RefereeCombatService::apply_direct_effect(session, ruleset, module, "scene.policy",
        //      target, parameter_path, op, amount, reason, Visibility::GmOnly)（direct_effect.rs L84 真实签名）
    CreateFact     { target: String, fact: serde_json::Value },
        // → StatePatch::CreateFact { target, fact, reason }
    StartCountdown { label: String, amount: i64, scale: String, payload: serde_json::Value },
        // → WorldTimeService::schedule_in(session_id, TimeAmount, WorldEventKind, payload, Visibility, None)
        //   （trpg-time lib.rs L165 真实签名；scale 解析复用 tools/world.rs parse_time_scale）
    #[serde(untagged)]
    Other(serde_json::Value),  // fail-closed：不执行、不报错中断，落 CreateFact 记"unexecutable_intent"事实可观测
}
```

### 4. MechanicDue（watcher 产物；trpg-model/mechanics.rs 定义，trpg-mechanics/watcher.rs 产出，trpg-gm/obligations.rs 消费）

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MechanicDue {
    pub due_id: String,                       // "due_{uuid simple}"
    pub session_id: String,
    pub turn_id: String,
    pub source: DueSource,                    // Threshold | Hook
    #[serde(default)] pub source_track: Option<String>,        // 阈值型：kernel track id
    #[serde(default)] pub hook_event: Option<String>,          // 钩子型："scene_enter"/"calendar"…（EngineHook serde tag 值）
    #[serde(default)] pub mechanic_id: Option<String>,         // 钩子型：挂接的目录条目 id（结构化绑定，护栏 §3.5.1）
    pub threshold_desc: String,               // prose consequence——无 followup_procedure_id 时 agent 凭此语义处理（fail-closed 但不静默）
    #[serde(default)] pub followup_procedure_id: Option<String>,
    pub owner_kind: String,                   // "actor"|"scene"|"party"|"world"（开放枚举——Triangle chaos=scene、Fate 阶段=world 实证）
    pub owner_id: String,
    pub evidence: serde_json::Value,          // 阈值型 {"before":38,"after":32,"delta":-6}；钩子型 {"event":"scene_enter","scene_id":"sc02"}
    #[serde(default)] pub status: DueStatus,  // Open | Resolved | Waived
    pub created_at: DateTime<Utc>,
}

#[derive(... PartialEq)] #[serde(rename_all = "snake_case")]
pub enum DueSource { Threshold, Hook }
#[derive(... PartialEq, Default)] #[serde(rename_all = "snake_case")]
pub enum DueStatus { #[default] Open, Resolved, Waived }
```

```rust
// crates/trpg-mechanics/src/watcher.rs —— 两个产出原语 + 持久化
impl RefereeCombatService {   // grounded: pub struct RefereeCombatService { pub db: Db }（lib.rs L54）
    /// 阈值穿越检测。接线在结算单点：apply_outcome_resource_tracks 阈值分支（lib.rs L168-189，
    /// 既有 crossed/loss_in_one_go 判定逻辑就地复用——把"只发 CreateFact patch"升级为
    /// "CreateFact patch + 构造 MechanicDue + insert_mechanic_due"）与 apply_direct_effect
    /// 落账后（direct_effect.rs，同一套 thresholds 判定抽成共享纯函数后两处调）。
    /// 返回本次结算产生的 dues（roll_check 工具结果同步带回给 agent）。
    pub async fn detect_threshold_dues(&self, contract: &CheckContract, result: &CheckResultRecord,
        kernel: &RuleKernel, crossings: &[ThresholdCrossing]) -> Vec<MechanicDue>;

    /// EngineHook 事件点查询：遍历 kernel.mechanics_catalog 中挂接该事件的条目产 due。
    /// calendar 事件须传 from_tick/to_tick 做跨界判定（granularity_seconds 整除边界）。
    pub async fn dues_for_hook(&self, session_id: &str, turn_id: &str, ruleset_id: &str,
        event: &HookEvent) -> Result<Vec<MechanicDue>>;
}
/// 事件点入参（调用方：turn_loop 头部 TurnStart、NavigateSceneTool SceneEnter、AdvanceTimeTool TimeAdvance+Calendar）
pub enum HookEvent {
    TurnStart, SceneEnter { scene_id: String }, Rest,
    TimeAdvance { from_tick: i64, to_tick: i64 },   // calendar 跨界检测也在此分支内完成
    SessionEnd, DevelopmentPhase, CombatStart, CombatEnd,
}
/// 阈值穿越中间产物（结算逻辑→watcher 解耦的纯数据）
pub struct ThresholdCrossing { pub track_id: String, pub kind: String /* "cumulative"|"loss_in_one_go" */,
    pub consequence: String, pub followup_procedure_id: Option<String>,
    pub owner_kind: String, pub owner_id: String, pub before: i32, pub after: i32 }
```

```sql
-- migrations/0027_mechanic_dues_v120.sql
create table if not exists mechanic_dues (
  due_id text primary key,
  session_id text not null,
  turn_id text not null,
  source text not null,                       -- 'threshold' | 'hook'
  source_track text, hook_event text, mechanic_id text,
  threshold_desc text not null default '',
  followup_procedure_id text,
  owner_kind text not null default 'actor', owner_id text not null default '',
  evidence jsonb not null default '{}'::jsonb,
  status text not null default 'open',        -- 'open' | 'resolved' | 'waived'
  waive_reason text, waive_scope text,        -- waive 记账（勘误记忆另记一份 tags=["gm_waive"]）
  created_at timestamptz not null default now(), updated_at timestamptz not null default now()
);
create index if not exists idx_mechanic_dues_session_status on mechanic_dues (session_id, status);
```

```rust
// trpg-db 新原语（对标既有 insert_check_contract 风格）：
pub async fn insert_mechanic_due(&self, due: &MechanicDue) -> Result<()>;
pub async fn list_open_mechanic_dues(&self, session_id: &str) -> Result<Vec<MechanicDue>>;
pub async fn update_mechanic_due_status(&self, due_id: &str, status: &str,
    waive_reason: Option<&str>, waive_scope: Option<&str>) -> Result<()>;
```

### 5. 机械债务清单（trpg-gm/src/obligations.rs）

```rust
/// 回合债务清单：①未结算 CheckContract（一期已有概念升格——roll_check 落了契约但
/// execute_system_roll_bundle 被 blocked / request_player_roll gate 开着不算债，
/// 已 resolve 的不算）②未处理 MechanicDue（本回合新产 + list_open_mechanic_dues 跨回合遗留）
/// ③追溯债务。门控规则（spec §5.3）：blocking() 非空 → loop 不进叙事轮。
#[derive(Debug, Default)]
pub struct ObligationLedger {
    open_check_ids: Vec<String>,
    dues: Vec<MechanicDue>,
    retro_debts: Vec<RetroactiveEffectDebt>,
    waivers: Vec<WaiverRecord>,          // 本回合 waive 记录（scope=scene 的跨回合豁免凭 db status=waived + 场景切换重开）
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetroactiveEffectDebt {
    pub debt_id: String,                 // "debt_{uuid simple}"
    pub turn_id: String,                 // 产生债务的回合（叙事已流出不可回收）
    pub finding_detail: String,          // VerifierFinding::InventedEffect 的 detail 原文
    pub created_at: DateTime<Utc>,
}
// grounded: VerifierFindingKind::InventedEffect 已存在（trpg-agent gm_loop.rs L312 枚举 /
// trpg-gm errata.rs L73 kind_key "invented_effect"）。verify_after_stream（turn_loop.rs L153）
// 在 errata.record 之后按 kind 过滤 findings 生成债务，挂进 GmLoop 持久字段（同 ErrataMemory
// 跨回合存活），下一回合 assemble 进 obligations_block。

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaiverRecord { pub target_id: String, pub reason: String, pub scope: WaiveScope, pub turn_id: String }
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaiveScope { Turn, Scene }   // Turn=默认，下回合仍提醒；Scene=本场景豁免，场景切换清除

impl ObligationLedger {
    pub fn record_open_check(&mut self, check_id: &str);
    pub fn mark_check_settled(&mut self, check_id: &str);       // record_execution / awaiting gate 时调
    pub fn absorb_dues(&mut self, dues: Vec<MechanicDue>);
    pub fn absorb_retro_debts(&mut self, debts: Vec<RetroactiveEffectDebt>);
    /// 未清债务视图（已 resolve/waive 的剔除）；空 ⇒ 放行叙事轮。
    pub fn blocking(&self) -> Vec<ObligationView>;
    /// system 观察回填文本（"[obligations] 以下机械债务必须处理或 waive_obligation：…"）；
    /// 含 due 的 threshold_desc/followup_procedure_id 与 evidence 摘要。无债务 → None。
    pub fn block_text(&self) -> Option<String>;
    /// waive：target_id 匹配 due_id/check_id/debt_id；不存在 → Err（工具层转 obligation_not_found）。
    pub fn waive(&mut self, target_id: &str, reason: &str, scope: WaiveScope) -> Result<WaiverRecord>;
    /// 回合收尾未清债务 → 下回合 BP3 obligations_block 文本 + 勘误记忆条目载荷。
    pub fn carryover_block(&self) -> Option<String>;
}
```

### 6. 新工具 schema（tools/mechanic.rs；OpenAI function 形状对标 tools/mod.rs ToolSpec 注释）

```jsonc
// lookup_mechanic —— 拉取目录条目全文（procedure/hooks/followup_links/source_refs）
{ "type":"function", "function": { "name":"lookup_mechanic",
  "description":"Fetch the full mechanics-catalog entry (procedure, hooks, followups, sources) by id from the BP1 index.",
  "parameters": { "type":"object", "properties": { "id": {"type":"string"} }, "required":["id"] } } }
// 错误码：mechanic_not_found（recoverable=true，hint：先看 BP1 索引或 retrieve_rules）

// waive_obligation —— 显式豁免一条债务，必须带理由
{ "type":"function", "function": { "name":"waive_obligation",
  "description":"Explicitly waive a pending mechanical obligation (due/check/debt) WITH a reason. Waivers are audited.",
  "parameters": { "type":"object", "properties": {
      "target_id": {"type":"string"},
      "reason":    {"type":"string"},
      "scope":     {"type":"string", "enum":["turn","scene"], "default":"turn"} },
    "required":["target_id","reason"] } } }
// 错误码：obligation_not_found（recoverable=true）；invalid_arguments（reason 空串拦截）
// 副作用：update_mechanic_due_status(waived) + 勘误记忆 MemoryEvent tags=["gm_waive"]（复用
// errata.to_memory_event 样板，errata.rs L60）+ ObligationLedger.waive

// roll_check 参数扩展（check.rs RollCheckArgs 增两个可选字段，schema properties 同步）：
"mechanic_id":       {"type":"string"}   // 给定 ⇒ 从目录继承 tested_parameter/骰程序；目录是知识不是枷锁
"scene_mechanic_id": {"type":"string"}   // 给定 ⇒ 从当前场景 intents 继承 tested_parameter/difficulty，结算后强制执行 effect_policy
// CheckContract 携带：复用既有 advice_refs: Vec<String> 追加 "mechanic:{id}" / "scene_mechanic:{id}"
// 结构化引用（CheckContract L2606 无专用字段，不动 5 处签名——on_outcome 结构绑定经
// tested_parameter + band trigger 实现，check_match regex 自动降级为无 mechanic_id 时的回退）。
```

---

## 任务索引

> 三段对应三切片：A 无运行时依赖可先行；B 依赖 A 的目录数据；C 不依赖 A/B（effect_policy 执行复用一期 after_check_resolved/StatePatch 链），与 B 同装时效果落账自然被 watcher 覆盖。每任务一段精确范围；验收编号引用 spec §8。

### Section A —— 解析侧（mechanics_compile 第三遍 + kernel 结构升级）

**A1. trpg-model 共享类型模块 + kernel/图谱字段扩展**
新建 `crates/trpg-model/src/mechanics.rs`：契约 §1/§3/§4 全部类型 + 纯函数 `expressiveness_tier`/`granularity_seconds`/`track_semantic_line(track:&Value, current:i32)->Option<String>`（B3 预埋：用 thresholds 区间 consequence + zero_means 渲染语义行，无可渲染语义 → None）。`lib.rs` 三处小改（pub mod + RuleKernel.mechanics_catalog + ScenarioNode.scene_mechanics，全 `#[serde(default)]`）。测试：老 kernel JSON（从真 :54347 CoC kernel dump 一份进 fixtures）反序列化照常 + 新区 round-trip + MechanicKind::Other 兜底反序列化 + ProcedureStep/FollowupCondition untagged 降档保留。先 `cargo tree -p serde` 核 variant-level untagged 支持，不支持则按契约注记手写 Deserialize。验证：`cargo test -p trpg-model`。

**A2. mechanics_compile 编译遍本体**
新建 `crates/trpg-rule-agent/src/reader/mechanics_compile.rs`：对标 `chargen_compile.rs` 样板——`MechCompileCtx`（units/sidecar_text/located_pages 复用 `CompileCtx` 形状）、`MECH_SYS` prompt（**开放枚举**：遍历规则书找一切"游戏进行中会被触发、需 GM 配合执行"的机制，kind 是表达形态不是过滤器；四档表达力指引；source_refs 必填；when_to_use 写触发语义）、`submit_mechanics` 工具（`tools::submit_tool("submit_mechanics", …, {"mechanics_catalog":{"type":"array"}}, &["mechanics_catalog"])`）、`run_compile_loop` 同款 `complete_with_tools` 子循环 + 两轮 merge-by-id（grounded: chargen_compile.rs L273-332）。`pub async fn compile_mechanics_catalog(client: &dyn LlmClient, kernel: &mut RuleKernel, ctx: MechCompileCtx<'_>, budget: usize) -> Vec<String>`（就地升级，返回 gap notes）。模型经 env `TRPG_MECHANICS_COMPILE_MODEL` 默认 gpt-5.4。`reader/mod.rs` 接线 pub mod。验证：`cargo test -p trpg-rule-agent`（MockLlm 脚本：submit 两轮 merge、未调 submit → 目录保持空不编造）。

**A3. mechanics_finalize 确定性护栏**
新建 `crates/trpg-rule-agent/src/reader/mechanics_finalize.rs`：`pub fn finalize_catalog(raw: Vec<Value>, kernel: &RuleKernel) -> (Vec<MechanicEntry>, Vec<ValidationMessage>)`——①tested_parameter 必须存在于 `kernel.character_sheet_schema.fields`（含 skills 桶约定，大小写不敏感），不存在 → 丢弃条目记 report（验收 3 的人为坏条目注入测试在此）；②followup_links[*].procedure_id 引用闭合（指向目录内真实 id），断链 → 丢 link 留条目；③无 source_refs → 丢弃（唯一丢弃理由=不编造）；④结构化不了的 procedure/hook 降档保留（Other/语义条目），**绝不"不认识就丢"**；⑤hooks 引用引擎无事件点的（对照 EngineHook 词表外的提交值）→ 剥 hook 降语义条目并记 report。validation_report 经 `kernel.validation_report.warnings` 写回。验证：`cargo test -p trpg-rule-agent`（坏条目注入=验收 3、断链、降档各一测）。

**A4. 配套升级同遍：thresholds followup + success_bands 语义化补全 + fields.notes**
扩展 A2 prompt 与 A3 写回：编译遍同时提交 `thresholds_upgrades`（给既有 resource_tracks[*].thresholds[*] 补 `followup_procedure_id`，finalize 按 track id+threshold 形状匹配后 merge 进 kernel.resource_tracks 的 Value，绝不覆盖既有字段）、`success_bands_upgrades`（每 band 补 `semantics`/可选 `mechanical_effects`；**band 完整性护栏**：缺 critical/failure band → 按 check_model prose 补全并记 validation_report——历史 `_fix_coc_success_bands.sql` 手修普及为编译护栏，验收 12①）、`field_notes`（character_sheet_schema.fields[*].notes 缺者补"是什么/怎么用"，含 sanity/luck/mp 派生值）。round-trip 护栏：写回后 `serde_json::from_value::<RuleKernel>(to_value(kernel))` 必须成功 + `normalize_resource_tracks`（model L1449）不丢新字段。注意 db 读取时 override 文件 merge（trpg-db L706 `read_kernel_override_file` 只管 resource_tracks/dice_core）天然兼容新字段——shallow merge 键级覆盖，加一条回归测试。验证：`cargo test -p trpg-rule-agent -p trpg-db`。

**A5. on_outcome 结构化 band 触发 + amount max_of（trpg-mechanics 读取点）**
修改 `crates/trpg-mechanics/src/lib.rs` `apply_outcome_resource_tracks`（L96-115 trigger 匹配分支）：trigger 先 `.as_str()` 走既有词表（always/on_success/on_failure/on_tier 行为零变化），再 `.as_object()` 认 `{kind:"band",band_id}`——对账 `result.outcome` 的 band 字段（grounded: 既有 on_tier 对账 `outcome.success_tier`/`success_tier_rank` L106-109；band 形态对账同源字段，band_id 与 success_tier 同义则复用，执行时先 SQL 查真实 outcome JSON 形状定字段名）。`resolve_track_amount`（L132 调用处）增 `max_of:<dice>` 形态：解析骰式取最大值（纯函数 + 单测：`max_of:1d10`→10、`max_of:2d6`→12）。认不出的 trigger/amount → 该 rule 跳过（fail-closed，老 kernel 行为不变=验收 12② 后半）。验证：`cargo test -p trpg-mechanics`（cwd 在 crate 目录）。

**A6. parse 管线接线 + mechanics_proto bin**
修改 `crates/trpg-parser/src/staged.rs`：在 kernel 产出→`upsert_rule_kernel` 之间接 `compile_mechanics_catalog`（对标 L151 chargen 接线：同款 compiler client 构造、sidecar/units 来源——**注意 chargen 同款 sidecar 坑**：`layout_sidecar_path` metadata 恒 None，直读 `markdown/…` 同 module_reader 修法）；门 `TRPG_MECHANICS_COMPILE` 默认开，编译 Err/超时 → 保留原 kernel 照常 upsert（fail-closed，老规则集零损）。新建 `crates/trpg-rule-agent/src/bin/mechanics_proto.rs`：`--ruleset <id>` 单规则直打 units 编译并打印目录摘要/validation_report（对标 module_reader_proto），供 A7 跑批复用。验证：`cargo build -p trpg-parser -p trpg-rule-agent` + proto 对 CoC 单跑冒烟。

**A7. 六规则跑批 + 附录 A 覆盖率审计（Slice A 验收闸门）**
真库（docker exec chatrpg-postgres-rulesets psql，:54347）对六套规则（CoC/Cyberpunk/Triangle/ORC/D&D中文/剑世界中文）跑 mechanics_proto / parse 重抽：①六套 mechanics_catalog 生成成功、validation_report 记录丢弃及原因（验收 1）；②CoC 断言：含 sanity_check 与 temporary_insanity 且 followup 链通（sanity.thresholds.loss_in_one_go→temporary_insanity）、含跳跃语义条目带 when_to_use（验收 2）；③覆盖率审计 agent（subagent 或 proto --audit 模式）以附录 A 137 条为基线对照目录，缺失记 validation_report；每套抽查 ≥1 非预设类别机制被收录 + 至少一例纯语义条目 + 一例 EngineHook 条目（验收 3a）；④CoC bands 含 critical/failure 且 report 记录补全动作（验收 12①）。产出审计报告落 `docs/` 一份（中文）。零 per-ruleset 代码分支核查（grep 无规则集名字面量）。

### Section B —— 运行时（注入 + watcher + 机械债务）

**B1. BP1 目录索引投影 + kernel BP1 块瘦身（缓存稳定关键任务）**
修改 `crates/trpg-runtime/src/lib.rs` `rule_steward_prefix_blocks_for_turn`（L1432）：①既有 RuleStewardKernel 块的 `BlockContent::Json(to_value(&kernel))` 改为序列化前置空 `mechanics_catalog`（克隆 kernel 清该区——否则 137 条全文直接灌 BP1 爆 `validate_compiled_budget` 的 fail-closed Err）；②新增紧凑索引块 `mechanics_catalog_index.{ruleset_id}`：每条一行 `id | name | when_to_use`（CoC 约 50-70 条 ≈2-3k token），`CacheZone::Prefix`/`Stability::RarelyChanged`/`Scope::ruleset`，渲染纯函数 `catalog_index_text(&[MechanicEntry], limit)` 放 trpg-model/mechanics.rs；③分级 data-driven：条目数 > env `TRPG_MECHANICS_INDEX_BP1_LIMIT`（默认 96）时仅 `SubsystemProcedure` + 带 hooks/passive_projection 条目进索引，长尾靠 lookup_mechanic（按条目数阈值非规则集名——护栏 §3.5.3）；④PM 档常驻投影：passive_projection 非空的条目按 owner 当前值渲染语义行进同一索引块尾部（值取 actor sheet/track current，取不到 → 跳过该行 fail-closed）。缓存回归：扩展 prompts.rs `cache_stability_tests` 同款——目录注入后跨回合 `prefix_byte_hash(2)` 不变（验收 13 的单测半边）。验证：`cargo test -p trpg-runtime -p trpg-gm`。

**B2. lookup_mechanic 工具 + roll_check(mechanic_id) 继承**
新建 `crates/trpg-gm/src/tools/mechanic.rs`：LookupMechanicTool 按契约 §6 schema，`ctx.engine.db.load_rule_kernel(&ctx.request.ruleset_id)` 取目录按 id 查（大小写不敏感），命中回条目全文 JSON + expressiveness_tier，miss → `mechanic_not_found`（recoverable，hint 指 BP1 索引）。修改 `tools/check.rs`：RollCheckArgs 增 `mechanic_id: Option<String>`/`scene_mechanic_id: Option<String>`；给定 mechanic_id 时从目录继承 tested_parameter（显式 args 优先——目录是知识不是枷锁）、procedure 首个 Roll 步的 dice 覆盖 kernel_dice 缺省、advice_refs 追加 `mechanic:{id}`；目录 miss → `mechanic_not_found`。修改 `tools/mod.rs`：`ToolRegistry::standard()` 尾部追加两工具（10→12，既有顺序字节不变）、错误码登记、`registry_has_ten_tools_in_stable_order` 测试更新为 12。验证：`cargo test -p trpg-gm`（继承绑定正确=验收 6 的 mechanic_id 半边；schema_stability 测试照绿）。

**B3. band 语义投影 + track 投影语义化（背景板防治三件套之"语义可见"）**
①修改 `tools/check.rs` roll_check 结果 JSON：增 `band_semantics` 字段——从 kernel `dice_core.success_bands` 按 outcome band 查 `semantics` 渲染（"extreme——极难成功：贯穿/卓越效果"），无 semantics → 裸 band id+label（fail-closed，验收 12③ 单测半边）；②修改 `crates/trpg-mechanics/src/lib.rs` `mechanical_ledger_context_block`（L271）与 generic_parameter_states 投影：资源行附 `track_semantic_line`（A1 纯函数：当前值所处 thresholds 区间 consequence + zero_means 渲染，如 `sanity: 38 —— 距临时疯狂阈值正常`）；③owner_kind 通用：投影按 actor/scene/party/world 开放处理（grounded: apply_outcome_resource_tracks L89 现仅二分 actor/scene——投影侧补 party/world 的 generic_parameter_states 读取路径，写入路径维持现状记 validation 可观测），**不得只投 actor 级**（Triangle chaos=scene 实证）。fail-closed：无 thresholds/zero_means → 裸数值。验证：`cargo test -p trpg-mechanics -p trpg-gm`。

**B4. watcher 阈值检测 + mechanic_dues 持久化**
新建 `crates/trpg-mechanics/src/watcher.rs` + `migrations/0027_mechanic_dues_v120.sql` + trpg-db 三原语（契约 §4 全量签名）。重构 `apply_outcome_resource_tracks` 阈值分支（L168-189）：crossed/loss_in_one_go 判定抽纯函数 `detect_crossings(track:&Value, before, after, op) -> Vec<ThresholdCrossing>`（读新 `followup_procedure_id` 字段），既有 CreateFact patch 行为不变 + 新增构造 MechanicDue（evidence 含 before/after/delta）`insert_mechanic_due` 落库；`apply_direct_effect`（direct_effect.rs L84）落账后对同一纯函数补调（旧路径免费受益——拍板决策）。roll_check 工具结果带回本次 dues（agent 当场看见）。单测（验收 4）：合成结算 SAN 单次 -6 → due 产出且 evidence 含前后值；-4 → 无 due；HP 穿 0（at_or_below）→ due；无 followup_procedure_id 的阈值 → due 仍发带 prose consequence。验证：`cargo test -p trpg-mechanics -p trpg-db`（迁移注意 include_str! 数组手动加行）。

**B5. EngineHook 事件点发 due（第三触发通路运行时侧）**
watcher.rs 增 `dues_for_hook`（契约 §4）：单点查询原语——遍历 kernel.mechanics_catalog 匹配 event 的 hooks 产 due（mechanic_id 结构化绑定）。接线三处：①`turn_loop.rs` 确定性头部（步骤 1 末尾）`HookEvent::TurnStart`；②`tools/world.rs` NavigateSceneTool `set_session_scene` 成功后 `SceneEnter{scene_id}`；③AdvanceTimeTool `advance_world_time` 后 `TimeAdvance{from_tick: result.from.world_tick, to_tick: result.to.world_tick}`——calendar 跨界在该分支内按 `granularity_seconds` 整除边界判定（Fate 8 段=10800s 粒度数据驱动），scale=downtime 同时发 `Rest`。SessionEnd/DevelopmentPhase 二期仅原语支持 + CLI 会话收尾处预留调用点，无事件点的解析钩子已被 A3 降级（缺口可观测）。单测：MockKernel 含 calendar{segments_per_day:8} 条目，advance 跨段 → due，段内 → 无 due。验证：`cargo test -p trpg-mechanics -p trpg-gm`。

**B6. 债务清单门控 + waive_obligation + 追溯债务（loop 心脏改动）**
新建 `crates/trpg-gm/src/obligations.rs`（契约 §5 全量）+ `tools/mechanic.rs` 增 WaiveObligationTool（契约 §6：必带 reason，副作用三连——db status、勘误记忆 tags=["gm_waive"]、ledger.waive）。修改 `turn_loop.rs`：①GmLoop 增 `obligations: ObligationLedger` 持久字段（同 errata 跨回合存活）+ 回合头部 `list_open_mechanic_dues` 装载遗留；②工具轮 `if !saw_tool`（L116）改：先 `obligations.blocking()`——非空则 `messages` 回填 `block_text()` 的 system 观察继续下一轮（**不进叙事轮**），空才 `narrated=true; break`；③轮耗尽带债 → 既有 ToolChoice::None 强制叙事语义不变（一期 L131-142 原样），收尾把 `carryover_block()` 写勘误记忆 + 存 GmLoop 字段供下回合 `DynamicTailInput.obligations_block`（prompts.rs 扩展）；④`verify_after_stream`：InventedEffect findings → `absorb_retro_debts`（契约 §5 grounded 注记）。借用注意：ToolCtx 是 `&self.engine` 块级借用（turn_loop L65-118），ObligationLedger 须经 `&mut` 传 dispatch 或经 RefCell——优先把 dues 状态读写走 db + 工具轮后统一 absorb，避免与 ledger `&mut` 冲突（执行时按编译器裁决，两形态都已预留）。单测（验收 5）：MockLlm 断言有未处理 due 时回填债务观察且当轮不结束；waive 带理由后放行 + 勘误记忆落账；轮耗尽债务进下回合 BP3。验证：`cargo test -p trpg-gm`。

**B7. verifier referenced_ledger_ids 结构化升级（一期遗留清算，护栏 §3.5.5）**
修改 `crates/trpg-agent/src/gm_loop.rs` `NarrationVerifier::verify`（L176）：`submission.referenced_ledger_ids` 非空 ⇒ 按 `TurnLedgerSnapshot` 的 check_id/roll_id/effect id 集合结构核对（引用 id 不在账本 → InventedEffect finding），子串扫描仅在引用为空时回退且 finding detail 前缀 `fallback:substring_scan`（可观测技术债，不静默）。修改 `turn_loop.rs` `verify_after_stream`（L155）：`referenced_ledger_ids` 从 `vec![]` 改为传本回合账本 id 全集（ledger.snapshot() 的 contracts/results/effects id——"已落账事实全集"语义）。既有 gm_loop.rs 测试（L468-600 区）referenced_ledger_ids 用例全部照绿 + 新增 fallback 标注断言。验证：`cargo test -p trpg-agent -p trpg-gm`。

**B8. gm_skill 准则两条 + Slice B 收口单测包**
新建 `data/agent/gm_skill/global/40_mechanics_catalog.md`（目录优先于自由发挥：BP1 索引→lookup_mechanic→roll_check(mechanic_id)；含"按 ruleset 条款持续评估玩家言行并发放/扣减元资源"行为语义监听条款——Triangle 嘉奖/Fate FP 双实证）与 `50_obligation_policy.md`（due 必须回应：处理或 waive 带理由；追溯债务=补 apply_effect 或 waive）。grounded: load_gm_skill 按文件名字典序合并（prompts.rs L85-94），新文件排 30_output_contract.md 之后追加不挪既有字节。收口：跑全 Slice B 单测矩阵（验收 4/5/6 全绿）+ 一次真库 CLI 冒烟（`trpg play --agent`，CoC，确认 BP1 索引出现 + lookup_mechanic 可调 + 无债空转回合行为与一期无差异）。

### Section C —— 模组侧 + e2e 黄金链

**C1. SceneMechanicIntent 落模型 + 旧图谱向后兼容**
（若 A1 已并入 ScenarioNode.scene_mechanics 字段则本任务只做测试半边。）`crates/trpg-model/src/mechanics.rs` 契约 §3 类型 + ScenarioNode 字段。测试：真模组 bundle JSON（血色公路/The Vault fixtures dump）反序列化照常（default 空 vec=旧模组不重抽不受损，spec §9 风险 3）、EffectPatchIntent 五形态 round-trip、Other 形态不丢原 JSON。验证：`cargo test -p trpg-model`。

**C2. 深抽契约扩展：scene_mechanics 顺路产出（零新增管线）**
修改 `crates/trpg-rule-agent/src/reader/module_reader_loop.rs`：`submit_deep_tool`（L102）schema 的 scene object 描述加 scene_mechanics 数组；`oneshot_deep_extract` user prompt（L201）与 `DEEP_SYS`（module_reader.rs）加指引——"场景文本**明确写出**的检定（DV/技能/后果）编译为 scene_mechanics，每条必带 source_anchor 摘自页面原文；没写明的绝不编造"。修改 `apply_deep_to_node`（module_reader.rs L117）：解析 `scene.scene_mechanics` → `serde_json::from_value::<SceneMechanicIntent>` 逐条，source_anchor 空/缺 → 丢弃该条（fail-closed，对标 links 的 anchor 收口先例 L161-165）；tested_parameter 空 → 丢弃。oneshot 与 ReAct 回退两路径共用（grounded: 都走 apply_deep_to_node）。单测：MockLlm submit 带 2 条 intents（1 条无 anchor）→ 仅 1 条入 node；旧 deep payload 无 scene_mechanics → 空 vec 照常。验证：`cargo test -p trpg-rule-agent`。

**C3. 当前场景 intents 投影（BP2）**
修改 `crates/trpg-runtime/src/lib.rs` `scene_node_to_blocks`（L2233）：node.scene_mechanics 非空时追加独立块 `module.{module_id}.scene.{node_id}.mechanics`——每条一行 `intent_id | description | tested_parameter | difficulty 摘要`（不含 effect_policy 全文，结算才用），`CacheZone::PinnedMiddle`/`Stability::SceneStable`/`expires_at_scene=node_id`（spec §6"随 P4 投影进 BP2"；场景切换换块=SceneStable 语义，pinned_hash 场景内稳定）。渲染纯函数 `scene_intents_text(&[SceneMechanicIntent])` 放 trpg-model/mechanics.rs。fail-closed：空 vec → 不出块（旧模组零变化）。单测：对标既有 scene_node_to_blocks 测试样板，intents 块存在性 + 场景内跨回合字节稳定。验证：`cargo test -p trpg-runtime`。

**C4. roll_check(scene_mechanic_id) + effect_policy Rust 强制执行**
新建 `crates/trpg-gm/src/scene_policy.rs`：`pub async fn apply_effect_policy(engine: &RuntimeEngine, request: &ContextRequest, intent: &SceneMechanicIntent, success: bool, ledger: &mut TurnLedger) -> Result<Vec<StatePatch>>`——按 outcome 选 on_success/on_failure，逐条 EffectPatchIntent 按契约 §3 映射执行（ObjectPatch 经 db 既有对象态原语 / ModifyTrack 经 `apply_direct_effect` L84 真实签名 / CreateFact 直落 / StartCountdown 经 `WorldTimeService::schedule_in` L165；Other → 落 unexecutable_intent 事实）；每条产出入账 ledger（效果落账自然进 watcher/债务体系——与 B 同装时 ModifyTrack 触发 B4 阈值检测零额外代码）。修改 `tools/check.rs`：scene_mechanic_id 给定时从 `ctx.engine.db.load_module_graph(module_id)` 当前场景（ctx.state.scene_id）查 intent（miss → `scene_mechanic_not_found`），继承 tested_parameter/difficulty，`execute_system_roll_bundle` 结算后调 `apply_effect_policy`（**效果不留给叙事**），结果 JSON 带 executed_patches 摘要。单测：MockEngine 路径下 success/failure 分支各执行正确集合、Other 不炸。验证：`cargo test -p trpg-gm`。

**C5. e2e 黄金链一：CoC SAN→疯狂全链 + 跳坑感知 + 成功度三件套（验收 7/8/12）**
真 :54347 DB + 真 LLM + CoC 规则集（`call_of_cthulhu_7e`，.env DATABASE_URL 端口坑注意）。①SAN→疯狂（验收 7）：目睹恐怖场景 → agent 放 SAN check（目录驱动非玩家明示）→ 失败掉 SAN ≥5 → watcher due → agent 放临时疯狂检定**或** waive 带理由——SQL 终验 check_contracts / generic_parameter_states / mechanic_dues / 勘误记忆 gm_waive 全链可查；②跳坑感知（验收 8）：玩家"我跳过裂隙"（不提检定）→ roll_check 的 tested_parameter 与**目录跳跃语义条目所绑参数一致**（断言从目录解析得出，不写死技能名——护栏 §3.5）；③成功度三件套（验收 12②③）：SAN 检定 fumble → max_of 损失真实落库（解析出 band 触发验机械路径，未解析出记目录缺口**不静默通过**）；extreme vs regular 工具结果含不同语义行、叙事分档有 band 语义证据。记录回合时长 vs 一期基线（风险 4）。

**C6. e2e 黄金链二：Homecoming 场景意图 + Triangle chaos 累积感知（验收 9/10）**
①Homecoming（CPR 线性模组，已 3 结构实测基线）：重抽含切线缆场景 → scene_mechanics 含 cut_cable 类 intent（DV13/brawling/effect_policy）→ play 至该场景切线缆 → 成功后 SQL 验 `athena_cable` 对象状态 + 倒计时（scheduled_events 或 track）真实落库——effect_policy 强制执行非叙事声明（验收 9）；②Triangle（The Vault）：连跑 ≥3 回合，SQL 验 `chaos_pool`（scene 级 owner_kind 通路）current 递增、BP3 投影含语义状态行（B3 的 track_semantic_line）、目录含 chaos 花费/异常体条目（kind=spend/subsystem_procedure）、池子高位时 agent 行动/叙事反映池状态（账本/叙事证据判定——验收 10"累计系统真的被记得并加强影响"）。两模组均先删 bundle 重抽（规则书 cached 只重抽模组，既有 e2e 流程）。

**C7. e2e 黄金链三：追溯债务闭环 + 缓存回归 + 总验收核对（验收 11/13）**
①追溯债务（验收 11，MockLlm 脚本可控）：构造 agent 叙事声称伤害但未调工具的回合 → verifier 抓 InventedEffect → 下回合债务清单含 RetroactiveEffectDebt（断言 block_text 出现）→ 脚本第二回合补 apply_effect → SQL 验状态真实变化 + 债务清除；②缓存回归（验收 13 e2e 半边）：真库连续 3 回合（场景不变）gm_cache tracing 的 prefix_hash/pinned_hash/request_prefix_hash 全相同，BP1 含目录索引、BP2 含 intents 块时 hash 仍按 ruleset/场景固定；③总验收：对照 spec §8 全 14 条逐项打勾出报告（中文，落 `docs/`），含工程约束三查——新文件全 `wc -l` ≤400、grep 零 per-ruleset 硬编码、全新结构 `#[serde(default)]`；遗留/缺口（SessionEnd 事件点、party/world 写路径、band trigger 未解析规则集）以"可观测技术债"清单收口。

---

## 验收映射速查（spec §8 ↔ 任务）

| 验收 | 任务 | 验收 | 任务 |
|---|---|---|---|
| 1 六套目录生成 | A7 | 7 SAN→疯狂全链 | C5 |
| 2 CoC sanity/jump 断言 | A7 | 8 跳坑感知 | C5 |
| 3 坏条目护栏 | A3 | 9 Homecoming 切线缆 | C6 |
| 3a 覆盖率审计 vs 附录 A | A7 | 10 Triangle chaos | C6 |
| 4 watcher 单测 | B4 | 11 追溯债务闭环 | C7 |
| 5 债务门控单测 | B6 | 12 成功度三件套 | A4①/A5②/B3③ + C5 |
| 6 lookup/继承单测 | B2 | 13 缓存回归 | B1（单测）+ C7（e2e） |




---

## 任务正文

## Section A —— 解析侧（mechanics_compile 第三遍 + kernel 结构升级）任务正文

> 母计划：`docs/superpowers/plans/2026-06-10-rule-aware-gm.md`（共享类型契约逐字以母计划为准，本文件引用处均为逐字摘录）。spec：`docs/superpowers/specs/2026-06-10-rule-aware-gm-design.md`（§4 支柱 1、§3.5 理念护栏、§8 验收 1/2/3/3a/12①、附录 A 137 条审计基线）。
>
> **依赖序**：A1（类型）→ A2（编译遍）→ A3（护栏）→ A4（配套升级，扩 A2/A3）→ A6（接线+proto）→ A7（跑批闸门）。A5（trpg-mechanics 读取点）只依赖 A4 定下的 JSON 形状，可与 A6 并行。
>
> **本节通用工程约束**（每任务默认包含，不再逐条重复）：新文件 ≤400 行（超则按 `module_reader_loop_tests.rs` 先例把测试拆 `#[cfg(test)] #[path = "..._tests.rs"] mod tests;` 子文件）；零 per-ruleset 硬编码（代码逻辑分支零规则集名字面量；注释/prompt 示例文案可举例）；全部新结构 `#[serde(default)]` 向后兼容；无 git，每任务以 cargo 验证收尾。
>
> **本节源码核对记录**（写本文件时已逐一打开核实，执行 worker 可直接信任行锚，但改动前仍须打开目标文件）：
> - `chargen_compile.rs`：`CompileCtx`（L223-232）、`COMPILE_SYS`（L234-257）、两轮 merge-by-id（L306-332）、`run_compile_loop` complete_with_tools 子循环（L404-442）、`compile_dispatch`（L444-463）、`read_layout` 为 **pub**（L34）、`finalize_compiled` 护栏样板（L177-217）、`sanitize_record`（L140-171）。
> - `trpg-model/src/lib.rs`：`RuleKernel` L1370-1404（`resource_tracks`/`dice_core`/`character_sheet_schema` 全是 `serde_json::Value` 形态）、`ScenarioNode` L1602-1619、`ValidationReport`/`ValidationMessage` L1650-1662、`SourceRef` L139-148、`normalize_resource_tracks` L1449、`CharacterTemplate`/`CharacterField`/`DerivedValue` L904-962。lib.rs 现为单文件 6620 行、无子模块——`pub mod mechanics;` 是该 crate 第一个子模块。
> - `trpg-db/src/lib.rs`：`upsert_rule_kernel` L660（整 kernel serde 进 content_json）、`load_rule_kernel` L691（含 normalize + override 分层）、`read_kernel_override_file` L3922、`merge_dice_core` L3934（shallow key 替换整值）、`merge_resource_tracks` L3943（按 id 整 track 替换）、既有测试 mod `dice_core_override_tests` L3954。
> - `trpg-mechanics/src/lib.rs`：`apply_outcome_resource_tracks` L65（trigger 匹配 L96-115、`resolve_track_amount` 调用 L132-139、阈值分支 L168-189）、`resolve_track_amount` L1114-1157、`roll_amount_dice` L1035、`is_plain_dice` L1050、`on_tier` 对账 `outcome.success_tier`/`success_tier_rank` L106-109。全文件 1334 行（存量超 400，本节只做局部小改不强拆）。
> - `trpg-parser`：staged 路径 `stage2_deep` chargen 接线 L134-151、`persist_stage2_kernel` L1816-1836（内部 upsert）；非 staged 路径 `parse_rulebook` kernel 产出 L703-707、chargen ctx/sidecar 直读修法 L539-565（`markdown/rulebooks/{source_id}.md`，`layout_sidecar_path` 恒 None 的坑及其修法注释 L542-550）、`build_compiler_llm` L1541-1548；CLI `parse-all` 走非 staged 路径（main.rs L485-502），`parse-staged`/API ingest 走 staged（main.rs L2085、api L543）。
> - `module_reader_proto.rs`：proto bin 样板（位置参数 + env LLM + 摘要打印 + tracing subscriber）。
> - `module_reader_loop_tests.rs`：ReplayClient（手写 `impl LlmClient`，complete_with_tools 回放 tool_call）测试样板 L11-42。
> - 版本：workspace serde **1.0.228**（≥1.0.171，变体级 `#[serde(untagged)]` 可用）；schemars **0.8.22**（变体级 untagged 的 JsonSchema 派生支持存疑，A1 给退路）。
> - 命名冲突检查：MechanicEntry/MechanicKind/ProcedureStep/EngineHook/CalendarGranularity/FollowupLink/FollowupCondition/SceneMechanicIntent/EffectPolicy/EffectPatchIntent/MechanicDue/DueSource/DueStatus/ExpressivenessTier 在 trpg-model 现无同名类型，`pub use mechanics::*` 平铺安全。

---

## A1. trpg-model 共享类型模块 + kernel/图谱字段扩展

二期全部共享类型落一个新模块，绝不往 6620 行的 lib.rs 里塞类型。同时给 RuleKernel/ScenarioNode 各加一个 `#[serde(default)]` 字段——这是本期对这两个核心结构**仅有的字段级改动**，老 kernel/老图谱 JSON 必须照常反序列化（专门回归测试押阵）。

### ① Files

- **Create** `crates/trpg-model/src/mechanics.rs` —— 母计划《共享类型契约》§1/§3/§4 全部类型 + 3 个纯函数 + 单测（行数超 400 时测试拆 `crates/trpg-model/src/mechanics_tests.rs`，经 `#[cfg(test)] #[path = "mechanics_tests.rs"] mod tests;` 引入）。
- **Create** `crates/trpg-model/tests/kernel_backcompat.rs` + `crates/trpg-model/tests/fixtures/coc_kernel_pre_mechanics.json` —— 真库 dump 的老 kernel 回归 fixture（该 crate 现无 tests/ 目录，本任务新开）。
- **Modify** `crates/trpg-model/src/lib.rs` 仅 3 处：
  1. 文件头部声明 `pub mod mechanics;` + `pub use mechanics::*;`（与该 crate 既有平铺导出风格一致，下游 `trpg_model::MechanicEntry` 直接可用）；
  2. `RuleKernel`（L1370）在 `object_schemas` 字段（L1395）之后插入 `#[serde(default)] pub mechanics_catalog: Vec<MechanicEntry>,`；
  3. `ScenarioNode`（L1602）在 `referenced_encounter_ids`（L1618）之后插入 `#[serde(default)] pub scene_mechanics: Vec<SceneMechanicIntent>,`。

### ② 接口契约

类型定义**逐字**采用母计划《共享类型契约》§1（`MechanicEntry`/`MechanicKind`/`ProcedureStep`/`EngineHook`/`CalendarGranularity`/`granularity_seconds`/`FollowupLink`/`FollowupCondition`/`ExpressivenessTier`/`expressiveness_tier`）、§3（`SceneMechanicIntent`/`EffectPolicy`/`EffectPatchIntent`）、§4 类型部分（`MechanicDue`/`DueSource`/`DueStatus`——结构体归 A1，watcher 产出原语归 Section B），此处不重抄。A1 额外补足母计划任务索引点名的预埋纯函数（B3 消费，纯函数先行落位带测试）：

```rust
/// B3 预埋：track 投影语义行。用 track（kernel resource_tracks 的一个 Value）已解析的
/// `thresholds`（当前值所处区间的 consequence，按 at + direction 定位）与 `zero_means`
/// 渲染语义状态行，如 "sanity: 38 —— 距临时疯狂阈值正常" / "chaos_pool: 7 —— <consequence 摘要>"。
/// 无 thresholds/zero_means 可渲染 → None（fail-closed，调用方退回裸数值）。
pub fn track_semantic_line(track: &serde_json::Value, current: i32) -> Option<String>;
```

`granularity_seconds` 的判定契约（执行歧义点，定死）：`seconds_per_unit` 有值优先生效；否则 `unit=="day"`→86400、`"week"`→604800、`"segment"` 且 `segments_per_day` 有值→`86400/segments`；其余（含 `"month"`——日历月不规则，秒数必须由数据给 `seconds_per_unit`，不猜）→ `None`。

### ③ 测试清单（测试即规格）

mechanics.rs 单测：
- `mechanics_catalog_round_trips` —— 构造全字段 MechanicEntry（含 procedure 4 形态、hooks 含 Calendar、followup_links、passive_projection、locked_until、source_refs）入 kernel → `to_value` → `from_value::<RuleKernel>` 字段全等。
- `mechanic_kind_other_fallback` —— `"kind":"honor_economy"` → `Other("honor_economy")`；`"kind":"skill_check"` → `SkillCheck`；缺 kind 字段 → default（`Other(String::new())`）。
- `procedure_step_unknown_shape_preserved` —— `{"step":"weird_custom","foo":1}` → `ProcedureStep::Other(v)` 且 round-trip 后原 JSON 一字不丢（降档保留，理念护栏 §3.5）。
- `followup_condition_other_preserved` —— 同上对 `FollowupCondition`。
- `engine_hook_serde_tags` —— `{"event":"scene_enter"}`→SceneEnter、`{"event":"calendar","granularity":{"unit":"segment","segments_per_day":8}}`→Calendar；`{"event":"galactic_alignment"}` 反序列化 **Err**（A3 护栏靠这个 Err 判"引擎无事件点"，词表单一事实源就是这个 enum）。
- `granularity_seconds_units` —— day=86400；week=604800；segment(8)=10800（Fate 一天 8 段实证）；`seconds_per_unit:3600` 优先于 unit；`unit:"month"` 无 seconds_per_unit → None。
- `expressiveness_tier_priority` —— procedure 非空→Procedure；procedure 空 hooks 非空→Hook；仅 passive_projection→PassiveModifier；全空→Semantic（按数据特征算、不存冗余字段，护栏 §3.5.3）。
- `track_semantic_line_renders_threshold_zone` —— 含 `thresholds:[{at:0,direction:"at_or_below",consequence:"死亡"},{loss_in_one_go:5,consequence:"临时疯狂"}]` 与 `zero_means` 的 track、current=38 → `Some` 且文案含可定位语义；无 thresholds 无 zero_means → None。
- `scene_mechanic_intent_round_trips` + `effect_patch_intent_other_keeps_json` —— §3 类型 round-trip；`EffectPatchIntent::Other` 不丢原 JSON（C1 只补真模组 fixture 半边，类型测试在本任务做掉）。

tests/kernel_backcompat.rs 集成测试：
- `old_coc_kernel_json_deserializes_unchanged` —— fixture（真库 dump、不含 mechanics_catalog 键）`from_str::<RuleKernel>` 成功；`mechanics_catalog` 为空 vec；`resource_tracks.len()`/`dice_core` 等关键区与 fixture 内 jsonb 逐项一致（防新字段挤坏老布局）。
- `old_scenario_node_json_no_scene_mechanics_ok` —— 旧 ScenarioNode JSON（无 scene_mechanics 键）反序列化成功且空 vec。

### ④ 实现要点

- **serde/schemars 预检**：serde 1.0.228 已核，变体级 `#[serde(untagged)]` 可用；**风险在 schemars 0.8.22 的 JsonSchema 派生**——若对变体级 untagged 编译报错，按序退路：(a) 对该 enum 手写 `impl JsonSchema`（返回宽松 object schema，这些 schema 仅内部用）；(b) 仍不行则按母计划契约注记手写 `Deserialize`（字符串匹配不中归 Other）。两退路按编译器裁决，测试矩阵不变。
- `MechanicKind` 的 `Default` 落 `Other(String::new())`（母计划注记原文）。
- fixture dump 命令（执行时按 .env 核对真实库名/用户）：`docker exec chatrpg-postgres-rulesets psql -U <user> -d <db> -t -A -c "select content_json from rule_kernels where ruleset_id='call_of_cthulhu_7e' and active=true order by updated_at desc limit 1" > crates/trpg-model/tests/fixtures/coc_kernel_pre_mechanics.json`。CoC 库在 :54347（memory 坑：.env DATABASE_URL 可能指 :54346 赛博库，先 `select ruleset_id from rule_kernels` 确认连对了库）。dump 若已含本期新字段（先跑过 A6/A7 的库）则手工剔除该键保持"老 kernel"语义。
- `track_semantic_line` 渲染规则：thresholds 按 `at` 排序定位 current 所处区间取最近 consequence 做摘要（截 ~40 字符）；`loss_in_one_go` 型阈值不参与区间定位（它是单次幅度语义），但可作为"距阈值"补充行；current==0 且有 `zero_means` 时优先 zero_means。输出不含表情/标记，纯文本一行。
- RuleKernel 加字段会改变 `stable_json_hash`（upsert content_hash 变化），无下游依赖该 hash 的相等性，无害——但写进任务说明防执行者疑惑。

### ⑤ 验证

```bash
cd crates/trpg-model && cargo test -p trpg-model
wc -l crates/trpg-model/src/mechanics.rs   # ≤400
```

<!-- SECTION_A_APPEND_MARKER -->


---

# Section A 续 —— A2~A7 任务正文

> 接续 `section_A.md`（A1 + 本节通用工程约束 + 源码核对记录，均对本文件生效，不再重复）。母计划：`docs/superpowers/plans/2026-06-10-rule-aware-gm.md`——共享类型契约逐字以母计划为准。

---

## A2. mechanics_compile 编译遍本体

对标 `chargen_compile` 的聚焦编译遍样板，新增解析侧第三遍：一个目标单一的 LLM loop，唯一产出是 `mechanics_catalog` 数组（A4 再扩三个配套升级数组）。开放枚举是本遍的灵魂——"遍历规则书找一切剧情配合机制"，kind 是表达形态分类不是发现过滤器；fail-closed 的语义是"不编造"：模型不调 submit、LLM 出错 → kernel 一字不动。

### ① Files

- **Create** `crates/trpg-rule-agent/src/reader/mechanics_compile.rs` —— `MechCompileCtx` + `MECH_SYS` prompt + `submit_mechanics` 工具 + `run_mech_loop` 子循环 + `compile_mechanics_catalog` 入口 + 单测（超 400 行时测试拆 `mechanics_compile_tests.rs`，经 `#[cfg(test)] #[path = "mechanics_compile_tests.rs"] mod tests;` 引入——`module_reader_loop_tests.rs` 先例）。
- **Modify** `crates/trpg-rule-agent/src/reader/mod.rs` —— `pub mod mechanics_compile;`（按字典序插在 `module_graph_build` 之前、`chargen_compile` 之后均可，与既有列表风格一致）+ `pub use mechanics_compile::{compile_mechanics_catalog, MechCompileCtx};`（对标 L21 chargen 的 re-export 行）。

### ② 接口契约

```rust
/// 编译上下文——复用 CompileCtx 形状（grounded: chargen_compile.rs L223-232）。
pub struct MechCompileCtx<'a> {
    pub units: &'a [Unit],
    /// merged 规则书 `.md` 全文（调用方直读 markdown/rulebooks/{source_id}.md，
    /// 同 chargen 的 sidecar 坑修法——A6 接线时照抄，绝不读 layout_sidecar_path metadata）。
    pub sidecar_text: Option<String>,
    /// 提示页（可空串——机制目录无先验定位页，模型自行 get_toc/search 全书扫）。
    pub located_pages: String,
    /// crate::skill_ids 产物：option catalogs 技能可能不在 sheet schema fields，
    /// 作为 tested_parameter 合法键的补充集传给 A3 finalize。
    pub skill_names: Vec<String>,
}

/// 第三编译遍入口：就地升级 kernel.mechanics_catalog（A4 扩展后还写回 thresholds/
/// success_bands/field_notes 升级），返回 gap notes 供调用方 tracing。
/// 模型客户端由调用方注入（A6 的 build_mechanics_compiler_llm，
/// env TRPG_MECHANICS_COMPILE_MODEL 默认 gpt-5.4）；本文件不读任何 env。
/// 未调 submit / LLM Err / 提交全部不合格 → kernel 不动 + gap note（fail-closed 不编造）。
pub async fn compile_mechanics_catalog(
    client: &dyn LlmClient,
    kernel: &mut RuleKernel,
    ctx: MechCompileCtx<'_>,
    budget: usize,
) -> Vec<String>;
```

submit 工具（grounded: `tools::submit_tool(name, desc, properties, required)` tools.rs L178）：

```rust
let submit = tools::submit_tool(
    "submit_mechanics",
    "Submit the compiled mechanics catalog for this ruleset.",
    json!({"mechanics_catalog": {"type": "array", "items": {"type": "object"}}}),
    &["mechanics_catalog"],
);
// 工具集 = tools::nav_tools()（get_toc/search/read）+ read_layout（描述同 chargen L302）+ submit
```

`MECH_SYS` prompt 全文执行时写，**要点定死**（缺一不可，全部通用零规则集名逻辑）：
1. 角色：把规则书"游戏进行中会被触发、需 GM 在叙事中配合执行"的机制编译为机器目录；产出唯一数组 `mechanics_catalog`，每条按母计划契约 §1 字段形状（id/name/kind/description/when_to_use/tested_parameter/procedure/hooks/passive_projection/followup_links/locked_until/source_refs）。
2. **开放枚举 MUST**：get_toc 后逐章扫，不按预设类别清单抓取；归不进 skill_check/subsystem_procedure/reaction/spend 的机制用自由字符串 kind 照常收录（serde 落 `MechanicKind::Other`）。
3. 表达力四档指引（spec §4）：可结构化的写 procedure（roll/apply/table_roll/gate 步）；事件触发型写 hooks（词表 = EngineHook serde tag 值：scene_enter/turn_start/time_advance/rest/combat_start/combat_end/calendar{granularity}/session_end/development_phase，calendar 粒度按书中真实周期数据化）；持续被动场写 passive_projection 模板；结构化不了的留空 procedure 也必须收录（semantic 档合法）。
4. `when_to_use` 写触发语义（玩家行动/剧情情境），禁止关键词列表。
5. `tested_parameter` 只能取 seed 给出的角色卡真实键；followup_links.procedure_id 指向本次提交目录内的 id。
6. **source_refs 必填**：读过的页才可提交；查无实据的机制宁缺勿造。
7. EXCLUDE：建卡派生数学（chargen 遍已编译）、单次检定的即时数学（check_model/dice_core 已有）——本目录收"何时触发 + 执行程序 + 后续联动"，不重复既有层。

seed user prompt 内容（数据全部从 kernel 现场取，零硬编码）：ruleset_id/书名、located_pages、角色卡合法参数键清单（`character_sheet_schema.fields[*].field_id` + `derived_values[*].field_id` + ctx.skill_names + `resource_tracks[*].id`）、既有 resource_tracks 的 id+thresholds 摘要（让 followup 链对准真实阈值）、dice_core.success_bands 现有 id 列表。

两轮 merge-by-id（grounded: chargen L306-332 样板，差异点如下）：
- `BTreeMap<String, Value>`，键 = `id.trim().to_ascii_lowercase()`，后轮覆盖前轮、并集保留；
- round 0 全书开放枚举；round 1 **无条件跑**（与 chargen 的"required 缺失才跑"不同——机制目录无先验必备清单，第二轮是查漏覆盖率主护栏、验收 3a 的根基）：把已收 `id | name | kind` 清单回给模型，要求对照 TOC 各章节补遗漏，round budget = `budget.min(8)`；
- `run_mech_loop` 照抄 `run_compile_loop` 样板（L404-442）：complete_with_tools、空 tool_calls 回填提醒、submit 即返回数组、dispatch 复用 chargen 四工具分发形状（get_toc/search/read/read_layout——`read_layout` 直接调 `chargen_compile::read_layout` pub fn，输出 cap 3000 字符）。

**写回（A3 叠改点，单点标注）**：本任务先落最小写回——merge 结果逐条 `serde_json::from_value::<MechanicEntry>`，Ok 入 `kernel.mechanics_catalog`、Err 记 gap note（不静默丢）；A3 将把这一处调用替换为 `mechanics_finalize::finalize_catalog`（护栏全集），叠改面即此一个函数调用。

### ③ 测试清单（测试即规格；ReplayClient 样板 = module_reader_loop_tests.rs L11-42 手写 impl LlmClient 回放 tool_calls）

- `submit_two_rounds_merge_by_id` —— 脚本：round1 submit `[{id:"x.a",name:"v1",source_refs:[…]},{id:"x.b",…}]`，round2 submit `[{id:"X.A",name:"v2",…}]` → `kernel.mechanics_catalog` 含 2 条；id "x.a" 的 name=="v2"（后轮胜、大小写不敏感合并）、"x.b" 保留（并集）。
- `no_submit_keeps_catalog_empty` —— 脚本耗尽预算不调 submit → `mechanics_catalog` 为空、返回 gaps 非空（含 "produced nothing" 类 note）、kernel 其余区（resource_tracks/dice_core）serde 字节不变。
- `uncoercible_entry_becomes_gap_not_panic` —— submit 含一条缺 id/类型错的垃圾 + 一条合格 → 合格条入目录、垃圾条变 gap note、不 panic。
- `read_layout_without_sidecar_degrades` —— ctx.sidecar_text=None 时 read_layout 工具回 `[no layout view available…]` 提示文本（对标 compile_dispatch L457-460），loop 继续不中断。
- `seed_contains_sheet_keys_and_track_ids` —— 构造含 fields/resource_tracks 的 kernel → 生成的 seed 字符串含真实 field_id 与 track id（防 seed 构造退化为空清单）。

### ④ 实现要点

- 本文件不读 env、不构造 LLM 客户端——可测性与 chargen 一致（客户端注入）。
- prompt/seed 中只允许通用举例文案，代码逻辑分支零规则集名字面量（A7 有 grep 闸门）。
- submit 解析：`args.get("mechanics_catalog").and_then(Value::as_array)`；缺键/非数组 → 视同未提交，回填提醒继续轮（不要把畸形提交当空目录提前收工）。
- 轮上限 `budget + 8`（chargen 同款），两轮共用同一 merged map。
- gap notes 语义：仅日志用途（A6 tracing::info），不进 validation_report——report 条目是 A3/A4 的职责，分工清晰。

### ⑤ 验证

```bash
cd crates/trpg-rule-agent && cargo test -p trpg-rule-agent
wc -l crates/trpg-rule-agent/src/reader/mechanics_compile.rs   # ≤400
```

---

## A3. mechanics_finalize 确定性护栏

编译产物入 kernel 前的确定性护栏（纯函数、无 IO、可单测密集）。核心纪律两条：**丢弃只有两个理由**（无 source_refs = 编造嫌疑；连 id 都救不回的 uncoercible 垃圾），其余一律降档保留——"不认识就丢"是本遍最大的禁忌；**每次丢弃/降档必有一条 ValidationMessage**，绝不静默。

### ① Files

- **Create** `crates/trpg-rule-agent/src/reader/mechanics_finalize.rs` —— `finalize_catalog` + `sheet_parameter_keys` + 单测。
- **Modify** `crates/trpg-rule-agent/src/reader/mechanics_compile.rs` —— A2 标注的写回单点：最小写回替换为 `finalize_catalog`，messages 追加进 `kernel.validation_report.warnings`（grounded: ValidationReport L1650-1655，warnings: Vec<ValidationMessage>）。
- **Modify** `crates/trpg-rule-agent/src/reader/mod.rs` —— `pub mod mechanics_finalize;`（finalize_catalog 经 mechanics_compile 内部消费即可，re-export 仅 `finalize_catalog` 供 proto/测试直调）。

### ② 接口契约

```rust
/// 确定性护栏：raw（submit_mechanics 的 mechanics_catalog 合并后原始数组）→
/// 合格 MechanicEntry 集 + validation 条目。绝不编造、绝不"不认识就丢"。
/// extra_parameter_keys：sheet schema fields 之外的合法 tested_parameter 键
/// （= A2 ctx.skill_names——option catalogs 技能可能不在 fields；纯 kernel 调用方传 &[]）。
/// 注：较任务索引签名增 extra_parameter_keys 一参，理由如上（grounded:
/// skill_ids 旁注 trpg-parser lib.rs L1710-1713：技能清单 = template skill 字段 ∪ option catalogs）。
pub fn finalize_catalog(
    raw: Vec<serde_json::Value>,
    kernel: &RuleKernel,
    extra_parameter_keys: &[String],
) -> (Vec<MechanicEntry>, Vec<ValidationMessage>);

/// 合法参数键集合（全小写）：character_sheet_schema.fields[*].field_id ∪
/// derived_values[*].field_id（sheet schema 是 CharacterTemplate 的序列化，两桶都在该
/// Value 里）∪ resource_tracks[*].id ∪ extra。skills 桶约定：待校验键带
/// "skills." 前缀时剥前缀再匹配；匹配一律大小写不敏感。
pub fn sheet_parameter_keys(
    kernel: &RuleKernel,
    extra: &[String],
) -> std::collections::HashSet<String>;
```

护栏五条（顺序即执行序；ValidationMessage.code 词表在此定死，target 一律放条目 id）：

1. **tested_parameter 存在性**：`Some(p)` 时 p 必须命中 `sheet_parameter_keys`；不命中 → **丢弃条目** + warning `mechanic_dropped_unknown_parameter`（验收 3 的坏条目注入测试押此条）。`None` 合法——hook/semantic/passive 档条目本就无被试参数。
2. **followup 引用闭合**：先收集本批全部待入条目 id（允许两两互指），`followup_links[*].procedure_id` 不指向其中任何一个 → **丢该 link 留条目** + warning `followup_link_broken`。
3. **source_refs 空 → 丢弃条目** + warning `mechanic_dropped_no_source`（"不编造"的唯一整条丢弃理由之一）。
4. **降档保留**：procedure step / followup condition 结构化不了 → A1 的 untagged `Other` 变体自然兜底（无需本层动作）；整条 `from_value::<MechanicEntry>` Err → 剥非法子结构后以语义条目重试（保 id/name/description/when_to_use/source_refs 五键重建），仍 Err（连 id 都没有）→ 丢弃 + warning `mechanic_uncoercible`。
5. **hooks 词表校验**：逐个 `from_value::<EngineHook>`（词表单一事实源就是该 enum——A1 测试 `engine_hook_serde_tags` 押"未知 event → Err"），Err → 剥该 hook + warning `hook_downgraded_no_engine_event`；剥完后条目照常保留（expressiveness_tier 按数据特征自然降档，无需存字段）。

### ③ 测试清单（测试即规格）

- `bad_tested_parameter_dropped_and_reported` ——（验收 3）注入 `tested_parameter:"nonexistent_xyz"` 的条目 → 不入目录；messages 含 code=`mechanic_dropped_unknown_parameter` 且 target=该条目 id。
- `tested_parameter_skills_prefix_and_case` —— `"Jump"`/`"skills.jump"` 均命中键集合里的 `"jump"`（fields 或 extra 来源各测一次）。
- `none_tested_parameter_is_legal` —— 纯 hook 条目（无 tested_parameter）原样保留、零 message。
- `broken_followup_link_dropped_link_kept_entry` —— followup 指向不存在 id → 条目在、该 link 没了、code=`followup_link_broken`。
- `intra_batch_followup_resolves` —— A→B、B→A 同批互指 → 两条全保留、links 完好（先收集后校验的顺序押此测）。
- `no_source_refs_dropped` —— code=`mechanic_dropped_no_source`；同批其余合格条目不受牵连。
- `unknown_hook_stripped_entry_downgraded` —— `hooks:[{event:"galactic_alignment"}]` 且 procedure 空 → 条目保留、hooks 空、code=`hook_downgraded_no_engine_event`、`expressiveness_tier()==Semantic`。
- `weird_procedure_step_preserved_as_other` —— `{"step":"weird_custom","foo":1}` 入条目后 round-trip 原 JSON 一字不丢（降档保留，理念护栏 §3.5）。
- `uncoercible_salvaged_as_semantic_or_reported` —— 有 id/source 但 procedure 是字符串（类型错）→ 降为语义条目保留；连 id 都缺 → 丢弃 + `mechanic_uncoercible`。
- `duplicate_ids_last_wins` —— 同 id（大小写变体）重复提交 → 仅后者入目录（与 A2 merge-by-id 语义一致）。
- （mechanics_compile 集成半边）`validation_messages_written_to_kernel_report` —— ReplayClient submit 坏条目 → `kernel.validation_report.warnings` 含对应 code（写回接线生效的证据）。

### ④ 实现要点

- 全部纯函数（无 async/IO），id 规范化 = trim + to_ascii_lowercase 后判重。
- 五条护栏的执行序有讲究：先 uncoercible 救援（拿到结构化条目）→ source_refs → tested_parameter → hooks → followup 闭合（闭合必须在丢弃完成后做，否则"指向一个即将被丢的条目"会误判为通）。
- ValidationMessage.message 带可定位细节（被丢的键名、断链目标 id、剥掉的 event 字符串），便于 A7 审计直读。
- `sheet_parameter_keys` 解析 character_sheet_schema 用 Value 路径取数（`.get("fields")`/`.get("derived_values")` + as_array），不反序列化 CharacterTemplate——schema 可能是 fallback 形状 `{"title":…}`（grounded: rule_kernel_from_run_kit L1697 unwrap_or_else 分支），取不到就空集，护栏自然把带 tested_parameter 的条目全降——此时 messages 会堆 `mechanic_dropped_unknown_parameter`，A7 审计可见（fail-closed 不静默）。
- 本文件零 LLM 依赖，proto/A7 可对任意 dump 的 raw 数组离线重放护栏。

### ⑤ 验证

```bash
cd crates/trpg-rule-agent && cargo test -p trpg-rule-agent
wc -l crates/trpg-rule-agent/src/reader/mechanics_finalize.rs   # ≤400
```

---

## A4. 配套升级同遍：thresholds followup + success_bands 语义化补全 + fields.notes

机制目录不是孤岛——kernel 既有区要同遍升级才能成链：阈值挂 followup（SAN loss_in_one_go → 临时疯狂）、band 长语义（fumble 在剧情/机制上意味着什么）、字段补 notes。本任务扩 A2 的 prompt/schema 与 A3 的写回，全部 merge **绝不覆盖既有字段**——既有 kernel 是一期解析的事实源，本遍只做加法。历史 `_fix_coc_success_bands.sql` 手修在此普及为编译护栏（验收 12①）。

### ① Files

- **Modify** `crates/trpg-rule-agent/src/reader/mechanics_compile.rs` —— ①`MECH_SYS` 增配套升级指引（见②末）；②`submit_mechanics` schema 的 properties 增三个**可选**数组键 `thresholds_upgrades`/`success_bands_upgrades`/`field_notes`（required 仍仅 `["mechanics_catalog"]`）；③`run_mech_loop` 的 submit 返回值从 `Vec<Value>` 改为四元组/小结构体把三数组一并带回；④`compile_mechanics_catalog` 在 finalize_catalog 之后按序调三个 apply_* + round-trip 护栏。
- **Modify** `crates/trpg-rule-agent/src/reader/mechanics_finalize.rs` —— 三个 merge 写回纯函数 + `kernel_round_trip_guard`。
- **Modify** `crates/trpg-db/src/lib.rs` —— 仅测试：`dice_core_override_tests` mod（L3954）追加 override-merge 与新字段兼容的回归测试。

### ② 接口契约

```rust
// mechanics_finalize.rs 追加（全纯函数；messages 同 A3 汇入 kernel.validation_report.warnings）：

/// thresholds_upgrades 元素形状：{track_id, at?|loss_in_one_go?, direction?, followup_procedure_id}。
/// 按 track id（大小写不敏感）+ 阈值形状匹配 kernel.resource_tracks 内既有 threshold 对象
/// （形状匹配 = at 数值相等且 direction 相等（双方缺省视为相等），或 loss_in_one_go 数值相等），
/// 命中后**只插入缺失的 `followup_procedure_id` 键**——绝不覆盖既有任何键、绝不新建阈值
/// （阈值本体是一期解析的事实源）。followup_procedure_id 必须指向 catalog 内真实条目
/// （断引 → 跳过 + warning `threshold_followup_broken`）；匹配不到阈值 → 跳过 + warning
/// `threshold_upgrade_unmatched`。
pub fn apply_thresholds_upgrades(
    kernel: &mut RuleKernel,
    upgrades: &[serde_json::Value],
    catalog: &[MechanicEntry],
) -> Vec<ValidationMessage>;

/// success_bands_upgrades 元素形状：{id, semantics?, mechanical_effects?, …}。
/// 按 band id（大小写不敏感）merge 进 kernel.dice_core.success_bands：
/// - 既有 band：只补缺失键（semantics/mechanical_effects），绝不覆盖既有键；
/// - 新 band id：整对象原样追加 + info `success_band_completed`（band 补全审计：
///   缺 critical/failure 的历史手修普及为编译护栏，验收 12① 的 report 证据就是这条）。
/// dice_core 非对象 / success_bands 缺失 → 仅当 upgrades 非空时整建 success_bands 数组
/// （全部按"新 band 追加"处理）；upgrades 空 → no-op。
pub fn apply_success_bands_upgrades(
    kernel: &mut RuleKernel,
    upgrades: &[serde_json::Value],
) -> Vec<ValidationMessage>;

/// field_notes 元素形状：{field_id, notes}。character_sheet_schema.fields[*] 按
/// field_id 匹配（大小写不敏感），仅当该 field 的 notes 缺失/空串时写入；
/// 不新建 field（field_id 匹配不到 → 跳过 + warning `field_note_unmatched`）。
pub fn apply_field_notes(
    kernel: &mut RuleKernel,
    notes: &[serde_json::Value],
) -> Vec<ValidationMessage>;

/// round-trip 护栏：写回全部完成后整 kernel `to_value` → `from_value::<RuleKernel>`
/// 必须成功，且 normalize_resource_tracks（trpg-model L1449，只按 id/name 过滤、
/// 不动 track 内键）后新字段仍在。Err → 调用方放弃本遍全部写回（恢复进遍前
/// clone 的 kernel，fail-closed 整体性），错误描述进 gap notes。
pub fn kernel_round_trip_guard(kernel: &RuleKernel) -> Result<(), String>;
```

`MECH_SYS` 配套升级指引要点（追加进 A2 prompt）：seed 已给既有 thresholds/bands/fields 摘要——①阈值有后续程序的提交 `thresholds_upgrades` 把它链到目录条目；②每个 band 补 `semantics`（该成功度在剧情/机制上的意味）；③书中存在但 kernel 缺的成功度等级（critical/fumble/failure 类）提交完整 band 对象补全（id/label/semantics + 判定边界字段与既有 band 同形状）；④fields 缺 notes 的补"是什么/怎么用"（含 sanity/luck/mp 类派生值）；全部必须读页有据。

### ③ 测试清单（测试即规格）

- `thresholds_upgrade_adds_followup_without_clobbering` —— track sanity 既有 `{loss_in_one_go:5, consequence:"…"}` + upgrade `{track_id:"Sanity", loss_in_one_go:5, followup_procedure_id:"x.temp_insanity"}`（catalog 含该 id）→ threshold 多出 followup_procedure_id；consequence/loss_in_one_go 原值逐字不变。
- `thresholds_upgrade_never_overwrites_existing_followup` —— threshold 已有 followup_procedure_id → upgrade 不改它 + warning。
- `thresholds_upgrade_broken_ref_skipped` —— followup 指向 catalog 外 id → 不写 + `threshold_followup_broken`。
- `thresholds_upgrade_no_match_no_create` —— track/形状匹配不到 → resource_tracks serde 字节不变 + `threshold_upgrade_unmatched`。
- `bands_upgrade_fills_semantics_not_overwrite` —— 既有 `{id:"extreme", label:"L"}` + upgrade `{id:"extreme", semantics:"S", label:"EVIL"}` → semantics=="S"、label 仍 "L"。
- `bands_upgrade_appends_missing_band_with_report` —— upgrade 带新 id "fumble" 完整对象 → 追加成功 + info code=`success_band_completed`。
- `field_notes_only_fill_empty` —— notes 已有的 field 不被覆盖；缺/空串的被补；不存在的 field_id 不新建 + `field_note_unmatched`。
- `kernel_round_trips_after_all_upgrades` —— 三类升级全做完后 `from_value::<RuleKernel>` Ok 且 normalize_resource_tracks 后 followup_procedure_id/semantics 仍在。
- `empty_upgrades_are_noop` —— 三数组全缺省 → kernel serde 字节不变、零 message（老规则集行为零变化）。
- （trpg-db，dice_core_override_tests mod 追加）`override_merge_keeps_upgraded_fields` —— ①base dice_core.success_bands 带 semantics、override 只设 "dice" 键 → semantics 保留（merge_dice_core L3934 shallow key 语义天然兼容）；②override 设 success_bands 键 → 整值替换（既有语义照旧，断言文档化"override 文件必须带全字段"）；③resource_tracks override：未被覆盖的 track 的 followup_procedure_id 保留、被覆盖 track 整体替换（merge_resource_tracks L3943 语义不变）。

### ④ 实现要点

- 调用序（compile_mechanics_catalog 内）：进遍前 `let backup = kernel.clone()` → finalize_catalog 落目录 → apply_thresholds_upgrades（要 catalog 定稿后才能校验引用）→ apply_success_bands_upgrades → apply_field_notes → kernel_round_trip_guard，Err → `*kernel = backup` + gap note（一票否决，绝不半写）。
- band 完整性的**确定性检测**克制处理：执行时先 dump 真库 CoC band JSON 核字段形状；仅当 band 对象含可判失败侧的结构化字段时才做"缺失败侧"warning（code `success_bands_incomplete`）；形状无法判定 → 不报不动（fail-closed 不猜，语义优先于字符匹配——禁止按 band label 关键词判）。完整性的主保障 = prompt 指令（语义层）+ A7 跑批断言（验收 12① CoC 必含 critical/failure）。
- field_notes 写回经 Value 路径（`character_sheet_schema["fields"][i]["notes"]`），与 A3 sheet_parameter_keys 同款取数方式，fallback 形状 schema 自然 no-op。
- 三个 apply_* 与 guard 全是纯函数（&mut kernel 进出，无 IO），测试不需要 LLM/DB；trpg-db 那条回归测试也纯（merge_* 是纯函数，无需 DATABASE_URL）。

### ⑤ 验证

```bash
cd crates/trpg-rule-agent && cargo test -p trpg-rule-agent
cd ../trpg-db && cargo test -p trpg-db dice_core_override_tests
wc -l crates/trpg-rule-agent/src/reader/mechanics_finalize.rs   # ≤400（A3+A4 共文件，超则按先例拆测试）
```

---

## A5. on_outcome 结构化 band 触发 + amount max_of（trpg-mechanics 读取点）

A4 定下的 JSON 新形态要有人消费：trigger 的结构化 band 形态与 amount 的 `max_of:` 迷你语言在 trpg-mechanics 结算读取点落地。纪律：**既有字符串词表行为零变化**（always/on_success/on_failure/on_tier 一字节不动），新形态是并存加法；认不出的 trigger/amount → 该 rule 跳过（fail-closed，老 kernel 行为不变 = 验收 12② 后半）。只依赖 A4 定的 JSON 形状，可与 A6 并行。

### ① Files

- **Modify** `crates/trpg-mechanics/src/lib.rs`（存量 1334 行，只做局部小改不强拆）：
  1. `apply_outcome_resource_tracks` trigger 匹配分支（L98 `match rule.get("trigger").and_then(|v| v.as_str()).unwrap_or("always")` 起的 L98-115 区）——升级双形态；
  2. `resolve_track_amount`（L1115-1157）的 `read_expr` 闭包——`is_plain_dice` 分支之前插 `max_of:` 形态；
  3. 新纯函数 `band_trigger_id` / `dice_max` 落在 `roll_amount_dice`（L1035）/`is_plain_dice`（L1050）旁；
  4. 单测追加进文件既有 `#[cfg(test)]` mod。

### ② 接口契约

```rust
/// trigger 双形态解析（纯函数）：
/// - 对象 {kind:"band", band_id:"fumble"} → Some("fumble")
/// - 字符串 / 其他对象 / 认不出 → None
/// （字符串词表不经此函数——调用处先 as_str() 走既有分支，行为零变化。）
fn band_trigger_id(trigger: &serde_json::Value) -> Option<String>;

/// 骰式最大值（纯函数，与 roll_amount_dice 同一 NdM±K 解析规则，确定性无随机）：
/// "1d10"→10、"2d6"→12、"2d6+3"→15、"2d6-1"→11、"d8"→8、纯整数 "7"→7（退化合法）；
/// 非纯骰式（命名 token / 公式串）→ None。
fn dice_max(expr: &str) -> Option<i32>;
```

调用点改动契约（两处，各一段）：

1. **trigger gate**（L98-115）：现 `.as_str().unwrap_or("always")` 拆为三层——
   - `trigger` 键**缺失** → 维持既有 `"always"` 默认（缺失 ≠ 认不出，老 kernel 大量裸 rule 靠它活）；
   - `as_str()` Some → 既有四词表分支原样（含 on_tier 的 tier/min_rank 对账 L104-113，零改动）；
   - 字符串不是 → `band_trigger_id(trigger)`：Some(band_id) → 对账 `result.outcome` 的 band 字段——band_id 与 `success_tier` 同词汇表（grounded: on_tier 既对账 `outcome.success_tier`/`success_tier_rank` L106-109，结算器写 outcome 的就是这套键），比较 `success_tier == band_id` 大小写不敏感，不匹配 → `continue`；
   - `band_trigger_id` 也是 None（认不出的对象/数组/数字）→ `continue`（**跳过该 rule**，绝不回落 "always"——认不出还触发是错误放大）。
   - 执行前置核实：先 SQL 查真实 outcome JSON 形状定字段名（`select outcome from check_results order by created_at desc limit 5`）；若真实字段名与 success_tier 不同，以实际为准并在代码注释记录核实结果——但**只读结算器同源的那一个字段**，不做多候选字段扫描（词表单一事实源，护栏 §3.5）。
2. **amount 迷你语言**（resolve_track_amount 的 read_expr 闭包 L1124-1139）：`=value`/`=<field>` 分支之后、`is_plain_dice` 之前插——`e.strip_prefix("max_of:")` Some(rest) → `dice_max(rest.trim())`；None 结果沿既有 None 语义（track_is_tested 时落 default_amount 兜底、否则跳过该 rule）。文档注释同步更新（L1103-1113 的迷你语言清单加一行 `max_of:<dice>`）。

### ③ 测试清单（测试即规格）

- `string_triggers_behave_unchanged` —— always/on_success/on_failure/on_tier 既有测试照绿；若现文件缺直测，补 on_success 正反两断言（success=true 触发 / false 不触发）作回归锚。
- `missing_trigger_defaults_to_always` —— rule 无 trigger 键 → 照常触发（老 kernel 裸 rule 行为锁死）。
- `band_trigger_matches_outcome_band` —— trigger=`{kind:"band",band_id:"fumble"}`、outcome.success_tier="fumble" → rule 触发（资源被改）；success_tier="regular" → 不触发。
- `band_trigger_case_insensitive` —— band_id="Fumble" vs outcome "fumble" 命中。
- `unrecognized_trigger_object_skips_rule` —— trigger=`{kind:"phase_of_moon"}` / trigger=`42` → 该 rule 跳过、同 track 其余 rule 照常、不 panic（验收 12② 后半：未解析出 band 触发的规则集行为不变）。
- `dice_max_forms` —— "1d10"→10、"2d6"→12、"2d6+3"→15、"2d6-1"→11、"d8"→8、"7"→7、"max(0,x)"→None、""→None。
- `amount_max_of_resolves_max` —— amount="max_of:1d10" → 恰 10（确定性断言，无随机容差——fumble 掉最大值由 prose 变机械事实的最小证据）。
- `amount_max_of_garbage_falls_back` —— amount="max_of:garbage" + track_is_tested=true + default_amount="1d6" → 走 default_amount 兜底；track_is_tested=false → None（rule 跳过）。

### ④ 实现要点

- `band_trigger_id` 只认 `{"kind":"band","band_id":<str>}` 精确形状（kind 必须是 "band"），多余键容忍、缺 band_id → None。
- `dice_max` 与 `roll_amount_dice` 共享 NdM±K 解析认知但实现独立小函数（≈10 行：N×M±K；N/M clamp 同 roll_amount_dice 的 1..=100/1..=1000）——复制十行换可读性，不强拧 DRY。
- trigger 三层判定写成一个小辅助（如 `enum TriggerGate { Fire, Skip }`）或就地 if-let 链均可，以既有函数风格为准；**不动** check_match/op/seeds/thresholds 等周边逻辑半个字节。
- cwd 坑：`cargo test -p trpg-mechanics` 必须在 crate 目录跑，workspace 根会撞别的 crate（memory 既有坑）。

### ⑤ 验证

```bash
cd crates/trpg-mechanics && cargo test -p trpg-mechanics
```

---

## A6. parse 管线接线 + mechanics_proto bin

把第三遍接进真实 parse 管线（**两条路径都要**：CLI `parse-all` 走非 staged 的 `parse_rulebook`，`parse-staged`/API ingest 走 staged 的 stage2——缺一条 A7 跑批就瘸），门 `TRPG_MECHANICS_COMPILE` 默认开、失败 fail-closed 原 kernel 照常 upsert。另建 mechanics_proto bin 供 A7 跑批/审计直打 units（对标 module_reader_proto 的"无 DB/engine"定位）。

### ① Files

- **Modify** `crates/trpg-parser/src/lib.rs`：
  1. 新 `build_mechanics_compiler_llm()`（紧挨 `build_compiler_llm` L1541-1548，同款样板）；
  2. 新 `compile_mechanics_into_kernel(...)` 门控 + fail-closed 包装（单一接线原语，两路径共用）；
  3. 非 staged 接线：`parse_rulebook` 的 kernel 产出点（L703-707）——`rule_kernel` 构造后、`write_rule_kernel_artifact`（L707）与 `rule_kernel_context_block`（L708）**之前**调②（先编译再投块/写 artifact，目录随 kernel 整体入库）；
  4. `persist_stage2_kernel`（L1816）签名增三参（units/sidecar_text/fallback llm），内部 kernel 构造（L1830）后、`upsert_rule_kernel`（L1834）前调②。
- **Modify** `crates/trpg-parser/src/staged.rs` —— `stage2_deep` 的 `persist_stage2_kernel` 调用点（L172）同步传新参：`&self.units`、`self.sidecar_text.clone()`、`&self.llm`。
- **Create** `crates/trpg-rule-agent/src/bin/mechanics_proto.rs` —— src/bin 自动发现（已核实 Cargo.toml 无 [[bin]] 条目，无需注册）。

### ② 接口契约

```rust
// crates/trpg-parser/src/lib.rs

/// 机制目录编译遍专属客户端：env TRPG_MECHANICS_COMPILE_MODEL 默认 "gpt-5.4"
/// （沿用模组抽取拍板：后台跑、质量优先；不用 -fast 变体——更贵；不用 codex-spark
/// ——经 relay 空 tool 参数不可用）。None → 调用方回退 fallback 主客户端。
pub(crate) fn build_mechanics_compiler_llm() -> Option<Arc<dyn LlmClient>>;

/// 门控读取（纯函数，可单测）：TRPG_MECHANICS_COMPILE 未设/非 "0"/"false" → true（默认开）。
pub(crate) fn mechanics_compile_enabled() -> bool;

/// 门控 + fail-closed 包装（两路径共用的唯一接线原语）：
/// 门关 → 直接返回；客户端构造失败 → fallback；进遍前 clone kernel，
/// compile_mechanics_catalog 内部已带 round-trip 回滚（A4），本层兜底任何 panic 之外的
/// Err/空产出 → kernel 保持进入前状态；gaps 经 tracing::info 落日志（不中断 parse）。
pub(crate) async fn compile_mechanics_into_kernel(
    fallback: &Arc<dyn LlmClient>,
    kernel: &mut RuleKernel,
    units: &[reader::Unit],
    sidecar_text: Option<String>,
    skill_names: Vec<String>,
);

// persist_stage2_kernel 签名升级（唯一调用方 staged.rs L172 同步改）：
pub(crate) async fn persist_stage2_kernel(
    db: &Db, ruleset_id: &str, title: &str, rg: Option<&reader::ResolutionGm>,
    template: &CharacterTemplate, option_catalogs: &Value, object_stubs: Vec<Value>,
    units: &[reader::Unit], sidecar_text: Option<String>, fallback_llm: &Arc<dyn LlmClient>,
) -> Result<()>;
```

mechanics_proto bin（对标 module_reader_proto 样板：位置参数 + env LLM + tracing subscriber + 摘要打印 + 计时）：

```
Usage: mechanics_proto <units.jsonl> <kernel.json> [sidecar.md] [budget] [--out <path>]
  kernel.json = data/parsed/rules/{ruleset}.rule_kernel.json artifact（grounded:
                write_rule_kernel_artifact L1031-1036）或真库 content_json dump
Env: TRPG_LLM_BASE_URL / TRPG_LLM_API_KEY + TRPG_MECHANICS_COMPILE_MODEL（默认 gpt-5.4，
     与生产同模型审计才有意义）
打印：目录条目数、按 kind 与 expressiveness_tier 的分布、validation_report.warnings 全文、
     gap notes、整遍 wall-clock；--out 写升级后 kernel JSON（不连 DB——真库落库走 parse 重抽）
```

### ③ 测试清单（接线任务以构建+冒烟为主，mock 价值低）

- `mechanics_gate_env_reads_three_states` ——（lib.rs 单测）`mechanics_compile_enabled`：未设→true、"0"/"false"→false、"1"→true（env 测试串行注意，用既有 env 测试的加锁/串行先例，无则 `std::env` 设完即恢复）。
- 既有 trpg-parser 全部测试照绿（接线不破坏既有路径——尤其 chargen/object 接线周边）。
- proto 冒烟（手动，见⑤）：CoC 单跑出非空目录摘要。

### ④ 实现要点

- **units/sidecar 来源照抄 chargen 接线**：非 staged 路径 L540-553 的 `load_units(parsed/source_units/{source_id}.semantic_units.jsonl)` + 直读 `markdown/rulebooks/{source_id}.md`（**sidecar 坑已在该处修过**——`layout_sidecar_path` metadata 恒 None，绝不回去读 metadata；注释 L542-550 是修法原文）；staged 路径 `self.units`/`self.sidecar_text` 现成（已是修后来源）。两处把已加载的 units/sidecar **复用变量**传入，不二次读盘。
- skill_names：非 staged 用 L557-566 同款 option_catalogs 提取（与 chargen 共用已算好的 `skill_names` 变量）；staged 用 `crate::skill_ids(template, option_catalogs)`（L1713）。
- budget 缺省 14（对标 chargen 接线 L571 的 14；proto 缺省同）。
- 门控样板对标 `TRPG_MODULE_READER`（L1651-1656）但**默认值反转**（本门默认开）；门关/失败时 tracing 必留一行（缺口可观测，不静默）。
- proto 构造客户端：`LlmConfig::from_env()` 后 model 改取 `TRPG_MECHANICS_COMPILE_MODEL`（缺省 gpt-5.4）；tracing subscriber/计时照抄 module_reader_proto L33-35/L59-61；kernel 读取 `serde_json::from_str::<RuleKernel>`（A1 的 serde(default) 保证老 artifact 也能进）。
- 接线后 `rule_kernel_context_block`/BP1 投影的体积问题是 **B1 的职责**（B1 在该投影处剔除 mechanics_catalog 全文）——A6 不动 runtime；Slice A 先行落库是安全的（kernel JSON 变大但 BP1 瘦身前不要在真库长跑 GM 回合，A7 只做 parse/审计不跑回合）。

### ⑤ 验证

```bash
cd crates/trpg-parser && cargo build -p trpg-parser && cargo test -p trpg-parser
cd ../trpg-rule-agent && cargo build -p trpg-rule-agent
wc -l crates/trpg-rule-agent/src/bin/mechanics_proto.rs   # ≤400
# 冒烟（真 LLM 经 relay :18888；macOS 无 timeout 命令，用 Bash 工具超时）：
cargo run -p trpg-rule-agent --bin mechanics_proto -- \
  "$TRPG_DATA_DIR/parsed/source_units/<coc_source_id>.semantic_units.jsonl" \
  "$TRPG_DATA_DIR/parsed/rules/call_of_cthulhu_7e.rule_kernel.json" \
  "$TRPG_DATA_DIR/markdown/rulebooks/<coc_source_id>.md" 14
# 预期：目录条目数 > 0、warnings 可读、无 panic、计时打印
```

---

## A7. 六规则跑批 + 附录 A 覆盖率审计（Slice A 验收闸门）

Slice A 的收口闸门：不是写新代码，而是用真库真 LLM 把 A1-A6 的产物在六套规则（CoC/Cyberpunk/Triangle/ORC/D&D中文/剑世界中文）上跑实，对照 spec 附录 A 的 137 条勘查基线做覆盖率审计，断言全勾才放行 Section B。发现的系统性编译缺陷一律回改 A2 prompt 重跑——**绝不写 per-ruleset 补丁**。

### ① Files

- 不新增生产代码。可选小扩展：`crates/trpg-rule-agent/src/bin/mechanics_proto.rs` 增 `--audit` 输出模式（目录 dump 成审计友好 JSON：每条 id/name/kind/tier/when_to_use 一行）与 `--missing <json>`（把审计 subagent 产出的缺失清单以 code=`catalog_coverage_gap` 写进 `--out` kernel 的 validation_report.warnings——"缺失记 validation_report"的机器可见半边；最低要求是缺失清单落审计报告）。
- **Create** `docs/机制目录跑批审计_2026-06-XX.md` —— 中文审计报告（验收产物，日期按实跑日）。

### ② 跑批与断言契约（本任务无新 Rust 接口；以下断言矩阵即"契约"）

环境与跑法：
- 库的枚举：两个端口都查（**.env DATABASE_URL 端口坑**：:54346 赛博库 / :54347 CoC 等库）——`docker exec chatrpg-postgres-rulesets psql -U <user> -d <db> -c "select ruleset_id from rule_kernels where active=true"` 确认六套各在哪个库（psql 未本机安装，一律 docker exec）。
- 每套先 mechanics_proto 直打 units（快速迭代：units.jsonl + kernel artifact + markdown 三件套，`TRPG_DATA_DIR` 指对 data 目录）；**至少 CoC 走一次完整 parse 重抽**验证 A6 接线全链（规则书 cached 时按既有 e2e 流程强制重抽），SQL 确认库内 content_json 真含 mechanics_catalog。

断言矩阵（每条在报告里给证据：SQL 输出 / proto 摘要 / validation_report 摘录）：
1. **验收 1**：六套 mechanics_catalog 非空生成；validation_report 记录每次丢弃及原因（warnings 的 A3/A4 code 词表可读）。
2. **验收 2（CoC）**：含 sanity_check 类与 temporary_insanity 类条目（**语义判定**——按 when_to_use/description 认定，不按 id 字面写死）；followup 链通：sanity track 的 `thresholds[loss_in_one_go].followup_procedure_id` 指向临时疯狂条目且该 id 在目录内；含跳跃语义条目带非空 when_to_use。
3. **验收 3a 覆盖率审计**：审计 subagent（sonnet，沿用审计 subagent 先例）输入 = spec 附录 A 137 条（§A1-A11）+ 该套目录 dump（proto --audit 输出），逐套产出"书中存在但目录遗漏"清单 → 落审计报告（+ 可选 --missing 写回 validation_report）；六套各抽查 **≥1 非预设类别机制**（kind=Other 或语义新类，如荣誉/令咒/堕落类——具体条目以审计为准不写死）、**≥1 纯语义条目**（procedure 空 hooks 空）、**≥1 EngineHook 条目**被收录。
4. **验收 12①**：CoC dice_core.success_bands 含 critical 与 failure 侧 band；validation_report 含补全动作证据（`success_band_completed`）。
5. **工程三查**：新文件 `wc -l` 全 ≤400；`grep -rn -i "cthulhu\|cyberpunk\|triangle\|sword.world\|剑世界\|龙与地下城" crates/trpg-model/src/mechanics.rs crates/trpg-rule-agent/src/reader/mechanics_*.rs crates/trpg-rule-agent/src/bin/mechanics_proto.rs` 命中仅限注释/prompt 举例文案、零逻辑分支；新结构 serde(default)（A1 回归测试在 CI 即证据）。

### ③ 测试清单（本任务的"测试"= 上述断言矩阵 + 收口 SQL）

```sql
-- 落库证据（CoC parse 重抽后）：
select ruleset_id, jsonb_array_length(content_json->'mechanics_catalog') as n_mechanics
from rule_kernels where active=true order by ruleset_id;
-- followup 链证据：
select t->'thresholds' from rule_kernels,
  jsonb_array_elements(content_json->'resource_tracks') t
where ruleset_id='call_of_cthulhu_7e' and active=true;
```

### ④ 跑批操作要点

- macOS 无 `timeout` 命令——长跑用 Bash 工具的 timeout 参数；编译模型 gpt-5.4 走 relay :18888（codex-relay 拒收 temperature，proto/管线的既有客户端配置已处理）。
- 六套**串行**跑（防 relay 压力 + 本工作区并发 worker 碰撞前科）；每套预算 budget=14、预估 2-5 分钟/套（对标 chargen/骨架遍时长，gpt-5.4 后台质量优先）。
- 六套的 units/kernel/markdown 三件套路径先盘点齐再开跑（D&D中文/剑世界中文的 source_id 与 ruleset_id 不一定同名，从 rule_kernels/source 表反查）。
- 迭代纪律：审计揪出某档表达力整体漏抽（如 passive_projection 全空、hooks 全空）→ 回改 A2 的 MECH_SYS 指引重跑该套 + 复跑 CoC 防回归；个别条目遗漏属正常（LLM 非确定，模组骨架边数飘移先例），记报告不强求 137/137。
- 报告结构建议：每套一节（条目数/分布/三抽查/缺失清单）+ 跨套结论（零硬编码核查、prompt 迭代记录、遗留缺口清单——供 C7 总验收引用）。

### ⑤ 验证

```bash
# 闸门 = 审计报告落 docs/ 且断言矩阵 1-5 全勾；最低硬证据：
docker exec chatrpg-postgres-rulesets psql -U <user> -d <db> -c \
  "select ruleset_id, jsonb_array_length(content_json->'mechanics_catalog') from rule_kernels where active=true"
# 预期：六套全部 > 0；CoC 含 sanity/temporary_insanity 语义条目与 critical/failure band（报告附录贴 SQL 输出）
```

---

## Section B —— 运行时（目录注入 + watcher + 机械债务）任务正文

> 本节展开骨架 `2026-06-10-rule-aware-gm.md` 的 B1–B8。骨架「共享类型契约」§1/§2/§4/§5/§6 **逐字生效**（本节不重复粘贴，仅引用并补本节新增的接口）。依赖关系：B1/B2/B3 互相独立、仅依赖 A1 的 trpg-model 类型（可并行起跑，但 **B2→B3→B4 在 `tools/check.rs` 上有叠改，必须按序合入**，本工作区无 git、并发 worker 直接覆盖会互相冲掉——前车之鉴见 memory 并发碰撞坑）；B5 依赖 B4 的 watcher.rs 与 db 原语；B6 依赖 B4/B5；B7 独立；B8 收口。全节公共约束：新文件 `wc -l` ≤400、零 per-ruleset 硬编码（grep 不得出现规则集名字面量）、新结构 `#[serde(default)]`、fail-closed 不编造、一期缓存稳定测试全程照绿。

---

## B1. BP1 目录索引投影 + kernel BP1 块瘦身（缓存稳定关键任务）

**对应验收**：13（单测半边）；spec §5.1 ①②④ + §3.5.3 分级 data-driven。

### Files

- Modify `crates/trpg-runtime/src/lib.rs` —— `rule_steward_prefix_blocks_for_turn`（L1432–1475；RuleStewardKernel 块构造在 L1440–1455）
- Modify `crates/trpg-model/src/lib.rs` —— `BlockKind` 枚举（L225 区）追加一个变体 `MechanicsCatalogIndex`
- Modify `crates/trpg-model/src/mechanics.rs` —— A1 已建模块，本任务追加 3 个纯函数（若文件逼近 400 行，渲染函数拆 `crates/trpg-model/src/mechanics_render.rs` + `pub use`）

### 接口契约

```rust
// trpg-model/src/mechanics.rs（纯函数，无 IO）

/// BP1 投影视图：clone kernel 并清空 mechanics_catalog（其余字段逐字节保留）。
/// grounded: runtime L1445 `BlockContent::Json(serde_json::to_value(&kernel)?)` 把
/// kernel 整体灌 BP1——不清区则 CoC 级目录（50–137 条全文）直接顶爆
/// validate_compiled_budget（prompts.rs L98，fail-closed Err）。
pub fn kernel_bp1_view(kernel: &RuleKernel) -> RuleKernel;

/// 紧凑索引文本：每条一行 `id | name | when_to_use`，顺序=目录顺序（确定性）。
/// entries.len() > limit 时分级（data-driven，按条目数与数据特征，绝不按规则集名）：
/// 仅保留 kind==SubsystemProcedure 或 hooks 非空 或 passive_projection 非空的条目，
/// 尾部追加一行 `(+N entries omitted; use lookup_mechanic by id)`。
pub fn catalog_index_text(entries: &[MechanicEntry], limit: usize) -> String;

/// PM 档（第四表达档）常驻投影行：以 entry.passive_projection 为模板做 `{value}`
/// 替换；值按 entry.tested_parameter 键在 sheet_json 的 stats/skills/resources/
/// tracks/field 桶里大小写不敏感查找（对齐 npc_synth 的 sheet 桶约定，
/// grounded: runtime L1571 `sheet_json.pointer("/{bucket}/{param}")`）。
/// 查不到值 / entry 无 passive_projection / 无 tested_parameter → None（fail-closed 跳行）。
pub fn passive_projection_line(entry: &MechanicEntry, sheet_json: &serde_json::Value) -> Option<String>;
```

```rust
// trpg-runtime/src/lib.rs（private，挂在 RuntimeEngine impl 内）

/// PM 行装配：viewer 卡经 RuntimeParameterService::load_actor_parameters
/// （grounded: runtime L1548–1551 同款用法）取 sheet_json，逐条调
/// passive_projection_line；actor 不存在 / 无 PM 条目 → 空 Vec。
async fn passive_lines_for_viewer(&self, session_id: &str, viewer_actor_id: &str, entries: &[MechanicEntry]) -> Vec<String>;
```

### 测试清单

1. `trpg-model::kernel_bp1_view_strips_catalog_only` —— 构造含 3 条 MechanicEntry + 非空 resource_tracks/dice_core 的 kernel；断言视图 mechanics_catalog 为空、且除该字段外 `serde_json::to_value` 与原 kernel 逐键相等。
2. `trpg-model::catalog_index_text_is_deterministic_one_line_per_entry` —— 同一 entries 两次渲染字节相同；行数 == entries.len()；每行含 `id | name | when_to_use` 三段。
3. `trpg-model::catalog_index_over_limit_keeps_procedures_hooks_and_pm` —— 构造 limit+4 条（混 SkillCheck 长尾与 SubsystemProcedure/带 hooks/带 passive_projection 各一）；断言超限后长尾 SkillCheck 的 id 不出现、三类保留条目出现、尾行含 `lookup_mechanic`。
4. `trpg-model::passive_projection_line_substitutes_value_or_skips` —— sheet `{"skills":{"credit_rating":55}}` + 模板 `"信用评级 {value}"` → `Some("信用评级 55")`；sheet 无该键 → None；entry 无 passive_projection → None。
5. `trpg-gm::prompts::cache_stability_tests` 全模块照绿（既有 `cross_turn_stable_segments_keep_bytes_with_growing_history` 即验收 13 单测半边的承载体：索引文本进 prefix 后，输入不变 ⇒ assemble 字节不变由 catalog_index_text 确定性 + assemble 既有性质合成保证，无需新测试改 prompts.rs）。

### 实现要点

- RuleStewardKernel 块（L1445）改 `BlockContent::Json(serde_json::to_value(&kernel_bp1_view(&kernel))?)`——**这是防爆预算的硬前置**，必须与索引块同一任务落地。
- 新索引块：`block_id = format!("rule_steward.mechanics_index.{}", request.ruleset_id)`、`BlockKind::MechanicsCatalogIndex`（新变体，serde 默认派生即可）、`BlockContent::Text(catalog_index_text(...) + PM 行)`、`Visibility::GmOnly`、`Stability::RarelyChanged`、`CacheZone::Prefix`、`Scope::ruleset(&request.ruleset_id)`、priority **116**（紧贴 kernel 块 118 之下、onboarding 104 之上）。`kernel.mechanics_catalog` 为空 → 不出块（老规则集零变化，fail-closed）。
- 分级阈值：`std::env::var("TRPG_MECHANICS_INDEX_BP1_LIMIT")` 默认 **96**；解析失败用默认（对齐 L1412 既有 env 解析样板）。
- PM 行拼在**同一索引块尾部**（spec §5.1 ④ / 骨架拍板）：viewer_actor_id 取 `request.viewer.actor_id.as_deref().unwrap_or("pc.current")`（对齐 turn_loop.rs L34 同款取法）。缓存语义说明：PM 行含当前值 ⇒ 值变化（成长/效果）时该块字节合法变化（RarelyChanged 级失效，等同 kernel 重抽）；值未变的常规回合字节稳定——验收 13 测的是"场景不变时跨回合 hash 不变"，PM 值未变即满足。
- `rule_steward_prefix_blocks_for_turn` 签名不变（`&self, request`），PM 装配在其内部 await `passive_lines_for_viewer`；db 失败/卡不存在 → 空 Vec 不出 PM 行（绝不让 BP1 组装因 PM 投影挂掉）。
- 超预算行为不动：prefix 超 `TokenBudget.prefix_max` 仍由一期 `validate_compiled_budget` fail-closed Err（配置错误，不静默裁剪——spec §9 风险 2）。

### 验证

```
cd crates/trpg-model && cargo test
cd crates/trpg-runtime && cargo test
cd crates/trpg-gm && cargo test
wc -l crates/trpg-model/src/mechanics.rs   # ≤400（超则按 Files 注拆 render 文件）
```

---

## B2. lookup_mechanic 工具 + roll_check(mechanic_id) 继承

**对应验收**：6（mechanic_id 继承半边）；spec §5.1 ③⑤ + §3.5.1 结构化 id 绑定。

### Files

- Create `crates/trpg-gm/src/tools/mechanic.rs` —— 本任务只放 `LookupMechanicTool` + 纯函数（`WaiveObligationTool` 由 B6 在同文件追加；两任务合计 ≤400 行）
- Modify `crates/trpg-gm/src/tools/check.rs` —— `RollCheckArgs`（L23–31）、`parse_roll_check_args`（L47–54）、`RollCheckTool::spec/call`（L143–193）
- Modify `crates/trpg-gm/src/tools/mod.rs` —— `pub mod mechanic;`、`ToolRegistry::standard()`（L133–146）尾部追加、错误码注释区（L70–82）登记 `mechanic_not_found` / `scene_mechanic_not_found`
- Modify `crates/trpg-gm/src/tools/world.rs` —— `registry_has_ten_tools_in_stable_order` 测试（L112–117）更新
- Modify `crates/trpg-gm/src/lib.rs` —— re-export 不强制（tools 模块整体已 pub）

### 接口契约

```rust
// tools/mechanic.rs
pub struct LookupMechanicTool;
// spec(): name="lookup_mechanic"，schema 用骨架契约 §6 第一段逐字。
// call(): ctx.engine.db.load_rule_kernel(&ctx.request.ruleset_id) →
//   find_mechanic(&kernel.mechanics_catalog, &id) →
//   命中：ToolOutput::ok(json!({"entry": <MechanicEntry 全文 serde>, "tier": <expressiveness_tier 字符串>}))
//   miss / kernel 无目录：ToolError::recoverable("mechanic_not_found", …,
//     Some("Check the BP1 mechanics index for valid ids, or use retrieve_rules."))

/// 纯函数：目录查找，id trim + 大小写不敏感相等（id 相等比较是 §3.5.4 合法字面边界）。
pub fn find_mechanic<'a>(catalog: &'a [MechanicEntry], id: &str) -> Option<&'a MechanicEntry>;
```

```rust
// tools/check.rs —— RollCheckArgs 两个新字段 + tested_parameter 改为可缺省
#[derive(Debug, Clone, Deserialize)]
pub struct RollCheckArgs {
    pub check_label: String,
    #[serde(default)] pub tested_parameter: String,        // 一期 required → 二期：无 mechanic_id 时仍必填（运行时校验）
    pub actor_id: Option<String>,
    pub opposed: Option<OpposedArgs>,
    #[serde(default = "default_public")] pub visibility: String,
    pub intent_kind: Option<String>,
    #[serde(default)] pub mechanic_id: Option<String>,
    #[serde(default)] pub scene_mechanic_id: Option<String>, // 字段+schema 本任务落；消费逻辑归 C4
}

/// 纯函数：目录继承。显式 args 优先（目录是知识不是枷锁）：
/// - args.tested_parameter 为空且 entry.tested_parameter 为 Some → 写入 args；
/// - 返回 entry.procedure 首个 ProcedureStep::Roll 的 dice（Some 时调用方用它替代
///   kernel_dice 缺省；显式骰式本工具本无入参，不存在覆盖冲突）。
pub fn apply_mechanic_inheritance(args: &mut RollCheckArgs, entry: &MechanicEntry) -> Option<String>;
```

校验顺序（`RollCheckTool::call` 内，全部 recoverable）：
1. `mechanic_id` 给定 → 查目录，miss → `mechanic_not_found`；
2. 继承后 `tested_parameter` 仍空 → `invalid_arguments`（一期语义保持：检定必须绑参数）；
3. 契约构建后 `contract.advice_refs.push(format!("mechanic:{id}"))`（骨架契约 §6 注记：结构化引用走 advice_refs，**不动 CheckContract 5 处签名**；on_outcome 结构绑定经 tested_parameter + band trigger 落地，`check_match` regex 自动降级为无 mechanic_id 时的回退）。

schema 变更：`roll_check` 的 `parameters.properties` 增 `mechanic_id`/`scene_mechanic_id`（string）；`required` 从 `["check_label","tested_parameter"]` 改为 `["check_label"]`（tested_parameter 必填性移到运行时条件校验）。

### 测试清单

1. `lookup_finds_entry_case_insensitive` —— `find_mechanic(catalog, " COC.Sanity_Check ")` 命中 id=`coc.sanity_check` 条目。
2. `lookup_missing_id_is_mechanic_not_found` —— call 路径（lazy pool dummy_ctx 样板，对标 tools/mod.rs L207–213）对无目录 kernel 断言 ToolError code==`mechanic_not_found` 且 recoverable==true、hint 含 `retrieve_rules`。
3. `mechanic_inheritance_fills_tested_parameter_and_dice` —— entry{tested_parameter:Some("jump"), procedure:[Roll{dice:"1d100"}]} + args.tested_parameter 空 → 继承后 args.tested_parameter=="jump"、返回 Some("1d100")（验收 6 的继承绑定断言）。
4. `explicit_args_beat_catalog` —— args.tested_parameter="climb" + entry 绑 "jump" → 继承后仍 "climb"。
5. `roll_check_without_mechanic_still_requires_tested_parameter` —— 既有 `roll_args_require_tested_parameter`（check.rs L229）改写：无 mechanic_id 且 tested_parameter 空 → invalid_arguments（一期回归不丢）。
6. `registry_has_eleven_tools_in_stable_order`（world.rs 测试更名+更新）—— 前 10 个名字与顺序**逐字不变**，第 11 个是 `lookup_mechanic`（B6 再更 12）。
7. `schema_stability_tests::schema_serialization_is_stable` 照绿（确定性，非字节锁定，schema 变更不破）。

### 实现要点

- 注册表只在**尾部追加**（L146 `RememberTool` 之后），既有 10 个的 schema 字节与顺序绝不动——一期缓存稳定靠 schemas() 顺序确定性。
- `kernel_dice`（check.rs L129）保持缺省路径：继承 dice 为 Some 用之，None 仍走 `kernel_dice(ctx).await?`（`missing_kernel_dice` 错误语义不变）。
- 目录条目全文可能很大（procedure/followup/source_refs），lookup 结果直接回 JSON 不截断——这是 agent 主动拉取（按需付费），不进 BP1。
- `expressiveness_tier` 用 A1 纯函数，工具内不重算分类逻辑。

### 验证

```
cd crates/trpg-gm && cargo test
wc -l crates/trpg-gm/src/tools/mechanic.rs crates/trpg-gm/src/tools/check.rs
```

---

## B3. band 语义投影 + track 投影语义化（背景板防治三件套之"语义可见"）

**对应验收**：12③（语义投影单测半边）、10（BP 投影语义状态行的单测半边）；spec §5.1 ②⑤ + §3.5.6 三件套。

### Files

- Modify `crates/trpg-gm/src/tools/check.rs` —— `RollCheckTool::call` 结果 JSON（L184–192）增 `band_semantics`
- Modify `crates/trpg-mechanics/src/lib.rs` —— `mechanical_ledger_context_block`（L271–362）签名升级 + 语义行附着
- Modify `crates/trpg-runtime/src/lib.rs` —— 唯一调用点 L607 同步传 `&request.ruleset_id`
- Modify `crates/trpg-model/src/mechanics.rs` —— 追加 1 个纯函数（`track_semantic_line` A1 已建，本任务消费）

### 接口契约

```rust
// trpg-model/src/mechanics.rs
/// band 语义行：从 kernel dice_core.success_bands（Value 数组）按 outcome 的 band
/// 取 semantics 渲染 "{band_id}——{label}：{semantics}"。
/// outcome band 字段先读 "success_tier" 再读 "band"（grounded: mechanics lib.rs
/// L106 on_tier 对账 outcome.success_tier；trpg-agent 测试 fixture 用 "band" 键，
/// gm_loop.rs L558——两形态都真实存在）。band 无 semantics → None（fail-closed，
/// 调用方退回裸 band id+label）。
pub fn band_semantics_line(dice_core: &serde_json::Value, outcome: &serde_json::Value) -> Option<String>;
```

```rust
// trpg-mechanics/src/lib.rs —— 签名升级（唯一调用点在 runtime L607，破坏面=1 处）
pub async fn mechanical_ledger_context_block(
    &self,
    session_id: &str,
    ruleset_id: &str,          // 新增：scene/party/world 行无 actor 可查 ruleset，语义行需 kernel
    world_tick: i64,
) -> Result<ContextBlock>;
```

roll_check 结果 JSON 增量（check.rs L184 的 `json!` 增一键）：

```jsonc
"band_semantics": "extreme——极难成功：贯穿/卓越效果"   // band_semantics_line 产出；None 时整键缺省（fail-closed 裸 band 仍在 outcome 里）
```

### 测试清单

1. `trpg-model::band_semantics_line_renders_from_success_tier_or_band` —— dice_core 含 `success_bands:[{id:"extreme",label:"极难成功",semantics:"贯穿/卓越效果"}]`：outcome `{"success_tier":"extreme"}` 与 `{"band":"extreme"}` 均产出含三段的行；outcome 无 band 字段 → None。
2. `trpg-model::band_without_semantics_is_none` —— band 命中但条目无 semantics 键 → None（验收 12③ 的 fail-closed 半边）。
3. `trpg-mechanics::ledger_block_attaches_semantic_line_per_owner_kind` —— 合成 generic_parameter_states 投影输入：actor 行（`resources.sanity.current`=38，kernel sanity 轨带 thresholds）与 scene 行（`tracks.chaos_pool.current`=7，带 zero_means/thresholds）各一——断言两行 JSON 都带 `"semantic"` 键且文本含对应 consequence（**不得只投 actor 级**，Triangle chaos=scene 实证）；无 thresholds/zero_means 的行 → 无 `"semantic"` 键（裸数值）。注：投影查询本就不按 target_kind 过滤（lib.rs L317 `where session_id=$1`），party/world 行走同一路径，测试至少覆盖 actor+scene 两形态。
4. `trpg-gm::roll_check` 既有测试照绿；新增 `roll_check_result_carries_band_semantics_when_kernel_has_it`（可放纯函数级：构造 outcome+dice_core 调 band_semantics_line 断言渲染——工具级走真 db 的留 C5 e2e）。

### 实现要点

- `mechanical_ledger_context_block` 内：对每条 generic_state 行，`resolve_resource_track_id(&parameter_path, &kernel)`（trpg-model L1458，`resources.X.current`/`tracks.` 头都先剥——注意该函数对 `tracks.X.current` 形态会以 head=`tracks` 失配，**须先 strip "tracks." 前缀再喂**，或在 mechanics 侧小适配；fail-closed 失配 → 不附语义行）→ 命中 kernel track → `track_semantic_line(track, current)`（A1 纯函数：当前值所处 thresholds 区间的 consequence + zero_means 渲染）。
- kernel 加载：`ruleset_id` 参数加载一次复用；既有 per-actor `actor_ruleset_id` + kernel_cache（L291–307）保留不动（多规则集 actor 共存场景仍然正确），新参数只服务非 actor 行与缺 actor 映射时的回退。
- `actor_mechanical_states` 行（hp_current 等）同样附语义行：HP 轨 id 经 `hp_resource_track_id` 已有（L300），顺手对 live hp_current 调 `track_semantic_line`。
- **写入路径不动**：`apply_outcome_resource_tracks` L89–92 的 owner 二分（actor/其余→scene.current）维持现状；party/world 写路径缺口在 due/投影侧可观测（threshold_desc/semantic 行仍渲染），记 C7 总验收"可观测技术债"清单——本任务只管读侧通用。
- roll_check 侧：`exec.primary.outcome` + `kernel.dice_core`（call 内已有 kernel_dice 的 load，复用同一次 `load_rule_kernel` 结果，避免双查——把 `kernel_dice` 改为接受 `&RuleKernel` 的纯辅助 + call 顶部加载一次，是本任务允许的小重构）。

### 验证

```
cd crates/trpg-model && cargo test
cd crates/trpg-mechanics && cargo test     # cwd 必须在 crate 目录（workspace 根会带起全家）
cd crates/trpg-runtime && cargo build
cd crates/trpg-gm && cargo test
```

---

## B4. watcher 阈值检测 + mechanic_dues 持久化

**对应验收**：4（watcher 单测全套）；spec §5.2 阈值通路 + 拍板"阈值检测在 trpg-mechanics 效果落账单点（旧路径免费受益）"。

### Files

- Create `crates/trpg-mechanics/src/watcher.rs` —— `detect_crossings`（纯）+ `detect_threshold_dues` + due 构造
- Create `migrations/0027_mechanic_dues_v120.sql` —— 骨架契约 §4 SQL **逐字**
- Modify `crates/trpg-mechanics/src/lib.rs` —— `pub mod watcher;`；`apply_outcome_resource_tracks` 阈值分支（L168–191）重构为调纯函数；`apply_direct_effect` 落账后补调（direct_effect.rs L113 `apply_effect_roll_with_decision` 返回之后）
- Modify `crates/trpg-db/src/lib.rs` —— `migrate()` include_str! 数组 L50（0026 行）之后手动加 0027 行；新原语 4 个（骨架 3 个 + 本节补 1 个状态查询）
- Modify `crates/trpg-gm/src/tools/check.rs` —— roll_check 结果 JSON 带回本回合新产 dues

### 接口契约

骨架契约 §4 的 `MechanicDue`/`DueSource`/`DueStatus`/`ThresholdCrossing`/`detect_threshold_dues`/`dues_for_hook` 签名与 SQL **逐字采用**（类型落 trpg-model/src/mechanics.rs，A1 已建）。本任务补充：

```rust
// trpg-mechanics/src/watcher.rs
/// PURE：阈值穿越检测——把 lib.rs L168–191 的 crossed/loss_in_one_go 判定逻辑
/// 原样抽出（行为零变化），并读取 A4 新写回的 thresholds[*].followup_procedure_id。
/// owner_kind/owner_id 由调用方传入（apply_outcome_resource_tracks 的 is_actor 二分
/// 产物 / apply_direct_effect 的 target_actor）。
pub fn detect_crossings(
    track: &serde_json::Value,    // kernel.resource_tracks[i]
    owner_kind: &str,
    owner_id: &str,
    before: i32,
    after: i32,
    op: ParameterOperation,
) -> Vec<ThresholdCrossing>;

/// PURE：crossing → MechanicDue（due_id="due_{uuid simple}"，evidence
/// {"before","after","delta"}，status=Open，threshold_desc=crossing.consequence）。
pub fn due_from_crossing(session_id: &str, turn_id: &str, c: &ThresholdCrossing) -> MechanicDue;
```

```rust
// trpg-db/src/lib.rs（对标 insert_check_contract L1803 的 bind 风格）
pub async fn insert_mechanic_due(&self, due: &MechanicDue) -> Result<()>;
pub async fn list_open_mechanic_dues(&self, session_id: &str) -> Result<Vec<MechanicDue>>;
pub async fn update_mechanic_due_status(&self, due_id: &str, status: &str,
    waive_reason: Option<&str>, waive_scope: Option<&str>) -> Result<()>;
/// 本节补充（B5 的 scene-waive 抑制 + B6 的场景切换重开都要按状态查）：
pub async fn list_mechanic_dues_with_status(&self, session_id: &str, status: &str) -> Result<Vec<MechanicDue>>;
```

roll_check 结果 JSON 增量（check.rs，`execute_system_roll_bundle` 之后）：

```jsonc
"dues": [ { "due_id":"due_…", "threshold_desc":"…", "followup_procedure_id":"coc.temporary_insanity", "evidence":{…} } ]
// 取法：ctx.engine.db.list_open_mechanic_dues(session) 过滤 turn_id == 当前 turn（agent 当场看见，spec §5.2）；空数组时整键缺省。
```

### 测试清单

（纯函数测试不碰 db；track fixture 直接抄真 CoC kernel sanity 轨形状：`{"id":"sanity","owner_kind":"actor","thresholds":[{"loss_in_one_go":5,"consequence":"may trigger temporary insanity","followup_procedure_id":"coc.temporary_insanity"},{"at":0,"direction":"at_or_below","consequence":"permanent insanity"}]}`）

1. `san_loss_of_6_in_one_go_produces_crossing` —— before=38, after=32, op=Subtract → 1 条 crossing：kind=="loss_in_one_go"、followup_procedure_id==Some("coc.temporary_insanity")、before/after 原样（验收 4 主断言的纯函数半边）。
2. `san_loss_of_4_produces_no_crossing` —— before=38, after=34 → 空 Vec（验收 4 阴性例）。
3. `hp_reaching_zero_crosses_at_or_below` —— track 带 `{at:0,direction:"at_or_below"}`，before=3, after=0 → crossing kind=="cumulative"；before=0,after=0（未穿越，边沿触发）→ 空。
4. `threshold_without_followup_still_yields_due_with_prose` —— thresholds 条目无 followup_procedure_id → crossing.followup_procedure_id==None，`due_from_crossing` 产 due 且 threshold_desc==consequence 原文（fail-closed 但不静默，spec §5.2）。
5. `due_from_crossing_evidence_has_before_after_delta` —— evidence 三键齐且 delta==after-before；status==Open；source==Threshold。
6. `apply_outcome_threshold_facts_unchanged` —— 重构回归：既有 `resource_threshold_consequence` CreateFact patch 在同输入下逐字节不变（重构是抽函数不是改行为；以 L168–191 现行为做金样断言）。
7. trpg-db（真库 :54347，对标既有 db 测试样板）：`mechanic_due_roundtrip_and_status_update` —— insert → list_open 含之 → update_mechanic_due_status("waived", Some("reason"), Some("scene")) → list_open 不含、list_mechanic_dues_with_status("waived") 含之且 waive 字段落库。

### 实现要点

- 重构边界：`apply_outcome_resource_tracks` 的阈值 for 循环（L168–191）整体替换为 `for c in detect_crossings(track, owner_kind, owner_id, before, after, op)`：CreateFact patch 从 crossing 重建（字段一一对应，金样测试钉死）+ 新增 `insert_mechanic_due(&due_from_crossing(...)).await.ok()`（落库失败 `.ok()` 吞——结算主链绝不因 watcher 持久化失败中断，与同函数既有 `.ok()` 风格一致）。`owner_kind` 取 track 的 `owner_kind` 字段字符串原值（无则 "actor"/"scene" 按 is_actor 二分回填）——due 携带开放枚举原值，不丢 party/world 信息。
- `apply_direct_effect` 接线点：direct_effect.rs L113 之后——从 `applied.effect.impacts` 取 before/after（`ParameterImpact.before/after` 是 `Option<Value>`，as_i64 取整失败 → 跳过该 impact，fail-closed），按 `resolve_resource_track_id(parameter_path, kernel)` 找 track 后调同一纯函数。**旧路径（非 agent 的 play_turn_sse）经 after_check_resolved→apply_outcome_resource_tracks 免费受益**，无需另接。
- `MechanicDue` 落库列与契约 §4 SQL 一一对应；`evidence` jsonb；`created_at/updated_at` default now()。
- migrate 数组：**手动加行**（include_str! 不会自己长，0026 先例，memory 有记录）。
- roll_check 带回 dues 的查询按 `turn_id` 过滤本回合新产——直接复用 list_open（轻查询、回合内行数极小），不为此加专用 SQL。
- 工程红线：watcher.rs 全文件无任何规则集名/轨名字面量；阈值词表只认 `at`/`direction`/`loss_in_one_go`/`consequence`/`followup_procedure_id`（既有 kernel 数据契约 + A4 新增键）。

### 验证

```
cd crates/trpg-mechanics && cargo test
cd crates/trpg-db && cargo test            # 需 :54347 真库在跑（docker chatrpg-postgres-rulesets）
cd crates/trpg-gm && cargo test
wc -l crates/trpg-mechanics/src/watcher.rs migrations/0027_mechanic_dues_v120.sql
```

---

## B5. EngineHook 事件点发 due（第三触发通路运行时侧）

**对应验收**：3a 的"EngineHook 条目可被运行时触发"半边；spec §4 EngineHook 通路 + §5.2 事件点清单 + 附录 A5 周期节拍器实证。

### Files

- Modify `crates/trpg-mechanics/src/watcher.rs` —— 增 `dues_for_hook` + `HookEvent` + 两个纯函数（B4 已建文件）
- Modify `crates/trpg-gm/src/turn_loop.rs` —— 确定性头部（步骤 1，L41 gate 结算之后）接 `HookEvent::TurnStart`
- Modify `crates/trpg-gm/src/tools/world.rs` —— `NavigateSceneTool::call`（L94 `set_session_scene` 成功后）接 `SceneEnter`；`AdvanceTimeTool::call`（L64 `advance_world_time` 成功后）接 `TimeAdvance` + downtime 时附发 `Rest`

### 接口契约

骨架契约 §4 的 `dues_for_hook`/`HookEvent` 签名**逐字采用**。本任务补充两个纯函数：

```rust
// trpg-mechanics/src/watcher.rs

/// PURE：hook × 事件匹配。Calendar 变体单独走 calendar_crossed；
/// SceneEnter/TurnStart/TimeAdvance/Rest/SessionEnd/DevelopmentPhase/
/// CombatStart/CombatEnd 按变体一一对应（serde tag 值即 due.hook_event 字符串）。
pub fn hook_matches_event(hook: &EngineHook, event: &HookEvent) -> bool;

/// PURE：日历跨界判定——granularity_seconds(g)（A1 纯函数：unit/seconds_per_unit/
/// segments_per_day → 周期秒数，认不出 None）为 Some(s) 时
/// `from_tick / s != to_tick / s`（整除地板跨界；grounded: trpg-time L92–95
/// world_tick 增量即秒数，tick 与 absolute_seconds 同步走）。
/// granularity 认不出 → false（fail-closed：不猜、不发 due）。
pub fn calendar_crossed(g: &CalendarGranularity, from_tick: i64, to_tick: i64) -> bool;
```

`dues_for_hook` 内部规约（实现体不写，行为定义如下）：

1. `load_rule_kernel(ruleset_id)` → 无 kernel / 目录空 → `Ok(vec![])`；
2. 遍历 `kernel.mechanics_catalog`，对每条 entry 的每个 hook 做 `hook_matches_event`（Calendar 在 `HookEvent::TimeAdvance` 分支内走 `calendar_crossed`）；
3. **抑制规则**（防债务刷屏）：命中前先查 ①`list_open_mechanic_dues` 已有同 `(mechanic_id, hook_event)` 的 open due → 跳过（同一钩子债未清不重复催）；②`list_mechanic_dues_with_status(session,"waived")` 中存在同 `(mechanic_id, hook_event)` 且 `waive_scope=="scene"` 的行 → 跳过（场景内豁免有效；场景切换重开后该行回到 open、规则 ① 接管）；`waive_scope=="turn"` 的 waived 行**不抑制**（下回合仍提醒——spec §5.3 语义）；
4. 产 due：`source=DueSource::Hook`、`hook_event=Some(<serde tag>)`、`mechanic_id=Some(entry.id)`（§3.5.1 结构化绑定）、`threshold_desc=entry.when_to_use 非空否则 entry.description`、`followup_procedure_id=entry.followup_links 首个 procedure_id`（无则 None）、`owner_kind/owner_id`：entry.tested_parameter 能解析到 actor 参数则 `actor/pc.current`，否则 `session/{session_id}`（开放枚举原值，不强塞 actor）、`evidence`：钩子型形状（`{"event":"scene_enter","scene_id":…}` / `{"event":"calendar","from_tick":…,"to_tick":…,"granularity":…}`）；
5. 每条 `insert_mechanic_due` 落库后归入返回 Vec。

三个接线点（调用方）：

```rust
// turn_loop.rs 步骤 1 末尾（resolve_pending_gate 之后、prepare_turn_context 之前）：
let hook_dues = trpg_mechanics::RefereeCombatService::new(self.engine.db.clone())
    .dues_for_hook(&input.request.session_id, &input.request.turn_id, &input.request.ruleset_id, &watcher::HookEvent::TurnStart)
    .await.unwrap_or_default();
// （B6 把它 absorb 进 ObligationLedger；本任务先以 `let _ =` 落库即可独立交付）

// world.rs NavigateSceneTool（set_session_scene 成功后）：
HookEvent::SceneEnter { scene_id: args.target_node_id.clone() }
// world.rs AdvanceTimeTool（advance_world_time 成功后）：
HookEvent::TimeAdvance { from_tick: result.from.world_tick, to_tick: result.to.world_tick }
// 且 scale == TimeScale::Downtime 时再发一次 HookEvent::Rest（数据映射，非新工具——契约 §1 EngineHook 注记）
// 两工具结果 JSON 增 "hook_dues" 数组（due_id/threshold_desc/mechanic_id），空缺省。
```

### 测试清单

（纯函数 + MockKernel 风格，无 LLM；db 往返用 :54347 真库测试归 trpg-db 既有样板）

1. `calendar_crossing_fires_only_across_segment_boundary` —— `CalendarGranularity{unit:"segment",segments_per_day:Some(8),..}`（段长 10800s，Fate 一天 8 段实证）：from=5000,to=12000 → true；from=1000,to=9000 → false（段内）；`unit:"day"` from=86000,to=87000 → true。
2. `unknown_granularity_is_fail_closed` —— `unit:"???"` 且无 seconds_per_unit → `calendar_crossed==false`（不猜不发）。
3. `hook_matches_event_maps_variants_one_to_one` —— TurnStart↔TurnStart true；SceneEnter↔TurnStart false；Rest↔Rest true；Calendar 不经此函数直配（断言 false，强制走 calendar_crossed 分支）。
4. `turn_start_hook_entry_produces_due_with_mechanic_id` —— 目录含 `hooks:[TurnStart]` 条目 → due.source==Hook、mechanic_id==Some(entry.id)、hook_event==Some("turn_start")（dues_for_hook 拆纯部分测：把"目录×事件→候选 due"抽成 `pub fn hook_due_candidates(catalog,&event,session,turn)->Vec<MechanicDue>` 纯函数，db 抑制留集成层——执行时按此拆分实现）。
5. `open_due_suppresses_refire_and_turn_waive_does_not`（trpg-db 集成测，真库）—— 同 (mechanic_id,hook_event) 已有 open due → dues_for_hook 不再产；将其 waive(scope=turn) 后 → 再产；waive(scope=scene) 后 → 不产。
6. trpg-gm：`advance_time_result_carries_hook_dues_key_shape`——纯断言结果 JSON 形状（lazy pool 下 dues 为空数组缺省键即可，真触发归 C5/C6 e2e）。

### 实现要点

- **检测逻辑单点**：所有事件点共用 `dues_for_hook` 一个原语（拍板"统一在一个 hook 查询原语里"）；接线点只负责构造 `HookEvent` 并附结果，绝不在 trpg-gm 里重复遍历目录。
- `SessionEnd`/`DevelopmentPhase`/`CombatStart`/`CombatEnd`：本期**仅原语支持**（HookEvent 变体齐全、dues_for_hook 能处理），无引擎调用点——CLI 会话收尾的预留调用点写一行注释不接线；解析出这些钩子但没事件点的条目已被 A3 finalize 降级语义并记 validation_report（缺口可观测，spec §5.2）。
- turn_loop 接线放步骤 1 末尾的理由：dues 须在 `prepare_turn_context` 之前落库，这样 B3 的 BP3 投影与 B6 的债务装载同回合可见。失败 `unwrap_or_default()`——头部任何 watcher 故障不得阻断回合（与既有 `let _ =` 风格一致）。
- AdvanceTime 的 calendar 与 time_advance 是**同一个 HookEvent**：目录里 `Calendar{granularity}` 钩子与 `TimeAdvance` 钩子都在 `TimeAdvance{from,to}` 分支内判定（前者加 calendar_crossed 闸，后者直发）——不发明第二个事件变体。
- 工程红线：粒度全部来自 ruleset 数据（CalendarGranularity 是解析产物），watcher 内无任何"日/周/月"字面分支——`granularity_seconds` 认 unit 词表 + seconds_per_unit 优先（A1 契约），认不出就 fail-closed。

### 验证

```
cd crates/trpg-mechanics && cargo test
cd crates/trpg-db && cargo test
cd crates/trpg-gm && cargo test
wc -l crates/trpg-mechanics/src/watcher.rs   # B4+B5 合计 ≤400
```


---

## B6. 债务清单门控 + waive_obligation + 追溯债务（loop 心脏改动）

### Files
- Create: `crates/trpg-gm/src/obligations.rs`（契约 §5 全量落地）
- Modify: `crates/trpg-gm/src/tools/mechanic.rs`（B2 已建该文件——增 WaiveObligationTool）
- Modify: `crates/trpg-gm/src/turn_loop.rs:28`（GmLoop 字段）、`:63-148`（工具轮门控）、`:150-170`（verify_after_stream 接追溯债务）
- Modify: `crates/trpg-gm/src/prompts.rs`（DynamicTailInput 增 `obligations_block: Option<String>`，装配进 dynamic tail——排在 errata_blocks 之后、Player Input 之前）
- Modify: `crates/trpg-gm/src/lib.rs`（`pub mod obligations;` + re-export ObligationLedger/WaiveScope）
- Test: 各文件同文件 `#[cfg(test)]`；turn_loop 场景测试进 `turn_loop_tests.rs`

### 接口契约
骨架契约 §5（ObligationLedger / RetroactiveEffectDebt / WaiverRecord / WaiveScope 及全部方法签名）与 §6 的 waive_obligation 工具 schema **逐字生效**，本任务不新增类型。工具注册：`ToolRegistry::standard()` 固定顺序追加 `waive_obligation`（在 B2 的 lookup_mechanic 之后，11→12 工具）。

### 测试清单（测试即规格）
1. `obligations::tests::blocking_view_excludes_settled_and_waived` —— record_open_check + mark_check_settled + waive 后 blocking() 只剩未处理项；空清单 block_text() 返回 None。
2. `obligations::tests::waive_unknown_target_errs` —— waive("不存在的 id") → Err；工具层断言折成 `obligation_not_found` recoverable 错误。
3. `obligations::tests::carryover_block_lists_unresolved_dues_with_evidence` —— 含 threshold_desc 与 evidence 摘要。
4. `turn_loop_tests::open_due_blocks_narration_round`（spec 验收 5）—— MockLlm 脚本：第一轮 content delta（试图叙事终态）但 ObligationLedger 有未处理 due → 断言该轮 messages 被回填 block_text() 的 system 观察、循环继续、content **未**流给 on_delta；第二轮脚本 waive 后 content 正常流出。
5. `turn_loop_tests::waive_emits_audit_and_unblocks` —— waive 带理由 → 勘误记忆 MemoryEvent tags 含 "gm_waive"（MockLlm 路径用 ErrataMemory 断言载荷，不连 DB）+ blocking() 清空放行。
6. `turn_loop_tests::round_exhaustion_carries_debt_to_next_turn` —— 轮耗尽仍有债务 → ToolChoice::None 强制叙事语义不变（一期断言照绿）+ GmLoop.obligations 持久字段含 carryover + 下回合 assemble 的 dynamic tail 含 obligations_block。
7. `turn_loop_tests::invented_effect_becomes_retro_debt`（spec 验收 11 单测侧）—— verify_after_stream 抓到 InventedEffect → absorb_retro_debts → 下回合 blocking() 含该 debt_id。

### 实现要点
- GmLoop 增 `obligations: ObligationLedger` 持久字段（与 errata 同生命周期跨回合存活）；回合头部 `db.list_open_mechanic_dues(session_id)` 装载遗留（B4 已建 db 原语）。
- 工具轮门控插点：现 turn_loop.rs `if !saw_tool`（约 L116，content 终态分支）前置 `obligations.blocking()` 检查——非空则回填 block_text() 为 system 观察并 `continue`（**该轮 content 不流出**：RedactingBuffer 缓冲的增量丢弃，on_delta 不调用——实现上把"流出"延后到 blocking 检查之后再 flush，参考一期混合轮的缓冲重建写法 L112-115）。
- 借用冲突预案（索引原文）：优先"dues 读写走 db + 工具轮结束统一 absorb"形态，避免 ObligationLedger 与 TurnLedger 的 `&mut` 在 dispatch 借用链上打架；若编译器允许直接传 `&mut` 则取直通形态。两形态测试断言相同。
- waive 工具副作用三连：`db.update_mechanic_due_status(due_id, "waived")`（仅 due 类）→ 勘误记忆（复用 errata.to_memory_event 样板，tags=["gm_waive"]）→ `obligations.waive`。scope=scene 的跨回合豁免凭 db status + 场景切换时（navigate_scene 成功路径）重开。
- 零 per-ruleset 硬编码：门控/豁免逻辑只认 id 与状态，不认机制名。

### 验证
Run: `cargo test -p trpg-gm`
Expected: 全绿（含一期 43 个既有测试照绿——缓存稳定测试尤其不得破）；`wc -l crates/trpg-gm/src/obligations.rs crates/trpg-gm/src/turn_loop.rs crates/trpg-gm/src/tools/mechanic.rs` 全部 ≤400。

---

## B7. verifier referenced_ledger_ids 结构化升级（一期遗留清算，护栏 §3.5.5）

### Files
- Modify: `crates/trpg-agent/src/gm_loop.rs:176-283`（NarrationVerifier::verify）
- Modify: `crates/trpg-gm/src/turn_loop.rs:150-170`（verify_after_stream 传账本 id 全集）
- Test: gm_loop.rs 同文件 `#[cfg(test)]`（既有测试区 L468-600 扩展）

### 接口契约
`NarrationVerifier::verify(&self, ledger: &TurnLedgerSnapshot, submission: &FinalNarrationSubmission) -> NarrationVerifierResult` 签名不变。行为升级：
- `submission.referenced_ledger_ids` **非空** ⇒ 结构化核对优先：每个引用 id 必须 ∈ 账本 id 全集（check_contracts 的 check_id ∪ dice_rolls 的 roll_id ∪ effect_contracts 的 effect_id ∪ parameter_impacts 的 impact_id）；引用了不存在的 id → `InventedEffect` finding（detail 含该 id）。
- 引用为**空** ⇒ 回退现有子串扫描，且所有回退产生的 finding 的 detail 加前缀 `fallback:substring_scan: `（可观测技术债，不静默——护栏 §3.5.1 同款思想）。

### 测试清单
1. `verifier_structured_refs_accept_known_ids` —— 引用账本真实 id 全集 → accepted（即使叙事文本不含可见 token，结构化引用优先于 OmittedVisibleResult 子串检查？**否**——OmittedVisibleResult 语义保留：可见结果 token 检查独立运行；本测试构造 token 在文本中存在的用例）。
2. `verifier_structured_refs_reject_unknown_id` —— 引用 "check_不存在" → InventedEffect finding 含该 id。
3. `verifier_empty_refs_falls_back_with_marker` —— 引用为空 + 文本声称伤害无账本证据 → finding detail 以 `fallback:substring_scan: ` 开头。
4. 既有 gm_loop.rs 全部测试照绿（含 L468-600 区 referenced_ledger_ids 既有用例）。

### 实现要点
- id 全集收集为 gm_loop.rs 私有 helper `ledger_id_set(&TurnLedgerSnapshot) -> HashSet<String>`（≤15 行）。
- turn_loop.rs 的 verify_after_stream：`referenced_ledger_ids` 从 `vec![]` 改为 `ledger_id_set` 物化的 Vec（"已落账事实全集"语义——agent 不显式声明引用，引擎代填全集，结构化核对退化为"声称的 id 必在账本"恒真 + 子串回退被关闭 → 实际效果是 fallback 标注路径只在账本为空时出现）。注释写明此语义决策。
- 不动 FinalNarrationSubmission 结构（字段已存在），向后兼容。

### 验证
Run: `cargo test -p trpg-agent -p trpg-gm`
Expected: 全绿；gm_loop.rs ≤400 行（现 740 行——**豁免**：一期遗留文件本任务只做增量，不强拆；新增 helper ≤15 行）。

---

## B8. gm_skill 准则两条 + Slice B 收口

### Files
- Create: `data/agent/gm_skill/global/40_mechanics_catalog.md`
- Create: `data/agent/gm_skill/global/50_obligation_policy.md`
- Test: `crates/trpg-gm/src/prompts.rs` 同文件 `#[cfg(test)]` 补一条合并顺序断言

### 接口契约（数据文件内容要点——执行时成文，不是占位）
- `40_mechanics_catalog.md`：①目录优先于自由发挥（流程：看 BP1 索引 → 需要细节 lookup_mechanic → 开检定带 mechanic_id 继承绑定；目录没有的机制按语义裁量并考虑 retrieve_rules）；②**行为语义监听条款**：按本 ruleset 目录中 kind=spend/其它元资源条目的 when_to_use，持续评估玩家言行并发放/扣减对应元资源（Triangle 嘉奖/记过、Fate FP 双实证——条款引用目录条目而非写死机制名，零硬编码）。
- `50_obligation_policy.md`：①due 必须回应——处理（roll_check/request_player_roll）或 waive_obligation 带理由；②追溯债务=上回合叙事声称但未落账的效果——补 apply_effect 或 waive（理由如"叙事中已收回"）；③waive 是裁量权不是逃生舱：高频 waive 同类 due 会进勘误统计。
- grounded：load_gm_skill 按文件名字典序合并（prompts.rs L85-94），40_/50_ 排在 30_output_contract.md 之后**追加**，既有三份文件零字节变动（缓存稳定：BP1 文本变化属"规则变更"级有因变化，跨回合 hash 基线在本任务后重置一次，缓存回归测试同步更新基线注释）。

### 测试清单
1. `prompts::tests::gm_skill_merge_order_includes_new_entries` —— 合并文本中 40_ 内容出现在 30_ 之后、50_ 在 40_ 之后。
2. Slice B 收口矩阵：`cargo test -p trpg-gm -p trpg-agent -p trpg-mechanics` 全绿（spec 验收 4/5/6 对应单测全数通过）。

### 实现要点
- 两份数据文件为纯 prose 准则（中文），不含任何代码/规则集名字面量。
- 收口冒烟（真库手动步骤，写入执行记录）：`TRPG_LLM_MODEL=gpt-5.5 cargo run -p trpg-cli --bin trpg -- play --ruleset call_of_cthulhu_7e --agent` 跑 1 回合，确认：BP1 含目录索引段、lookup_mechanic 在工具表中、无债务回合行为与一期无差异（叙事正常流出）。

### 验证
Run: `cargo test -p trpg-gm -p trpg-agent -p trpg-mechanics && ls data/agent/gm_skill/global/`
Expected: 全绿；global/ 下六份文件（10/15/20/30 + 新 40/50——15 为一期收尾新增的 context_projection）。

---

## Section C —— 模组侧 + e2e 黄金链（任务正文）

> 隶属计划：`docs/superpowers/plans/2026-06-10-rule-aware-gm.md`（骨架）；权威设计 spec：`docs/superpowers/specs/2026-06-10-rule-aware-gm-design.md`。
> 本 section 覆盖骨架任务索引 C1–C7：场景机制意图落模型（C1）→ 深抽契约扩展（C2）→ BP2 投影（C3）→ effect_policy Rust 强制执行（C4）→ 三条 e2e 黄金链（C5/C6/C7，对应 spec §8 验收 7/8/9/10/11/12/13 + 总对账）。
> 全部 `// grounded:` 行号已对照真实源码核实（2026-06-10 工作副本）。无 git，每任务以验证收尾；新文件 ≤400 行；零 per-ruleset 硬编码；全部新结构 `#[serde(default)]`。

**依赖关系（执行顺序约束）**：
- C1→C2→C3→C4 顺序内聚，**代码上不依赖 Section A/B**（effect_policy 执行复用一期 `execute_system_roll_bundle`/`apply_direct_effect`/StatePatch 链）；与 B 同装时 C4 的 ModifyTrack 落账自然被 B4 watcher 覆盖，零额外代码。
- 唯一共享文件：`crates/trpg-model/src/mechanics.rs` 与 A1 共用。**若 A1 已执行**，C1 只做"测试半边"（契约 §3 类型已在）；**若 C 先行**，C1 建该文件只放 §3 类型（SceneMechanicIntent/EffectPolicy/EffectPatchIntent），A1 后续往同文件追加 §1/§4 类型——两侧都不得动对方的类型定义。
- C5 是 A+B+C 合体验收（目录驱动 + watcher due + 债务门控），**必须排在 A7/B8 之后**；C6 只需 C 切片自身（验收 9）+ A/B（验收 10 的目录与语义投影半边）；C7 的验收 11 依赖 B6/B7（RetroactiveEffectDebt/ObligationLedger），验收 13 依赖 B1。

**e2e 环境总备忘（C5/C6/C7 共用，全部命令在 workspace 根 `chatrpg-rs-v1.20-formula/` 执行）**：
```bash
# ① DB 端口坑：.env 写的是 :54346（赛博库）；六规则 + CoC/Triangle 模组在 :54347
#    （容器 chatrpg-postgres-rulesets）。dotenv 不覆盖已导出的 env → 显式 export 必赢。
export DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg
# ② 数据目录：default_data_dir() 认 TRPG_DATA_DIR，否则 ./data（cwd 相对）——
#    grounded: trpg-cli/src/main.rs L2064-2066。在 workspace 根跑则可省略。
export TRPG_DATA_DIR="$PWD/data"
# ③ psql：宿主机没装 psql，一律 docker exec：
docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg -c "<SQL>"
# ④ macOS 无 timeout 命令：长命令交给执行工具自带的超时参数，不写 timeout(1)。
# ⑤ agent 回合入口：`trpg turn` 走旧路径 run_turn_once（不经 GmLoop）——
#    grounded: main.rs L537 Commands::Turn → turn_cli（非 agent loop）。
#    e2e 必须用交互 play 的 --agent 管道喂输入：
printf '<玩家输入1>\n<玩家输入2>\n/quit\n' | cargo run -p trpg-cli -- play --ruleset <rs> --module <mid> --agent
#    session_id 从 stdout 第一行 "agent session: <id>" 抓取（grounded: agent_play.rs L33）。
# ⑥ 库内现有模组（实查 :54347 parsed_bundles）：
#    call_of_cthulhu_7e.document（血色公路，CoC sandbox 中文）
#    triangle_agency.the_vault（The Vault，Triangle 任务集英文）
#    Homecoming（CPR）只有源文件 data/modules/CPR One Shot - Homecoming ver3.0 (Colored).pdf
#    + data/markdown/modules/cpr_one_shot_homecoming_ver3_0_colored.md，库里无 bundle——C6 先 parse。
```

---

## C1. SceneMechanicIntent 落模型 + 旧图谱向后兼容

### Files
- **Create（或 Modify，若 A1 已建）** `crates/trpg-model/src/mechanics.rs` —— 契约 §3 三个类型 + 渲染纯函数占位（C3 用的 `scene_intents_text` 在 C3 任务加，避免本任务超范围）。
- **Modify** `crates/trpg-model/src/lib.rs` —— ScenarioNode（L1601-1619 真实结构体）追加一个字段；若 A1 未执行，同时加 `pub mod mechanics; pub use mechanics::*;`（lib.rs 头部 mod 区）。
- **Create** `crates/trpg-model/tests/scene_mechanics_compat.rs` —— 集成测试（trpg-model 目前无 tests/ 目录，新建）。
- **Create** `crates/trpg-model/tests/fixtures/scene_node_legacy.json` —— 真图谱节点 fixture（dump 命令见实现要点）。

### 接口契约（计划骨架"共享类型契约 §3"逐字落地）
```rust
// trpg-model/src/lib.rs ScenarioNode（L1601）末尾追加（对齐既有 referenced_*_ids 风格）：
#[serde(default)] pub scene_mechanics: Vec<SceneMechanicIntent>,

// trpg-model/src/mechanics.rs：
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SceneMechanicIntent {
    pub intent_id: String,         // "homecoming.lawmen.cut_cable_force"
    pub description: String,       // 何种行动触发（语义对应玩家行动；结构引用按 intent_id——护栏 §3.5.2）
    pub tested_parameter: String,  // "brawling"
    #[serde(default)] pub difficulty: Option<serde_json::Value>, // {kind:"dv",value:13} / CoC 难度档——保持 Value 开放
    #[serde(default)] pub effect_policy: EffectPolicy,
    pub source_anchor: String,     // 空 → apply_deep_to_node 丢弃该条（fail-closed 不编造）
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct EffectPolicy {
    #[serde(default)] pub on_success: Vec<EffectPatchIntent>,
    #[serde(default)] pub on_failure: Vec<EffectPatchIntent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EffectPatchIntent {
    SetObjectState { object_id: String, patch: serde_json::Value },
    ModifyTrack    { owner_kind: String, owner_id: Option<String>, track_id: String, op: String, amount: i64 },
    CreateFact     { target: String, fact: serde_json::Value },
    StartCountdown { label: String, amount: i64, scale: String, payload: serde_json::Value },
    #[serde(untagged)]
    Other(serde_json::Value),  // fail-closed：不执行、不报错中断（C4 落 unexecutable_intent 事实）
}
```
- `// grounded:` ScenarioNode 全字段 `#[serde(default)]` 习惯已确立（L1612-1618）；ScenarioNode derive 含 JsonSchema → 新类型必须同样 derive JsonSchema，否则编译断。
- `// grounded:` 深抽提交经 submit_deep（module_reader_loop.rs L102-112 schema `{scene, entities}`），apply_deep_to_node（module_reader.rs L117）从 `deep.scene` 取字段——scene_mechanics 同路（C2 接线）。

### 测试清单（测试是规格）
1. `legacy_scenario_node_deserializes_with_empty_scene_mechanics`：fixture（真 :54347 血色公路图谱 dump 的单个 scene 节点 JSON，**不含** scene_mechanics 键）→ `serde_json::from_str::<ScenarioNode>` 成功且 `scene_mechanics.is_empty()`——旧模组不重抽不受损（spec §9 风险 3）。
2. `legacy_module_graph_roundtrip_is_lossless`：fixture 反序列化 → 再序列化 → 关键字段（node_id/title/links 数量/referenced_npc_ids）逐一相等（确认新字段不破坏既有 serde 流）。
3. `effect_patch_intent_five_forms_roundtrip`：五形态各构造一个 → to_value → from_value → `matches!` 断言变体与字段值原样（含 ModifyTrack 的 owner_id=None）。
4. `effect_patch_intent_unknown_kind_falls_to_other_losslessly`：`{"kind":"summon_demon","x":1}` → 解析为 `Other(v)` 且 `v` 与原 JSON 逐字节相等（to_string 比对）——降档保留绝不丢原文。
5. `scene_mechanic_intent_defaults`：仅含 intent_id/description/tested_parameter/source_anchor 的 JSON → difficulty=None、effect_policy 两 vec 全空。

### 实现要点
- fixture dump 命令（一次性，结果裁剪到单节点防 fixture 文件臃肿）：
  `docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg -tAc "select content_json->'module_graph'->'scenes'->0 from parsed_bundles where bundle_kind='module' and content_json->>'module_id'='call_of_cthulhu_7e.document'" > crates/trpg-model/tests/fixtures/scene_node_legacy.json`
- `#[serde(untagged)]` 变体兜底：与 A1 契约注记同款——untagged **变体**（非整枚举）需要 serde ≥1.0.171；动手前 `cargo tree -p serde | head -1` 核版本，不支持则给 EffectPatchIntent 手写 Deserialize（先按 tag 匹配五形态，匹配不中整值进 Other）。
- 若 A1 已落地：mechanics.rs 里 §3 类型可能已在——本任务只补 tests/fixtures 半边，逐项核对契约签名与上文逐字一致（不一致以骨架契约为准修正）。
- ScenarioNode 是 `Default` derive 成员——`Vec<SceneMechanicIntent>` 满足 Default，无额外改动；`stub_to_node`（module_reader.rs L66）走 `ScenarioNode::default()` 自动得空 vec，不用碰。

### 验证
```bash
cd crates/trpg-model && cargo test -p trpg-model    # 全绿，含新 5 测
wc -l src/mechanics.rs                              # ≤400
```

---

## C2. 深抽契约扩展：scene_mechanics 顺路产出（零新增管线）

### Files
- **Create** `crates/trpg-rule-agent/src/reader/scene_mechanics.rs` —— 解析/过滤纯函数 + 内联单测（**为何新文件**：module_reader.rs 现 396 行，再塞解析逻辑必破 400；这里只放确定性纯函数，无 LLM）。
- **Modify** `crates/trpg-rule-agent/src/reader/module_reader.rs` —— `apply_deep_to_node`（L117-181）加 3-4 行调用；文件头 `mod` 声明不在此（reader/mod.rs 统一管）。
- **Modify** `crates/trpg-rule-agent/src/reader/module_reader_loop.rs` —— `DEEP_SYS`（L19-26）加指引段；`submit_deep_tool`（L102-112）description 提及 scene_mechanics；`oneshot_deep_extract` user prompt（L201-208）加一句产出要求。
- **Modify** `crates/trpg-rule-agent/src/reader/mod.rs` —— `pub(crate) mod scene_mechanics;`。
- **Modify** `crates/trpg-rule-agent/src/reader/module_reader_loop_tests.rs` —— ReplayClient 样板上加 2 个用例。

### 接口契约
```rust
// crates/trpg-rule-agent/src/reader/scene_mechanics.rs
use serde_json::Value;
use trpg_model::SceneMechanicIntent;

/// 从深抽提交的 scene 对象解析 scene_mechanics。fail-closed 三重过滤（逐条独立丢弃，
/// 绝不因一条坏整批废）：① 整条 JSON 反序列化失败 → 丢；② source_anchor 空/全空白 → 丢
/// （不编造——对标 links 的 anchor 收口先例，module_reader_loop.rs L158-165）；
/// ③ tested_parameter / intent_id 空 → 丢。按 intent_id 去重（保首条）。
pub(crate) fn parse_scene_mechanics(scene: &Value) -> Vec<SceneMechanicIntent>;
```
- `apply_deep_to_node` 内接线（紧跟 referenced_*_ids 的 `if let Some(v) = ids(...)` 区之后，L155 附近）：
```rust
// 深抽交了非空 scene_mechanics 才覆盖；空/缺 → 保留既有（再抽不冲掉已得，
// 对标 referenced_*_ids 的 Some 才覆盖样板 L144-155）。
let mechanics = super::scene_mechanics::parse_scene_mechanics(scene);
if !mechanics.is_empty() { node.scene_mechanics = mechanics; }
```
- `DEEP_SYS` 追加指引（语义级，零规则集专名）：「scene_mechanics：仅当场景文本**明确写出**检定（技能/难度/后果）时编译为结构化条目 `{intent_id, description, tested_parameter, difficulty, effect_policy{on_success,on_failure}, source_anchor}`；source_anchor 必须摘当前页原文片段；effect_policy 条目只用 kind∈{set_object_state, modify_track, create_fact, start_countdown}；文本没写明的**绝不编造**，没有就交空数组。」

### 测试清单
1. `scene_mechanics.rs` 内联单测（纯函数）：
   - `parse_filters_missing_anchor_and_empty_param`：3 条输入（1 全合法 / 1 无 source_anchor / 1 tested_parameter 空串）→ 仅 1 条返回，且字段值原样。
   - `parse_dedups_by_intent_id_keeps_first`：同 intent_id 两条 → 1 条（首条的 description）。
   - `parse_tolerates_malformed_entry`：数组里混一个非对象（字符串）→ 其余正常解析、不 panic。
   - `parse_missing_key_returns_empty`：scene 无 scene_mechanics 键 / 键为 null → 空 vec。
2. `module_reader_loop_tests.rs`（ReplayClient 样板，对标 `oneshot_slices_page_and_filters_links_fail_closed`）：
   - `oneshot_deep_carries_scene_mechanics_fail_closed`：ReplayClient 的 deep_args 带 2 条 intents（1 条无 anchor）→ `deep_extract_scene_in_place` 后 `readout.scenes[idx].scene_mechanics.len()==1` 且 intent_id 正确。
   - `legacy_deep_payload_keeps_existing_mechanics`：节点预置 1 条 scene_mechanics，deep_args 不含该键 → 深抽后仍是原 1 条（空不覆盖）。
3. prompt 字面回归（防手滑）：`deep_sys_mentions_scene_mechanics`——`assert!(DEEP_SYS.contains("scene_mechanics"))`（DEEP_SYS 是 `const`，loop tests 经 `use super::*` 可见）。

### 实现要点
- oneshot 与 ReAct 回退两路径**都走 apply_deep_to_node**（grounded: deep_extract_scene_in_place L156 单点调用）→ 只接一处，两路径全覆盖。
- submit_deep schema 的 `"scene": {"type":"object"}` 是开放对象（L107），scene_mechanics 作为 scene 子键**不需要改 schema 结构**——只改 description 与 prompt 指引；这保持与既有提交契约的兼容（旧模型提交不带该键照常工作）。
- 与 P5 后台续抽 / scene_navigator 到场深抽自动同享（三调用方共用 deep_extract_scene_in_place，grounded: L125-127 注释）——**零新增管线**这一 spec 承诺由此兑现，不需要碰 trpg-api。
- 不要在 reader 侧校验 tested_parameter 是否存在于 kernel sheet schema——那是 A3（kernel 编译护栏）的职责边界；模组侧参数名以模组原文为准，运行时 roll_check 经 `execute_system_roll_bundle` 的 source-backed 校验兜底（grounded: runtime L998 `contract_missing_source_backed_parameters` → blocked_missing_source）。
- module_reader.rs 改后跑 `wc -l`：若 +4 行后破 400，把 `entry_scene_index`/`resolve_entry_index`（L91-113，纯函数）外移到 scene_mechanics.rs 同级新位置或 module_graph_edges.rs——**先量再挪，不预挪**。

### 验证
```bash
cd crates/trpg-rule-agent && cargo test -p trpg-rule-agent   # 全绿（既有 60+ lib 测试照绿 + 新 7 测）
wc -l src/reader/module_reader.rs src/reader/module_reader_loop.rs src/reader/scene_mechanics.rs  # 全 ≤400
```

---

## C3. 当前场景 intents 投影（BP2）

### Files
- **Modify** `crates/trpg-model/src/mechanics.rs` —— 渲染纯函数 `scene_intents_text`。
- **Modify** `crates/trpg-runtime/src/lib.rs` —— `scene_node_to_blocks`（L2233-2309）追加第二个块；调用方 `module_scene_blocks_for_turn`（L1489-1541）**零改动**（已传整 node，返回 Vec 自然携带新块）。
- **Modify** trpg-runtime 既有 scene_node_to_blocks 测试处 —— 加 2 个用例（对标既有场景投影测试样板；用 `grep -rn "scene_node_to_blocks" crates/trpg-runtime` 找到现测试模块落点）。

### 接口契约
```rust
// trpg-model/src/mechanics.rs
/// 当前场景 intents 索引的渲染纯函数：每条一行
/// `intent_id | description | tested_parameter | difficulty摘要`。
/// difficulty 摘要 = serde_json::to_string 紧凑串（None → "-"）；不含 effect_policy 全文
/// （结算才用，C4 从图谱按 intent_id 取）。空切片 → None（调用方不出块）。
pub fn scene_intents_text(intents: &[SceneMechanicIntent]) -> Option<String>;
```
```rust
// trpg-runtime scene_node_to_blocks 内（既有 SceneStatic 块 push 之后）：
if let Some(text) = trpg_model::scene_intents_text(&n.scene_mechanics) {
    let mut b = ContextBlock::new(
        format!("module.{module_id}.scene.{}.mechanics", n.node_id),
        BlockKind::SceneStatic,                       // 复用既有 kind：不动 trpg-db 的 block_kind 词表（L3822）
        format!("{} —— 场景机制意图", n.title),
        BlockContent::Text(text),
        Visibility::GmOnly,
        Stability::SceneStable,
        CacheZone::PinnedMiddle,                      // 骨架契约：pinned_hash 场景内稳定，场景切换换块
        Scope { scope_type: ScopeType::Scene, scope_id: n.node_id.clone() },
        58,                                            // 略低于正文块（60）
    );
    b.expires_at_scene = Some(n.node_id.clone());
    b.load_reason = Some("current_scene_mechanic_intents".into());
    b.tags = vec!["module_scene".into(), "scene_mechanics".into()];
    blocks.push(b);
}
```
- `// grounded:` ContextBlock::new 参数序/expires_at_scene/load_reason/tags 逐字对标既有块构造（runtime L2294-2307）；CacheZone/Stability 枚举真实变体（model L152-156/L198-204）。

### 测试清单
1. `scene_intents_text_renders_one_line_per_intent`（trpg-model）：2 条 intents（1 条 difficulty=Some({"kind":"dv","value":13})、1 条 None）→ 两行、各含 intent_id 与 tested_parameter、difficulty 摘要分别为 `{"kind":"dv","value":13}` 紧凑串与 `-`。
2. `scene_intents_text_empty_returns_none`（trpg-model）：空切片 → None。
3. `scene_with_mechanics_projects_intents_block`（trpg-runtime）：DeepExtracted 节点带 1 条 intent → `scene_node_to_blocks` 返回 2 块；第二块 block_id 以 `.mechanics` 结尾、zone==PinnedMiddle、stability==SceneStable、expires_at_scene==Some(node_id)、内容含 intent_id **不含** "effect_policy"/"on_success" 字样（全文不投影）。
4. `scene_without_mechanics_projects_single_block_unchanged`（trpg-runtime）：空 vec → 仍 1 块且与改动前字节一致（旧模组零变化=fail-closed；断言块数 + 首块 content 不含 "机制意图"）。
5. `intents_block_bytes_stable_across_calls`（trpg-runtime）：同一节点连调两次 `scene_node_to_blocks` → 两次第二块 `serde_json::to_vec` 字节相等（场景内跨回合字节稳定 = pinned_hash 稳定的函数级前提；e2e 半边在 C7②）。

### 实现要点
- **SkeletonOnly 场景天然不出块**：scene_node_to_blocks 顶部 early-return（L2239-2241）只放行 DeepExtracted——intents 本来就只在深抽时产出，语义自洽，不需要额外门。
- 渲染行内做轻量截断（description 超长 take 200 chars）防单条 intent 顶爆 BP2——截断阈值写常量并注释，不写 env（这不是行为开关是排版细节）。
- 既有第一块（正文 SceneStatic，CacheZone::DynamicTail）**保持原样不动**——本任务只加块不改块，缓存回归（C7②）才有干净的对照面。
- `Scope`/`ScopeType` 已在 runtime 文件内使用（L2302），import 零新增。

### 验证
```bash
cd crates/trpg-model && cargo test -p trpg-model
cd ../trpg-runtime && cargo test -p trpg-runtime
wc -l crates/trpg-model/src/mechanics.rs    # ≤400（与 A1 共享时一起量）
```

---

## C4. roll_check(scene_mechanic_id) + effect_policy Rust 强制执行

### Files
- **Create** `crates/trpg-gm/src/scene_policy.rs` —— effect_policy → 既有原语映射执行 + 纯函数。
- **Modify** `crates/trpg-gm/src/tools/check.rs` —— RollCheckArgs（L22-31）增 `scene_mechanic_id: Option<String>`；RollCheckTool::spec schema（L144）properties 同步；RollCheckTool::call（L147-193）插继承与结算后执行。
- **Modify** `crates/trpg-gm/src/tools/mod.rs` —— 错误码注释全集（L72-82）登记 `scene_mechanic_not_found`（与 B2 的 `mechanic_not_found`/`obligation_not_found` 同区；两 section 各自登记自己的码，merge 时并存）。
- **Modify** `crates/trpg-gm/src/lib.rs` —— `pub mod scene_policy;`。
- **Modify** `crates/trpg-gm/Cargo.toml` —— 增 `trpg-object = { path = "../trpg-object" }`、`trpg-time = { path = "../trpg-time" }`（workspace 内既有 crate，runtime 已依赖，无循环）。
- **Modify** `crates/trpg-object/src/lib.rs` —— 一个新 pub 方法（见契约；既有私有 `apply_patch` L400 不动可见性）。

### 接口契约
```rust
// crates/trpg-gm/src/scene_policy.rs —— 骨架契约逐字 + grounded 补全
use trpg_model::{CheckTargetModel, ContextRequest, EffectPatchIntent, EffectPolicy,
                 SceneMechanicIntent, StatePatch, ModuleGraph, Visibility, WorldEventKind};
use trpg_runtime::RuntimeEngine;
use crate::ledger::TurnLedger;

/// 结算后强制执行 effect_policy（spec §6"效果不留给叙事"）。按 outcome 选
/// on_success/on_failure，逐条映射到既有原语；每条产出入账 ledger；整批执行完
/// 落一条 world_event(kind=EffectApplied, event_json.source="scene.policy")——
/// e2e SQL 可查的单一证据点。单条失败：记 unexecutable 事实 + 继续下一条
/// （不中断、不静默）。返回已执行的 StatePatch 列表（工具结果 JSON 摘要用）。
pub async fn apply_effect_policy(
    engine: &RuntimeEngine,
    request: &ContextRequest,
    intent: &SceneMechanicIntent,
    success: bool,
    ledger: &mut TurnLedger,
) -> anyhow::Result<Vec<StatePatch>>;

/// PURE：按 outcome 选分支。
pub(crate) fn select_intents(policy: &EffectPolicy, success: bool) -> &[EffectPatchIntent];

/// PURE：difficulty Value → CheckTargetModel。认得的形态：{kind:"dv"|"static"|"target_number",
/// value:<num>} → StaticNumber{value, label=原 JSON 紧凑串}。其余（CoC 难度档字符串等）→ None
/// （保持 UnknownUntilLookup，交给 execute_system_roll_bundle 的 kernel defaults——fail-closed，
/// 形态映射非规则集分支，零硬编码）。
pub(crate) fn difficulty_to_target(difficulty: Option<&serde_json::Value>) -> Option<CheckTargetModel>;

/// PURE：当前场景查 intent。场景解析次序对标 module_scene_blocks_for_turn（runtime
/// L1513-1526）：scene_id 显式命中 → 该场景；否则回退首个 DeepExtracted 场景。
/// 在解析出的场景的 scene_mechanics 内按 intent_id 精确匹配（id 相等比较=合法字面匹配，
/// 护栏 §3.5.4）。
pub(crate) fn find_scene_intent<'a>(
    graph: &'a ModuleGraph,
    scene_id: Option<&str>,
    intent_id: &str,
) -> Option<&'a SceneMechanicIntent>;
```
```rust
// crates/trpg-object/src/lib.rs —— 新 pub 方法（scene_policy 的对象态落库单点）
impl ObjectService {
    /// effect_policy SetObjectState 的执行原语：object_instances 无该 object_id 行 →
    /// 先建最小实例（模组声明即存在：display_name=object_id、object_kind 默认、
    /// scope={Session, session_id}、mechanical_state={}、active=true，经既有
    /// upsert_instance——grounded: apply_patch 的 CreateObjectInstance 分支 L423），
    /// 再走既有 SetMechanicalState merge SQL（grounded: L425 update ... mechanical_state
    /// = coalesce(...) || $2）。返回 created（是否新建了实例——可观测）。
    pub async fn apply_external_mechanical_patch(
        &self,
        session_id: &str,
        object_id: &str,
        patch_json: serde_json::Value,
        reason: &str,
        world_tick: i64,
    ) -> anyhow::Result<bool>;
}
```

**EffectPatchIntent → 原语映射表（apply_effect_policy 内，全部真实签名已核）**：

| intent | 执行 | grounded |
|---|---|---|
| `ModifyTrack{owner_kind,owner_id,track_id,op,amount}` | `RefereeCombatService::new(engine.db.clone()).apply_direct_effect(&request.session_id, &request.ruleset_id, request.module_id.as_deref(), "scene.policy", target_actor, &path, op, amount, reason, Visibility::GmOnly)`；`path = format!("resources.{track_id}.current")`（对标 effect.rs `effect_parameter_path` L49-54，resolve_resource_track_id 可解析的唯一前缀形）；`target_actor = owner_id.as_deref().unwrap_or("pc.current")`；op 字符串→ParameterOperation 词表对标 effect.rs `op_from_str` L56-67（私有 → 提升 `pub(crate)` 复用，不复制粘贴）；产出 `ledger.record_effect(&outcome.effect)` + 逐条 `record_impact`（对标 ApplyEffectTool L82-83） | direct_effect.rs L84-96 |
| `SetObjectState{object_id,patch}` | `ObjectService::new(engine.db.clone()).apply_external_mechanical_patch(session, &object_id, patch, reason, world_tick)`；world_tick 取 `WorldTimeService::new(engine.db.clone()).ensure_session_time(session, None).await?.world_tick`；返回 patch 折为 `StatePatch::ObjectPatch{patch_id:"scene_policy_{uuid}", object_id:Some, patch_json, reason}` | trpg-object L400-427、trpg-time L166 |
| `CreateFact{target,fact}` | 折 `StatePatch::CreateFact{target,fact,reason}` 进返回集（持久化证据=整批收尾的 world_event；StatePatch::CreateFact 真实变体 model L2323） | model L2313-2328 |
| `StartCountdown{label,amount,scale,payload}` | `WorldTimeService::new(engine.db.clone()).schedule_in(session, time_amount, WorldEventKind::ClockTick, json!({"label":label,"intent_id":intent.intent_id,"payload":payload}), Visibility::GmOnly, None)`；scale→TimeAmount 复用 `crate::tools::world::parse_time_scale`（pub，L20-31）+ AdvanceTimeTool 同款构造映射（combat_round→combat_rounds / scene_beat→scene_beats / 其余→minutes，L59-63） | trpg-time L165-182 |
| `Other(v)` | 不执行不中断：折 `StatePatch::CreateFact{target:"scene.policy", fact:json!({"unexecutable_intent":v}), reason}` 进返回集（骨架契约：可观测） | — |

**check.rs 接线（RollCheckTool::call 内）**：
```rust
// args 解析后、kernel_dice 前：
let scene_intent: Option<SceneMechanicIntent> = match args.scene_mechanic_id.as_deref() {
    None => None,
    Some(id) => {
        let mid = ctx.request.module_id.as_deref().ok_or_else(|| ToolError::recoverable(
            "no_module_loaded", "scene_mechanic_id requires a module", None))?;
        let graph = ctx.engine.db.load_module_graph(mid).await?
            .ok_or_else(|| ToolError::recoverable("no_module_loaded", format!("module graph not loaded: {mid}"), None))?;
        Some(crate::scene_policy::find_scene_intent(&graph, ctx.state.scene_id.as_deref(), id)
            .ok_or_else(|| ToolError::recoverable("scene_mechanic_not_found",
                format!("scene mechanic not found in current scene: {id}"),
                Some("Check the BP2 scene-mechanics block, or roll without scene_mechanic_id.".to_string())))?
            .clone())
    }
};
// 契约构造后（build_check_contract_for_args 之后）：
if let Some(intent) = &scene_intent {
    if let Some(t) = crate::scene_policy::difficulty_to_target(intent.difficulty.as_ref()) { contract.target = t; }
    contract.advice_refs.push(format!("scene_mechanic:{}", intent.intent_id));   // 结构化引用进账（护栏 §3.5.1）
}
// execute_system_roll_bundle 之后、ToolOutput 之前：
if let Some(intent) = &scene_intent {
    match exec.primary.outcome.get("success").and_then(Value::as_bool) {
        Some(success) => {
            let patches = crate::scene_policy::apply_effect_policy(ctx.engine, ctx.request, intent, success, ledger).await?;
            /* 结果 JSON 增 "executed_patches": 摘要数组（op/target/数额；含 unexecutable 条目）*/
        }
        None => { /* fail-closed：outcome 无 success 布尔 → 不执行 policy，结果 JSON 标
                     "effect_policy_skipped":"outcome has no success field"（可观测不静默）*/ }
    }
}
```
- `// grounded:` `exec.primary.outcome` 是 Value 且 success 为 Bool（mechanics 合成 outcome `json!({"success":...})` 先例 direct_effect.rs L78；ledger 测试同假设 L114-117）；`ctx.state.scene_id` 真实存在（world.rs L93 同用法）；`load_module_graph` 是"当前 module bundle 单一事实源"（trpg-db L450-457 注释）——P5 续抽后的 intents 运行时可见。
- **tested_parameter 继承语义**：schema 里 tested_parameter 维持 required（缓存敏感面最小：B2/C4 对 schema 的字段追加各只发生一次）；显式 args 优先——args.tested_parameter 与 intent.tested_parameter 不一致时**用 args 值**并在结果 JSON 标 `"parameter_overridden_from_intent": true`（目录/意图是知识不是枷锁，且可观测）。

### 测试清单（trpg-gm 单测，纯函数 + 解析层；DB 路径归 C6 e2e）
1. `select_intents_picks_branch`：on_success 2 条/on_failure 1 条 → success=true 返回 2 条、false 返回 1 条。
2. `difficulty_to_target_maps_dv_and_static`：`{"kind":"dv","value":13}` → `StaticNumber{value:13,..}`；`{"kind":"target_number","value":50}` → StaticNumber；`"hard"`（裸字符串）→ None；`{"kind":"dv","value":"thirteen"}`（非数值）→ None。
3. `find_scene_intent_resolves_current_scene_then_falls_back`：graph 两场景（sc1 DeepExtracted 带 intent_a、sc2 带 intent_b）——scene_id=Some("sc2") 查 intent_b 命中；scene_id=None 查 intent_a 命中（回退首个 DeepExtracted）；scene_id=Some("sc2") 查 intent_a → None（**不跨场景兜底**：当前场景没有就是没有，防把别处的 effect_policy 错绑）。
4. `roll_args_accept_scene_mechanic_id`：`parse_roll_check_args(json!({...,"scene_mechanic_id":"x"}))` → 字段就位；不给 → None（向后兼容）。
5. `other_intent_folds_to_unexecutable_fact`：构造含 Other 的 policy → （把 Other 折叠逻辑抽成 PURE `fold_unexecutable(intent,reason)->StatePatch` 一并测）返回 CreateFact 且 fact 含原 JSON。
6. `schema_stability_tests::schema_serialization_is_stable`（既有，tools/mod.rs L243-248）照绿——确认 schema 改动是确定性的。

### 实现要点
- **执行点选在工具层（roll_check 内）而非 mechanics after_check_resolved 内**：execute_system_roll_bundle 深处拿不到 ModuleGraph/intent（mechanics crate 不依赖模组图谱，引依赖会反向耦合）；spec §6 的"在 after_check_resolved 链路强制执行"语义=「结算完成后由 Rust 立即执行、不经叙事」，工具层紧跟 exec 满足之。与 B 同装时 ModifyTrack 走 apply_direct_effect → B4 在该单点的 watcher 检测自然覆盖（骨架 Architecture 承诺）。
- **world_event 证据单点**：apply_effect_policy 收尾 `engine.record_world_event(session, Some(turn_id), None, WorldEventKind::EffectApplied, json!({"source":"scene.policy","intent_id":...,"success":...,"patches":[...]}), Visibility::GmOnly)`（grounded: runtime L155 包装 + WorldEventKind::EffectApplied 真实变体 L3266-3272）——C6 验收 9 的 SQL 锚点，比散查三张表稳。
- **借用纪律**：apply_effect_policy 收 `&mut TurnLedger` 与 `&RuntimeEngine`——调用处 ledger 本来就是 `&mut`（dispatch 链），无新借用冲突；不要把 ObjectService/WorldTimeService 存成字段，每次 `new(engine.db.clone())`（Db 是 clone-cheap pool 包装，全代码库同款用法）。
- **scene_policy.rs 守 400 行**：映射表五分支各 ≤15 行；超了把 ObjectService 调用与 world_tick 获取折进 trpg-object 的新方法侧。
- 错误码 `scene_mechanic_not_found` recoverable=true——agent 改道：不带 id 自由 roll_check 或 request_player_roll（hint 写明）。
- **不缩水检查**：spec §6 词表四原语全映射（set_object_state/modify_track/create_fact/start_countdown）+ Other 可观测，一个不少；effect 落账（EffectContract/ParameterImpact 经 ledger，world_event 经 db）齐活。

### 验证
```bash
cd crates/trpg-gm && cargo test -p trpg-gm          # 全绿（既有 schema/ledger/turn_loop 测试照绿 + 新 6 测）
cd ../trpg-object && cargo test -p trpg-object       # 既有测试照绿（新方法编译过即可，DB 行为归 e2e）
wc -l crates/trpg-gm/src/scene_policy.rs             # ≤400
cargo build -p trpg-cli                              # 全链编译
```

---


---

# Section C（续）—— e2e 黄金链任务正文（C5–C7）

> 接 `section_C.md`（C1–C4）；隶属计划骨架 `docs/superpowers/plans/2026-06-10-rule-aware-gm.md`，权威 spec `docs/superpowers/specs/2026-06-10-rule-aware-gm-design.md`（§8 共 14 条验收 + 附录 A 137 条审计基线）。
> 本文件三任务全部是 **e2e 验收任务**：原则上零代码改动——发现 bug 回所属实现任务（A/B/C1-C4）修复后重跑，**不顺手打补丁**；唯一例外 C7① 允许新建一个 `#[ignore]` 集成测试文件。
> e2e 环境总备忘（端口 swap / TRPG_DATA_DIR / docker psql / play --agent 管道 / 库内模组清单）见 `section_C.md` 头部；本文件命令可直接照抄，全部在 workspace 根执行。所有 `// grounded:` 锚点已对照真实源码/迁移核实（2026-06-10 工作副本）。

**共用准备（每个 shell 会话先粘贴这一段）**：
```bash
cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
export DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg   # .env 写的是 :54346（赛博库）——swap 坑，显式 export 必赢
export TRPG_DATA_DIR="$PWD/data"
PSQL() { docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg -tAc "$1"; }   # 宿主机无 psql，一律 docker exec
cargo build -p trpg-cli 2>&1 | tail -2     # 预热编译，防首回合计时掺编译时间
# 行级时间戳跑批：stdout 每行打相对秒（TTFT/回合时长都从这份 log 读）；macOS 无 timeout(1)，超时交给执行工具参数
run_play() {  # $1=ruleset_id  $2=module_id  $3=输入文件  $4=日志前缀
  RUST_LOG=gm_cache=info,info cargo run -q -p trpg-cli -- play --ruleset "$1" --module "$2" --agent \
    < "$3" 2>"$4.stderr.log" | python3 -u -c '
import sys,time; t0=time.time()
for line in sys.stdin: print(f"[{time.time()-t0:7.2f}s] {line}",end="",flush=True)' | tee "$4.stdout.log"
}
sid_of() { grep -m1 'agent session:' "$1.stdout.log" | sed 's/.*agent session: //' | tr -d '[:space:]'; }
```
- `// grounded:` `play --agent` 走 GmLoop（main.rs L530 `Commands::Play{ruleset,module,agent}`）；session_id 打印格式 `agent session: <id>`（agent_play.rs L33）；**`trpg turn` 走旧路径 run_turn_once 不经 GmLoop，e2e 一律用 play --agent**。
- `// grounded:` gm_cache tracing 每工具轮一行，字段 prefix_hash/pinned_hash/request_prefix_hash/tool_round（turn_loop.rs L70-80）；回合收尾 context_hashes 持久化进 `turns` 表（turn_loop.rs L169 + trpg-db save_turn L986：`turns(session_id,turn_id,user_input,assistant_output,context_hashes,...)`）。

**计时与 TTFT 记录方式（三链统一，记进各任务执行记录）**：
- TTFT：`*.stdout.log` 中「输入回显行（上一个 `[chatrpg:agent]>` 提示行之后）」到「本回合第一行叙事字节」的相对秒差；回合总时长：到「下一个提示符行」的差。
- 工具轮数：`grep 'gm agent prompt cache anchors' *.stderr.log` 按 turn_id 分组计数（tool_round 字段）——债务门控回填会体现为额外轮。
- 对照基线：B8 收口冒烟记录的无债回合时长/轮数（spec §9 风险 4 的实测回答）。记录表模板：`| turn | tool_rounds | TTFT(s) | total(s) | 备注（due/waive/effect_policy 发生否）|`。

**chatrpg-product-evaluator 对照说明（三链统一）**：每条链 SQL 断言全过之后，可调 `chatrpg-product-evaluator` skill 以产品视角复评该把 transcript（GM 是否把机制叙事化得自然：SAN 链恐怖节奏、band 分档张力、池子高位的基调变化）。其结论**只作叙事质量的辅助证据**（验收 10"叙事反映池状态"、12③"叙事分档"两处判定参考）；机械断言一律以 SQL/账本为准——evaluator 不通过不阻塞机械验收，但结论必须记进执行记录与 C7 对账表备注。

---

## C5. e2e 黄金链一：CoC SAN→疯狂全链 + 跳坑感知 + 成功度三件套（验收 7/8/12）

### 范围与前置
- 真 :54347 DB + 真 LLM（relay 既有配置，loop 模型不动）+ 规则集 `call_of_cthulhu_7e` / 模组 `call_of_cthulhu_7e.document`（血色公路，已在库）。
- 前置硬依赖：**A7**（CoC 目录已编译入活动 kernel）、**B8**（Slice B 收口冒烟过）、**C1–C4 已装**。任一预检不过 → 回对应任务修，本任务不开跑。
- 零代码任务；产出 = 执行记录段落（计时表 + SQL 证据原文 + 把数与 log 文件名），供 C7 对账表逐条引用。

### 预检（照抄；任一为空/不符 → 停，回 A4/A7）
```bash
# ① 目录非空（A7 产物）
PSQL "select jsonb_array_length(content_json->'mechanics_catalog') from rule_kernels where ruleset_id='call_of_cthulhu_7e' and active=true"
# ② sanity 轨阈值带 followup_procedure_id（A4 写回）；同时记下轨 id、loss_in_one_go 阈值 N、tested_parameter 绑定
PSQL "select t->>'id', jsonb_pretty(t->'thresholds') from rule_kernels, jsonb_array_elements(content_json->'resource_tracks') t where ruleset_id='call_of_cthulhu_7e' and active=true and t->>'id' ilike '%san%'"
# ③ success_bands 含 critical/failure 且 semantics 非空（验收 12① 复检；主验已在 A7，缺 → 回 A4）
PSQL "select b->>'id', coalesce(b->>'semantics','<NULL>') from rule_kernels, jsonb_array_elements(content_json->'dice_core'->'success_bands') b where ruleset_id='call_of_cthulhu_7e' and active=true"
# ④ 验收 8 的"答案钥匙"：拉目录全量 → **语义判定**跳跃条目（禁 ILIKE 关键词筛——人或审计 subagent 读 when_to_use 语义选取），
#    记 JUMP_ID 与 JUMP_PARAM 备用（技能名由此从目录解析得出，绝不写死进断言——护栏 §3.5）
PSQL "select e->>'id', e->>'tested_parameter', e->>'when_to_use' from rule_kernels, jsonb_array_elements(content_json->'mechanics_catalog') e where ruleset_id='call_of_cthulhu_7e' and active=true"
```

### ① SAN→疯狂全链（验收 7）
```bash
cat > /tmp/c5_san.txt <<'EOF'
我推开太平间冷柜，凑近看清那具不该存在的尸体的脸
我深呼吸，强迫自己继续检查尸体上的伤痕
/quit
EOF
run_play call_of_cthulhu_7e call_of_cthulhu_7e.document /tmp/c5_san.txt c5_san_run1
SID=$(sid_of c5_san_run1)
```
- 输入只给恐怖刺激，**绝不出现"检定/SAN/理智/roll"字样**——SAN check 必须由目录 when_to_use 语义驱动放出（验收 7"非玩家明示"）。第二行输入是 agent 处理 due 的窗口（due 也可能当回合就被债务门控回填逼着处理——两种时序都合法）。
- **非确定性纪律**：全链要求「SAN check 失败且单次损失 ≥ 预检②的阈值 N」。真骰随机 → 最多重跑 5 把（c5_san_run2…，每把新 session），任一把命中即取该把做终验；5 把全未命中 → 记录各把实际 loss 值，验收 7 的 due 半边以 B4 单测 + 下方 1)2) 半链 SQL 为证据，C7 对账表标「e2e 全链未观测（概率未命中）」。**禁止**直改 DB 凑数——绕开 watcher 结算单点 = 假证据。

终验 SQL（命中把跑全部；未命中把跑 1)2) 半链）：
```bash
# 1) SAN check 契约存在，tested_parameter 与预检②记下的 sanity 轨绑定参数比对（变量对账，不写死字面）
PSQL "select check_id, contract_json->>'tested_parameter', status, created_at from check_contracts where session_id='$SID' order by created_at"
# 2) SAN current 真实下降 ≥N（current 单一存储 generic_parameter_states）
PSQL "select target_id, parameter_path, value_json, world_tick, updated_at from generic_parameter_states where session_id='$SID' order by updated_at"
# 3) watcher due（B4 / 迁移 0027）：source='threshold'、evidence 含 before/after/delta、followup_procedure_id 指向预检②的疯狂条目
PSQL "select due_id, source, source_track, followup_procedure_id, evidence, status, waive_reason, waive_scope from mechanic_dues where session_id='$SID' order by created_at"
# 4) due 去向二选一（验收 7 收口）：
#    a. 疯狂检定：后续契约 advice_refs 含 mechanic:{followup_procedure_id}（或 tested_parameter 与该条目绑定参数一致）且 due status='resolved'
PSQL "select check_id, contract_json->'advice_refs', contract_json->>'tested_parameter' from check_contracts where session_id='$SID' order by created_at"
#    b. waive：due status='waived' 且 waive_reason 非空 + 勘误记忆 gm_waive 落账（B6 副作用三连）
PSQL "select summary, tags from memory_events where session_id='$SID' and tags @> array['gm_waive']"
```

### ② 跳坑感知（验收 8）
```bash
cat > /tmp/c5_jump.txt <<'EOF'
走廊地板塌出一道两米宽的裂隙，我后退几步助跑跳过去
/quit
EOF
run_play call_of_cthulhu_7e call_of_cthulhu_7e.document /tmp/c5_jump.txt c5_jump_run1
SID2=$(sid_of c5_jump_run1)
PSQL "select contract_json->>'tested_parameter', contract_json->'advice_refs' from check_contracts where session_id='$SID2' order by created_at"
```
- 断言（护栏 §3.5——技能名来自目录非测试硬编码）：契约存在（玩家没提检定，**契约存在本身就是"感知"的证据**）且 `tested_parameter == JUMP_PARAM`（预检④语义选出）。advice_refs 含 `mechanic:$JUMP_ID` 为加分项非必须——目录是知识不是枷锁，agent 不带 mechanic_id 但参数一致同样通过。
- 失败分型（指回所属任务，不在本任务修）：无契约 = 背景板回潮 → 查 BP1 索引是否注入（stderr gm_cache 行存在性 + B1 单测）与 gm_skill 40 号准则装载（B8）；契约参数错绑 → 查 A7 目录该条目 tested_parameter 与 A3 finalize 校验。

### ③ 成功度三件套（验收 12②③；12① 主验在 A7、预检③复检）
```bash
# ② 机械触发定性：CoC kernel 是否真解析出 band 触发 / max_of（A4 写回 + A5 读取点）
PSQL "select t->>'id', o->'trigger', o->>'amount' from rule_kernels, jsonb_array_elements(content_json->'resource_tracks') t, jsonb_array_elements(t->'on_outcome') o where ruleset_id='call_of_cthulhu_7e' and active=true and (o->'trigger'->>'kind'='band' or coalesce(o->>'amount','') like 'max_of:%')"
```
- **解析出** → 机械路径证据二选一：(a) 本任务各把里真碰到 fumble（`PSQL "select check_id, outcome from check_results where session_id in ('$SID','$SID2')"` 看 band 字段）→ 对照 generic_parameter_states 前后值验损失 = 骰式最大值；(b) fumble ~5% **不强等**——以 A5 单测（max_of 纯函数 + band trigger 结算路径）+ 上面 SQL 的真 kernel 数据为证据，记录「e2e 未观测 fumble，机械路径由 A5 单测 + kernel 真数据保障」。
- **未解析出** → 记**目录缺口**：查 `PSQL "select validation_report from rule_kernels where ruleset_id='call_of_cthulhu_7e' and active=true"` 应有对应记录；C7 对账表 12② 标「缺口可观测」——**不许静默通过**（spec §8.12 原文要求）。
- ③ 语义投影双面取证：
  - 落库面：`check_results.outcome` 的 band/success_tier 字段（band 真实发生的证据）；工具结果 JSON 的 band_semantics 行**不落库**，其渲染正确性由 B3 单测保障——e2e 不重复验函数。
  - 叙事面：对照 `*.stdout.log` transcript——extreme 把与 regular 把的叙事分档明显（贯穿/卓越效果 vs 普通成功），evaluator 对照辅助判定；两个不同 band 的把数不足时多跑 1-2 把凑齐对照面（每把都查 check_results 记 band）。

### 验证（任务收口自查）
- 验收 7：四步 SQL 证据齐（或半链 + 概率未命中记录）；验收 8：参数对账通过；验收 12②③：机械定性结论 + 双面取证记录在案。
- 计时表（头部模板）必交：每把 TTFT / 回合时长 / 工具轮数，与 B8 基线对比，债务处理多出的轮数/秒数单独标注。
- 所有 SQL 输出原文 + log 文件名整理成执行记录段落（C7 对账表的 7/8/12 行直接引用）。

---

## C6. e2e 黄金链二：Homecoming 场景意图 + Triangle chaos 累积感知（验收 9/10）

### 范围与前置
- 验收 9（Homecoming）只需 **C1–C4**（effect_policy 执行复用一期原语，不依赖 A/B）；验收 10（Triangle）还需 **A7**（Triangle 目录含 chaos 条目）+ **B3**（track 语义投影 + owner_kind 通用）+ **B4**（落账单点 watcher——chaos 是 scene 级实证）。
- 模组现状（section_C.md 备忘⑥）：Triangle `triangle_agency.the_vault` 已在库；**Homecoming 库里无 bundle**（只有 `data/modules/CPR One Shot - Homecoming ver3.0 (Colored).pdf` + `data/markdown/modules/cpr_one_shot_homecoming_ver3_0_colored.md`），先 parse。
- 零代码任务；ruleset_id / module_id / 对象 id / chaos 轨 id **全部实查 SQL 得出，不在命令里硬编码**（下文 `<占位>` 执行时替换）。

### ① Homecoming 场景意图（验收 9：effect_policy 强制执行非叙事声明）

**入库与重抽（照抄）**：
```bash
# 1) 实查 CPR 规则集 id（命名以库为准）
PSQL "select ruleset_id from rule_kernels where active=true"
# 2) Homecoming 入库：parse-all 扫 data/ 全量，规则书/已入库模组 cached 跳过，净增量只有 Homecoming
cargo run -q -p trpg-cli -- parse-all 2>&1 | tail -20
# 3) 实查模组 id（含 Homecoming 的新行；下文记为 <HC_MID>）
PSQL "select content_json->>'module_id' from parsed_bundles where bundle_kind='module'"
#    若此前抽过旧版（无 scene_mechanics 的深抽）→ 删 bundle 强制重抽（既有 e2e 流程：规则书 cached 只重抽模组）：
#    PSQL "delete from parsed_bundles where bundle_kind='module' and content_json->>'module_id'='<HC_MID>'" && cargo run -q -p trpg-cli -- parse-all
# 4) 预检：图谱存在带 effect_policy 的切线缆类 intent（C2 深抽产物）。
#    懒抽架构下该场景 parse 时可能仍 SkeletonOnly（intents 只在深抽时产出）——本查询为空且场景未深抽 ≠ 失败，
#    play 导航到场深抽后再查（load_module_graph 读当前 bundle 单一事实源，到场深抽产物运行时可见——grounded: trpg-db L450-460）。
PSQL "select n->>'node_id', n->>'extraction_status', jsonb_pretty(n->'scene_mechanics') from parsed_bundles, jsonb_array_elements(content_json->'module_graph'->'scenes') n where bundle_kind='module' and content_json->>'module_id'='<HC_MID>' and jsonb_array_length(coalesce(n->'scene_mechanics','[]'::jsonb))>0"
#    记下目标 INTENT_ID、其 tested_parameter/difficulty、effect_policy 内的 object_id（如 athena_cable 类）与各分支条目数
```
- 深抽后仍无任何 intent（含目标场景已 DeepExtracted）→ 回 C2（DEEP_SYS 指引/解析过滤）排查；模组原文真没写明检定则换含明确 DV 检定的场景做锚（以模组 markdown 原文为准），**不许放宽 source_anchor 收口来凑条目**。

**play 到场并触发**：
```bash
cat > /tmp/c6_cable.txt <<'EOF'
我们沿着任务简报的路线，直接摸到执法者据点后侧的设备井
我抄起断线钳，全力剪断那根主电缆
让我看看周围有什么动静
/quit
EOF
run_play <CPR_RULESET_ID> <HC_MID> /tmp/c6_cable.txt c6_cable_run1
SID=$(sid_of c6_cable_run1)
```
- 输入按当把实际开场调整（线性模组 1-2 回合可导航到场；scene_navigator 语义切换 + 到场深抽自动发生）。第二行行动**语义对应 intent description，不提检定/DV/技能名**；第三行留给倒计时/后果的观察回合。
- 重跑纪律同 C5（≤5 把）；**失败把不白跑**：on_failure 分支真实执行同样是验收 9 证据（成功/失败各验各的分支集合）。

**终验 SQL**：
```bash
# 1) 契约带结构化引用（C4 写入 advice_refs）：scene_mechanic:{INTENT_ID}
PSQL "select check_id, contract_json->'advice_refs', contract_json->>'tested_parameter', status from check_contracts where session_id='$SID' order by created_at"
# 2) C4 证据单点：world_event kind=EffectApplied、event_json.source='scene.policy'（含 success 与 patches 摘要）
PSQL "select event_kind, jsonb_pretty(event_json) from world_events where session_id='$SID' and event_json->>'source'='scene.policy'"
# 3) 对象态真实落库（object_id 用预检 4) 从 effect_policy 读出的值，不硬编码）：mechanical_state 含 patch 内容
PSQL "select object_id, jsonb_pretty(mechanical_state), active from object_instances where session_id='$SID'"
# 4) 倒计时真实落库（StartCountdown → scheduled_events pending，payload 含 intent_id）
PSQL "select scheduled_event_id, due_tick, event_kind, payload_json, status from scheduled_events where session_id='$SID' order by created_at"
# 5) ModifyTrack 类条目（若该 intent 有）：generic_parameter_states 对应 path 变化
PSQL "select target_kind, target_id, parameter_path, value_json from generic_parameter_states where session_id='$SID'"
```
- **判定**：实际走到的分支（成功→on_success / 失败→on_failure）的 EffectPatchIntent **逐条**对得上落库证据——几条 intent 就几条证据行（SetObjectState→3)、StartCountdown→4)、ModifyTrack→5)、CreateFact/Other→2) 的 patches 摘要含 unexecutable_intent）。**叙事里声称的效果在库里没有对应行 = 验收 9 失败**（效果留给了叙事）。难度继承核对：契约 target 与 intent.difficulty 一致（contract_json 里看 dv/static 值）。

### ② Triangle chaos 累积感知（验收 10）

**预检**：
```bash
# 1) Triangle 规则集 id 实查（同上 rule_kernels）；记 <TRI_RID>
# 2) 目录含 chaos 花费/异常体条目（A7 产物；kind=spend/subsystem_procedure）——语义判定哪几条是，记 id
PSQL "select e->>'id', e->>'kind', e->>'when_to_use' from rule_kernels, jsonb_array_elements(content_json->'mechanics_catalog') e where ruleset_id='<TRI_RID>' and active=true"
# 3) chaos 轨真实 id 与 owner_kind（应为 scene 级——B3/B4 的 owner_kind 通用通路实证面）；记 <CHAOS_ID>
PSQL "select t->>'id', t->>'owner_kind', t->'thresholds' is not null, t->>'zero_means' from rule_kernels, jsonb_array_elements(content_json->'resource_tracks') t where ruleset_id='<TRI_RID>' and active=true"
```

**play ≥3 回合**（每回合行动自然引发检定——Triangle 每掷 6d4 非 3 面进池，池子必然递增）：
```bash
cat > /tmp/c6_chaos.txt <<'EOF'
我用能力扫描金库大厅里的异常痕迹
我撬开通风管道钻进去，朝核心区摸过去
我直接对守卫使出我的异常能力
事情闹这么大了，我环顾四周看看现实出了什么问题
/quit
EOF
run_play <TRI_RID> triangle_agency.the_vault /tmp/c6_chaos.txt c6_chaos_run1
SID2=$(sid_of c6_chaos_run1)
```

**终验**：
```bash
# 1) chaos 池递增可查（scene 级通路）：target_kind 与 kernel owner_kind 一致、value 随 updated_at 单调不减且 > 初值
PSQL "select target_kind, target_id, parameter_path, value_json, updated_at from generic_parameter_states where session_id='$SID2' and parameter_path like '%<CHAOS_ID>%' order by updated_at"
# 2) 检定真实发生 ≥3 次（池子来源）：
PSQL "select count(*) from check_results r join check_contracts c on r.check_id=c.check_id where c.session_id='$SID2'"
# 3) 池子高位的 due/账面反应（若 kernel chaos 轨有 thresholds → B4 应产 scene 级 due）：
PSQL "select due_id, source_track, owner_kind, owner_id, evidence, status from mechanic_dues where session_id='$SID2' order by created_at"
# 4) 叙事/行动反映池状态（验收 10"累计系统真的被记得并加强影响"）——transcript 落库面：
PSQL "select turn_id, left(assistant_output, 300) from turns where session_id='$SID2' order by created_at"
```
- **判定三层**：
  1. **结算落账**：1) 递增 + 2) 检定计数 ≥3（机械事实，硬断言）；
  2. **语义可见**：BP3 语义状态行（B3 `track_semantic_line`：当前值所处 thresholds 区间 consequence / zero_means）——BP3 全文不持久化，函数面由 B3 单测保障；e2e 行为面证据 = 后段回合叙事/工具调用体现池状态知识（4) 的 transcript + evaluator 对照判定）。kernel 无 thresholds/zero_means 可渲染 → 裸数值是正确的 fail-closed 行为，记录之，不算失败；
  3. **联动可触发**：池子高位时 agent 行动或叙事反映（GM 替异常体花池/现实扭曲基调/due 处理）——凭 3) 账面 + 4) 叙事证据综合判定，evaluator 结论记备注。
- 背景板防治三件套（护栏 §3.5.6）齐活才算验收 10 通过；缺哪件标哪件，指回所属任务（落账→B4、可见→B3、联动→B5/B6）。

### 验证（任务收口自查）
- 验收 9：分支集合逐条对账表（intent 条目 ↔ SQL 证据行）+ 成功/失败至少各观测一种分支（把数允许内）；验收 10：三件套各自证据齐。
- 计时表照 C5 模板记录（Homecoming 含到场深抽的把，深抽耗时单独标注，不混进回合时长基线）。
- SQL 输出原文 + log 文件名整理成执行记录段落（C7 对账表 9/10 行直接引用）。

---

## C7. e2e 黄金链三：追溯债务闭环 + 缓存回归 + spec 14 条验收逐条对账收口（验收 11/13 + 总对账）

### 范围与前置
- 前置：**B6/B7**（ObligationLedger/RetroactiveEffectDebt/referenced_ledger_ids）——验收 11；**B1 + C3**（BP1 索引/BP2 intents 块）——验收 13；**全部任务 A1–C6 完成**——总对账才有意义。本任务是整个二期的**最后收口闸门**。
- 唯一允许的新文件：`crates/trpg-gm/tests/retro_debt_e2e.rs`（`#[ignore]` 集成测试，真 DB + MockLlm 脚本——验收 11 的"叙事声称伤害但未调工具"靠真 LLM 不可复现，必须脚本可控；spec §8.11 原文即"MockLlm 脚本"）。
- 产出：总验收报告（中文）落 `docs/规则感知GM二期验收报告_2026-06-10.md`，核心是下方对账表执行时逐条填实。

### ① 追溯债务闭环（验收 11，MockLlm 脚本可控 + 真库状态变化）

**Create** `crates/trpg-gm/tests/retro_debt_e2e.rs`（`#[ignore]`，运行需 `DATABASE_URL` 指 :54347；对标 turn_loop_tests.rs 的 MockLlm/GmLoop 装配样板，但 engine 用真 Db——`Db::connect(env DATABASE_URL)` + `migrate()`，session 经 `engine.start_session` 真 bootstrap，绝不自造 session_id）：
```rust
// 测试是规格——两回合脚本：
// 回合 1：MockLlm 不调任何工具，直接叙事「子弹擦过你的肩膀，你掉了 3 点生命」
//   断言：verify_after_stream 后 GmLoop 持久字段含 1 条 RetroactiveEffectDebt
//   （finding_detail 含 InventedEffect 的 detail；grounded: B6 契约 §5 + B7 的
//   referenced_ledger_ids 传账本 id 全集——本回合账本为空 → 子串扫描回退路径，
//   finding detail 应带 "fallback:substring_scan" 前缀，一并断言=B7 可观测技术债半边）。
// 回合 2：MockLlm 脚本三步：
//   step1 收到 BP3/障碍观察（断言 messages 含 obligations_block 或债务回填文本，
//          block_text 里能看到 debt_id 与 finding 摘要）；
//   step2 调 apply_effect（HP -3，target=测试角色）补落账 → 债务清除；
//   step3 正常叙事收尾。
//   断言：ledger.blocking() 为空 → 进叙事轮（narrated）；
// 库面终验（测试内 sqlx 直查或测试后手动 PSQL）：
//   generic_parameter_states 该角色 HP path 的 value 真实 -3（验收 11 的"库中状态真实变化"）。
```
```bash
# 跑法（ignored 测试显式点名；cwd 在 crate 目录——cargo test -p 的既有坑）：
cd crates/trpg-gm && DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg cargo test -p trpg-gm --test retro_debt_e2e -- --ignored --nocapture
# 库面复核（拿测试打印的 session_id）：
PSQL "select target_id, parameter_path, value_json from generic_parameter_states where session_id='<测试打印的SID>'"
```
- 边界：MockLlm 脚本与断言**不得**依赖勘误记忆的具体措辞（B6 实现细节），只断言结构面（债务存在/回填发生/债务清除/库值变化）。测试文件 ≤400 行。
- waive 出口对照（验收 11 的或然分支）：再加一个用例——回合 2 脚本改调 `waive_obligation(debt_id, reason="叙事中已收回该说法")` → blocking 清空 + memory_events tags 含 gm_waive（库面 PSQL 复核）。

### ② 缓存回归 e2e 半边（验收 13；单测半边在 B1，函数级在 C3 测 5）

```bash
# 同场景连跑 3 回合（输入选纯对话/观察类，不触发 navigate_scene/advance_time——场景与时间不动才是对照面）：
cat > /tmp/c7_cache.txt <<'EOF'
我环顾四周，把现场细节再确认一遍
我蹲下来仔细看地上的痕迹
我把看到的一切在脑子里过一遍
/quit
EOF
run_play call_of_cthulhu_7e call_of_cthulhu_7e.document /tmp/c7_cache.txt c7_cache_run1
SID=$(sid_of c7_cache_run1)
# 证据面 1（tracing）：三回合所有工具轮的三个 hash 各自恒等
grep 'gm agent prompt cache anchors' c7_cache_run1.stderr.log | grep -oE '(prefix_hash|pinned_hash|request_prefix_hash)=[^ ]+' | sort | uniq -c
#   判定：prefix_hash / pinned_hash / request_prefix_hash 各只出现 1 个值（计数=轮数总和）
# 证据面 2（落库，比 tracing 稳）：turns.context_hashes 三行逐字段相等
PSQL "select turn_id, context_hashes from turns where session_id='$SID' order by created_at"
```
- **前提断言**（hash 恒等才有意义）：本把 BP1 真含目录索引、BP2 真含 intents 块——
  - BP1 半边：B1 单测已断言索引块在 Prefix；e2e 行为面 = C5② 里 agent 能感知目录（契约产生）即 BP1 注入的行为证据；
  - BP2 半边：血色公路当前场景若无 scene_mechanics 则 BP2 无 intents 块（fail-closed 正确行为）——此时换 Homecoming 场景（C6① 的 `<HC_MID>`，到含 intent 场景后）重复本节三回合跑批，两个 hash 对照面都要留档；
  - 反向对照（可选加强）：navigate 一次后 pinned_hash **应当**变（SceneStable 语义=场景切换换块），变了反而是对的——记录之，防"恒等"断言被误读为"永不变"。
- 任一 hash 漂移 → 回 B1（索引块字节不稳/PM 档投影把动态值渲染进了 Prefix）或 C3（intents 行序/截断不稳）修复后重跑。

### ③ spec §8 全 14 条验收逐条对账（总报告核心；执行时逐条填"证据/状态"两列）

> 状态词表：**通过** / **通过（带缺口）**（fail-closed 正确退化或概率未观测，备注写明） / **未通过**（指回任务） / **N/A**（写理由）。证据列必须是可复查的实物：测试名、SQL 原文输出、log 文件名、报告路径——**不接受"已实现"三个字**。

| # | spec §8 验收 | 实现位置（任务/锚点） | 测试/SQL 证据（执行时填实物） | 状态 |
|---|---|---|---|---|
| 1 | 六套规则目录生成 + validation_report 记录丢弃及原因 | A2/A3/A6 → A7 跑批 | A7 审计报告路径 + `select ruleset_id, jsonb_array_length(content_json->'mechanics_catalog') from rule_kernels where active=true`（6 行非空） | |
| 2 | CoC sanity_check/temporary_insanity followup 链通 + jump 条目带 when_to_use | A4/A7 | A7 断言记录 + C5 预检②④ SQL 原文 | |
| 3 | 坏条目护栏（tested_parameter 不存在 → 丢弃且报告） | A3 | `cargo test -p trpg-rule-agent` 坏条目注入测试名 | |
| 3a | 覆盖率审计 vs 附录 A 137 条 + 每套 ≥1 非预设类别 + ≥1 纯语义 + ≥1 EngineHook 条目 | A7 | 审计报告（缺失项须在 validation_report 有记录）+ 抽查条目 id 清单 | |
| 4 | watcher 单测（SAN -6→due/-4→无/HP 穿 0→due/无 followup 仍发） | B4 | `cargo test -p trpg-mechanics` 测试名×4 | |
| 5 | 债务门控单测（due 不清不进叙事轮/waive 放行+落账/轮耗尽进下回合 BP3） | B6 | `cargo test -p trpg-gm` 测试名×3 | |
| 6 | lookup_mechanic / roll_check(mechanic_id) 继承绑定 | B2 | `cargo test -p trpg-gm` 测试名 + schema_stability 照绿 | |
| 7 | e2e CoC SAN→疯狂全链 SQL 可查 | C5① | C5 终验 SQL 四步原文（或半链+未观测记录） | |
| 8 | e2e 跳坑感知（tested_parameter 与目录条目一致，不写死技能名） | C5② | JUMP_ID/JUMP_PARAM 语义选取记录 + 契约 SQL 原文 | |
| 9 | e2e Homecoming 切线缆 effect_policy 强制执行落库 | C6①（实现 C2/C3/C4） | world_events(scene.policy)/object_instances/scheduled_events SQL 原文 + 分支逐条对账表 | |
| 10 | e2e Triangle chaos 累积+语义投影+行为反映（三件套） | C6②（实现 A7/B3/B4） | 递增 SQL + transcript 摘录 + evaluator 结论备注 | |
| 11 | e2e 追溯债务闭环（InventedEffect→债务→补 apply_effect→库变化） | C7①（实现 B6/B7） | retro_debt_e2e 测试名×2 + 库面 PSQL 原文 | |
| 12 | 成功度三件套（①band 补全护栏 ②fumble max_of 机械落库 ③语义投影分档） | ①A4 ②A5 ③B3 + C5③ | ①A7 报告+C5 预检③ ②定性 SQL+（A5 测试名 或 fumble 实测）③check_results band+叙事对照 | |
| 13 | 缓存回归（目录注入后跨回合 prefix/pinned hash 不变） | B1（单测）+ C7②（e2e） | B1 测试名 + c7_cache uniq 输出 + turns.context_hashes SQL 原文 | |
| 工程 | 文件 ≤400 行 / 零 per-ruleset 硬编码 / 新结构全 `#[serde(default)]` | 全任务 | 下方三查命令输出 | |

**工程约束三查（照抄，输出贴进报告）**：
```bash
# ① 新文件行数（File Structure 的 Create 清单全列 + 本 section 新增的 scene_mechanics.rs/retro_debt_e2e.rs）
wc -l crates/trpg-model/src/mechanics.rs crates/trpg-rule-agent/src/reader/mechanics_compile.rs \
  crates/trpg-rule-agent/src/reader/mechanics_finalize.rs crates/trpg-rule-agent/src/bin/mechanics_proto.rs \
  crates/trpg-rule-agent/src/reader/scene_mechanics.rs crates/trpg-mechanics/src/watcher.rs \
  crates/trpg-gm/src/obligations.rs crates/trpg-gm/src/scene_policy.rs crates/trpg-gm/src/tools/mechanic.rs \
  crates/trpg-gm/tests/retro_debt_e2e.rs    # 全部 ≤400
# ② 零 per-ruleset 硬编码（命中只允许出现在测试 fixture/注释/文档；代码分支命中=未通过）
grep -rn -iE "call_of_cthulhu|cthulhu|cyberpunk|triangle_agency|sword_world|dnd|d&d|coc[^a-z]" \
  crates/trpg-model/src/mechanics.rs crates/trpg-rule-agent/src/reader/mechanics_*.rs \
  crates/trpg-rule-agent/src/reader/scene_mechanics.rs crates/trpg-mechanics/src/watcher.rs \
  crates/trpg-gm/src/obligations.rs crates/trpg-gm/src/scene_policy.rs crates/trpg-gm/src/tools/mechanic.rs
# ③ serde 向后兼容回归（A1/C1 的老 JSON fixture 测试 + 全 workspace 测试矩阵收口）
cd crates/trpg-model && cargo test -p trpg-model
cd ../trpg-rule-agent && cargo test -p trpg-rule-agent
cd ../trpg-mechanics && cargo test -p trpg-mechanics
cd ../trpg-gm && cargo test -p trpg-gm
cd ../trpg-runtime && cargo test -p trpg-runtime && cd ../.. && cargo build -p trpg-cli
```

**可观测技术债清单收口（报告固定章节；骨架 C7③ 点名项 + 执行中新增项）**：
| 债项 | 来源 | 可观测面 |
|---|---|---|
| SessionEnd/DevelopmentPhase 无引擎事件点（仅原语支持） | B5 | A3 降级记录在 validation_report |
| party/world owner_kind 写路径维持现状（投影侧已通用） | B3 | validation 记录 |
| band trigger 未解析出的规则集（若有） | A4/A5 | validation_report + 对账表 12② 备注 |
| verifier 子串扫描回退路径 | B7 | finding detail 前缀 fallback:substring_scan（C7① 已断言） |
| check_match regex 兼容回退（契约无 mechanic_id 时） | 护栏 §3.5.1 | 账本/validation_report 标注 |
| （执行中发现的新债逐条追加） | | |

### 验证（任务收口 = 二期收口）
- 验收 11：retro_debt_e2e 两用例过 + 库面 PSQL 原文；验收 13：两证据面 hash 恒等 + 前提断言留档。
- 对账表 14 行 + 工程行全部填实（无空格无"已实现"），状态列出现「未通过」则二期不收口——指回任务修复后重跑该行证据。
- 总报告落 `docs/规则感知GM二期验收报告_2026-06-10.md`（中文），含：对账表、三链计时表汇总（vs 一期基线，回答 spec §9 风险 4）、技术债清单、evaluator 对照结论摘要。

---
