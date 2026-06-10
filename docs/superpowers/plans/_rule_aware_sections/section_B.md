# Section B —— 运行时（目录注入 + watcher + 机械债务）任务正文

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
