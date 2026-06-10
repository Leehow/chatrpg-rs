# 模组场景导航 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** 让模组进游戏即激活入口场景、GM 叙事推进时语义切换当前场景、整本模组可玩且 P5 预抽场景可达。

**Architecture:** `sessions.current_scene_id` 持久化当前场景；start_session(module) 设入口；runtime 单点把 current_scene_id 载入 RuntimeState.scene_id（P4 投影）；turn_postprocess 内 scene_navigator 用一次 LLM 语义判定党是否移动到模组某真实场景（fail-closed 默认不动）+ 到场深抽。

**Tech Stack:** Rust（trpg-db / trpg-runtime / trpg-api / trpg-rule-agent），Postgres（sqlx，迁移在 `Db::migrate` 的 include_str! 数组），LLM via codex-relay。

**护栏：** 语义优先零硬编码 · fail-closed 默认不动/绝不编造 target · 文件 ≤400 行 · 复用 load_module_graph/deep_extract_scene_in_place/turn_postprocess。

**Git：** 工作副本无 git，commit 步骤跳过（不 init/push）。

**并行提示（用户建议多用 subagent）：** Task 1（db 层）是地基先做。其后 Task 4（P4 links，纯 runtime）、Task 5a（validate_transition 纯函数）相互独立，可并行。Task 2/3 依赖 Task 1。

**Spec:** `docs/superpowers/specs/2026-06-09-module-scene-navigation-design.md`

---

## File Structure

| 文件 | 责任 | 动作 |
|---|---|---|
| `migrations/0026_session_current_scene_v120.sql` | sessions 加列 | Create |
| `crates/trpg-db/src/lib.rs` | migrate() 注册迁移 + set/load_session_scene | Modify |
| `crates/trpg-runtime/src/lib.rs` | module_entry_scene_id + start_session 激活 + prepare_turn_context 载入 + P4 出口投影 | Modify |
| `crates/trpg-api/src/lib.rs` | validate_transition + scene_navigator + turn_postprocess 接入 + 复用 continue 的单场景深抽 | Modify |

---

## Task 1: 持久化 sessions.current_scene_id（db 层）

**Files:**
- Create: `migrations/0026_session_current_scene_v120.sql`
- Modify: `crates/trpg-db/src/lib.rs`（migrate() 数组末尾加 include_str!；新增两方法）

- [ ] **Step 1: 建迁移文件**

`migrations/0026_session_current_scene_v120.sql`:
```sql
-- Module scene navigation: track the party's current module scene per session.
alter table sessions add column if not exists current_scene_id text;
```

- [ ] **Step 2: 注册迁移**

在 `crates/trpg-db/src/lib.rs` 的 `migrate()` 的 `include_str!` 数组**末尾**（0025 之后）加一行：
```rust
            include_str!("../../../migrations/0026_session_current_scene_v120.sql"),
```

- [ ] **Step 3: 加 db 方法**（放在 `create_session`（~929）附近）

```rust
/// 设置该会话当前所在的模组场景 node_id（场景导航）。
pub async fn set_session_scene(&self, session_id: &str, scene_id: &str) -> Result<()> {
    sqlx::query("update sessions set current_scene_id = $2, updated_at = now() where session_id = $1")
        .bind(session_id)
        .bind(scene_id)
        .execute(&self.pool)
        .await?;
    Ok(())
}

/// 读取该会话当前模组场景 node_id（无则 None）。
pub async fn load_session_scene(&self, session_id: &str) -> Result<Option<String>> {
    let row: Option<(Option<String>,)> = sqlx::query_as("select current_scene_id from sessions where session_id = $1")
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;
    Ok(row.and_then(|r| r.0).filter(|s| !s.trim().is_empty()))
}
```
> 执行者：确认 sessions 表有 `updated_at` 列（0001_init.sql:135 附近）；若无则去掉 `updated_at = now()`。确认 `self.pool` 是连接池字段名（看相邻方法）。

- [ ] **Step 4: 编译**

Run: `cargo build -p trpg-db 2>&1 | tail -10`
Expected: 通过。

- [ ] **Step 5: 迁移可应用（冒烟）**

