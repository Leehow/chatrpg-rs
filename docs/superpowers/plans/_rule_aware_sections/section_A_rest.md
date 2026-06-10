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
