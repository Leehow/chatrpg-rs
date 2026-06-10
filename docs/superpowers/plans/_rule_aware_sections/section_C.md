# Section C —— 模组侧 + e2e 黄金链（任务正文）

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