Run（CoC DB 已在 :54347，迁移幂等 `if not exists`，直接验证 SQL 合法）:
```bash
docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg -c "alter table sessions add column if not exists current_scene_id text;"
docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg -c "\d sessions" | grep current_scene_id
```
Expected: 显示 `current_scene_id | text`。（无单测——db 方法需活 DB，由 Task 6 e2e 验证。）

- [ ] **Step 6: Commit**（无 git → 跳过）

---

## Task 2: 入口激活（start_session）

**Files:** Modify `crates/trpg-runtime/src/lib.rs`（新增纯函数 `module_entry_scene_id` + 其单测；`start_session`（~93）末尾加激活）

- [ ] **Step 1: 写失败测试**（加到 trpg-runtime 的 `#[cfg(test)] mod` 内，与 scene_node_to_blocks 测试同区）

```rust
#[test]
fn module_entry_scene_id_prefers_spine_then_deep_then_first() {
    use trpg_model::{ModuleGraph, ScenarioNode, SceneExtractionStatus};
    let mk = |id: &str, st: SceneExtractionStatus| {
        let mut n = ScenarioNode::default(); n.node_id = id.into(); n.extraction_status = st; n
    };
    let mut g = ModuleGraph::default();
    g.scenes = vec![mk("preface", SceneExtractionStatus::SkeletonOnly), mk("prologue", SceneExtractionStatus::DeepExtracted)];
    g.spine = serde_json::json!({"entry_node_id": "prologue"});
    assert_eq!(module_entry_scene_id(&g).as_deref(), Some("prologue"), "spine.entry_node_id 优先");
    g.spine = serde_json::json!({});
    assert_eq!(module_entry_scene_id(&g).as_deref(), Some("prologue"), "无 spine → 首个 deep");
    g.scenes[1].extraction_status = SceneExtractionStatus::SkeletonOnly;
    assert_eq!(module_entry_scene_id(&g).as_deref(), Some("preface"), "无 deep → scenes[0]");
    assert_eq!(module_entry_scene_id(&ModuleGraph::default()), None, "空 → None");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p trpg-runtime module_entry_scene_id 2>&1 | tail -8`
Expected: 编译失败（缺 `module_entry_scene_id`）。

- [ ] **Step 3: 实现纯函数**（放在 scene_node_to_blocks 附近）

```rust
/// 选模组入口场景 node_id：spine.entry_node_id（须在 scenes 中）→ 首个 DeepExtracted → scenes[0]。空→None。
pub fn module_entry_scene_id(graph: &trpg_model::ModuleGraph) -> Option<String> {
    use trpg_model::SceneExtractionStatus;
    if let Some(id) = graph.spine.get("entry_node_id").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()) {
        if graph.scenes.iter().any(|s| s.node_id == id) { return Some(id.to_string()); }
    }
    if let Some(s) = graph.scenes.iter().find(|s| s.extraction_status == SceneExtractionStatus::DeepExtracted) {
        return Some(s.node_id.clone());
    }
    graph.scenes.first().map(|s| s.node_id.clone())
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p trpg-runtime module_entry_scene_id 2>&1 | tail -8`
Expected: `1 passed`。

- [ ] **Step 5: start_session 末尾加激活**（`Ok(session_id)` 之前）

```rust
    if let Some(mid) = module_id {
        if let Ok(Some(graph)) = self.db.load_module_graph(mid).await {
            if let Some(entry) = module_entry_scene_id(&graph) {
                let _ = self.db.set_session_scene(&session_id, &entry).await; // fail-closed：失败不阻断开局
            }
        }
    }
    Ok(session_id)
```

- [ ] **Step 6: 编译**

Run: `cargo build -p trpg-runtime 2>&1 | tail -10`
Expected: 通过。

---

## Task 3: 每回合载入（runtime 单点）

