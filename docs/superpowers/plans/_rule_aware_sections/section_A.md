# Section A —— 解析侧（mechanics_compile 第三遍 + kernel 结构升级）任务正文

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