**Files:** Modify `crates/trpg-runtime/src/lib.rs`（`prepare_turn_context`（148）顶部把 current_scene_id 载入；纯 helper `resolve_turn_scene_id` + 单测）

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn resolve_turn_scene_id_loads_when_absent_with_module() {
    assert_eq!(resolve_turn_scene_id(Some("loc1"), Some("m"), Some("loaded".into())).as_deref(), Some("loc1"), "已有 scene → 不覆盖");
    assert_eq!(resolve_turn_scene_id(None, Some("m"), Some("loaded".into())).as_deref(), Some("loaded"), "缺+有模组 → 载入");
    assert_eq!(resolve_turn_scene_id(None, None, Some("loaded".into())), None, "无模组 → 不载");
    assert_eq!(resolve_turn_scene_id(Some(""), Some("m"), Some("loaded".into())).as_deref(), Some("loaded"), "空串视为缺");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p trpg-runtime resolve_turn_scene_id 2>&1 | tail -8`
Expected: 缺函数。

- [ ] **Step 3: 实现 helper**

```rust
/// 决定本回合生效的 scene_id：已有非空则保留；否则仅当有 module 时用载入值。
fn resolve_turn_scene_id(state_scene: Option<&str>, module_id: Option<&str>, loaded: Option<String>) -> Option<String> {
    match state_scene {
        Some(s) if !s.trim().is_empty() => Some(s.to_string()),
        _ => if module_id.is_some() { loaded } else { None },
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p trpg-runtime resolve_turn_scene_id 2>&1 | tail -8`
Expected: `1 passed`。

- [ ] **Step 5: prepare_turn_context 顶部接入**（在用到 `state.scene_id` 之前，约 155 行附近）

`prepare_turn_context(&self, request, state: &RuntimeState, ...)` 内 state 是不可变借用 → clone 出可变副本填 scene_id，后续改用副本：
```rust
    let mut state_owned = state.clone();
    if resolve_turn_scene_id(state_owned.scene_id.as_deref(), state_owned.module_id.as_deref(), None).is_none()
        && state_owned.module_id.is_some()
    {
        let loaded = self.db.load_session_scene(&request.session_id).await.ok().flatten();
        state_owned.scene_id = resolve_turn_scene_id(state_owned.scene_id.as_deref(), state_owned.module_id.as_deref(), loaded);
    }
    let state = &state_owned;
```
> 执行者：插在 prepare_turn_context 开头、**所有 `state.` 读取之前**（最早读取在 ~194）。`let state = &state_owned;` 遮蔽原参数，下文无需改。确认 RuntimeState 实现 Clone（应有）。

- [ ] **Step 6: 编译**

Run: `cargo build -p trpg-runtime 2>&1 | tail -12`
Expected: 通过。

---

## Task 4: P4 投影当前场景出口（links）

**Files:** Modify `crates/trpg-runtime/src/lib.rs`（`scene_node_to_blocks`（~1952）加 scenes 参数 + 出口；调用方 `module_scene_blocks_for_turn`（~1423）传 scenes；扩展现有单测）

- [ ] **Step 1: 改现有测试断言出口**（找现有 `deep_scene_projects_scenestatic_dynamictail` 测试，给 node 加一条 link + 一个目标场景，断言块含目标标题）

```rust
#[test]
fn deep_scene_block_includes_exit_titles() {
    use trpg_model::{ScenarioNode, ScenarioLink, LinkType, SceneExtractionStatus};
    let mut entry = ScenarioNode::default();
    entry.node_id = "loc1".into(); entry.title = "加油站".into();
    entry.read_aloud = Some("你们停车。".into());
    entry.extraction_status = SceneExtractionStatus::DeepExtracted;
    entry.links = vec![ScenarioLink { to_node_id: "loc2".into(), reason: "主路通往".into(), clue_id: None, link_type: LinkType::Spatial }];
    let mut town = ScenarioNode::default(); town.node_id = "loc2".into(); town.title = "镇中心".into();
    let scenes = vec![entry.clone(), town];
    let blocks = scene_node_to_blocks("m1", &entry, &[], &scenes);
    let body = match &blocks[0].content { trpg_model::BlockContent::Text(t) => t.clone(), _ => String::new() };
    assert!(body.contains("镇中心"), "出口应含目标场景标题");
}
```
> 执行者：`BlockContent::Text` 取值方式以现有测试为准（现有 skeleton_scene_projects_nothing / deep_scene_projects 测试里有写法）。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p trpg-runtime deep_scene_block_includes_exit 2>&1 | tail -8`
Expected: 签名不匹配（scene_node_to_blocks 缺 scenes 参数）。

- [ ] **Step 3: scene_node_to_blocks 加 scenes 参数 + 出口段**

签名改 `fn scene_node_to_blocks(module_id: &str, n: &ScenarioNode, npcs: &[serde_json::Value], scenes: &[ScenarioNode]) -> Vec<ContextBlock>`。在 NPC 循环之后、构造 ContextBlock 之前加：
```rust
    let exits: Vec<String> = n.links.iter().filter_map(|l| {
        let title = scenes.iter().find(|s| s.node_id == l.to_node_id).map(|s| s.title.as_str()).unwrap_or("");
        if title.trim().is_empty() { None } else { Some(format!("- {} → {}", title, l.reason)) }
    }).collect();
    if !exits.is_empty() {
        body.push_str("\n【出口】\n");
        body.push_str(&exits.join("\n"));
        body.push('\n');
    }
```

- [ ] **Step 4: 调用方传 scenes**

在 `module_scene_blocks_for_turn`（~1450 找到 `scene_node_to_blocks(` 调用处）把当前 graph 的 scenes 传入：`scene_node_to_blocks(module_id, node, &npcs, &graph.scenes)`（确认 graph/scenes 变量名）。

- [ ] **Step 5: 跑测试 + 编译**

Run: `cargo test -p trpg-runtime scene_node_to_blocks 2>&1 | tail -8 ; cargo build -p trpg-runtime 2>&1 | tail -6`
Expected: 测试绿（含新出口断言 + 原有 deep/skeleton 测试仍过）+ 编译通过。

---

## Task 5: scene_navigator 语义切换 + 到场深抽（trpg-api）

**Files:** Modify `crates/trpg-api/src/lib.rs`（纯 `validate_transition` + 单测；`scene_navigator`；turn_postprocess 接入；复用/小重构 continue 的单场景深抽）

### Task 5a: validate_transition 纯函数（可与 Task 4 并行）

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod scene_nav_tests {
    use super::*;
    use trpg_model::ScenarioNode;
    fn scenes() -> Vec<ScenarioNode> {
        let mk = |id: &str| { let mut n = ScenarioNode::default(); n.node_id = id.into(); n };
        vec![mk("loc1"), mk("loc2"), mk("loc3")]
    }
    #[test]
    fn validate_transition_is_fail_closed() {
        let s = scenes();
        assert_eq!(validate_transition(&serde_json::json!({"moved":true,"target_node_id":"loc2"}), &s, "loc1").as_deref(), Some("loc2"));
        assert_eq!(validate_transition(&serde_json::json!({"moved":false,"target_node_id":"loc2"}), &s, "loc1"), None, "moved=false → 不动");
        assert_eq!(validate_transition(&serde_json::json!({"moved":true,"target_node_id":"ghost"}), &s, "loc1"), None, "target 不在列表 → 不动");
        assert_eq!(validate_transition(&serde_json::json!({"moved":true,"target_node_id":"loc1"}), &s, "loc1"), None, "target==当前 → 不动");
        assert_eq!(validate_transition(&serde_json::json!({}), &s, "loc1"), None, "缺字段 → 不动");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p trpg-api validate_transition 2>&1 | tail -8`
Expected: 缺函数。

- [ ] **Step 3: 实现 validate_transition**

```rust
/// fail-closed 校验场景切换决策：仅当 moved=true 且 target 是 scenes 中真实存在、且 != 当前场景，才返回 target。
pub fn validate_transition(decision: &serde_json::Value, scenes: &[trpg_model::ScenarioNode], current: &str) -> Option<String> {
    if !decision.get("moved").and_then(|v| v.as_bool()).unwrap_or(false) { return None; }
    let target = decision.get("target_node_id").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty())?;
    if target == current { return None; }
    scenes.iter().any(|s| s.node_id == target).then(|| target.to_string())
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p trpg-api validate_transition 2>&1 | tail -8`
Expected: `1 passed`。

### Task 5b: scene_navigator + 到场深抽 + 接入

- [ ] **Step 1: 复用单场景深抽（小重构 continue）**

把 Phase 5 `continue_module_extraction` 的"载 bundle→ModuleGraph→ModuleReadout→对选中 idx 调 deep_extract_scene_in_place→写回"核心抽成：
```rust
/// 深抽该模组里满足 `select` 的场景（only=Some(node_id) 只抽该场景；None 抽所有 SkeletonOnly），写回 bundle。返回深抽数。
async fn extract_module_scenes(db: &Db, llm: &dyn LlmClient, module_id: &str, source_id: &str, ruleset_id: Option<&str>, data_dir: &std::path::Path, only: Option<&str>) -> anyhow::Result<usize>
```
`continue_module_extraction` 改为调 `extract_module_scenes(..., None)`。逻辑与原 Phase 5 一致，仅多一个 `only` 过滤（only=Some 时只取 `node_id==only && SkeletonOnly` 的 idx）。fail-closed 同原样。

- [ ] **Step 2: SCENE_NAV_SYS + scene_navigator**

```rust
const SCENE_NAV_SYS: &str = "你是模组场景导航器。给定『当前场景』『模组全部场景列表(node_id|kind|title)』『本回合叙事』，\
语义判断玩家党是否离开当前场景、走到列表里另一个真实存在的场景。按语义判断（人物移动/进入新地点/任务推进），\
不要按标题字面猜。输出 JSON：{\"moved\": bool, \"target_node_id\": string|null, \"reason\": string}。\
fail-closed：不确定、没有明确移动、或目标不在列表里 → moved=false。target_node_id 必须是给定列表中的 node_id，绝不编造。";

/// turn_postprocess 内的语义场景导航：判定党是否移动 → 更新 current_scene_id → 到场深抽。全程 fail-closed。
async fn scene_navigator(db: &Db, llm: &dyn LlmClient, session_id: &str, module_id: &str, source_id: &str, ruleset_id: Option<&str>, data_dir: &std::path::Path, narration: &str) -> anyhow::Result<()> {
    let Some(graph) = db.load_module_graph(module_id).await? else { return Ok(()); };
    if graph.scenes.is_empty() { return Ok(()); }
    let current = db.load_session_scene(session_id).await?.unwrap_or_default();
    let list = graph.scenes.iter().map(|s| format!("{} | {} | {}", s.node_id, s.node_type, s.title)).collect::<Vec<_>>().join("\n");
    let cur_title = graph.scenes.iter().find(|s| s.node_id == current).map(|s| s.title.as_str()).unwrap_or("(未定)");
    let usr = format!("当前场景: {current} ({cur_title})\n模组全部场景:\n{list}\n\n本回合叙事:\n{}", narration.chars().take(2500).collect::<String>());
    let decision = match llm.complete_json(vec![trpg_llm::system(SCENE_NAV_SYS), trpg_llm::user(&usr)], 0.0).await {
        Ok(v) => v, Err(err) => { tracing::warn!(error=%err, "scene_navigator llm failed; stay"); return Ok(()); }
    };
    let Some(target) = validate_transition(&decision, &graph.scenes, &current) else { return Ok(()); };
    db.set_session_scene(session_id, &target).await?;
    tracing::info!(session_id, from=%current, to=%target, "scene transition");
    // 到场深抽（目标若 SkeletonOnly）；P5 已预抽则该调用内部判定为 0、无害。
    let _ = extract_module_scenes(db, llm, module_id, source_id, ruleset_id, data_dir, Some(&target)).await;
    Ok(())
}
```
> 执行者：`trpg_llm::system/user` 的实际路径以 crate 导出为准（continue_module_extraction 已 import llm 相关）。`source_id` 从 module bundle 或约定 `module_id.split('.')` 取——以 continue 里取 source_id 的现成方式为准。

- [ ] **Step 3: 接入 turn_postprocess spawn**

在 trpg-api turn_postprocess 的 `tokio::spawn` 块内（save_turn/save_memory_event 之后、job done 之前，约 1420-1440），当 `runtime_state.module_id` 有值时调用：
```rust
            if let Some(mid) = runtime_state.module_id.clone() {
                let sid = source_id_for_module(&mid); // 或从 bundle 取；见 extract_module_scenes 的取法
                let _ = scene_navigator(&db2, llm_for_nav.as_ref(), &session_id, &mid, &sid, runtime_state.ruleset_id.clone().into(), &data_dir, &full).await;
            }
```
> 执行者：spawn 当前捕获 db2/runtime2，**需额外捕获 llm + data_dir**（从 AppState 克隆，参照 continue handler 怎么拿 llm/data_dir）。`full` 是 GM 叙事文本。若 runtime2 已持有 llm，可用 `runtime2` 的 llm 访问而不另捕获——以现有可达性为准。

- [ ] **Step 4: 编译 + 单测**

Run: `cargo test -p trpg-api validate_transition 2>&1 | tail -6 ; cargo build -p trpg-api 2>&1 | tail -15 ; cargo build 2>&1 | tail -6`
Expected: 测试绿 + 全工作区编译通过。

---

## Task 6: in-game e2e（CoC 真模组）

**Files:** 无新增（运行 + docker exec 验证）。CoC 模组已 37 场景在 :54347 DB。

- [ ] **Step 1: 迁移列存在**（Task 1 已加；确认）

```bash
docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg -c "\d sessions" | grep current_scene_id
```
Expected: 有该列。

- [ ] **Step 2: 起一局并跑首回合（看入口投影）**

```bash
cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
set -a; . ./.env; set +a; COC_URL="${DATABASE_URL/54346/54347}"
DATABASE_URL="$COC_URL" TRPG_MODULE_READER=1 TRPG_LLM_MODEL=gpt-5.4-mini \
  cargo run -q -p trpg-cli -- play --ruleset call_of_cthulhu_7e --module call_of_cthulhu_7e.document <<< $'我们停车下来看看\n前往镇中心\n/quit' 2>&1 | tail -60
```
Expected: 开局叙事含入口场景"屠宰场/加油站"念白；交互 1 后 GM 叙事前往镇中心。
> 执行者：play 的交互输入方式以 play_cli 实现为准；若 play 不便脚本化，改用 `trpg turn` 连跑两回合（先 start-session 再 turn），并在回合间 docker exec 查 sessions.current_scene_id 是否被 scene_navigator 更新。

- [ ] **Step 3: 验证 current_scene_id 随叙事切换**

```bash
docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg -tA -c "select session_id, current_scene_id from sessions where ruleset_id='call_of_cthulhu_7e' order by created_at desc limit 1;"
```
Expected: current_scene_id 从入口场景变为党移动到的场景（如镇中心对应 node_id）。

- [ ] **Step 4: 验证切换后场景投影 + 到场深抽**

```bash
docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg -tA -c "select s->>'title', s->>'extraction_status' from parsed_bundles, jsonb_array_elements(content_json->'module_graph'->'scenes') s where bundle_kind='module' and content_json->>'ruleset_id'='call_of_cthulhu_7e' and s->>'extraction_status'='deep_extracted';"
```
Expected: 除入口外，党到达的目标场景也变 deep_extracted（到场深抽生效）。

- [ ] **Step 5: 记录 e2e 结果**（中文，写入超夜报告或新报告）

---

## Self-Review（已对照 spec）

- **Spec 覆盖**：§4.1 持久化→Task1；§4.2 入口激活→Task2；§4.3 每回合载入→Task3；§4.4 P4 出口→Task4；§4.5 语义切换+到场深抽→Task5；§7 测试→各 Task 单测 + Task6 e2e。✅
- **占位符**：无 TBD/TODO；纯函数（module_entry_scene_id/resolve_turn_scene_id/validate_transition）均给完整代码+测试；集成步骤给完整代码 + "执行者：以现成方式为准"的精确指引（source_id 取法、llm/data_dir 捕获、BlockContent 取值）非真空占位。
- **类型一致**：`module_entry_scene_id`/`resolve_turn_scene_id`/`validate_transition`/`scene_navigator`/`extract_module_scenes`/`set_session_scene`/`load_session_scene`/`scene_node_to_blocks(.., scenes)` 跨 Task 命名一致；复用 `load_module_graph`/`deep_extract_scene_in_place`/`LinkType`/`ScenarioNode` 与既有签名一致。
