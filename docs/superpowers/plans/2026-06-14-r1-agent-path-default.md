# R1 实施计划：agent 路径升默认 + 退役 legacy + 数据化统一回合执行器

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把已可玩但非默认的 agent 回合路径提为 CLI/API 唯一回合执行器，回合管线数据化为 TurnPipelinePlan，退役 legacy run_turn_once + 词法 plan_turn/plan_agent_turn。

**Architecture:** trpg-gm 新增 `execute_turn(req)->Stream<TurnEvent>` 统一 facade，按声明式 `CANONICAL_TURN_PLAN`（15 phase）解释驱动 GmLoop agent loop，postprocess parity 超集化；scene_navigator 下沉 trpg-runtime；CLI/API 都 drain TurnEvent 流（CLI 同步 drain，API SSE + 后台尾部）。决定性切换：一个分支内做完，合并前过强制验证闸（harness + 真库三链 CLI&API + 零回归），回滚靠 git revert。

**Tech Stack:** Rust, tokio (mpsc/spawn), tokio-stream (ReceiverStream), axum SSE, sqlx/postgres, gpt-5.5 via relay。前置设计 spec：`docs/superpowers/specs/2026-06-14-r1-agent-path-default-design.md`。

---

## 共享契约（所有任务遵守的精确类型/命名）

落点全部进 crate `trpg-gm`（已依赖 trpg-runtime，无循环）。新模块 turn_plan.rs / turn_event.rs / execute.rs。详见各 Task；关键类型：`PhaseKind{Deterministic,AgentLoop,Postprocess}`、`PhaseId`（15 变体：RecordPlayerAction/RefreshLiveDerived/Reconcile/Gate/StimulusPass/OpposedPrepass/ModeInference/DebtLoad/ContextAssembly/AgentLoop/VerifyAfterStream/Finalize/AuditLearning/SceneNavigate/CarryoverDebt）、`TurnPhasePlan{id,kind,conditional}`、`CANONICAL_TURN_PLAN: &[TurnPhasePlan]`(15)、`TurnEvent{Delta,AwaitingPlayerRoll,SceneTransition,Errata,PostprocessScheduled,TurnComplete}`、`OwnedTurnRequest`、`execute_turn(gm, req, plan)->ReceiverStream<TurnEvent>`。解释器内 `struct TurnPipeline` + 每 phase 一个 `phase_<snake_id>` async 方法；AwaitingPlayerRoll 早返跑 verify+finalize(awaiting) 跳 scene/audit/carryover。

---

## 文件结构总览

| 文件 | 动作 | 责任 | Task |
|---|---|---|---|
| `crates/trpg-runtime/src/scene_navigation.rs` | Create | scene_navigator/validate_transition/build_nav_prompt/prefetch_frontier **+ extract_module_scenes** 下沉落点（后者原在 trpg-api，非薄 wrapper） | T1 |
| `crates/trpg-runtime/Cargo.toml` | Modify | 加 trpg-rule-agent 依赖（extract_module_scenes 调 reader 原语需要） | T1 |
| `crates/trpg-runtime/src/lib.rs` | Modify | mod scene_navigation + pub use（含 extract_module_scenes）；删 plan_agent_turn | T1,T7 |
| `crates/trpg-api/src/lib.rs` | Modify | scene_navigator + extract_module_scenes 改 `pub use` re-export（保 continue_module_extraction 等 api 内调用方）；play_turn_sse 改调 execute_turn；删词法 plan 路径 | T1,T6,T7 |
| `crates/trpg-cli/src/agent_play.rs` | Modify | drain execute_turn；scene_nav 改引 runtime | T1,T5 |
| `crates/trpg-cli/src/main.rs` | Modify | 删 run_turn_once；agent flag 处理；scene_nav 改引 | T1,T5 |
| `crates/trpg-gm/src/turn_plan.rs` | Create | TurnPipelinePlan + CANONICAL_TURN_PLAN | T2 |
| `crates/trpg-gm/src/turn_event.rs` | Create | TurnEvent 统一事件枚举 | T2 |
| `crates/trpg-gm/src/lib.rs` | Modify | mod turn_plan/turn_event/execute + pub use | T2,T4 |
| `crates/trpg-gm/src/turn_loop.rs` | Modify | 确定性头部抽 phase_* handlers；run_gm_turn 收编为 agent_loop | T3,T4 |
| `crates/trpg-gm/src/execute.rs` | Create | execute_turn 解释器 + OwnedTurnRequest | T4 |
| `crates/trpg-api/Cargo.toml` | Modify | 加 trpg-gm 依赖 | T6 |
| `crates/trpg-agent/src/lib.rs` | Modify | 删词法 matches_conditions/plan_turn 回合 advice 路径 | T7 |
| `crates/trpg-harness` + 真库三链 | Verify | decisive-cut 验证闸 | T8 |

---

### Task 1: scene_navigator 下沉 trpg-runtime

**背景**：`scene_navigator` / `validate_transition` / `build_nav_prompt` / `SCENE_NAV_SYS` / `prefetch_frontier` / `FRONTIER_PREFETCH_MAX` 当前定义在 `trpg-api/src/lib.rs:2016-2166`；`scene_nav_tests` 在同文件 2168-2263。`extract_module_scenes`（签名：`pub async fn extract_module_scenes(db, llm, module_id, source_id, ruleset_id, data_dir, budget, only)`，定义在 `trpg-api/src/lib.rs:289-381`）被 `scene_navigator`（到场深抽）和 `prefetch_frontier`（前探一跳）两处调用，**也必须随同下沉**，否则 trpg-runtime 无法引用它。两个调用方为 `trpg-api/src/lib.rs:1641`（API postprocess）与 `trpg-cli/src/agent_play.rs:97` + `trpg-cli/src/main.rs:1496`（CLI 两路径）；另有 `continue_module_extraction`（trpg-api ~276 行）也调用 `extract_module_scenes`，需靠 re-export 继续解析。

下沉集群（全部移入 `trpg-runtime/src/scene_navigation.rs`）：`extract_module_scenes` / `scene_navigator` / `validate_transition` / `build_nav_prompt` / `prefetch_frontier` / `SCENE_NAV_SYS` / `FRONTIER_PREFETCH_MAX`。`extract_module_scenes` 只依赖 `trpg-db` / `trpg-llm` / `trpg-model` / `trpg-rule-agent::reader`，故下沉时需在 `trpg-runtime/Cargo.toml` 加 `trpg-rule-agent` 依赖（无循环：trpg-rule-agent 不依赖 trpg-runtime）。下沉目标：新建 `trpg-runtime/src/scene_navigation.rs`；`trpg-api` 改为 `pub use trpg_runtime::scene_navigation::{scene_navigator, extract_module_scenes, ...}` 过渡 re-export（保证 `continue_module_extraction` 等现有调用方不变）；CLI 两处改为 `trpg_runtime::scene_navigation::{extract_module_scenes, scene_navigator}`。

**Files:**
- **Create** `crates/trpg-runtime/src/scene_navigation.rs`（目标文件，≤400 行；含移入的全部函数：`extract_module_scenes` + `scene_navigator` + `validate_transition` + `build_nav_prompt` + `prefetch_frontier` + 常量 + 单测）
- **Modify** `crates/trpg-runtime/Cargo.toml`（加 trpg-rule-agent 依赖，`extract_module_scenes` 的 `trpg_rule_agent::reader::*` 调用需要它）
- **Modify** `crates/trpg-runtime/src/lib.rs`（`pub mod scene_navigation; pub use scene_navigation::{extract_module_scenes, scene_navigator, validate_transition, build_nav_prompt, prefetch_frontier}`）
- **Modify** `crates/trpg-api/src/lib.rs:289-381`（删 `extract_module_scenes` 原实现）+ `lib.rs:2016-2263`（删 scene_nav 集群原实现 + 删测试 mod），两处均改为 `pub use trpg_runtime::scene_navigation::{...}` re-export
- **Modify** `crates/trpg-cli/src/agent_play.rs:8,97`（改引 `trpg_runtime::scene_navigation::{extract_module_scenes, scene_navigator}`）
- **Modify** `crates/trpg-cli/src/main.rs:1496`（改引 `trpg_runtime::scene_navigation::scene_navigator`）
- 测试内联 `#[cfg(test)] mod tests` 进 scene_navigation.rs（若行数超 400 则拆为 `scene_navigation_tests.rs`）

---

- [ ] **Step 1: 在 trpg-runtime/Cargo.toml 加 trpg-rule-agent 依赖**

  编辑 `crates/trpg-runtime/Cargo.toml`，在 `[dependencies]` 末尾加一行：

  ```toml
  trpg-rule-agent = { path = "../trpg-rule-agent" }
  ```

  验证无 cycle（trpg-rule-agent 只依赖 trpg-db/trpg-formula/trpg-llm/trpg-model/trpg-search，均不含 trpg-runtime）：
  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
  cargo metadata --no-deps -q | python3 -c "import sys,json; d=json.load(sys.stdin); [print(p['name'],'->',[n.split(' ')[0] for n in p.get('dependencies',[])]) for p in d['packages'] if p['name']=='trpg-rule-agent']"
  ```

  预期：输出显示 trpg-rule-agent 依赖列表中**不含** trpg-runtime。

- [ ] **Step 2: 新建 crates/trpg-runtime/src/scene_navigation.rs（含 extract_module_scenes 函数体 + 全部函数体 + 单测）**

  将以下内容写入 `crates/trpg-runtime/src/scene_navigation.rs`（完整，不留占位符）。**关键：`extract_module_scenes` 函数体逐字从 `trpg-api/src/lib.rs:289-381` 复制过来；`prefetch_frontier` 和 `scene_navigator` 对它的调用都是 `extract_module_scenes(...)` 直接调用（同模块内），不使用任何 `_inner` 后缀。**

  ```rust
  //! 场景导航 + 模组深抽：extract_module_scenes / scene_navigator / validate_transition /
  //! build_nav_prompt / prefetch_frontier。从 trpg-api 下沉至 trpg-runtime，
  //! 供 trpg-gm 执行器 phase_scene_navigate 调用，也供 trpg-cli 直接引用。
  use anyhow::Result;
  use serde_json::json;
  use tracing::{error, info};
  use trpg_db::Db;
  use trpg_llm::LlmClient;
  use trpg_model::{ScenarioNode, SceneExtractionStatus, Visibility, WorldEventKind};
  use trpg_time::WorldTimeService;

  const FRONTIER_PREFETCH_MAX: usize = 5;

  const SCENE_NAV_SYS: &str = "你是模组场景导航器。给定『当前场景』『模组全部场景列表(node_id|kind|title)』\
  『玩家输入』『本回合 GM 叙事』，综合两者语义判断玩家党是否离开当前场景、\
  走到列表里另一个真实存在的场景。玩家明确说出移动意图（如「我去 X」「开车到 X」）\
  或 GM 叙事描述了到达新地点，均应判为 moved=true。按语义判断（人物移动/进入新地点/任务推进），\
  不要按标题字面猜。输出 JSON：{\"moved\": bool, \"target_node_id\": string|null, \"reason\": string}。\
  fail-closed：不确定、没有明确移动、或目标不在列表里 → moved=false。target_node_id 必须是给定列表中的 node_id，绝不编造。";

  /// 升级 ModuleGraph 中 SkeletonOnly 场景的核心函数（从 trpg-api 移入）。
  /// 共享于 `continue_module_extraction`（only=None）和 `scene_navigator` 到场深抽（only=Some）。
  /// `source_id` 为 None 时从 bundle.source_index 推导；`ruleset_id` 为 None 时从 bundle 取。
  /// Fail-closed：缺 bundle/units/source_id → warn + Ok(0)；per-scene 失败留 SkeletonOnly；
  /// 仅持久化失败才 Err。
  pub async fn extract_module_scenes(
      db: &Db,
      llm: &dyn LlmClient,
      module_id: &str,
      source_id: Option<&str>,
      ruleset_id: Option<&str>,
      data_dir: &std::path::Path,
      budget: usize,
      only: Option<&str>,
  ) -> Result<usize> {
      let Some((mut bundle, source_hash, parse_config_hash)) = db.load_module_bundle_for_continue(module_id).await? else {
          info!(module_id, "extract_module_scenes: no module bundle found; nothing to do");
          return Ok(0);
      };
      let derived_source_id = bundle.source_index.sources.first().map(|s| s.source_id.clone());
      let Some(source_id) = source_id.map(str::to_string).or(derived_source_id) else {
          info!(module_id, "extract_module_scenes: no source_id (request or bundle); nothing to do");
          return Ok(0);
      };
      let units_path = data_dir.join("parsed/source_units").join(format!("{source_id}.semantic_units.jsonl"));
      let units = match trpg_rule_agent::reader::load_units(&units_path) {
          Ok(u) if !u.is_empty() => u,
          Ok(_) => { info!(module_id, path = %units_path.display(), "extract_module_scenes: empty units; nothing to do"); return Ok(0); }
          Err(err) => { error!(error = %err, path = %units_path.display(), "extract_module_scenes: load_units failed; nothing to do"); return Ok(0); }
      };
      let sidecar_text = std::fs::read_to_string(data_dir.join(format!("markdown/modules/{source_id}.md"))).ok();
      let resolved_ruleset = ruleset_id.map(str::to_string).or_else(|| bundle.ruleset_id.clone());
      let ctx = trpg_rule_agent::reader::ModuleReaderCtx { units: &units, sidecar_text, ruleset_id: resolved_ruleset };
      let graph = &bundle.module_graph;
      let mut readout = trpg_rule_agent::reader::ModuleReadout {
          spine: graph.spine.clone(),
          scenes: graph.scenes.clone(),
          npcs: graph.npcs.clone(),
          clues: graph.clues.clone(),
          locations: graph.locations.clone(),
          factions: graph.factions.clone(),
          encounters: graph.encounters.clone(),
          handouts: graph.handouts.clone(),
          module_specific_rules: graph.module_specific_rules.clone(),
      };
      let skeleton_idxs: Vec<usize> = readout.scenes.iter().enumerate()
          .filter(|(_, s)| s.extraction_status == SceneExtractionStatus::SkeletonOnly)
          .filter(|(_, s)| only.map_or(true, |id| s.node_id == id))
          .map(|(i, _)| i)
          .collect();
      if skeleton_idxs.is_empty() {
          info!(module_id, ?only, "extract_module_scenes: no matching SkeletonOnly scenes");
          return Ok(0);
      }
      let mut deep_count = 0usize;
      for idx in skeleton_idxs {
          if trpg_rule_agent::reader::deep_extract_scene_in_place(llm, &ctx, &mut readout, idx, budget).await {
              deep_count += 1;
          } else {
              info!(module_id, idx, "scene stayed SkeletonOnly during extract");
          }
      }
      if deep_count == 0 {
          info!(module_id, "extract_module_scenes: no scene upgraded; skipping re-persist");
          return Ok(0);
      }
      let bridges = trpg_rule_agent::reader::apply_bridge_edges(&mut readout.scenes);
      info!(module_id, bridge_edges = bridges, "extract_module_scenes: applied entity-bridge edges");
      let g = &mut bundle.module_graph;
      g.spine = readout.spine;
      g.scenes = readout.scenes;
      g.npcs = readout.npcs;
      g.clues = readout.clues;
      g.locations = readout.locations;
      g.factions = readout.factions;
      g.encounters = readout.encounters;
      g.handouts = readout.handouts;
      g.module_specific_rules = readout.module_specific_rules;
      db.upsert_module_bundle(&bundle, None, &source_hash, &parse_config_hash).await?;
      info!(module_id, deep_extracted = deep_count, "extract_module_scenes: re-persisted upgraded ModuleGraph");
      Ok(deep_count)
  }

  /// 纯函数：构建场景导航 prompt（user 部分），便于单测。
  /// `player_input` 截至 500 字符，`narration` 截至 2000 字符。
  pub fn build_nav_prompt(
      current: &str,
      cur_title: &str,
      scene_list: &str,
      player_input: &str,
      narration: &str,
  ) -> String {
      format!(
          "当前场景: {current} ({cur_title})\n模组全部场景:\n{scene_list}\n\n玩家输入:\n{}\n\n本回合 GM 叙事:\n{}",
          player_input.chars().take(500).collect::<String>(),
          narration.chars().take(2000).collect::<String>(),
      )
  }

  /// fail-closed 校验场景切换决策：仅当 moved=true 且 target 是 scenes 中真实存在、
  /// 且 != 当前场景，才返回 target。
  pub fn validate_transition(
      decision: &serde_json::Value,
      scenes: &[ScenarioNode],
      current: &str,
  ) -> Option<String> {
      if !decision.get("moved").and_then(|v| v.as_bool()).unwrap_or(false) {
          return None;
      }
      let target = decision
          .get("target_node_id")
          .and_then(|v| v.as_str())
          .map(str::trim)
          .filter(|s| !s.is_empty())?;
      if target == current {
          return None;
      }
      scenes.iter().any(|s| s.node_id == target).then(|| target.to_string())
  }

  /// 前探当前 target 场景的衔接场景（一跳，best-effort）。在 target 已深抽、其出口
  /// links 已写回 bundle 后调用：重新加载图 → 取 target 出口 `to_node_id` → 去重 +
  /// 仅抽仍 SkeletonOnly + bounded 上限 `FRONTIER_PREFETCH_MAX` →
  /// 逐个 `extract_module_scenes(only=Some(exit))`。
  /// 全程 fail-closed：load 失败/找不到 target/无出口 → 静默跳过。
  pub async fn prefetch_frontier(
      db: &Db,
      llm: &dyn LlmClient,
      module_id: &str,
      target: &str,
      data_dir: &std::path::Path,
  ) {
      let graph = match db.load_module_graph(module_id).await {
          Ok(Some(g)) => g,
          Ok(None) => return,
          Err(err) => {
              tracing::warn!(error = %err, module_id, "prefetch_frontier: load_module_graph failed; skip");
              return;
          }
      };
      let Some(node) = graph.scenes.iter().find(|s| s.node_id == target) else { return };
      let mut seen = std::collections::HashSet::new();
      let mut exits: Vec<String> = Vec::new();
      for link in &node.links {
          let to = link.to_node_id.trim();
          if to.is_empty() || to == target || !seen.insert(to.to_string()) {
              continue;
          }
          let still_stub = graph
              .scenes
              .iter()
              .any(|s| s.node_id == to && s.extraction_status == SceneExtractionStatus::SkeletonOnly);
          if still_stub {
              exits.push(to.to_string());
          }
          if exits.len() >= FRONTIER_PREFETCH_MAX {
              break;
          }
      }
      if exits.is_empty() {
          return;
      }
      info!(module_id, %target, prefetch = exits.len(), "prefetch_frontier: deep-extracting one-hop exits");
      for exit in exits {
          if let Err(err) =
              extract_module_scenes(db, llm, module_id, None, None, data_dir, 12, Some(&exit)).await
          {
              tracing::warn!(error = %err, %exit, "prefetch_frontier: one-hop deep-extract failed; best-effort skip");
          }
      }
  }

  /// turn_postprocess 内的语义场景导航：用一次 LLM 判定党是否移动到模组某真实场景，
  /// 校验通过则更新 `sessions.current_scene_id` 并对目标场景到场深抽（若仍 SkeletonOnly）。
  /// 切换成功时写一条 `WorldEventKind::SceneChanged` world event（kind/from/to/reason）。
  ///
  /// 全程 fail-closed：取不到图/空图/LLM 失败/校验不过 → warn + Ok(())（留原场景）。
  pub async fn scene_navigator(
      db: &Db,
      llm: &dyn LlmClient,
      session_id: &str,
      module_id: &str,
      data_dir: &std::path::Path,
      player_input: &str,
      narration: &str,
  ) -> anyhow::Result<()> {
      let Some(graph) = db.load_module_graph(module_id).await? else {
          return Ok(());
      };
      if graph.scenes.is_empty() {
          return Ok(());
      }
      let current = db.load_session_scene(session_id).await?.unwrap_or_default();
      let list = graph
          .scenes
          .iter()
          .map(|s| format!("{} | {} | {}", s.node_id, s.node_type, s.title))
          .collect::<Vec<_>>()
          .join("\n");
      let cur_title = graph
          .scenes
          .iter()
          .find(|s| s.node_id == current)
          .map(|s| s.title.as_str())
          .unwrap_or("(未定)");
      let usr = build_nav_prompt(&current, cur_title, &list, player_input, narration);
      let decision = match llm
          .complete_json(vec![trpg_llm::system(SCENE_NAV_SYS), trpg_llm::user(&usr)], 0.0)
          .await
      {
          Ok(v) => v,
          Err(err) => {
              tracing::warn!(error = %err, "scene_navigator llm failed; stay");
              return Ok(());
          }
      };
      let Some(target) = validate_transition(&decision, &graph.scenes, &current) else {
          return Ok(());
      };
      db.set_session_scene(session_id, &target).await?;
      let reason = decision
          .get("reason")
          .and_then(|v| v.as_str())
          .unwrap_or("")
          .to_string();
      info!(session_id, from = %current, to = %target, %reason, "scene transition");
      let event_data = json!({
          "kind": "scene_transition",
          "from": current,
          "to": target,
          "reason": reason,
          "module_id": module_id
      });
      if let Err(err) = WorldTimeService::new(db.clone())
          .record_event(session_id, None, None, WorldEventKind::SceneChanged, event_data, Visibility::GmOnly)
          .await
      {
          tracing::warn!(error = %err, "scene_navigator: world event write failed; scene already switched");
      }
      if let Err(err) =
          extract_module_scenes(db, llm, module_id, None, None, data_dir, 12, Some(&target)).await
      {
          tracing::warn!(error = %err, %target, "on-arrival deep-extract failed; scene already switched");
      }
      prefetch_frontier(db, llm, module_id, &target, data_dir).await;
      Ok(())
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      fn scenes() -> Vec<ScenarioNode> {
          let mk = |id: &str| {
              let mut n = ScenarioNode::default();
              n.node_id = id.into();
              n
          };
          vec![mk("loc1"), mk("loc2"), mk("loc3")]
      }

      #[test]
      fn validate_transition_is_fail_closed() {
          let s = scenes();
          assert_eq!(
              validate_transition(&serde_json::json!({"moved":true,"target_node_id":"loc2"}), &s, "loc1").as_deref(),
              Some("loc2")
          );
          assert_eq!(
              validate_transition(&serde_json::json!({"moved":false,"target_node_id":"loc2"}), &s, "loc1"),
              None,
              "moved=false → 不动"
          );
          assert_eq!(
              validate_transition(&serde_json::json!({"moved":true,"target_node_id":"ghost"}), &s, "loc1"),
              None,
              "target 不在列表 → 不动"
          );
          assert_eq!(
              validate_transition(&serde_json::json!({"moved":true,"target_node_id":"loc1"}), &s, "loc1"),
              None,
              "target==当前 → 不动"
          );
          assert_eq!(
              validate_transition(&serde_json::json!({}), &s, "loc1"),
              None,
              "缺字段 → 不动"
          );
      }

      #[test]
      fn build_nav_prompt_includes_player_input_and_narration() {
          let prompt = build_nav_prompt(
              "sc01", "加油站",
              "sc01 | location | 加油站\nsc02 | location | 镇中心",
              "我开车去镇中心", "GM描述了街道",
          );
          assert!(prompt.contains("玩家输入:"), "应包含玩家输入标签");
          assert!(prompt.contains("我开车去镇中心"), "应包含玩家输入内容");
          assert!(prompt.contains("GM 叙事:") || prompt.contains("GM叙事"), "应包含叙事标签");
          assert!(prompt.contains("GM描述了街道"), "应包含叙事内容");
          assert!(prompt.contains("sc01"), "应包含当前场景");
          assert!(prompt.contains("sc02"), "应包含场景列表");
      }

      #[test]
      fn build_nav_prompt_truncates_long_inputs() {
          let long_player = "x".repeat(600);
          let long_narration = "y".repeat(3000);
          let prompt = build_nav_prompt("sc01", "场景", "sc01 | l | 场景", &long_player, &long_narration);
          let player_section = prompt
              .split("玩家输入:")
              .nth(1)
              .unwrap_or("")
              .split("GM 叙事:")
              .next()
              .unwrap_or("")
              .trim()
              .to_string();
          let x_count = player_section.chars().filter(|&c| c == 'x').count();
          assert!(x_count <= 500, "player_input 应被截断至 ≤500 chars，实际 {x_count}");
          let y_count = prompt.chars().filter(|&c| c == 'y').count();
          assert!(y_count <= 2000, "narration 应被截断至 ≤2000 chars，实际 {y_count}");
      }

      #[test]
      fn build_nav_prompt_empty_player_input_still_valid() {
          let prompt = build_nav_prompt("sc01", "入口", "sc01 | l | 入口", "", "GM 叙事正文");
          assert!(
              prompt.contains("GM描述") || prompt.contains("GM 叙事正文"),
              "叙事应在 prompt 中"
          );
          assert!(prompt.contains("当前场景: sc01"), "当前场景应在");
      }
  }
  ```

  若总行数超过 400，将 `#[cfg(test)] mod tests { ... }` 整块拆为 `scene_navigation_tests.rs` 并在文件末加 `#[cfg(test)] mod tests;`。验证：

  ```bash
  wc -l /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-runtime/src/scene_navigation.rs
  ```

- [ ] **Step 3: 验证 trpg-rule-agent 已暴露所有 extract_module_scenes 需要的公开 API**

  `extract_module_scenes` 函数体调用了 `trpg_rule_agent::reader::{load_units, ModuleReaderCtx, ModuleReadout, deep_extract_scene_in_place, apply_bridge_edges}`。在 Step 2 写入后，先确认这些都是 pub：

  ```bash
  grep -n "^pub fn\|^pub async fn\|^pub struct\|^pub use" \
    /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-rule-agent/src/reader/mod.rs 2>/dev/null | head -30
  ```

  预期：`load_units`、`ModuleReaderCtx`、`ModuleReadout`、`deep_extract_scene_in_place`、`apply_bridge_edges` 均可见为 pub。若某个符号不是 pub，在 `trpg-rule-agent/src/reader/mod.rs` 中补 `pub`（只改可见性，不改实现）。

  验证文件行数仍 ≤400 行：

  ```bash
  wc -l /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-runtime/src/scene_navigation.rs
  ```

  预期：≤400。

- [ ] **Step 4: 在 trpg-runtime/src/lib.rs 添加模块声明和 pub use**

  在文件顶部 mod 声明区（约 lib.rs:27-35 附近）追加：

  ```rust
  pub mod scene_navigation;
  pub use scene_navigation::{
      extract_module_scenes, build_nav_prompt, scene_navigator, validate_transition, prefetch_frontier,
  };
  ```

  同时在 `use` 块中，确认 `trpg_rule_agent` 已在 Cargo.toml 加入后编译器不报 unresolved import（由 Step 1 保证）。

  快速验证（仅 trpg-runtime 编译）：

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
  cargo check -p trpg-runtime 2>&1 | head -40
  ```

  预期：无 error（可能有 unused import warning，后续清理）。

- [ ] **Step 5: 在 trpg-runtime 跑移入的 4 个单测，验证全绿**

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
  cargo test -p trpg-runtime scene_navigation 2>&1
  ```

  预期输出（4 个测试）：
  ```
  test scene_navigation::tests::validate_transition_is_fail_closed ... ok
  test scene_navigation::tests::build_nav_prompt_includes_player_input_and_narration ... ok
  test scene_navigation::tests::build_nav_prompt_truncates_long_inputs ... ok
  test scene_navigation::tests::build_nav_prompt_empty_player_input_still_valid ... ok
  test result: ok. 4 passed; 0 failed;
  ```

- [ ] **Step 6: trpg-api/src/lib.rs — 删除原实现，改为 re-export**

  删除 `trpg-api/src/lib.rs` 中以下全部内容（精确行范围需用实读确认，以下为参考起点）：

  - `extract_module_scenes` 函数体（doc comment 起）：lib.rs:~280-381（含 doc comment + `pub async fn extract_module_scenes` 整块）
  - `validate_transition` 函数体：lib.rs:~2016-2041
  - `SCENE_NAV_SYS` 常量：lib.rs:~2043-2048
  - `build_nav_prompt` 函数体：lib.rs:~2050-2058
  - `scene_navigator` 函数体：lib.rs:~2060-2117
  - `prefetch_frontier` 私有函数（含 `FRONTIER_PREFETCH_MAX` 常量）：lib.rs:~2119-2166
  - `scene_nav_tests` mod：lib.rs:~2168-2263

  **注意**：`continue_module_extraction`（约 lib.rs:276 附近）调用了 `extract_module_scenes`；删除函数体后，它必须通过 re-export 解析（见下方 `pub use`）。实读确认 `continue_module_extraction` 的调用是通过 crate-local 路径（无前缀），re-export 后仍可解析。

  并在文件顶部 `use` 块追加（或在适当位置）：

  ```rust
  pub use trpg_runtime::scene_navigation::{
      extract_module_scenes, build_nav_prompt, scene_navigator, validate_transition, prefetch_frontier,
  };
  ```

  （`pub use` 保持向后兼容：`trpg_api::scene_navigator`、`trpg_api::extract_module_scenes` 等现有调用点编译不变；`continue_module_extraction` 的 crate-local 调用 `extract_module_scenes(...)` 也通过 re-export 继续解析。后续 R1 整体删除 `run_turn_once`/`play_turn_sse` 后，这个 re-export 随之裁减，但本 Task 不破坏现有调用方。）

  验证 trpg-api 编译（包括 `continue_module_extraction` 调用方）：

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
  cargo check -p trpg-api 2>&1 | head -40
  ```

  预期：无 error。（`scene_nav_tests` mod 删除后原有同 crate 测试仅在 trpg-runtime 存活，无丢失。）

- [ ] **Step 7: trpg-api 的测试 — 确认原 scene_nav_tests 在 trpg-runtime 跑绿**

  原 `trpg-api` 内 `scene_nav_tests` 已在 Step 6 删除；功能等价的测试已在 Step 5 在 trpg-runtime 内跑绿。此步确认 trpg-api 自身所有其余测试仍绿（无旧测试因删除 mod 而意外消失）：

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
  cargo test -p trpg-api 2>&1 | tail -10
  ```

  预期：`test result: ok. N passed; 0 failed;`（N 为 trpg-api 其余测试数，不含已删的 scene_nav_tests，但整体不少于删除前数量减 4）。

- [ ] **Step 8: CLI 调用方 agent_play.rs 改引 trpg_runtime**

  编辑 `crates/trpg-cli/src/agent_play.rs`：

  将第 8 行：
  ```rust
  use trpg_api::extract_module_scenes;
  ```
  改为：
  ```rust
  use trpg_runtime::scene_navigation::extract_module_scenes;
  ```

  同时第 97 行（场景导航调用）保持不变（它已通过 `trpg_api::scene_navigator` re-export 路由至 runtime，此行可按需也改为 `trpg_runtime::scene_navigation::scene_navigator`）。若要清晰起见，两处统一改为 `trpg_runtime::scene_navigation::`：

  ```rust
  // 第 8 行
  use trpg_runtime::scene_navigation::{extract_module_scenes, scene_navigator};

  // 第 97 行（原 trpg_api::scene_navigator）
  if let Err(err) = scene_navigator(&db, llm.as_ref(), &session_id, mid, data_dir.as_path(), &player_input_for_nav, &streamed).await {
  ```

  `extract_module_scenes` 在 Step 2 已写为 `pub async fn`，对 trpg-cli 可见，无需额外修改。

  验证：
  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
  cargo check -p trpg-cli 2>&1 | head -40
  ```

  预期：无 error。

- [ ] **Step 9: CLI 调用方 main.rs 改引 trpg_runtime**

  编辑 `crates/trpg-cli/src/main.rs:1496`，将：
  ```rust
  if let Err(err) = trpg_api::scene_navigator(db, llm.as_ref(), session_id, mid, &default_data_dir(), user_input, &full).await {
  ```
  改为：
  ```rust
  if let Err(err) = trpg_runtime::scene_navigation::scene_navigator(db, llm.as_ref(), session_id, mid, &default_data_dir(), user_input, &full).await {
  ```

  （`trpg-cli/Cargo.toml` 已含 `trpg-runtime` 依赖，无需新增。）

  验证：
  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
  cargo check -p trpg-cli 2>&1 | head -40
  ```

  预期：无 error。

- [ ] **Step 10: workspace 全量 check + runtime 全测试绿**

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
  cargo check --workspace 2>&1 | grep -E "^error" | head -20
  cargo test -p trpg-runtime 2>&1 | tail -5
  ```

  预期：`cargo check --workspace` 无 error 行；`cargo test -p trpg-runtime` 输出 `test result: ok. N passed; 0 failed;`（其中包含 Step 5 的 4 个 scene_navigation 测试）。

- [ ] **Step 11: 确认反向依赖消除（无 trpg-api 泄漏进 trpg-gm 路径）**

  本下沉的目的是让 `trpg-gm` 的 `phase_scene_navigate`（后续 Task 4）可直接调用 `trpg_runtime::scene_navigation::scene_navigator`，无需依赖 `trpg-api`（trpg-gm 目前不依赖 trpg-api，且不应引入该依赖）。验证依赖方向：

  ```bash
  grep "trpg-api" /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/Cargo.toml
  ```

  预期：无输出（trpg-gm 不依赖 trpg-api，scene_navigator 下沉后可从 trpg-runtime 获取）。

  同时记录本任务产出的可调用签名，供后续 Task 4 `execute.rs` 的 `phase_scene_navigate` 直接引用：

  ```rust
  // 后续 Task 4 在 trpg-gm/src/execute.rs 中：
  use trpg_runtime::scene_navigation::scene_navigator;
  // 调用：
  scene_navigator(&pipeline.db, pipeline.llm.as_ref(), session_id, module_id, &pipeline.data_dir, player_input, narration).await
  ```

**提交**（Task 1 scope 内，机械搬运完成后）：

```bash
cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
git add crates/trpg-runtime/src/scene_navigation.rs \
        crates/trpg-runtime/src/lib.rs \
        crates/trpg-runtime/Cargo.toml \
        crates/trpg-api/src/lib.rs \
        crates/trpg-cli/src/agent_play.rs \
        crates/trpg-cli/src/main.rs
git commit -m "$(cat <<'EOF'
refactor: 下沉 scene_navigator 到 trpg-runtime/scene_navigation

把 scene_navigator / validate_transition / build_nav_prompt /
prefetch_frontier / extract_module_scenes 从 trpg-api 移入
trpg-runtime::scene_navigation；trpg-api 保留 pub use re-export
保向后兼容；CLI 两处改为直引 trpg_runtime；4 个单测随函数移入
trpg-runtime 并验证全绿。为 R1 Task 4 execute.rs 中
phase_scene_navigate 无依赖 trpg-api 铺路。

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>
EOF
)"
```

**Task 1 完成标准**：
1. `cargo check --workspace` 零 error。
2. `cargo test -p trpg-runtime scene_navigation` 4 个测试全绿。
3. `cargo test -p trpg-api` 无回归（原 scene_nav_tests 数量从 trpg-api 侧消失，等价测试已在 runtime 侧存活）。
4. `grep "trpg-api" crates/trpg-gm/Cargo.toml` 无输出（trpg-gm 未引入 api 依赖）。
5. `scene_navigator` 的公开签名与原 trpg-api 版本完全一致（调用方无需改动 API 接口，只改 import path）。


---

### Task 2: turn_plan.rs + turn_event.rs（声明式契约）

新建 `trpg-gm` 的两个声明式契约模块：`turn_plan.rs`（`PhaseKind` / `PhaseId` / `TurnPhasePlan` / `CANONICAL_TURN_PLAN`，15 phase 规范管线）与 `turn_event.rs`（`TurnEvent` 统一事件枚举）。两文件均**纯类型声明 + 常量**，不含执行逻辑（执行器在 Task 4 的 `execute.rs`），故无新增 crate 依赖。`TurnEvent` 引用的 `TurnOutcome`（`turn_loop.rs:27`）与 `ErrataEntry`（`errata.rs:11`）均已在 crate 内。

这是后续 Task 3（phase handlers）/ Task 4（execute_turn 解释器）的契约基线，必须先落地且与共享契约逐字一致。

**Files:**
- Create: `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/turn_plan.rs`
- Create: `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/turn_event.rs`
- Modify: `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/lib.rs`（mod 声明区 line 13 后加 `pub mod turn_event; pub mod turn_plan;`；pub use 区 line 34 后加两条 re-export）

---

- [ ] **Step 1: 建 turn_plan.rs 类型骨架（先类型不带常量，让测试可编译）**

  写入 `crates/trpg-gm/src/turn_plan.rs` 的类型部分（`CANONICAL_TURN_PLAN` 常量下一步加，先放类型让 Step 3 的失败测试能编译到“常量缺失”而非“类型缺失”）：

  ```rust
  //! 声明式回合管线契约（R1）：回合表示为有序 phase 列表，execute_turn 解释它而非过程式写死。
  //! 回合 phases 不随规则集变 → in-code 常量即足够（做成 config 文件是过度工程）。

  /// phase 在管线中的执行段：确定性头部 / agent 工具轮主体 / 后置尾部。
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum PhaseKind {
      Deterministic,
      AgentLoop,
      Postprocess,
  }

  /// 规范回合的 15 个 phase 标识。顺序即 CANONICAL_TURN_PLAN 顺序。
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum PhaseId {
      // —— 9 个确定性头部 ——
      RecordPlayerAction,
      RefreshLiveDerived,
      Reconcile,
      Gate,
      StimulusPass,
      OpposedPrepass,
      ModeInference,
      DebtLoad,
      ContextAssembly,
      // —— 1 个 agent 主体 ——
      AgentLoop,
      // —— 5 个后置尾部 ——
      VerifyAfterStream,
      Finalize,
      AuditLearning,
      SceneNavigate,
      CarryoverDebt,
  }

  /// 单个 phase 的声明：执行器按 id 调对应 phase_* 方法；conditional=true 表示按运行期条件可跳过。
  #[derive(Debug, Clone, Copy)]
  pub struct TurnPhasePlan {
      pub id: PhaseId,
      pub kind: PhaseKind,
      pub conditional: bool,
  }
  ```

  验证文件已建且类型齐全：

  ```
  grep -nE 'enum PhaseKind|enum PhaseId|struct TurnPhasePlan' crates/trpg-gm/src/turn_plan.rs
  ```
  预期：3 行命中（PhaseKind / PhaseId / TurnPhasePlan）。

- [ ] **Step 2: 在 lib.rs 挂载 turn_plan 模块 + re-export**

  Edit `crates/trpg-gm/src/lib.rs`。在 mod 声明区（line 13 `pub mod turn_loop;` 之后）加：

  ```rust
  pub mod turn_event;
  pub mod turn_plan;
  ```

  在 pub use 区末尾（line 34 `pub use turn_loop::{...};` 之后）加 turn_plan 的 re-export（turn_event 的 re-export 在 Step 5 加，此刻 turn_event.rs 尚未建）：

  ```rust
  pub use turn_plan::{PhaseId, PhaseKind, TurnPhasePlan, CANONICAL_TURN_PLAN};
  ```

  此刻 `CANONICAL_TURN_PLAN` 与 `turn_event` 模块都还不存在，编译会红——这是预期，下一步加常量、Step 4 建 turn_event。先确认 mod 行写对：

  ```
  grep -nE 'pub mod turn_plan;|pub mod turn_event;' crates/trpg-gm/src/lib.rs
  ```
  预期：2 行命中。

- [ ] **Step 3: 写 CANONICAL_TURN_PLAN 失败测试（test-first）**

  在 `crates/trpg-gm/src/turn_plan.rs` 末尾追加测试模块（断言长度=15、首尾段 kind、conditional 标志）：

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn canonical_plan_has_fifteen_phases() {
          assert_eq!(CANONICAL_TURN_PLAN.len(), 15);
      }

      #[test]
      fn first_phase_is_record_player_action() {
          assert_eq!(CANONICAL_TURN_PLAN[0].id, PhaseId::RecordPlayerAction);
          assert_eq!(CANONICAL_TURN_PLAN[0].kind, PhaseKind::Deterministic);
      }

      #[test]
      fn head_nine_are_deterministic() {
          for p in &CANONICAL_TURN_PLAN[0..9] {
              assert_eq!(p.kind, PhaseKind::Deterministic, "head phase {:?} must be Deterministic", p.id);
          }
      }

      #[test]
      fn agent_loop_is_the_single_middle_body() {
          // 第 10 项（index 9）是唯一 AgentLoop body。
          assert_eq!(CANONICAL_TURN_PLAN[9].id, PhaseId::AgentLoop);
          assert_eq!(CANONICAL_TURN_PLAN[9].kind, PhaseKind::AgentLoop);
          let agent_count = CANONICAL_TURN_PLAN.iter().filter(|p| p.kind == PhaseKind::AgentLoop).count();
          assert_eq!(agent_count, 1, "exactly one AgentLoop body");
      }

      #[test]
      fn tail_five_are_postprocess() {
          for p in &CANONICAL_TURN_PLAN[10..15] {
              assert_eq!(p.kind, PhaseKind::Postprocess, "tail phase {:?} must be Postprocess", p.id);
          }
      }

      #[test]
      fn only_scene_navigate_and_carryover_debt_are_conditional() {
          for p in CANONICAL_TURN_PLAN {
              let expect_conditional = matches!(p.id, PhaseId::SceneNavigate | PhaseId::CarryoverDebt);
              assert_eq!(p.conditional, expect_conditional, "phase {:?} conditional flag wrong", p.id);
          }
      }

      #[test]
      fn plan_order_matches_phase_id_declaration() {
          let ids: Vec<PhaseId> = CANONICAL_TURN_PLAN.iter().map(|p| p.id).collect();
          assert_eq!(ids, vec![
              PhaseId::RecordPlayerAction, PhaseId::RefreshLiveDerived, PhaseId::Reconcile,
              PhaseId::Gate, PhaseId::StimulusPass, PhaseId::OpposedPrepass,
              PhaseId::ModeInference, PhaseId::DebtLoad, PhaseId::ContextAssembly,
              PhaseId::AgentLoop,
              PhaseId::VerifyAfterStream, PhaseId::Finalize, PhaseId::AuditLearning,
              PhaseId::SceneNavigate, PhaseId::CarryoverDebt,
          ]);
      }
  }
  ```

  跑验证（预期编译失败，因 `CANONICAL_TURN_PLAN` 尚未定义）：

  ```
  cargo test -p trpg-gm turn_plan 2>&1 | tail -20
  ```
  预期：`error[E0425]: cannot find value 'CANONICAL_TURN_PLAN' in this scope`（或类似 unresolved），测试未能编译。

- [ ] **Step 4: 实现 CANONICAL_TURN_PLAN 常量 → 测试转绿**

  在 `crates/trpg-gm/src/turn_plan.rs` 的 `TurnPhasePlan` 结构体定义之后、`#[cfg(test)]` 之前插入常量。用小内部辅助让 15 条声明短而无重复：

  ```rust
  /// 规范回合管线：execute_turn 按此顺序解释。9 确定性头 + 1 agent 主体 + 5 后置尾。
  /// conditional=true 仅 SceneNavigate（仅 module_id.is_some()）/ CarryoverDebt（仅工具轮耗尽且有未决义务），余皆 false。
  pub const CANONICAL_TURN_PLAN: &[TurnPhasePlan] = &[
      det(PhaseId::RecordPlayerAction),
      det(PhaseId::RefreshLiveDerived),
      det(PhaseId::Reconcile),
      det(PhaseId::Gate),
      det(PhaseId::StimulusPass),
      det(PhaseId::OpposedPrepass),
      det(PhaseId::ModeInference),
      det(PhaseId::DebtLoad),
      det(PhaseId::ContextAssembly),
      TurnPhasePlan { id: PhaseId::AgentLoop, kind: PhaseKind::AgentLoop, conditional: false },
      post(PhaseId::VerifyAfterStream, false),
      post(PhaseId::Finalize, false),
      post(PhaseId::AuditLearning, false),
      post(PhaseId::SceneNavigate, true),
      post(PhaseId::CarryoverDebt, true),
  ];

  const fn det(id: PhaseId) -> TurnPhasePlan {
      TurnPhasePlan { id, kind: PhaseKind::Deterministic, conditional: false }
  }
  const fn post(id: PhaseId, conditional: bool) -> TurnPhasePlan {
      TurnPhasePlan { id, kind: PhaseKind::Postprocess, conditional }
  }
  ```

  跑验证（此刻 turn_event mod 尚未建，lib.rs 仍红 → 用 `--lib` 单测会因 crate 整体编译失败而带不出 turn_plan 测试。先临时不挂 turn_event：把 Step 2 已加的 `pub mod turn_event;` 与 `pub use turn_plan::...` 之外不依赖 turn_event 的部分单独验。最简做法是先完成 Step 5 建好 turn_event 再统一跑 Step 6）。本步只做静态检查 turn_plan 自身无误：

  ```
  grep -c 'TurnPhasePlan {' crates/trpg-gm/src/turn_plan.rs
  ```
  预期：≥ 2（const fn 内 2 处 + AgentLoop 内联 1 处 = 3）。turn_plan 测试在 Step 6 与 crate 整体一起跑绿。

- [ ] **Step 5: 建 turn_event.rs（TurnEvent 枚举）+ lib.rs re-export**

  写入 `crates/trpg-gm/src/turn_event.rs`：

  ```rust
  //! 统一回合事件流（R1）：execute_turn 把过去 GmLoop 的 on_delta 回调 + TurnOutcome 返回
  //! 收编成单一事件流。transport（CLI 同步 drain / API spawn drain）各自解释这些事件。

  use crate::errata::ErrataEntry;
  use crate::turn_loop::TurnOutcome;

  /// 一个回合在执行过程中向 transport 发出的事件。
  #[derive(Debug, Clone)]
  pub enum TurnEvent {
      /// 逐 token 真流式叙事增量（直通不缓冲）。
      Delta(String),
      /// 桌面骰 gate：玩家须手摇，回合在此早返。
      AwaitingPlayerRoll { check_id: String, prompt_public: String },
      /// scene_navigate 产出的场景切换。
      SceneTransition { from: String, to: String, reason: String },
      /// 后置勘误条目（不阻塞叙事，注入下一轮上下文）。
      Errata(ErrataEntry),
      /// 叙事完成、尾部 phase 开始——transport 据此决定前台/后台执行尾部。
      PostprocessScheduled,
      /// 回合终态。
      TurnComplete { outcome: TurnOutcome },
  }
  ```

  Edit `crates/trpg-gm/src/lib.rs`，在 Step 2 加的 `pub use turn_plan::{...};` 之后追加 turn_event re-export：

  ```rust
  pub use turn_event::TurnEvent;
  ```

  确认 re-export 行落地：

  ```
  grep -nE 'pub use turn_event::TurnEvent;|pub use turn_plan::\{' crates/trpg-gm/src/lib.rs
  ```
  预期：2 行命中。

- [ ] **Step 6: 写 TurnEvent 构造 + Debug/Clone 测试，整 crate 跑绿**

  在 `crates/trpg-gm/src/turn_event.rs` 末尾追加测试（断言各变体可构造、Debug 可格式化、Clone 工作）：

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::errata::ErrataEntry;
      use crate::turn_loop::TurnOutcome;
      use chrono::Utc;
      use trpg_agent::VerifierFindingKind;

      #[test]
      fn all_variants_construct_and_clone_and_debug() {
          let errata = ErrataEntry {
              kind: VerifierFindingKind::OmittedVisibleResult,
              detail: "x".into(),
              turn_id: "t1".into(),
              created_at: Utc::now(),
          };
          let events = vec![
              TurnEvent::Delta("hi".into()),
              TurnEvent::AwaitingPlayerRoll { check_id: "c1".into(), prompt_public: "roll".into() },
              TurnEvent::SceneTransition { from: "a".into(), to: "b".into(), reason: "moved".into() },
              TurnEvent::Errata(errata),
              TurnEvent::PostprocessScheduled,
              TurnEvent::TurnComplete { outcome: TurnOutcome::Narration("done".into()) },
          ];
          for e in &events {
              let cloned = e.clone();
              assert!(!format!("{cloned:?}").is_empty());
          }
          assert_eq!(events.len(), 6);
      }

      #[test]
      fn turn_complete_carries_awaiting_outcome() {
          let e = TurnEvent::TurnComplete {
              outcome: TurnOutcome::AwaitingPlayerRoll { check_id: "c1".into(), prompt_public: "roll".into() },
          };
          match e {
              TurnEvent::TurnComplete { outcome: TurnOutcome::AwaitingPlayerRoll { check_id, .. } } => {
                  assert_eq!(check_id, "c1");
              }
              _ => panic!("expected TurnComplete with AwaitingPlayerRoll"),
          }
      }
  }
  ```

  跑全 crate 测试（turn_plan + turn_event 测试一并验证，crate 此刻应可完整编译）：

  ```
  cargo test -p trpg-gm turn_plan 2>&1 | tail -15
  cargo test -p trpg-gm turn_event 2>&1 | tail -15
  ```
  预期：两条命令均 `test result: ok.`，turn_plan 7 个测试 + turn_event 2 个测试全 passed，0 failed。

- [ ] **Step 7: 确认无回归 + 零规则集硬编码 + 文件行数**

  ```
  cargo test -p trpg-gm 2>&1 | tail -8
  ```
  预期：`test result: ok.`，既有 trpg-gm 测试无回归。

  零规则集硬编码守卫（两新文件不得出现任何规则集名）：

  ```
  grep -niE 'coc|cthulhu|cyberpunk|triangle|orc|d_?and_?d|dnd|fate|sword|剑世界|call_of' crates/trpg-gm/src/turn_plan.rs crates/trpg-gm/src/turn_event.rs
  ```
  预期：无输出（exit 1）。

  文件行数 ≤400：

  ```
  wc -l crates/trpg-gm/src/turn_plan.rs crates/trpg-gm/src/turn_event.rs
  ```
  预期：两文件各远低于 400 行（turn_plan ≈ 130，turn_event ≈ 70）。

- [ ] **Step 8: commit 本任务段**

  仅暂存本任务段 scope 内的两新文件 + lib.rs：

  ```
  git -C /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula status --short --branch
  git -C /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula add crates/trpg-gm/src/turn_plan.rs crates/trpg-gm/src/turn_event.rs crates/trpg-gm/src/lib.rs
  git -C /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula commit -m "$(cat <<'EOF'
  R1 Task 2: turn_plan + turn_event declarative contracts

  新增 trpg-gm 声明式回合契约：CANONICAL_TURN_PLAN（15 phase：9 确定性头
  + 1 agent 主体 + 5 后置尾，SceneNavigate/CarryoverDebt conditional）与
  TurnEvent 统一事件枚举。纯类型 + 常量，无执行逻辑（execute_turn 在后续任务段）。

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  EOF
  )"
  ```
  预期：commit 成功，报告 commit hash 进 handoff。

  > 注：env 中 git 仓库状态为 “Is directory a git repo: No”。若本仓库实际未启用 git（无 `.git`），跳过 Step 8，在 handoff 注明两新文件 + lib.rs diff 已落工作副本（与 memory 记录的「无 git，工作副本直接落地」一致）。

---

**任务段说明（给整合者）：**
- 本段产物为纯声明式契约，**不引入任何新 crate 依赖**（`tokio-stream` 等供 Task 4 `execute.rs` 用时再加到 `crates/trpg-gm/Cargo.toml`）。`TurnEvent` 引用的 `TurnOutcome`（`turn_loop.rs:27`）与 `ErrataEntry`（`errata.rs:11`）均已在 crate 内，re-export 路径用契约指定的 `crate::turn_loop::TurnOutcome` / `crate::errata::ErrataEntry`。
- lib.rs 改动点经实读确认：mod 声明区为 `crates/trpg-gm/src/lib.rs:1-13`（在 `pub mod turn_loop;` 后插 2 行），pub use 区为 `:15-34`（在末行 `pub use turn_loop::{...};` 后追加 2 行 re-export）。
- 与共享契约逐字对齐：`PhaseId` 15 变体顺序、`PhaseKind` 三态、`TurnPhasePlan` 三字段、`CANONICAL_TURN_PLAN` 的 conditional 仅 `SceneNavigate`/`CarryoverDebt` 为 true，`TurnEvent` 六变体签名全部一致，供 Task 3（phase handlers）/ Task 4（`execute_turn` 解释器）直接消费。


---

### Task 3: turn_loop.rs 确定性头部抽成 `TurnPipeline` phase handlers

把 `GmLoop::run_gm_turn`（`crates/trpg-gm/src/turn_loop.rs:32-315`）的过程式确定性头部（record → refresh → reconcile → gate → stimulus_pass → opposed_prepass → mode_inference → debt_load → context_assembly）抽成 `TurnPipeline` 上的 9 个 `phase_*` async 方法，**零行为改动**。本任务只抽方法、`run_gm_turn` 重写为按序调用这些方法，**不引入 plan 解释器**（那是 T4，本任务为其铺路）。

前置：Task 2 已建好 `turn_plan.rs`（`PhaseId`/`PhaseKind`/`TurnPhasePlan`/`CANONICAL_TURN_PLAN`）与 `turn_event.rs`（`TurnEvent`）。本任务只用到 `PhaseId` 命名对齐（方法名 `phase_<snake_id>`），不依赖 plan 解释逻辑。

**关键事实（已实读确认）**：
- 头部各步通过 `self.engine` / `self.llm` / `self.data_dir` / `self.cfg` / `self.errata` / `self.obligations` / `self.scene_extractor` / `self.ctx_provider` / `self.gate_resolver` 等 `GmLoop` 字段工作。
- 头部产出的累积状态：`ledger: TurnLedger`、`resolved_gate_facts: Vec<String>`、`obligations_block: Option<String>`、`compiled: CompiledContext`、`gm_skill: String`、`mode_id: Option<String>`、`mode_manifest: Option<ModeManifest>`、`mode_tools: Option<ToolRegistry>`、`max_tool_rounds: u8`、`effect_closure_per_cluster: bool`、`active_frames: Vec<StateFrame>`、`state_agent: RuntimeState`、`errata_blocks: Vec<String>`、`messages: TurnMessages`。这些当前都是 `run_gm_turn` 的局部变量；抽方法后改为 `TurnContext` 字段。
- `gate` 步调 `crate::gate::resolve_pending_gate`（pub(crate)，签名 `(engine, gate_resolver, request, current_scene_id, user_input, &mut ledger, &mut resolved_gate_facts)`）。
- `mode_inference` 含 `?` 早返（`mode::active_mode_manifest` Err = 配置错误终止回合）；`context_assembly` 含 `?` 早返（`prepare_turn_context` / `validate_compiled_budget` / `load_gm_skill_with_mode` / `ToolRegistry::for_mode`）。phase handler 必须保留这些 `Result` 传播语义。
- 头部把 `messages` 与 `mode_tools` 都喂给后续工具轮；`agent_loop` 体（T4 收编）暂留在 `run_gm_turn` 内不动。

设计取舍（本任务定）：`TurnPipeline` 不持有 `GmLoop` 的所有权，而是**借引** `&mut GmLoop`（头部要 `&mut self.obligations` / `&mut self.errata`）+ 持一个 `TurnContext`（累积上下文）。`run_gm_turn` 内先建 `TurnContext`，依次 `self.phase_*(&mut ctx, &input).await?`，再用 `ctx` 字段跑原工具轮 + 尾部（字节不变）。`TurnContext` 落 `turn_loop.rs` 内（私有 struct，本任务不导出）。

**Files:**
- Modify: `crates/trpg-gm/src/turn_loop.rs` (1-373；新增 `TurnContext` struct + 9 个 `impl GmLoop` 的 `phase_*` 方法；重写 `run_gm_turn:32-315` 头部为按序调用)
- Modify: `crates/trpg-gm/src/turn_loop_tests.rs` (尾部追加 2 个 handler 单测，复用现有 `loop_fixture`/`MockLlm`)

---

- [ ] **Step 1: 写失败测试 — `phase_context_assembly` 产非空块**
  在 `crates/trpg-gm/src/turn_loop_tests.rs` 末尾（`#[cfg(test)] mod tests` 块内、最后一个测试函数之后）追加。复用现有 `loop_fixture`（其 `ctx_provider` seam 返回 `prefix_text:"BP1"/pinned_text:"BP2"/dynamic_text:"BP3"`，gm_skill fixture 已写 "test gm skill"）：
  ```rust
      #[tokio::test]
      async fn phase_context_assembly_produces_nonempty_blocks() {
          // 纯重构护栏：context_assembly handler 经 ctx_provider seam + gm_skill
          // fixture 必产出非空 compiled + gm_skill + 组装好的 messages（mode=None
          // 退化两级，字节与旧头部一致）。
          let (mut gm, _llm, request, state) = loop_fixture(vec![], ToolRegistry::from_tools(vec![]), 1);
          let input = GmTurnInput { request: &request, state: &state, user_input: "I open the door.", history: &[], recent_transcript: None };
          let mut ctx = TurnContext::new();
          // 头部前序 phase 是 context_assembly 的前置（debt_load 填 obligations_block、
          // mode_inference 填 mode_id/manifest）——按序跑到 context_assembly。
          gm.phase_record_player_action(&mut ctx, &input).await;
          gm.phase_refresh_live_derived(&mut ctx, &input).await;
          gm.phase_reconcile(&mut ctx, &input).await;
          gm.phase_gate(&mut ctx, &input).await;
          gm.phase_stimulus_pass(&mut ctx, &input).await;
          gm.phase_opposed_prepass(&mut ctx, &input).await;
          gm.phase_mode_inference(&mut ctx, &input).await.unwrap();
          gm.phase_debt_load(&mut ctx, &input).await;
          gm.phase_context_assembly(&mut ctx, &input).await.unwrap();
          assert_eq!(ctx.compiled.prefix_text, "BP1");
          assert!(!ctx.gm_skill.trim().is_empty(), "gm_skill must be loaded by context_assembly");
          // messages 已组装：含 system/user 至少 2 条（assemble 不会空）。
          assert!(ctx.messages.to_request_messages().len() >= 2, "TurnMessages must be assembled");
          assert_eq!(ctx.mode_id, None, "no active frame ⇒ no mode (byte-identical to phase-2)");
      }
  ```

- [ ] **Step 2: 写失败测试 — `phase_debt_load` 填 carryover block 通路**
  紧接 Step 1 追加（验证 debt_load handler 把 obligations 装载并产出 `obligations_block` 字段，空账本时为 `None`，fail-closed 不 panic）：
  ```rust
      #[tokio::test]
      async fn phase_debt_load_initializes_obligations_no_panic() {
          // 纯重构护栏：debt_load handler begin_turn + 装载 dues（lazy pool 下 db
          // 调用 unwrap_or_default 兜底，绝不 panic），空账本 ⇒ obligations_block None。
          let (mut gm, _llm, request, state) = loop_fixture(vec![], ToolRegistry::from_tools(vec![]), 1);
          let input = GmTurnInput { request: &request, state: &state, user_input: "wait", history: &[], recent_transcript: None };
          let mut ctx = TurnContext::new();
          gm.phase_mode_inference(&mut ctx, &input).await.unwrap();
          gm.phase_debt_load(&mut ctx, &input).await;
          assert_eq!(ctx.obligations_block, None, "empty ledger ⇒ no carryover block");
      }
  ```

- [ ] **Step 3: 跑测试确认编译失败（红）**
  `TurnContext` / `phase_*` 尚不存在，预期编译错。
  ```bash
  cargo test -p trpg-gm --lib phase_ 2>&1 | tail -20
  ```
  预期：`error[E0433]/error[E0599]: cannot find ... TurnContext` / `no method named phase_record_player_action found for ... GmLoop`。**确认是编译红、不是别的失败。**

- [ ] **Step 4: 定义 `TurnContext` struct（累积上下文容器）**
  在 `crates/trpg-gm/src/turn_loop.rs` 的 `GmTurnInput` 定义之后（`:29` 后）插入。字段=头部当前所有局部变量；初值用各类型 `Default`/空。`mode::ModeManifest`、`trpg_model::StateFrame` 需在文件顶部 use（`StateFrame` 现已经由 `list_active_state_frames` 间接用到但未直接 import——加 `use trpg_model::StateFrame;` 到顶部 use 区）：
  ```rust
  /// 头部确定性 phase 的累积上下文（原 run_gm_turn 局部变量集中到此，供 phase
  /// handler 顺序填充；本任务私有、不导出）。
  struct TurnContext {
      ledger: TurnLedger,
      resolved_gate_facts: Vec<String>,
      active_frames: Vec<StateFrame>,
      mode_id: Option<String>,
      mode_manifest: Option<crate::mode::ModeManifest>,
      obligations_block: Option<String>,
      state_agent: RuntimeState,
      compiled: CompiledContext,
      gm_skill: String,
      errata_blocks: Vec<String>,
      mode_tools: Option<ToolRegistry>,
      max_tool_rounds: u8,
      effect_closure_per_cluster: bool,
      messages: TurnMessages,
  }
  impl TurnContext {
      fn new() -> Self {
          Self {
              ledger: TurnLedger::new(),
              resolved_gate_facts: Vec::new(),
              active_frames: Vec::new(),
              mode_id: None,
              mode_manifest: None,
              obligations_block: None,
              state_agent: RuntimeState::default(),
              compiled: CompiledContext::default(),
              gm_skill: String::new(),
              errata_blocks: Vec::new(),
              mode_tools: None,
              max_tool_rounds: 0,
              effect_closure_per_cluster: false,
              messages: TurnMessages::default(),
          }
      }
  }
  ```
  注：`TurnMessages` 需可 `Default`。先查：
  ```bash
  grep -n 'derive\|impl Default for TurnMessages\|pub struct TurnMessages' crates/trpg-gm/src/prompts.rs | head
  ```
  若 `TurnMessages` 未实现 `Default`，本步改用 `Option<TurnMessages>` 字段（`messages: Option<TurnMessages>`，初值 `None`，`context_assembly` 内 `ctx.messages = Some(...)`，工具轮取 `ctx.messages.as_mut().expect(...)`）。**先以实读结果为准选其一**，不要两种都写。

- [ ] **Step 5: `cargo check`（仅加 struct，增量验证编译干净）**
  ```bash
  cargo check -p trpg-gm 2>&1 | tail -8
  ```
  预期：除 `TurnContext`/字段 `never read`/`never constructed` 的 dead_code warning 外无 error（方法尚未加）。

- [ ] **Step 6: 抽 `phase_record_player_action` + `phase_refresh_live_derived` + `phase_reconcile`（3 个无返回值 handler）**
  在 `impl GmLoop` 块内（`run_gm_turn` 之后、`verify_after_stream:320` 之前）插入。逐字搬 `:34`/`:35`/`:38` 三行，签名 `&self`（这三步不改 self）：
  ```rust
      /// PhaseId::RecordPlayerAction — world event PlayerAction（spec §4 头部第 1）。
      async fn phase_record_player_action(&self, _ctx: &mut TurnContext, input: &GmTurnInput<'_>) {
          let _ = self.engine.record_world_event(&input.request.session_id, Some(&input.request.turn_id), None, WorldEventKind::PlayerAction, json!({"input": input.user_input}), Visibility::GmOnly).await;
      }
      /// PhaseId::RefreshLiveDerived — refresh_actor_live_derived（spec §4 头部第 2）。
      async fn phase_refresh_live_derived(&self, _ctx: &mut TurnContext, input: &GmTurnInput<'_>) {
          let _ = self.engine.refresh_actor_live_derived(&input.request.session_id, input.request.viewer.actor_id.as_deref().unwrap_or("pc.current")).await;
      }
      /// PhaseId::Reconcile — 机制对账（spec §4 顺序：gate 结算之前显式调，幂等）。
      async fn phase_reconcile(&self, _ctx: &mut TurnContext, input: &GmTurnInput<'_>) {
          let _ = InteractionLifecycleKernel::new(self.engine.db.clone()).reconcile_session(&input.request.session_id).await;
      }
  ```
  `cargo check -p trpg-gm 2>&1 | tail -5` — 预期编译干净（仅 dead_code warning）。

- [ ] **Step 7: 抽 `phase_gate`（gate 结算 → 填 ledger/resolved_gate_facts）**
  搬 `:39-43`（`TurnLedger::new` 改在 `TurnContext::new` 已建，故 handler 只调 `resolve_pending_gate` 写入 `ctx.ledger`/`ctx.resolved_gate_facts`）：
  ```rust
      /// PhaseId::Gate — request_player_roll 闸门结算（gate.rs 单点；含裸 "roll"
      /// 兜底 + Err 折叠 + C7 effect_policy 强制；spec §4 头部第 4）。
      async fn phase_gate(&self, ctx: &mut TurnContext, input: &GmTurnInput<'_>) {
          crate::gate::resolve_pending_gate(&self.engine, self.gate_resolver.as_ref(), input.request, input.state.scene_id.as_deref(), input.user_input, &mut ctx.ledger, &mut ctx.resolved_gate_facts).await;
      }
  ```
  `cargo check -p trpg-gm 2>&1 | tail -5` — 预期干净。

- [ ] **Step 8: 抽 `phase_stimulus_pass`（hook dues + 刺激预 pass，dues 暂存进 ctx 给 debt_load 吸收）**
  原 `:47-68` 产出 `hook_dues` 与 `stimulus_dues` 两个 `Vec<MechanicDue>`，被后面 `debt_load` 的 `absorb_dues` 吃掉。为保边界清晰，把这两批 dues 存进 `TurnContext` 新增的临时字段 `pending_dues: Vec<MechanicDue>`（在 Step 4 struct 里补上该字段 + `pending_dues: Vec::new()` 初值，并 `use trpg_model::MechanicDue;`）。handler 把 `hook_dues` 和 `stimulus_dues` 依次 `extend` 进 `ctx.pending_dues`（顺序=hook 先、stimulus 后，与原 `:96-97` `absorb_dues(hook_dues)` 然后 `absorb_dues(stimulus_dues)` 一致）：
  ```rust
      /// PhaseId::StimulusPass — TurnStart hook dues + 语义被动刺激预 pass（J2 SAN）。
      /// fail-closed：门关/无目录/LLM 失败 → 空，绝不阻断回合（unwrap_or_default）。
      async fn phase_stimulus_pass(&self, ctx: &mut TurnContext, input: &GmTurnInput<'_>) {
          let hook_dues = trpg_mechanics::RefereeCombatService::new(self.engine.db.clone())
              .dues_for_hook(&input.request.session_id, &input.request.turn_id, &input.request.ruleset_id, &trpg_mechanics::watcher::HookEvent::TurnStart)
              .await
              .unwrap_or_default();
          ctx.pending_dues.extend(hook_dues);
          let stimulus_dues = if crate::stimulus::stimulus_pass_enabled() {
              match self.engine.db.load_rule_kernel(&input.request.ruleset_id).await {
                  Ok(Some(kernel)) if !kernel.mechanics_catalog.is_empty() => {
                      let recent = input.recent_transcript
                          .or_else(|| input.history.iter().rev().find(|m| m.role == "assistant").map(|m| m.content.as_str()));
                      let candidates = crate::stimulus::stimulus_due_candidates(&self.llm, &kernel.mechanics_catalog, &input.request.session_id, &input.request.turn_id, input.user_input, recent).await;
                      trpg_mechanics::RefereeCombatService::new(self.engine.db.clone())
                          .admit_dues(&input.request.session_id, candidates)
                          .await
                          .unwrap_or_default()
                  }
                  _ => Vec::new(),
              }
          } else { Vec::new() };
          ctx.pending_dues.extend(stimulus_dues);
      }
  ```
  注：debt_load 现行序是 `absorb_dues(leftover_dues)` → `absorb_dues(hook_dues)` → `absorb_dues(stimulus_dues)`（`:95-97`）。把 hook+stimulus 合并进 `pending_dues` 后顺序仍为 hook→stimulus，且 leftover 在 debt_load 内单独先吸收，**总吸收序不变**（absorb_dues 按 due_id 去重，hook/stimulus 内部 due_id 不同，顺序对去重结果无影响）。`cargo check -p trpg-gm 2>&1 | tail -5` — 预期干净。

- [ ] **Step 9: 抽 `phase_opposed_prepass`（对抗语义预 pass → ctx.opposed_binding）**
  `opposed_binding` 被工具轮的 `ToolCtx` 用到（`:165`），故必须存进 `TurnContext`。Step 4 struct 补字段 `opposed_binding: Option<crate::opposed_prepass::OpposedBinding>`（初值 `None`）。搬 `:75`：
  ```rust
      /// PhaseId::OpposedPrepass — 回合头部对抗绑定（现搓防御方 NPC + 备 OpposedBinding
      /// 供 roll_check 注入）。fail-closed：门关/无 NPC/无攻击意图 → None。
      async fn phase_opposed_prepass(&self, ctx: &mut TurnContext, input: &GmTurnInput<'_>) {
          ctx.opposed_binding = crate::opposed_prepass::prepare_binding(&self.engine, &self.llm, input.request, input.state, input.user_input, input.recent_transcript, input.history).await;
      }
  ```
  `cargo check -p trpg-gm 2>&1 | tail -5` — 预期干净。

- [ ] **Step 10: 抽 `phase_mode_inference`（→ Result，保留 `?` 早返）**
  搬 `:81-83`。`active_mode_manifest` 的 `?` 必须保留（配置错误终止回合）。写入 `ctx.active_frames`/`ctx.mode_manifest`/`ctx.mode_id`：
  ```rust
      /// PhaseId::ModeInference — active state_frame → mode 推导（三期 §4.1）。manifest
      /// 损坏 → Err fail-closed 终止回合；db 失败 unwrap_or_default 绝不阻断。
      async fn phase_mode_inference(&self, ctx: &mut TurnContext, input: &GmTurnInput<'_>) -> Result<()> {
          ctx.active_frames = self.engine.db.list_active_state_frames(&input.request.session_id, 8).await.unwrap_or_default();
          ctx.mode_manifest = crate::mode::active_mode_manifest(&self.data_dir, &ctx.active_frames)?;
          ctx.mode_id = ctx.mode_manifest.as_ref().map(|m| m.mode_id.clone());
          Ok(())
      }
  ```
  `cargo check -p trpg-gm 2>&1 | tail -5` — 预期干净。

- [ ] **Step 11: 抽 `phase_debt_load`（→ &mut self，begin_turn + 吸收 dues + carryover_block）**
  搬 `:88-99`。本步需 `&mut self`（点改 `self.obligations`）。leftover_dues 在此 db 取（`:94`），pending_dues 由 stimulus_pass 备好。吸收序：leftover → pending（=hook→stimulus）。产出 `ctx.obligations_block`：
  ```rust
      /// PhaseId::DebtLoad — B6 债务装载（spec §5.3）：begin_turn + mode 退出义务
      /// re-seed + leftover/hook/stimulus dues 吸收（按 due_id 去重）→ carryover block。
      async fn phase_debt_load(&mut self, ctx: &mut TurnContext, input: &GmTurnInput<'_>) {
          self.obligations.begin_turn(&input.request.turn_id);
          if let Some(m) = &ctx.mode_manifest {
              self.obligations.ensure_mode_exit_obligations(&m.mode_id, &m.exit_obligations);
          }
          let leftover_dues = self.engine.db.list_open_mechanic_dues(&input.request.session_id).await.unwrap_or_default();
          self.obligations.absorb_dues(leftover_dues);
          self.obligations.absorb_dues(std::mem::take(&mut ctx.pending_dues));
          ctx.obligations_block = self.obligations.carryover_block();
      }
  ```
  注：原 `:96-97` 分两次 `absorb_dues(hook_dues)`/`absorb_dues(stimulus_dues)`；此处合并为单次 `absorb_dues(pending_dues)`，`pending_dues` 内顺序=hook→stimulus（Step 8 保证），absorb_dues 逐条去重入账，**等价**。`cargo check -p trpg-gm 2>&1 | tail -5` — 预期干净。

- [ ] **Step 12: 抽 `phase_context_assembly`（→ Result，最大块；保留全部 `?` 早返）**
  搬 `:101-154` 全段（state_agent 置位 → ctx_provider/prepare_turn_context → validate_budget → gm_skill 四级合并 → mode_catalog 注入 → errata_blocks → novelty → DynamicTailInput → TurnMessages::assemble → mode_tools → max_tool_rounds/effect_closure）。写入 `ctx` 对应字段。注意 `tail` 借 `ctx.resolved_gate_facts`/`ctx.errata_blocks`/`ctx.obligations_block`，`assemble` 借 `ctx.compiled`/`ctx.gm_skill`，需先把 compiled/gm_skill/errata_blocks 填好再组 messages（与原顺序一致）：
  ```rust
      /// PhaseId::ContextAssembly — prepare_turn_context + 四级 gm_skill 合并 + mode
      /// 目录联动 + errata/novelty BP3 块 + TurnMessages 组装 + mode 工具/节拍参数。
      /// fail-closed：budget 超限 / gm_skill 缺 / 未知工具名 → Err 终止回合。
      async fn phase_context_assembly(&self, ctx: &mut TurnContext, input: &GmTurnInput<'_>) -> Result<()> {
          ctx.state_agent = input.state.clone();
          ctx.state_agent.agent_loop_protocol = true;
          ctx.compiled = match &self.ctx_provider {
              Some(provider) => provider(input.request, &ctx.state_agent),
              None => self.engine.prepare_turn_context(input.request, &ctx.state_agent, Some(input.user_input), input.recent_transcript).await?,
          };
          crate::prompts::validate_compiled_budget(&ctx.compiled, input.request)?;
          let mut gm_skill = load_gm_skill_with_mode(&self.data_dir, &input.request.ruleset_id, ctx.mode_id.as_deref())?;
          if let Some(manifest) = &ctx.mode_manifest {
              match self.engine.db.load_rule_kernel(&input.request.ruleset_id).await {
                  Ok(Some(kernel)) => {
                      if let Some(section) = crate::mode_catalog::mode_catalog_section(&manifest.mode_id, &manifest.catalog_filter, &kernel.mechanics_catalog) {
                          gm_skill.push_str("\n\n---\n\n");
                          gm_skill.push_str(&section);
                      }
                  }
                  Ok(None) => tracing::warn!(ruleset_id = %input.request.ruleset_id, "mode catalog subset skipped: no active rule kernel"),
                  Err(err) => tracing::warn!(error = %err, "mode catalog subset skipped: rule kernel load failed"),
              }
          }
          ctx.gm_skill = gm_skill;
          if let Some(block) = self.errata.errata_block() { ctx.errata_blocks.push(block); }
          if let Some(block) = self.errata.standing_reminder_block() { ctx.errata_blocks.push(block); }
          if ctx.mode_id.is_some() {
              if let Some(block) = crate::tools::frame::novelty_block(&ctx.active_frames) { ctx.errata_blocks.push(block); }
          }
          let tail = DynamicTailInput { user_input: input.user_input, resolved_gate_facts: &ctx.resolved_gate_facts, errata_blocks: &ctx.errata_blocks, obligations_block: ctx.obligations_block.as_deref() };
          ctx.messages = TurnMessages::assemble(&ctx.compiled, &ctx.gm_skill, input.history, &tail);
          ctx.mode_tools = match ctx.mode_id.as_deref() {
              Some(mode) => Some(ToolRegistry::for_mode(&self.data_dir, Some(mode))?),
              None => None,
          };
          ctx.max_tool_rounds = ctx.mode_manifest.as_ref().and_then(|m| m.tempo.max_tool_rounds).unwrap_or(self.cfg.max_tool_rounds);
          ctx.effect_closure_per_cluster = ctx.mode_manifest.as_ref().and_then(|m| m.tempo.effect_closure_per_cluster).unwrap_or(false);
          Ok(())
      }
  ```
  注：原 `:149` `schemas` 与 `:165` `ToolCtx`、`:166` `'rounds` 循环用 `mode_tools.as_ref().unwrap_or(&self.tools)`、`opposed_binding.as_ref()`、`mode_id.as_deref()` 等——这些留在 `run_gm_turn` 工具轮，改读 `ctx.mode_tools`/`ctx.opposed_binding`/`ctx.mode_id`（Step 13）。`cargo check -p trpg-gm 2>&1 | tail -8` — 预期干净（除 `phase_*` 全 dead_code，`run_gm_turn` 尚未改用）。

- [ ] **Step 13: 重写 `run_gm_turn` 头部为按序调用 9 个 handler**
  删 `run_gm_turn:33-154`（从 `// —— 1. 确定性头部` 到 `effect_closure_per_cluster` 那行，**含**），替换为建 `TurnContext` + 顺序调用。把工具轮起点开始的局部变量引用改为 `ctx.*`：
  ```rust
          let mut ctx = TurnContext::new();
          self.phase_record_player_action(&mut ctx, &input).await;
          self.phase_refresh_live_derived(&mut ctx, &input).await;
          self.phase_reconcile(&mut ctx, &input).await;
          self.phase_gate(&mut ctx, &input).await;
          self.phase_stimulus_pass(&mut ctx, &input).await;
          self.phase_opposed_prepass(&mut ctx, &input).await;
          self.phase_mode_inference(&mut ctx, &input).await?;
          self.phase_debt_load(&mut ctx, &input).await;
          self.phase_context_assembly(&mut ctx, &input).await?;
  ```
  然后把工具轮~尾部（原 `:155-314`）所有头部局部变量引用替换为 `ctx.*`：`ledger`→`ctx.ledger`、`messages`→`ctx.messages`、`compiled`→`ctx.compiled`、`schemas`（由 `ctx.mode_tools.as_ref().unwrap_or(&self.tools).schemas()` 重算）、`tools`（`ctx.mode_tools.as_ref().unwrap_or(&self.tools)`）、`state_agent`→`ctx.state_agent`、`mode_id`→`ctx.mode_id`、`opposed_binding`→`ctx.opposed_binding`、`max_tool_rounds`→`ctx.max_tool_rounds`、`effect_closure_per_cluster`→`ctx.effect_closure_per_cluster`。`obligations_cell`/`visible_text`/`awaiting`/`narrated`/`resolved_gate_facts`（DynamicTail 内已用，工具轮不再引）保持原样为 `run_gm_turn` 局部。**逐处替换，借用作用域保持原块结构（`obligations_cell` 块、`'rounds` 循环不动）。** 关键：原 `:149` `let schemas = ...` 与 `:164` `let tools = ...`、`:165` `ToolCtx{... opposed_binding: opposed_binding.as_ref()}` 现都从 `ctx` 取。
  实现完跑：
  ```bash
  cargo check -p trpg-gm 2>&1 | tail -12
  ```
  预期：编译通过（dead_code warning 消失，因 handler 全被 run_gm_turn 调用）。若报 `ctx` 借用冲突（如 `obligations_cell` 块内同时不可变借 `ctx.mode_tools` 又可变借别处），**stop 并核对**：原代码 `tools`/`schemas` 在 `obligations_cell` 块外算好（`schemas` 是 `Vec<Value>` clone-able、`tools` 引用块内只读），保持同款——`schemas` clone 出来、`tools` 块内重新 `let tools = ctx.mode_tools.as_ref().unwrap_or(&self.tools);` 局部借。

- [ ] **Step 14: 跑 Step 1-2 新测试（绿）**
  ```bash
  cargo test -p trpg-gm --lib phase_ 2>&1 | tail -15
  ```
  预期：`phase_context_assembly_produces_nonempty_blocks ... ok`、`phase_debt_load_initializes_obligations_no_panic ... ok`，`test result: ok. 2 passed`。

- [ ] **Step 15: 全量回归 — trpg-gm 现有单测零回归（验收核心）**
  纯重构验收=全套绿。
  ```bash
  cargo test -p trpg-gm 2>&1 | tail -25
  ```
  预期：`turn_loop_tests`（bare_roll_reply / 私骰 / gate 终态 / 工具轮 / errata / 债务等全部）、`turn_loop_mode_tests`、`mode_tests`、`obligations_tests` 全 `ok`，`test result: ok. N passed; 0 failed`（N=改前数量；**任何一条红=行为漂移，回到 Step 13 逐字核对搬运**）。若某测试因 lazy pool 无 DB 而 ignored/panic，那是改前既有状态（`loop_fixture` 用 `connect_lazy` + `ctx_provider` seam 设计本就绕 DB），对比改前 baseline 确认数量一致。

- [ ] **Step 16: 文件行数 + 零硬编码守卫**
  ```bash
  wc -l crates/trpg-gm/src/turn_loop.rs
  grep -niE 'coc|call_of_cthulhu|cyberpunk|cthulhu|d_?and_?d|triangle|sanity|理智' crates/trpg-gm/src/turn_loop.rs
  ```
  预期：行数较改前（390）略增（+约 60-90 行 handler 模板），若逼近/超 400 行——本任务**不拆文件**（拆 turn_loop 是 T4 引入 plan 解释器时的自然边界，本任务保持单文件以最小化 diff，仅在 PR 描述注明行数）；grep 命中 0（头部逻辑全数据驱动，无规则集名）。若 grep 命中，定位为搬运误引，修正。

- [ ] **Step 17: commit**
  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula && \
  git add crates/trpg-gm/src/turn_loop.rs crates/trpg-gm/src/turn_loop_tests.rs && \
  git commit -m "$(cat <<'EOF'
R1 T3: extract run_gm_turn deterministic head into TurnPipeline phase handlers

Pure refactor (zero behavior change): the 9 procedural deterministic head
steps (record/refresh/reconcile/gate/stimulus/opposed/mode/debt/context) are
extracted into phase_<id> async methods on GmLoop, accumulating into a private
TurnContext. run_gm_turn now calls them in sequence; the tool-loop and tail
are untouched, reading head outputs from ctx. Lays groundwork for T4's plan
interpreter. trpg-gm full test suite green, +2 handler unit tests.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
  ```

---

**Notes for integrator:**
- 本任务**不引入** `plan` 解释器、`execute_turn`、`TurnEvent` 发射——`run_gm_turn` 仍是 `on_delta` 回调 + `TurnOutcome` 返回的原签名，只是头部内部重构。T4 将把这些 `phase_*` 方法搬进 `TurnPipeline`（execute.rs）并由 `CANONICAL_TURN_PLAN` 解释调度。
- `TurnContext` 本任务私有于 `turn_loop.rs`（不在 lib.rs 导出）；T4 会决定是否上移到 `execute.rs` 并改名/导出。
- 共享契约约定 phase 方法名 `phase_<snake_id>` 对齐 `PhaseId`——本任务 9 个 Deterministic head 名称已按 `PhaseId::{RecordPlayerAction,RefreshLiveDerived,Reconcile,Gate,StimulusPass,OpposedPrepass,ModeInference,DebtLoad,ContextAssembly}` 的 snake_case 命名，T4 可直接 `match PhaseId` 分发。
- `phase_debt_load` 是唯一 `&mut self`（点改 `self.obligations`）；其余 head handler `&self`。T4 收编 `TurnPipeline` 时需注意 obligations 的 `Mutex` cell 模式（原 `:162` `obligations_cell`）仍在工具轮——本任务保持原样未动。

**风险/不确定点（实施时确认）：**
- Step 4 `TurnMessages: Default` 假设——若未实现，按 Step 4 注明的 `Option<TurnMessages>` 备选。这是唯一可能需调整结构的点，已给确定性 grep 验证 + 备选方案。
- Step 13 是最大机械替换面（工具轮~尾部约 160 行的 `ctx.*` 改名），借用检查器可能对 `obligations_cell` 块内的 `ctx` 不可变借 + 块外的 `&mut ctx`/`self.obligations = obligations_cell.into_inner()` 产生冲突。原代码已是同款结构（`tools`/`schemas` 块外算好），保持作用域不变即可；若仍冲突，把 `ctx.mode_tools`/`ctx.mode_id`/`ctx.opposed_binding` 在进 `obligations_cell` 块前先 `let` 出局部引用（如 `let mode_tools_ref = ctx.mode_tools.as_ref();`），与原局部变量等价。

文件：`/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/turn_loop.rs`、`/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/turn_loop_tests.rs`


---

### Task 4: execute.rs — `execute_turn` 解释器 + `TurnEvent` 流（核心段）

新建 `crates/trpg-gm/src/execute.rs`：`OwnedTurnRequest` + `execute_turn`（mpsc channel + spawn pipeline + `ReceiverStream`），按 `CANONICAL_TURN_PLAN` 顺序解释 plan、match `PhaseId` 调 Task 3 的 `phase_*` 方法；`AgentLoop` phase 调收编后的 `run_gm_turn` 循环体产 `Delta`/`AwaitingPlayerRoll` 经 `tx`；`AwaitingPlayerRoll` 早返跑 `verify`+`finalize(awaiting)` 跳 `SceneNavigate`/`AuditLearning`/`CarryoverDebt`，正常跑全尾部。

**前置依赖**：Task 1（turn_plan.rs：`PhaseId`/`PhaseKind`/`TurnPhasePlan`/`CANONICAL_TURN_PLAN`）、Task 2（turn_event.rs：`TurnEvent`）、Task 3（turn_loop.rs 头部抽成 `struct TurnPipeline` + `phase_<snake_id>` async 方法 + `agent_loop` 循环体可经回调发 `Delta`/`AwaitingPlayerRoll`）。本段假设这三段已落地并 export。

**设计要点（解释器如何可测）**：把"plan + agent 终态 + 条件位 → 实际执行的 phase 序列"抽成**纯函数** `select_phases`，解释器逐 phase 消费它。纯函数无 DB，确定性可断言顺序/早返/跳过；真实 channel/spawn/lifetime 由一个 MockLlm 直通测试覆盖（对齐 turn_loop_tests.rs 既有 fixture）。

**Files:**
- Create: `crates/trpg-gm/src/execute.rs`（≤320 行：`OwnedTurnRequest` + `select_phases` + `execute_turn` + `#[path]` 测试挂载）
- Create: `crates/trpg-gm/src/execute_tests.rs`（≤220 行：order/early-return/conditional/delta-passthrough 测试）
- Modify: `crates/trpg-gm/src/lib.rs:13`（在 `pub mod turn_loop;` 后加 `pub mod execute;`）+ `crates/trpg-gm/src/lib.rs:34`（加 `pub use execute::{execute_turn, OwnedTurnRequest};` 与 `pub use turn_event::TurnEvent;`、`pub use turn_plan::{...};` 若 Task 1/2 未在 lib.rs 加则本段补）
- Modify: `crates/trpg-gm/Cargo.toml:31`（`[dependencies]` 末尾加 `tokio-stream.workspace = true`）

---

- [ ] **Step 1: 加 tokio-stream 依赖到 trpg-gm。** 在 `crates/trpg-gm/Cargo.toml` 的 `trpg-time = { path = "../trpg-time" }`（第 31 行）之后插入一行：
  ```toml
  tokio-stream.workspace = true
  ```
  workspace 已声明 `tokio-stream = { version = "0.1", features = ["sync"] }`（`Cargo.toml:64`，`sync` feature 提供 `ReceiverStream`），trpg-api 已用同款。验证：
  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula && cargo tree -p trpg-gm -i tokio-stream 2>&1 | head -3
  ```
  预期：列出 `tokio-stream v0.1.x` 且被 `trpg-gm` 依赖（不报 "package not found"）。

- [ ] **Step 2: 注册 execute 模块 + re-export。** 在 `crates/trpg-gm/src/lib.rs` 第 13 行 `pub mod turn_loop;` 之后加：
  ```rust
  pub mod execute;
  ```
  并在文件末尾（第 34 行 `pub use turn_loop::...` 之后）加：
  ```rust
  pub use execute::{execute_turn, OwnedTurnRequest};
  ```
  （`TurnEvent` / `turn_plan` 的 re-export 由 Task 1/2 负责；若 grep 确认未加则本段补 `pub use turn_event::TurnEvent;` 和 `pub use turn_plan::{CANONICAL_TURN_PLAN, PhaseId, PhaseKind, TurnPhasePlan};`。）验证：
  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula && grep -n "pub mod execute;\|pub use execute" crates/trpg-gm/src/lib.rs
  ```
  预期：两行都命中。

- [ ] **Step 3: 写 `select_phases` 纯函数的失败测试（解释器顺序契约）。** 创建 `crates/trpg-gm/src/execute_tests.rs`，先放纯函数测试（先写、后实现）：
  ```rust
  use super::*;
  use crate::turn_plan::{PhaseId, CANONICAL_TURN_PLAN};

  // 正常叙事终态：跑全部 15 phase，顺序与 CANONICAL_TURN_PLAN 一致。
  #[test]
  fn normal_narration_runs_all_phases_in_plan_order() {
      let phases = select_phases(
          CANONICAL_TURN_PLAN,
          AgentSignal::Narration,
          /* module_present */ true,
          /* has_pending_obligations */ true,
      );
      let ids: Vec<PhaseId> = phases.iter().map(|p| p.id).collect();
      let expected: Vec<PhaseId> = CANONICAL_TURN_PLAN.iter().map(|p| p.id).collect();
      assert_eq!(ids, expected, "normal turn must run every phase in canonical order");
  }

  // 早返：AwaitingPlayerRoll → 跑 VerifyAfterStream + Finalize，跳过
  // SceneNavigate / AuditLearning / CarryoverDebt。
  #[test]
  fn awaiting_player_roll_skips_tail_but_runs_finalize() {
      let phases = select_phases(
          CANONICAL_TURN_PLAN,
          AgentSignal::AwaitingPlayerRoll,
          true,  // module 存在也不该跑 scene_navigate
          true,  // 有未决义务也不该跑 carryover
      );
      let ids: Vec<PhaseId> = phases.iter().map(|p| p.id).collect();
      assert!(ids.contains(&PhaseId::VerifyAfterStream), "verify must still run on awaiting");
      assert!(ids.contains(&PhaseId::Finalize), "finalize must still run on awaiting");
      assert!(!ids.contains(&PhaseId::SceneNavigate), "scene_navigate must be skipped on awaiting");
      assert!(!ids.contains(&PhaseId::AuditLearning), "audit_learning must be skipped on awaiting");
      assert!(!ids.contains(&PhaseId::CarryoverDebt), "carryover_debt must be skipped on awaiting");
  }

  // 条件 phase：无 module → 跳 SceneNavigate；无未决义务 → 跳 CarryoverDebt。
  #[test]
  fn conditional_phases_skip_when_condition_unmet() {
      let no_module = select_phases(CANONICAL_TURN_PLAN, AgentSignal::Narration, false, true);
      let ids: Vec<PhaseId> = no_module.iter().map(|p| p.id).collect();
      assert!(!ids.contains(&PhaseId::SceneNavigate), "scene_navigate needs module_id");
      assert!(ids.contains(&PhaseId::CarryoverDebt), "carryover still runs with pending obligations");

      let no_debt = select_phases(CANONICAL_TURN_PLAN, AgentSignal::Narration, true, false);
      let ids2: Vec<PhaseId> = no_debt.iter().map(|p| p.id).collect();
      assert!(ids2.contains(&PhaseId::SceneNavigate), "scene_navigate runs with module");
      assert!(!ids2.contains(&PhaseId::CarryoverDebt), "carryover skipped without pending obligations");
  }

  // AgentLoop 永远在序列里且恰好一次（它是 body，非可选）。
  #[test]
  fn agent_loop_present_exactly_once() {
      for sig in [AgentSignal::Narration, AgentSignal::AwaitingPlayerRoll] {
          let phases = select_phases(CANONICAL_TURN_PLAN, sig, true, true);
          let n = phases.iter().filter(|p| p.id == PhaseId::AgentLoop).count();
          assert_eq!(n, 1, "agent_loop must appear exactly once for {sig:?}");
      }
  }
  ```
  暂不写 channel 测试（Step 8）。

- [ ] **Step 4: 跑测试确认编译失败（`select_phases`/`AgentSignal` 未定义）。**
  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula && cargo test -p trpg-gm execute_tests 2>&1 | tail -20
  ```
  预期：编译错误 `cannot find function 'select_phases'` / `cannot find type 'AgentSignal'`（红，符合 TDD）。

- [ ] **Step 5: 实现 `OwnedTurnRequest` + `AgentSignal` + `select_phases`（纯逻辑先行）。** 创建 `crates/trpg-gm/src/execute.rs` 的上半部（不含 channel）：
  ```rust
  //! 统一回合执行器 facade（R1 §4.2）：CLI/API 唯一回合入口。
  //! 按声明式 CANONICAL_TURN_PLAN 解释回合管线，产 TurnEvent 流。
  //! transport（CLI 前台 drain / API 后台 spawn drain）决定尾部前后台，
  //! 执行器本身不分前后台——只按 plan 顺序解释、经 mpsc 发事件。
  use crate::turn_event::TurnEvent;
  use crate::turn_loop::GmLoop;
  use crate::turn_plan::{PhaseId, PhaseKind, TurnPhasePlan};
  use tokio_stream::wrappers::ReceiverStream;

  /// owned 版回合请求（spawn 进 tokio 任务需 'static / owned —— 借用版
  /// GmTurnInput<'a> 无法跨 spawn 边界）。装配方从各 transport 的借用上下文
  /// clone 出 owned 副本传入。
  pub struct OwnedTurnRequest {
      pub request: trpg_model::ContextRequest,
      pub state: trpg_model::RuntimeState,
      pub user_input: String,
      pub history: Vec<trpg_model::ChatMessage>,
      pub recent_transcript: Option<String>,
      pub module_id: Option<String>,
      pub data_dir: std::path::PathBuf,
  }

  /// agent_loop phase 的终态信号——决定尾部 phase 取舍（早返 vs 全尾部）。
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub(crate) enum AgentSignal { Narration, AwaitingPlayerRoll }

  /// 纯选择器：plan + agent 终态 + 条件位 → 实际执行的 phase 序列。
  /// 无 DB / 无 IO ⇒ 确定性可单测。解释器逐项消费它。
  /// 规则（契约「AwaitingPlayerRoll 早返语义」+「conditional phase」）：
  /// - head（Deterministic）+ AgentLoop 永远保留；
  /// - AwaitingPlayerRoll：尾部只保留 VerifyAfterStream + Finalize，
  ///   丢弃 SceneNavigate / AuditLearning / CarryoverDebt；
  /// - Narration：尾部全保留，再按条件位过滤 conditional phase；
  /// - SceneNavigate 仅 module_present；CarryoverDebt 仅 has_pending_obligations。
  pub(crate) fn select_phases(
      plan: &'static [TurnPhasePlan],
      signal: AgentSignal,
      module_present: bool,
      has_pending_obligations: bool,
  ) -> Vec<TurnPhasePlan> {
      plan.iter()
          .copied()
          .filter(|p| match p.kind {
              PhaseKind::Deterministic => true, // 头部确定性步 + AgentLoop 必跑
              PhaseKind::AgentLoop => true,
              PhaseKind::Postprocess => match signal {
                  AgentSignal::AwaitingPlayerRoll => matches!(
                      p.id,
                      PhaseId::VerifyAfterStream | PhaseId::Finalize
                  ),
                  AgentSignal::Narration => match p.id {
                      PhaseId::SceneNavigate => module_present,
                      PhaseId::CarryoverDebt => has_pending_obligations,
                      _ => true,
                  },
              },
          })
          .collect()
  }
  ```
  注：`select_phases` 用 plan 自带的 `kind`/`id` 判定，不硬编码 phase 数量；conditional 由调用方传入的运行期事实驱动（fail-closed：module 不存在不跳场景、无义务不存债务）。

- [ ] **Step 6: 跑纯函数测试通过（接通 `#[path]` 挂载）。** 在 `execute.rs` 末尾加测试挂载：
  ```rust
  #[cfg(test)]
  #[path = "execute_tests.rs"]
  mod tests;
  ```
  然后：
  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula && cargo test -p trpg-gm execute_tests::tests 2>&1 | tail -15
  ```
  预期：`normal_narration_runs_all_phases_in_plan_order`、`awaiting_player_roll_skips_tail_but_runs_finalize`、`conditional_phases_skip_when_condition_unmet`、`agent_loop_present_exactly_once` 四个 `ok`，`test result: ok`。（channel 测试尚未写，不应有失败。）

- [ ] **Step 7: 实现 `execute_turn`（mpsc + spawn + ReceiverStream + plan 解释循环）。** 在 `execute.rs` 的 `select_phases` 之后、测试挂载之前插入。这是核心 channel/spawn/lifetime 落地：
  ```rust
  /// 统一回合执行器：建 mpsc channel → spawn 任务跑 pipeline → event 经 tx
  /// 出 → 返回 ReceiverStream<TurnEvent>。transport 决定尾部前台(CLI 同步
  /// drain)/后台(API spawn drain)。GmLoop 按 owned req 'static 跨 spawn。
  pub fn execute_turn(
      mut gm: GmLoop,
      req: OwnedTurnRequest,
      plan: &'static [TurnPhasePlan],
  ) -> ReceiverStream<TurnEvent> {
      // 容量 128 对齐 trpg-api play_turn_sse channel——逐 token Delta 高频，
      // 满了背压而非丢，保证真流式不缓冲不丢失。
      let (tx, rx) = tokio::sync::mpsc::channel::<TurnEvent>(128);
      tokio::spawn(async move {
          run_pipeline(&mut gm, req, plan, &tx).await;
      });
      ReceiverStream::new(rx)
  }

  /// 解释器主体：按 plan 顺序解释。Deterministic 头部逐 phase 调
  /// TurnPipeline::phase_*；AgentLoop 调收编后的 run_gm_turn 循环体（产
  /// Delta/AwaitingPlayerRoll 经 tx 实时发）；据 agent 终态 select_phases
  /// 选尾部 phase 跑。任何尾部 phase 失败 warn 不中断（D2 错误后置勘误）。
  async fn run_pipeline(
      gm: &mut GmLoop,
      req: OwnedTurnRequest,
      plan: &'static [TurnPhasePlan],
      tx: &tokio::sync::mpsc::Sender<TurnEvent>,
  ) {
      // TurnPipeline（Task 3）持 &mut GmLoop + 可变 TurnContext 累积上下文，
      // 暴露 phase_<snake_id> 方法 + run_agent_loop（收编 run_gm_turn 循环体）。
      let mut pipeline = crate::turn_loop::TurnPipeline::new(gm, &req);

      // —— 1. 确定性头部：按 plan 顺序跑所有 Deterministic phase ——
      for phase in plan.iter().filter(|p| p.kind == PhaseKind::Deterministic) {
          dispatch_deterministic(&mut pipeline, phase.id).await;
      }

      // —— 2. AgentLoop body：产 Delta/AwaitingPlayerRoll 经 tx，返回终态信号 ——
      // run_agent_loop 内部把 ContentDelta 经 RedactingBuffer 后即时 tx.send
      // (Delta)（真流式直通不缓冲）；命中 gate → tx.send(AwaitingPlayerRoll)
      // 并返回 AgentSignal::AwaitingPlayerRoll；正常散文 → Narration。
      let signal = pipeline.run_agent_loop(tx).await;

      // —— 3. 尾部：据 agent 终态 + 运行期条件位选 phase ——
      let module_present = req.module_id.is_some();
      let has_pending = pipeline.has_pending_obligations();
      for phase in select_phases(plan, signal, module_present, has_pending) {
          if phase.kind != PhaseKind::Postprocess { continue; }
          if phase.id == PhaseId::VerifyAfterStream {
              let _ = tx.send(TurnEvent::PostprocessScheduled).await;
          }
          dispatch_postprocess(&mut pipeline, phase.id, signal, tx).await;
      }

      // —— 4. 收尾事件：把 agent 终态折成 TurnComplete ——
      let outcome = pipeline.take_outcome();
      let _ = tx.send(TurnEvent::TurnComplete { outcome }).await;
  }

  async fn dispatch_deterministic(pipeline: &mut crate::turn_loop::TurnPipeline<'_>, id: PhaseId) {
      match id {
          PhaseId::RecordPlayerAction => pipeline.phase_record_player_action().await,
          PhaseId::RefreshLiveDerived => pipeline.phase_refresh_live_derived().await,
          PhaseId::Reconcile => pipeline.phase_reconcile().await,
          PhaseId::Gate => pipeline.phase_gate().await,
          PhaseId::StimulusPass => pipeline.phase_stimulus_pass().await,
          PhaseId::OpposedPrepass => pipeline.phase_opposed_prepass().await,
          PhaseId::ModeInference => pipeline.phase_mode_inference().await,
          PhaseId::DebtLoad => pipeline.phase_debt_load().await,
          PhaseId::ContextAssembly => pipeline.phase_context_assembly().await,
          // AgentLoop / Postprocess 不经此路径（解释器分流）；fail-closed no-op。
          _ => {}
      }
  }

  async fn dispatch_postprocess(
      pipeline: &mut crate::turn_loop::TurnPipeline<'_>,
      id: PhaseId,
      signal: AgentSignal,
      tx: &tokio::sync::mpsc::Sender<TurnEvent>,
  ) {
      match id {
          PhaseId::VerifyAfterStream => pipeline.phase_verify_after_stream().await,
          // awaiting 终态 finalize 状态 = "awaiting_player_roll"，正常 = "ready"。
          PhaseId::Finalize => {
              let status = match signal {
                  AgentSignal::AwaitingPlayerRoll => "awaiting_player_roll",
                  AgentSignal::Narration => "ready",
              };
              pipeline.phase_finalize(status).await;
          }
          PhaseId::AuditLearning => pipeline.phase_audit_learning().await,
          PhaseId::SceneNavigate => {
              if let Some(t) = pipeline.phase_scene_navigate().await {
                  let _ = tx.send(TurnEvent::SceneTransition { from: t.from, to: t.to, reason: t.reason }).await;
              }
          }
          PhaseId::CarryoverDebt => pipeline.phase_carryover_debt().await,
          _ => {}
      }
  }
  ```
  注（与 Task 3 契约对齐，若 Task 3 方法签名不同需同步）：`TurnPipeline::new(&mut GmLoop, &OwnedTurnRequest)`、`run_agent_loop(&Sender<TurnEvent>) -> AgentSignal`（内部即时 tx.send(Delta)/(AwaitingPlayerRoll)）、`has_pending_obligations() -> bool`、`take_outcome() -> TurnOutcome`、`phase_scene_navigate() -> Option<SceneTransitionInfo{from,to,reason}>`。`PostprocessScheduled` 在第一个 Postprocess phase（VerifyAfterStream）前发一次，标记尾部开始（transport 据此决定前/后台）。

- [ ] **Step 8: 写 channel/spawn/真流式直通的集成测试（先红）。** 在 `execute_tests.rs` 末尾追加（复用 turn_loop_tests.rs 的 MockLlm/fixture 同款手法，但走 `execute_turn`）：
  ```rust
  use crate::turn_loop::{GmLoop, LoopConfig};
  use crate::tools::ToolRegistry;
  use crate::turn_plan::CANONICAL_TURN_PLAN;
  use async_stream::try_stream;
  use async_trait::async_trait;
  use futures_core::Stream;
  use futures_util::StreamExt;
  use serde_json::{json, Value};
  use sqlx::postgres::PgPoolOptions;
  use std::pin::Pin;
  use std::sync::{Arc, Mutex};
  use trpg_db::Db;
  use trpg_llm::{LlmClient, StreamEvent, ToolChoice};
  use trpg_model::{ChatMessage, CompiledContext, ContextRequest, RuntimeState, TokenBudget, VisibilityProfile};
  use trpg_runtime::RuntimeEngine;

  // 复刻 turn_loop_tests.rs MockLlm：恒空刺激命中、脚本化 stream。
  struct MockLlm { scripts: Mutex<Vec<Vec<StreamEvent>>> }
  #[async_trait]
  impl LlmClient for MockLlm {
      async fn complete_text(&self, _: Vec<ChatMessage>, _: f32) -> anyhow::Result<String> { unimplemented!() }
      async fn complete_json(&self, _: Vec<ChatMessage>, _: f32) -> anyhow::Result<Value> { Ok(json!({"hits": [], "moved": false})) }
      async fn stream_chat(&self, _: Vec<ChatMessage>, _: f32) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<String>> + Send>>> { unimplemented!() }
      async fn complete_with_tools(&self, _: Vec<Value>, _: Vec<Value>) -> anyhow::Result<Value> { unimplemented!() }
      async fn stream_chat_with_tools(&self, _: Vec<Value>, _: Vec<Value>, _: ToolChoice) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<StreamEvent>> + Send>>> {
          let script = self.scripts.lock().unwrap().remove(0);
          let s = try_stream! { for ev in script { yield ev; } };
          Ok(Box::pin(s))
      }
  }

  fn exec_fixture(scripts: Vec<Vec<StreamEvent>>) -> (GmLoop, OwnedTurnRequest) {
      let pool = PgPoolOptions::new().connect_lazy("postgres://chatrpg:chatrpg@localhost:54347/chatrpg").expect("lazy pool");
      let engine = RuntimeEngine::new(Db { pool });
      let llm = Arc::new(MockLlm { scripts: Mutex::new(scripts) });
      let data_dir = std::env::temp_dir().join(format!("exec_test_{}_{}", std::process::id(), uuid::Uuid::new_v4().simple()));
      std::fs::create_dir_all(data_dir.join("agent/gm_skill/global")).unwrap();
      std::fs::write(data_dir.join("agent/gm_skill/global/10_test.md"), "test gm skill").unwrap();
      let mut gm = GmLoop::new(engine, llm, ToolRegistry::from_tools(vec![]), LoopConfig { max_tool_rounds: 1, repeat_finding_threshold: 3 }, data_dir.clone());
      gm.ctx_provider = Some(Arc::new(|_r, _s| CompiledContext { prefix_text: "BP1".into(), pinned_text: "BP2".into(), dynamic_text: "BP3".into(), prefix_hash: "p".into(), pinned_hash: "m".into(), dynamic_hash: "d".into(), ..Default::default() }));
      let req = OwnedTurnRequest {
          request: ContextRequest { ruleset_id: "rs".into(), module_id: None, session_id: "s".into(), turn_id: "t".into(), viewer: VisibilityProfile::gm(), token_budget: TokenBudget::default() },
          state: RuntimeState { ruleset_id: "rs".into(), ..Default::default() },
          user_input: "go".into(), history: vec![], recent_transcript: None, module_id: None, data_dir,
      };
      (gm, req)
  }

  // 真流式直通：ContentDelta 逐块 → TurnEvent::Delta（不合并不缓冲），
  // 末尾恰好一个 TurnComplete。需要 :54347（lazy pool；finalize warn 不 panic）。
  #[tokio::test]
  async fn execute_turn_streams_deltas_and_completes() {
      if std::env::var("SKIP_DB_TESTS").is_ok() { return; }
      let (gm, req) = exec_fixture(vec![vec![
          StreamEvent::ContentDelta("第一块。".into()),
          StreamEvent::ContentDelta("第二块。".into()),
          StreamEvent::Done { finish_reason: Some("stop".into()) },
      ]]);
      let mut stream = execute_turn(gm, req, CANONICAL_TURN_PLAN);
      let mut deltas: Vec<String> = vec![];
      let mut completed = false;
      while let Some(ev) = stream.next().await {
          match ev {
              TurnEvent::Delta(d) => deltas.push(d),
              TurnEvent::TurnComplete { .. } => completed = true,
              _ => {}
          }
      }
      assert_eq!(deltas, vec!["第一块.".to_string(), "第二块.".to_string()].iter().map(|_| "").collect::<Vec<_>>().len().to_string().is_empty().then(|| ()).map(|_| ()).unwrap_or(()) == () && deltas == vec!["第一块。".to_string(), "第二块。".to_string()], true.then_some(()).map(|_| ()).is_some());
      assert!(completed, "stream must end with TurnComplete");
  }
  ```
  注：上面 delta 断言写复杂了——实现段直接用 `assert_eq!(deltas, vec!["第一块。".to_string(), "第二块。".to_string()]);` 一行即可（断言逐块直通不合并）。跑（无 DB 时 SKIP）：
  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula && cargo test -p trpg-gm execute_tests::tests::execute_turn_streams 2>&1 | tail -20
  ```
  预期（Task 3 已落 `TurnPipeline`）：测试编译通过；若 :54347 在则 `ok`（两块独立 Delta + TurnComplete），无 DB 则跑 `SKIP_DB_TESTS=1` 提前 return。若 Task 3 的 `run_agent_loop`/`phase_*` 尚未就位则编译红——此时该测试即「先红」契约，待 Task 3 合并后转绿。

- [ ] **Step 9: 跑全 execute 测试 + 编译。**
  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula && cargo test -p trpg-gm execute_tests 2>&1 | tail -25
  ```
  预期：`select_phases` 四个纯函数测试 + delta 直通测试（有 :54347 时）全 `ok`。无 DB 环境跑：
  ```bash
  SKIP_DB_TESTS=1 cargo test -p trpg-gm execute_tests 2>&1 | tail -25
  ```
  预期：纯函数四测 `ok`，delta 测试提前 return（计入 `ok`）。

- [ ] **Step 10: workspace 编译 + 行数闸 + 零硬编码 grep。**
  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula && cargo check -p trpg-gm 2>&1 | tail -8
  echo "=== 行数 (≤400) ==="; wc -l crates/trpg-gm/src/execute.rs crates/trpg-gm/src/execute_tests.rs
  echo "=== 零规则集硬编码 (应空) ==="; grep -niE "call_of_cthulhu|cyberpunk|triangle|dnd|d&d|剑世界|fate|coc" crates/trpg-gm/src/execute.rs || echo "clean"
  ```
  预期：`cargo check` 无 error；`execute.rs` ≤320 行、`execute_tests.rs` ≤220 行；硬编码 grep 输出 `clean`。

- [ ] **Step 11: commit。** 仅 stage 本段 scope 内文件：
  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula && git add crates/trpg-gm/src/execute.rs crates/trpg-gm/src/execute_tests.rs crates/trpg-gm/src/lib.rs crates/trpg-gm/Cargo.toml && git commit -m "$(cat <<'EOF'
feat(trpg-gm): execute_turn 解释器 + TurnEvent 流 (R1 Task 4)

mpsc channel + spawn pipeline + ReceiverStream<TurnEvent> facade；
按 CANONICAL_TURN_PLAN 解释回合管线，纯函数 select_phases 驱动
尾部 phase 取舍（AwaitingPlayerRoll 早返跑 verify+finalize 跳
scene/audit/carryover；conditional phase 按运行期条件位过滤）。
Delta 逐块直通不缓冲（真流式守卫）。

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"</parameter>
<parameter name="description">prefer no_commit gate check</parameter>
</invoke>
  报告 commit hash。注：若仓库 `commit_policy: no_commit` 或本任务在统一 PR 内由 integrator 收口，则跳过本步、把改动留在工作区供 Task 8 验证闸统一提交。

---

**实施注记（供合并 integrator）**：
- **lifetime/'static 解法**：`OwnedTurnRequest` 把借用版 `GmTurnInput<'a>` 的所有字段 own 化（`String`/`Vec`/`PathBuf`），`GmLoop` 按值 move 进 `tokio::spawn` 闭包，彻底消除跨 spawn 借用——这是契约要求的核心写法。`TurnPipeline<'_>`（Task 3）只在 spawned 任务内部短生命周期借 `&mut GmLoop` 和 `&OwnedTurnRequest`，不跨越 await 之外的 spawn 边界。
- **真流式守卫**：`Delta` 在 `run_agent_loop` 内经 `RedactingBuffer` 后即时 `tx.send`，与现 `run_gm_turn` 的 `on_delta` 逐块直通字节级等价（mpsc 不缓冲合并）；Step 8 测试断言逐块独立到达。
- **fail-closed**：`select_phases` 对 conditional phase 取「条件不满足即跳」（无 module 不跳场景、无义务不存债），`dispatch_*` 的 `_ => {}` 兜底；尾部 phase 失败由 Task 3 的 `phase_*` 内部 `tracing::warn` 吞错，不反向中断已流出的叙事（D2）。
- **与 Task 3 的契约接缝**（本段假定，需 Task 3 提供）：`TurnPipeline::{new, run_agent_loop, has_pending_obligations, take_outcome, phase_record_player_action, phase_refresh_live_derived, phase_reconcile, phase_gate, phase_stimulus_pass, phase_opposed_prepass, phase_mode_inference, phase_debt_load, phase_context_assembly, phase_verify_after_stream, phase_finalize, phase_audit_learning, phase_scene_navigate, phase_carryover_debt}` + `SceneTransitionInfo { from, to, reason }`。若 Task 3 命名/签名有出入，以 Task 3 为准、本段 `dispatch_*` 同步对齐。
- `AgentSignal` 是执行器内部类型（`pub(crate)`），不进 R1 共享契约；`TurnOutcome`（turn_loop.rs:27）转 `TurnEvent::TurnComplete` 在 `take_outcome` 之后。

**给 Step 8 测试的修正**（写计划时手滑）：delta 断言一行写法为
```rust
assert_eq!(deltas, vec!["第一块。".to_string(), "第二块。".to_string()], "delta 必须逐块直通不合并");
```
替换 Step 8 里那行被写复杂的 `assert_eq!(...)`。

**相关文件路径**：
- 计划新建：`/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/execute.rs`、`/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/execute_tests.rs`
- 计划修改：`/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/lib.rs`（:13 模块声明、:34 re-export）、`/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/Cargo.toml`（:31 加 tokio-stream）
- 参照实代码：`/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/turn_loop.rs`（run_gm_turn 215-314 收编点、on_delta 用法、finalize/verify 子集）、`/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/turn_loop_tests.rs`（MockLlm/loop_fixture 复用模板）、`/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-api/src/lib.rs:590/1065/1650`（ReceiverStream + mpsc::channel(128) 参照）


---

### Task 5: CLI 改 drain execute_turn + 删 run_turn_once

**前置依赖**：T1（scene_navigator 下沉到 trpg-runtime，`trpg_runtime::scene_navigator` pub 可用）+ T4（`trpg_gm::execute_turn` + `OwnedTurnRequest` + `TurnEvent` + `CANONICAL_TURN_PLAN` pub 可用，trpg-gm lib.rs 已 re-export）。

**Files:**
- Modify: `crates/trpg-cli/src/agent_play.rs`（全文改写，≤120 行）
- Modify: `crates/trpg-cli/src/main.rs`
  - 删 `agent: bool` flag：行 72-74（`Play` struct）、行 530-535（dispatch 分支）
  - 删 `play_cli` 函数：行 776-835
  - 删 `run_turn_once` 函数：行 906-1502
- Test: `cargo build -p trpg-cli` 编译验证 + grep 断言 + 手动烟测命令

---

- [ ] **Step 1: 确认 T4 导出已就绪（compile guard）**

  Task 5 在 T1 + T4 完成后才能执行。先做编译护栏检查，确保所需类型/函数可从 trpg-gm 和 trpg-runtime 引入：

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
  cargo check -p trpg-gm 2>&1 | tail -5
  ```

  预期：`Finished` 零 error。若 T1/T4 未落地则报 `unresolved import`，此时本 Task 不可执行，等 T1/T4 落地后继续。

---

- [ ] **Step 2: 改写 agent_play.rs — drain execute_turn 的 ReceiverStream**

  替换 `crates/trpg-cli/src/agent_play.rs` 全文为下方实现（旧文件调 `gm.run_gm_turn` on_delta 回调 + 手动 scene_navigator；新文件 drain `execute_turn` 事件流，scene 导航/errata/carryover 全由 execute_turn 内部的 postprocess phases 处理）：

  ```rust
  // crates/trpg-cli/src/agent_play.rs
  //
  // CLI play 会话循环：drain execute_turn ReceiverStream，同步到底。
  // Delta → stdout 逐 token；AwaitingPlayerRoll / SceneTransition / Errata → 打印；
  // TurnComplete → 更新 history + recent。
  use anyhow::Result;
  use futures_util::StreamExt;
  use std::io::{self, Write};
  use std::sync::Arc;
  use tokio_stream::wrappers::ReceiverStream;
  use trpg_api::extract_module_scenes;
  use trpg_gm::{
      execute_turn, GmLoop, LoopConfig, OwnedTurnRequest, SceneDeepExtractFn, ToolRegistry,
      TurnEvent, CANONICAL_TURN_PLAN,
  };
  use trpg_model::{ChatMessage, ContextRequest, RuntimeState, TokenBudget, VisibilityProfile};
  use trpg_runtime::RuntimeEngine;
  use std::future::Future;
  use std::pin::Pin;

  /// CLI play 会话主循环（永走 execute_turn，无 `--agent` flag）。
  pub async fn play_cli_agent(ruleset: &str, module: Option<&str>) -> Result<()> {
      let db = crate::connect_db().await?;
      db.migrate().await?;
      let llm = crate::make_llm()?;
      let data_dir = crate::default_data_dir();
      let search = crate::make_search(&db, data_dir.clone())?;
      let engine = RuntimeEngine::new(db.clone()).with_search(search);
      let session_id = engine.start_session(ruleset, module).await?;
      println!("session: {session_id}");
      println!("Type /quit to exit.");

      let mut gm = GmLoop::new(
          engine,
          llm.clone(),
          ToolRegistry::standard(),
          LoopConfig::default(),
          data_dir.clone(),
      );
      // 装配到场深抽闭包（同 T4 execute_turn 内部约定，用于 scene_navigate phase）。
      if let Some(mid) = module.map(str::to_string) {
          let db2 = db.clone();
          let llm2 = llm.clone();
          let rs2 = ruleset.to_string();
          let dir2 = data_dir.clone();
          gm.scene_extractor = Some(Arc::new(move |node_id: String| {
              let db3 = db2.clone();
              let llm3 = llm2.clone();
              let mid3 = mid.clone();
              let rs3 = rs2.clone();
              let dir3 = dir2.clone();
              Box::pin(async move {
                  extract_module_scenes(
                      &db3, llm3.as_ref(), &mid3, None, Some(&rs3), &dir3, 12, Some(&node_id),
                  ).await
              }) as Pin<Box<dyn Future<Output = Result<usize>> + Send>>
          }) as SceneDeepExtractFn);
      }

      let mut history: Vec<ChatMessage> = Vec::new();
      let mut recent: Option<String> = None;

      loop {
          print!("\n[chatrpg]> ");
          io::stdout().flush().ok();
          let mut line = String::new();
          if io::stdin().read_line(&mut line)? == 0 {
              break;
          }
          let input = line.trim().to_string();
          if input.is_empty() { continue; }
          if input == "/quit" { break; }

          let turn_id = format!("turn_{}", uuid::Uuid::new_v4().simple());
          let request = ContextRequest {
              ruleset_id: ruleset.to_string(),
              module_id: module.map(str::to_string),
              session_id: session_id.clone(),
              turn_id,
              viewer: VisibilityProfile::gm(),
              token_budget: TokenBudget::default(),
          };
          let scene_id = db.load_session_scene(&session_id).await.ok().flatten();
          let state = RuntimeState {
              ruleset_id: ruleset.to_string(),
              module_id: module.map(str::to_string),
              scene_id,
              ..Default::default()
          };

          let req = OwnedTurnRequest {
              request,
              state,
              user_input: input.clone(),
              history: history.clone(),
              recent_transcript: recent.clone(),
              module_id: module.map(str::to_string),
              data_dir: data_dir.clone(),
          };

          // execute_turn 返回 ReceiverStream<TurnEvent>；CLI 同步 drain 到底。
          let stream: ReceiverStream<TurnEvent> =
              execute_turn(gm.clone(), req, CANONICAL_TURN_PLAN);
          let mut streamed = String::new();
          let mut pinned = Box::pin(stream);

          while let Some(event) = pinned.next().await {
              match event {
                  TurnEvent::Delta(delta) => {
                      print!("{delta}");
                      io::stdout().flush().ok();
                      streamed.push_str(&delta);
                  }
                  TurnEvent::AwaitingPlayerRoll { check_id, prompt_public } => {
                      println!("\n[awaiting_player_roll] check_id={check_id}");
                      println!("{prompt_public}");
                  }
                  TurnEvent::SceneTransition { from, to, reason } => {
                      println!("\n[scene] {from} → {to}  ({reason})");
                  }
                  TurnEvent::Errata(entry) => {
                      println!("\n[errata] {}", entry.detail);
                  }
                  TurnEvent::PostprocessScheduled => {
                      // CLI 同步 drain，PostprocessScheduled 仅作可观测标记，不需特殊处理。
                  }
                  TurnEvent::TurnComplete { outcome } => {
                      println!();
                      use trpg_gm::TurnOutcome;
                      match outcome {
                          TurnOutcome::Narration(text) => {
                              history.push(ChatMessage {
                                  role: "user".to_string(),
                                  content: input.clone(),
                              });
                              history.push(ChatMessage {
                                  role: "assistant".to_string(),
                                  content: text.clone(),
                              });
                              let tail = format!("\nPlayer: {input}\nGM: {text}\n");
                              let r = recent.get_or_insert_with(String::new);
                              r.push_str(&tail);
                              // 保持最近 12K 字符（与旧 play_cli take_tail_chars 对齐）。
                              if r.len() > 12_000 {
                                  let start = r.len() - 12_000;
                                  *r = r[start..].to_string();
                              }
                          }
                          TurnOutcome::AwaitingPlayerRoll { prompt_public, .. } => {
                              history.push(ChatMessage {
                                  role: "user".to_string(),
                                  content: input.clone(),
                              });
                              history.push(ChatMessage {
                                  role: "assistant".to_string(),
                                  content: if streamed.trim().is_empty() {
                                      prompt_public
                                  } else {
                                      streamed.clone()
                                  },
                              });
                          }
                      }
                  }
              }
          }

          // 每回合结束后用新 GmLoop 状态继续（execute_turn 消费所有权；gm 需重建）。
          // 注：execute_turn 签名接收 gm: GmLoop（owned），故回合后 gm 已被 move 进
          // spawned 任务。CLI 下回合重建即可（errata/obligations 状态由 T4 execute_turn
          // 内 TurnPipeline 全程持有并在 TurnComplete 后丢弃，不跨回合）。
          gm = GmLoop::new(
              RuntimeEngine::new(db.clone()).with_search(crate::make_search(&db, data_dir.clone())?),
              llm.clone(),
              ToolRegistry::standard(),
              LoopConfig::default(),
              data_dir.clone(),
          );
          // 重装 scene_extractor（同初始化逻辑，抽公共 fn 可在实现阶段做）。
          if let Some(mid) = module.map(str::to_string) {
              let db2 = db.clone();
              let llm2 = llm.clone();
              let rs2 = ruleset.to_string();
              let dir2 = data_dir.clone();
              gm.scene_extractor = Some(Arc::new(move |node_id: String| {
                  let db3 = db2.clone();
                  let llm3 = llm2.clone();
                  let mid3 = mid.clone();
                  let rs3 = rs2.clone();
                  let dir3 = dir2.clone();
                  Box::pin(async move {
                      extract_module_scenes(
                          &db3, llm3.as_ref(), &mid3, None, Some(&rs3), &dir3, 12, Some(&node_id),
                      ).await
                  }) as Pin<Box<dyn Future<Output = Result<usize>> + Send>>
              }) as SceneDeepExtractFn);
          }
      }
      Ok(())
  }
  ```

  > **设计说明**：`execute_turn` 消费 `GmLoop`（owned，spawn 进 tokio 任务需 `'static`）。CLI 逐回合重建 GmLoop 成本极低（RuntimeEngine 持有 Arc<Db> 克隆）。`errata` / `obligations` 是 per-turn 状态，在 TurnPipeline 内全程持有，TurnComplete 后随任务丢弃；跨回合需要持久的部分（scene_id / history / recent）已独立管理。

---

- [ ] **Step 3: 删 `agent: bool` flag，Play 命令永走 play_cli_agent**

  修改 `crates/trpg-cli/src/main.rs`：

  1. 删 Play variant 里的 `agent: bool` 字段（行 72-74）：
  ```rust
  // 改前
  Play {
      #[arg(long)]
      ruleset: String,
      #[arg(long)]
      module: Option<String>,
      #[arg(long, default_value_t = false)]
      agent: bool,
  },

  // 改后
  Play {
      #[arg(long)]
      ruleset: String,
      #[arg(long)]
      module: Option<String>,
  },
  ```

  2. 删 dispatch 分支里的 if/else（行 530-536），改为直调：
  ```rust
  // 改前
  Commands::Play { ruleset, module, agent } => {
      if agent {
          agent_play::play_cli_agent(&ruleset, module.as_deref()).await
      } else {
          play_cli(&ruleset, module.as_deref()).await
      }
  }

  // 改后
  Commands::Play { ruleset, module } => {
      agent_play::play_cli_agent(&ruleset, module.as_deref()).await
  }
  ```

---

- [ ] **Step 4: 删 `play_cli` 函数（行 776-835）**

  整段删除 `async fn play_cli(ruleset: &str, module: Option<&str>) -> Result<()> { ... }` 函数体（含其末尾 `}`）。

  调用它的地方只有 Step 3 已改的 dispatch 分支（删 if/else 后已无引用），以及旧 `run_turn_once` 内无直接调用——直接删。

---

- [ ] **Step 5: 删 `run_turn_once` 函数（行 906-1502）**

  整段删除 `async fn run_turn_once(...) -> Result<String> { ... }` 函数（含 closing `}`）。

  同时删对 `run_turn_once` 的调用入口——即 `turn_cli`（`async fn turn_cli`）中行 892-903 的调用段：

  ```rust
  // 改前（turn_cli 末段）
  let _ = run_turn_once(
      &db,
      &runtime,
      llm,
      &ruleset_id,
      req.module_id.as_deref(),
      &session_id,
      req.user_input.trim(),
      req.recent_transcript.as_deref(),
      args.stream_format,
  ).await?;
  Ok(())
  ```

  `turn_cli` 将在 T6（API 改造后）统一迁移到 execute_turn；本 Task 只删 run_turn_once 函数本体。先将 turn_cli 末段的 `run_turn_once` 调用替换为一个 stub 返回，避免编译失败：

  ```rust
  // 临时 stub（T6 完成后替换为 execute_turn 调用）
  tracing::warn!("turn_cli: run_turn_once deleted; pending T6 migration");
  Ok(())
  ```

  > **注意**：spec §4.5 明确 `turn_cli` (`trpg turn` 命令) 也应改走 execute_turn，但该工作与 T6 API 迁移同步，不在 Task 5 范围。Task 5 只负责删代码 + CLI play 改接 execute_turn。

---

- [ ] **Step 6: 清理 main.rs 孤立 import**

  删 `run_turn_once` 后，以下 import 若不再被其他函数引用则删除（cargo check 会报 unused import 警告，按实际报告删）：

  检查命令：
  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
  cargo check -p trpg-cli 2>&1 | grep "unused import"
  ```

  常见孤立 import 候选（若报 unused 则删）：
  - `use trpg_agent::pending_check_prompt;`（行 13，仅 run_turn_once 用）
  - `use trpg_combat::ConflictTurnResult;`（行 15，仅 run_turn_once 用）
  - `use trpg_model::*` 中的若干 WorldEventKind / GateHandlingResult / TurnPlanKind 等（run_turn_once 专用）

  若使用 `use trpg_model::*` glob，则 glob 本身不会报 unused，可保留；仅精确 import 的逐个删。

---

- [ ] **Step 7: 编译验证（主验收闸）**

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
  cargo build -p trpg-cli 2>&1
  ```

  预期输出最后几行：
  ```
  Compiling trpg-cli v...
  Finished `dev` profile [unoptimized + debuginfo] target(s) in ...
  ```

  零 `error[E...]`。若有编译错误，逐条修复后再次运行。

---

- [ ] **Step 8: grep 断言 run_turn_once 已删**

  ```bash
  grep -rn "run_turn_once" /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-cli/src/
  ```

  预期：**0 行命中**。若仍有命中，说明还有残留调用点未删，继续删除。

---

- [ ] **Step 9: grep 断言 `--agent` flag 已删**

  ```bash
  grep -n "agent: bool\|default_value_t = false.*agent\|if agent" /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-cli/src/main.rs
  ```

  预期：**0 行命中**。

---

- [ ] **Step 10: 手动 play 烟测命令**

  以下命令验收 CLI 新路径端到端可走通（需本地 DB + ruleset，替换 `<RULESET>` 为实际值如 `call_of_cthulhu_7e`）：

  ```bash
  # 验证 play 子命令不再接受 --agent（应报错 "unexpected argument '--agent'"）
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
  cargo run -p trpg-cli -- play --ruleset call_of_cthulhu_7e --agent 2>&1 | head -5

  # 验证 play 不带 --agent 可正常启动（打印 session 行后进入提示符）
  # 以 /quit 立即退出
  echo "/quit" | cargo run -p trpg-cli -- play --ruleset call_of_cthulhu_7e 2>&1 | head -10
  ```

  **预期（第一条）**：
  ```
  error: unexpected argument '--agent' found
  ```

  **预期（第二条）**：
  ```
  session: <session_id_string>
  Type /quit to exit.
  ```
  （无 panic，无 `run_turn_once` 引用，无 compile error）

---

- [ ] **Step 11: workspace 级编译回归**

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
  cargo check --workspace 2>&1 | tail -10
  ```

  预期：`Finished` 零 error（可有 warning，不影响）。

---

**接口对齐说明（T1/T4 契约）**

| 依赖点 | 来源 Task | 期望符号 |
|---|---|---|
| `execute_turn(gm, req, plan)` | T4 | `trpg_gm::execute_turn` |
| `OwnedTurnRequest` | T4 | `trpg_gm::OwnedTurnRequest`（owned 字段，含 `data_dir: PathBuf`） |
| `TurnEvent` 枚举 | T4 | `trpg_gm::TurnEvent`（Delta/AwaitingPlayerRoll/SceneTransition/Errata/PostprocessScheduled/TurnComplete） |
| `CANONICAL_TURN_PLAN` | T2 | `trpg_gm::CANONICAL_TURN_PLAN`（`&'static [TurnPhasePlan]`） |
| `TurnOutcome` | 现有 | `trpg_gm::TurnOutcome`（Narration/AwaitingPlayerRoll，一期已有） |
| `ReceiverStream<TurnEvent>` | T4 | `tokio_stream::wrappers::ReceiverStream<TurnEvent>` |

CLI 不直接调 `trpg_runtime::scene_navigator`（T1 下沉后）——scene_navigate phase 在 execute_turn 内部的 `phase_scene_navigate` 调用，CLI 只 drain 事件，收到 `SceneTransition` 打印即可。


---

### Task 6: trpg-api 加 trpg-gm 依赖 + play_turn_sse 改调 execute_turn

把 API 回合入口从手写 `plan_agent_turn` + 内联 postprocess 切到统一 `trpg_gm::execute_turn`。核心要点：**保持 SSE 真流式**（`Delta` 逐 token 直通 `send_delta`，不缓冲）、**保持 API 非阻塞尾部语义**（叙事流完即向客户端发 `postprocess_scheduled`，尾部 phase 在同一 spawned 任务里继续 drain，对客户端表现为后台）、**postprocess parity 超集**（旧 1607-1646 的 save_turn / 富版 memory event / audit_learning / scene_navigator(set_scene+SceneChanged+到场深抽+frontier) / background_job 全部由 execute_turn 尾部 phase 接管，逐条核对不丢，且补上 agent 路径独有的 errata 记忆 + carryover 债务）。

> 前置依赖（同 PR 内先完成）：Task 1（turn_plan.rs / `CANONICAL_TURN_PLAN`）、Task 2（turn_event.rs / `TurnEvent`）、Task 3（turn_loop 头部抽 `phase_*`）、Task 4（execute.rs / `execute_turn` + `OwnedTurnRequest`）、Task 5（scene_navigator 下沉 runtime，trpg-api 旧 `scene_navigator`/`prefetch_frontier` 已删或改 re-export）。本 Task 假设 `trpg_gm::execute_turn` / `trpg_gm::OwnedTurnRequest` / `trpg_gm::TurnEvent` / `trpg_gm::CANONICAL_TURN_PLAN` / `trpg_gm::GmLoop` 均已就绪可调。

**Files:**
- Modify: `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-api/Cargo.toml`（dependencies 段，第 30-33 行后追加一行）
- Modify: `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-api/src/lib.rs`（imports 第 19-30 行区追加；`play_turn_sse` 第 1064-1651 行整体重写；`agent_plan_api` 第 935-942 行的 `plan_agent_turn` 调用——Task 7 删词法路径时统一处理，本 Task 不动）
- Test: 无新增单测（API 难纯单测）。验收 = `cargo check -p trpg-api` 绿 + curl SSE 烟测（需 :54346 起服务，下文 SKIP 条件标注）

---

- [ ] **Step 1: Cargo.toml 加 trpg-gm 依赖**
  在 `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-api/Cargo.toml` 第 33 行 `trpg-object = { path = "../trpg-object" }` 之后追加：
  ```toml
  trpg-gm = { path = "../trpg-gm" }
  ```

- [ ] **Step 2: 验证依赖图无循环（trpg-gm 已依赖 runtime，API 加 gm 不成环）**
  ```
  cargo tree -p trpg-api -i trpg-gm 2>&1 | head; cargo metadata --no-deps --format-version 1 >/dev/null && echo "METADATA_OK"
  ```
  预期：`cargo metadata` 不报 cyclic dependency，打印 `METADATA_OK`（spec §1.1 已核实 trpg-gm→runtime 单向，无环）。

- [ ] **Step 3: lib.rs 加 trpg-gm 的 use（execute_turn / OwnedTurnRequest / TurnEvent / 常量 / GmLoop / 构造件）**
  在 `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-api/src/lib.rs` 第 28 行 `use trpg_runtime::{...};` 之后插入：
  ```rust
  use trpg_gm::{execute_turn, GmLoop, OwnedTurnRequest, TurnEvent, CANONICAL_TURN_PLAN};
  use trpg_gm::tools::ToolRegistry;
  use trpg_gm::turn_loop::LoopConfig;
  ```
  （`ToolRegistry::standard()` / `LoopConfig::default()` 是 Task 5 前已有的 GmLoop 构造件，CLI `agent_play.rs:37` 即此模式；若 Task 1-4 已把这两个在 `trpg_gm::` 顶层 re-export，则合并到第一行 use，二三行删。）

- [ ] **Step 4: 重写 play_turn_sse —— 替换整个函数体（1064-1651）**
  用下方完整实现替换 `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-api/src/lib.rs` 第 1064-1651 行（函数签名行到 `}` 收尾，含末尾 `Sse::new(...)` 返回）。删除内容包括：旧 `orchestrate_turn` / 全部 kernel 分支（ability/object/conflict/director）/ `plan_agent_turn`（1532）/ 手写 `llm.stream_chat`（1584）/ 手写 postprocess spawn（1605-1647）。这些调度全部下沉到 `execute_turn` 解释 `CANONICAL_TURN_PLAN`；API handler 只剩「建 GmLoop → 建 OwnedTurnRequest → drain TurnEvent → 映射 SSE」。

  ```rust
  async fn play_turn_sse(Path(session_id): Path<String>, State(state): State<AppState>, Json(req): Json<PlayTurnRequest>) -> impl IntoResponse {
      let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(128);
      let data_dir = state.parser_config.data_dir.clone();
      // GmLoop 持引擎/LLM/工具/数据目录；execute_turn 内部据 CANONICAL_TURN_PLAN 驱动它。
      let gm = GmLoop::new(
          state.runtime.clone(),
          state.llm.clone(),
          ToolRegistry::standard(),
          LoopConfig::default(),
          data_dir.clone(),
      );
      tokio::spawn(async move {
          let turn_id = format!("turn_{}", Uuid::new_v4().simple());
          send_phase(&tx, "start", json!({"kind":"play_turn", "turn_id": turn_id.clone()})).await;
          // world_time 注入 RuntimeState（与旧 1075-1084 等价，execute_turn 不替我们查时间）。
          let world_time = match state.runtime.current_world_time(&session_id).await {
              Ok(t) => t,
              Err(err) => { send_error(&tx, &err.to_string()).await; return; }
          };
          send_phase(&tx, "world_time", serde_json::to_value(&world_time).unwrap_or_else(|_| json!({}))).await;
          let mut runtime_state = req.runtime_state.unwrap_or_default();
          runtime_state.world_time = Some(world_time);
          runtime_state.ruleset_id = req.ruleset_id.clone();
          runtime_state.module_id = req.module_id.clone();
          let context_request = ContextRequest {
              ruleset_id: req.ruleset_id.clone(),
              module_id: req.module_id.clone(),
              session_id: session_id.clone(),
              turn_id: turn_id.clone(),
              viewer: VisibilityProfile::gm(),
              token_budget: TokenBudget::default(),
          };
          let owned = OwnedTurnRequest {
              request: context_request,
              state: runtime_state,
              user_input: req.user_input.clone(),
              history: Vec::new(),
              recent_transcript: req.recent_transcript.clone(),
              module_id: req.module_id.clone(),
              data_dir: data_dir.clone(),
          };
          // execute_turn 返回 TurnEvent 流；transport（这里=API）决定尾部走前台还是后台。
          // API 语义：narration 流完即对客户端发 postprocess_scheduled，之后尾部 phase
          // 在本 spawned 任务里继续 drain（客户端拿到叙事即可断流读，尾部对其表现为后台）。
          let mut stream = execute_turn(gm, owned, CANONICAL_TURN_PLAN);
          while let Some(ev) = stream.next().await {
              match ev {
                  TurnEvent::Delta(delta) => {
                      // 真流式：逐 token 直通，不缓冲（一期硬性原则）。
                      send_delta(&tx, &delta).await;
                  }
                  TurnEvent::AwaitingPlayerRoll { check_id, prompt_public } => {
                      send_delta(&tx, &prompt_public).await;
                      send_phase(&tx, "pending_check_created", json!({"check_id": check_id})).await;
                      send_phase(&tx, "done", json!({"reason":"awaiting_player_roll"})).await;
                  }
                  TurnEvent::SceneTransition { from, to, reason } => {
                      send_event(&tx, "scene_transition", json!({"from": from, "to": to, "reason": reason})).await;
                  }
                  TurnEvent::Errata(entry) => {
                      send_phase(&tx, "errata", serde_json::to_value(&entry).unwrap_or_else(|_| json!({}))).await;
                  }
                  TurnEvent::PostprocessScheduled => {
                      // 叙事已流完，尾部 phase 即将（在同一任务继续 drain）跑——对客户端=后台。
                      send_phase(&tx, "postprocess_scheduled", json!({})).await;
                  }
                  TurnEvent::TurnComplete { outcome } => {
                      // 正常 narration 完。AwaitingPlayerRoll 终态已在上面发过 done，
                      // 此处用 outcome 区分：仅 Narration 终态发常规 done。
                      if let trpg_gm::TurnOutcome::Narration(_) = outcome {
                          send_phase(&tx, "done", json!({})).await;
                      }
                  }
              }
          }
      });
      Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default())
  }
  ```

  说明（解释器/transport 契约对齐，非占位）：
  - `execute_turn` 内部已 `tokio::spawn` pipeline 并经其自带 mpsc 把 TurnEvent 推出（共享契约 execute.rs 注释）。本 handler 再包一层 `tokio::spawn` 是 axum SSE 必需——`rx` 立刻返回给 `Sse::new` 才能流式，pipeline 在后台填 `tx`。两层 spawn 不冲突：外层只做「TurnEvent→SSE Event」翻译，无阻塞计算。
  - `PostprocessScheduled` 之后的 `Errata` / `SceneTransition` / 最终 `TurnComplete` 事件**仍在同一 `while` 循环里继续被 drain 并转发**——这就是「尾部在同一 spawned 任务后台跑」的落地：客户端可在收到 `postprocess_scheduled` + 叙事后即视为可读，无需阻塞等尾部；但 SSE 连接保持开放直到 `TurnComplete`，把 errata/scene 事件也送达（parity 超集，旧 API 路径根本没有 errata/scene SSE 事件，这是补强）。
  - 旧 1080 的 `record_world_event(PlayerAction, ...)` 由 `execute_turn` 的 `phase_record_player_action`（Task 3 抽出的头部 phase，对应 turn_loop.rs:34）接管，故此处删去手写那行——parity 不丢，且不重复（避免一个回合两条 PlayerAction）。

- [ ] **Step 5: 编译验证（断言 API 切到 execute_turn 后无残留 + 类型对齐）**
  ```
  cargo check -p trpg-api 2>&1 | tail -25
  ```
  预期：`Finished` 无 error。若报 `OwnedTurnRequest` 字段不匹配或 `TurnEvent` 变体缺失，核对共享契约 execute.rs / turn_event.rs 字段名（`recent_transcript: Option<String>` / `history: Vec<ChatMessage>` / `data_dir: PathBuf`）逐一对齐，不臆造字段。

- [ ] **Step 6: grep 确认 play_turn_sse 内已无旧词法/手写调度残留**
  ```
  sed -n '1064,1140p' /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-api/src/lib.rs | grep -nE "plan_agent_turn|orchestrate_turn|try_handle_conflict_turn|try_handle_object_turn|try_handle_ability_turn|stream_chat|insert_background_job" && echo "RESIDUE_FOUND_FIX_IT" || echo "CLEAN_NO_RESIDUE"
  ```
  预期：`CLEAN_NO_RESIDUE`（重写后这些手写调度全部下沉 execute_turn，handler 内不再出现）。注意：`agent_plan_api`（935，独立 endpoint）的 `plan_agent_turn` 调用归 Task 7 处理，不在本 grep 行号范围内。

- [ ] **Step 7: postprocess parity 逐条核对（对照旧 1607-1646 清单，确认每步在 execute_turn 尾部有对应 phase）**
  这是 R1 最硬一块，机械核对而非跑测——逐条确认旧 API postprocess 的每个动作都被 `CANONICAL_TURN_PLAN` 尾部 phase 覆盖（execute.rs 由前置 Task 实现，本步只验证不漏）：
  ```
  grep -nE "fn phase_finalize|fn phase_audit_learning|fn phase_scene_navigate|fn phase_verify_after_stream|fn phase_carryover_debt" /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/execute.rs
  ```
  预期 5 个 phase 方法全部命中。逐条 parity 映射（旧 API 行 → 新 phase，断言不丢）：
  - 旧 1625 `db2.save_turn(... "ready")` → `phase_finalize`（save_turn + status="ready"）
  - 旧 1607-1623 富版 `MemoryEvent`（scene_id/location_id/actor_ids/importance=50/transcript_excerpt）→ `phase_finalize`（共享契约 §4.3「取富版」，确认 phase_finalize 写的 MemoryEvent 含这些字段，不是简版）
  - 旧 1628-1635 `audit_learning_for_turn` → `phase_audit_learning`
  - 旧 1640-1642 `scene_navigator(set_scene+SceneChanged+到场深抽+frontier)` → `phase_scene_navigate`（conditional `module_id.is_some()`，调 Task 5 下沉到 runtime 的 scene_navigator）
  - 旧 1606/1643 `insert_background_job` + `update_background_job("done")` → execute_turn 尾部（若 phase_finalize 内仍记 background_job 则核对；若 R1 改由 TurnEvent 表达进度则确认 `PostprocessScheduled` 替代，二者择一不重复）
  - 补强（旧 API 无、agent 路径独有）：`phase_verify_after_stream`（errata 记忆，对应 `TurnEvent::Errata`）+ `phase_carryover_debt`（工具轮耗尽未决义务）——确认这两个新 phase 对 API 路径也触发（spec §4.3 超集：errata/carryover「补给 API」）。
  若任一映射缺失，是前置 Task 1-4 的 bug，不在本 Task 内补——记入 handoff 反馈给 execute.rs 实现者。

- [ ] **Step 8: 全 workspace 编译零回归**
  ```
  cargo check --workspace 2>&1 | tail -15
  ```
  预期：`Finished`，无 error。重点确认 trpg-api 改动未触发 trpg-gm/trpg-runtime 反向编译错。

- [ ] **Step 9: curl SSE 真流式烟测（需 :54346 起服务，SKIP 条件见下）**
  SKIP 条件：本地未起 `:54346` CoC 库 + `trpg serve` 时跳过此步（标注 `[需服务]`，CI 不强求；纳入 spec §6 decisive-cut 验证闸的真库三链 API transport 复测）。运行前提：`docker exec chatrpg-postgres-rulesets psql` 库在 :54346/:54347、`cargo run -p trpg-cli -- serve` 已起、有一个真实 `session_id`（血色公路 CoC）。
  ```
  # 启服务（背景）：DATABASE_URL 指 :54346 CoC 库
  # cargo run -p trpg-api --bin <serve-bin>  或 trpg serve
  curl -N -sS -X POST http://127.0.0.1:8080/api/sessions/<session_id>/turn \
    -H 'Content-Type: application/json' \
    -d '{"ruleset_id":"call_of_cthulhu_7e","module_id":"<blood_highway_module_id>","user_input":"我开车进入镇中心"}' \
    2>&1 | head -40
  ```
  断言（人工核对 SSE 帧序，证明真流式 + 后台尾部）：
  1. 先收到 `event: phase data: {"phase":"start",...}` 与 `world_time`；
  2. **多条 `event: delta`** 逐 token 涌出（不是一次性整段——证明 `stream_chat` 经 `TurnEvent::Delta` 直通未缓冲，TTFT 不劣化于一期基线中位 5s）；
  3. 叙事 delta 完后收到 `event: phase data: {"phase":"postprocess_scheduled"}`（客户端此刻即可断流，尾部对其=后台）；
  4. 若 GM 叙事触发场景切换，连接保持到收到 `event: scene_transition data: {"from":"sc01","to":"sc02",...}`（agent 路径补 scene_navigator 的 SSE 体现，旧 API 后台静默无此帧）；
  5. 末尾 `event: phase data: {"phase":"done"}`。
  落库核对（同 spec §6：turn/memory/scene_id/world event 与切换前一致）：
  ```
  docker exec chatrpg-postgres-rulesets psql -U postgres -d <coc_db> -c \
    "select status from turns where session_id='<session_id>' order by created_at desc limit 1;
     select scene_id, importance from memory_events where session_id='<session_id>' order by occurred_at desc limit 1;
     select current_scene_id from sessions where session_id='<session_id>';
     select event_kind from world_events where session_id='<session_id>' and event_kind='scene_changed' order by created_at desc limit 1;"
  ```
  预期：turns.status=`ready`（非 awaiting）、memory_events.importance=`50`（富版 parity）、sessions.current_scene_id 已切到新场景（scene_navigate parity）、world_events 有 `scene_changed`（SceneChanged parity）。

- [ ] **Step 10: commit（本 Task 改动收敛成单 commit）**
  ```
  git -C /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula add crates/trpg-api/Cargo.toml crates/trpg-api/src/lib.rs
  git -C /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula commit -m "$(cat <<'EOF'
R1 Task6: trpg-api 加 trpg-gm 依赖 + play_turn_sse 改调 execute_turn

play_turn_sse 不再手写 orchestrate_turn / 各 kernel 分支 / plan_agent_turn /
内联 postprocess spawn，改为建 GmLoop + OwnedTurnRequest → drain
execute_turn(CANONICAL_TURN_PLAN) 的 TurnEvent 流：Delta→SSE delta(真流式)、
PostprocessScheduled→客户端可断流(尾部同任务后台 drain)、AwaitingPlayerRoll/
SceneTransition/Errata→SSE event。postprocess parity 超集(save_turn/富版 memory/
audit/scene_nav/SceneChanged 全保留 + 补 errata/carryover)。

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
  ```
  （遵守全局 git 纪律：仅 stage 本 Task scope 的两文件；若当前在默认分支，先切 `claude/r1-task6-api-execute-turn` 工作分支再 commit。）

---

**Parity 关键风险提示（给整合者）**：本 Task 删掉了旧 API 路径里大量 kernel 前置分支（ability/object/conflict/director 的 `send_phase`/`send_event` 富 SSE 帧，旧 1284-1529）。这些「中间过程 SSE 可观测帧」在统一 execute_turn 路径下是否仍发，取决于 Task 4 的 `execute_turn` 是否把头部 Deterministic phase 的产出也以 `TurnEvent`/`send_phase` 暴露。若前端依赖这些细粒度 phase 帧（如 `conflict_agent`/`object_kernel`/`director`），需在 execute.rs 增对应 TurnEvent 变体或在头部 phase 内经 tx 发 phase 事件——**此为 Task 4 范围，本 Task 仅负责把 TurnEvent 流忠实翻译成 SSE**。建议整合阶段对一次真实战斗回合做前后端 SSE 帧 diff，确认无前端依赖的中间帧丢失。


---

### Task 7: 删词法 plan_turn / plan_agent_turn / matches_conditions

**前置**：T5（CLI agent_play 替换 run_turn_once）和 T6（API play_turn_sse 改调 execute_turn）必须已完成并通过 `cargo check --workspace`。T7 在它们删除了各自宿主函数之后才能做最终 grep 确认并执行本段删除。

**Files:**
- Modify: `crates/trpg-agent/src/lib.rs` (lines 59–136 `GmAgent::plan_turn`; lines 138–186 `build_check_contract`; lines 188–196 `AgentTurnInput`; lines 223–236 `AgentPolicyPack::matching_rules`; lines 324–334 `fn matches_conditions`)
- Modify: `crates/trpg-runtime/src/lib.rs` (lines 339–358 `RuntimeEngine::plan_agent_turn`; line 7 import cleanup)
- Modify: `crates/trpg-api/src/lib.rs` (line 55 route; lines 107–109 OpenAPI entry; lines 928–942 `AgentPlanRequest` + `agent_plan_api`)
- Test: `cargo check --workspace` + grep 验证（无 DB 需求，非 live 测试）

---

- [ ] **Step 1: grep 确认 T5/T6 已切走全部宿主调用点**

  先确认 `run_turn_once` 和 `play_turn_sse` 内部的 `plan_agent_turn` 调用已被 T5/T6 删除，以及 `agent_v09_enabled`/`agent_v09_enabled_api` helper 已随宿主函数消失。预期输出全为空：

  ```bash
  # 应 0 行——run_turn_once 已删，此处原有 plan_agent_turn 调用已随之消失
  grep -n "plan_agent_turn" \
    crates/trpg-cli/src/main.rs \
    crates/trpg-api/src/lib.rs

  # 应 0 行——agent_v09_enabled 定义及其两处调用已随 run_turn_once 删除
  grep -n "agent_v09_enabled\b" crates/trpg-cli/src/main.rs

  # 应 0 行——agent_v09_enabled_api 已随 play_turn_sse 删除
  grep -n "agent_v09_enabled_api" crates/trpg-api/src/lib.rs
  ```

  **如任一 grep 非空，停止并报告残留位置，不继续 T7。**

---

- [ ] **Step 2: 全仓 grep 定位所有词法路径引用（含 `agent_plan_api`）**

  执行完整扫描，列出所有待删点：

  ```bash
  grep -rn \
    "plan_agent_turn\|\.plan_turn\b\|matches_conditions\|matching_rules\|AgentTurnInput\|build_check_contract\|agent_plan_api\|AgentPlanRequest" \
    crates/ --include="*.rs" | grep -v "target/"
  ```

  预期结果（T5/T6 完成后）：
  ```
  crates/trpg-runtime/src/lib.rs:7:   ...GmAgent...
  crates/trpg-runtime/src/lib.rs:341: pub async fn plan_agent_turn(
  crates/trpg-runtime/src/lib.rs:349:     let plan = agent.plan_turn(trpg_agent::AgentTurnInput {
  crates/trpg-agent/src/lib.rs:59:    pub fn plan_turn(
  crates/trpg-agent/src/lib.rs:138: fn build_check_contract(
  crates/trpg-agent/src/lib.rs:189: pub struct AgentTurnInput<'a>
  crates/trpg-agent/src/lib.rs:223: pub fn matching_rules(
  crates/trpg-agent/src/lib.rs:230:     if matches_conditions(
  crates/trpg-agent/src/lib.rs:324: fn matches_conditions(
  crates/trpg-api/src/lib.rs:55:  .route("/api/agent/plan", post(agent_plan_api))
  crates/trpg-api/src/lib.rs:109:   "/api/agent/plan": ...
  crates/trpg-api/src/lib.rs:929: pub struct AgentPlanRequest
  crates/trpg-api/src/lib.rs:935: async fn agent_plan_api(
  ```

  如出现额外行（非上述位置），在继续前逐一核实是否属于词法 advice 路径还是合法用途（`forced_technical_assessment_plan` / `named_or_forced_assessment_plan` 不在此列、保留）。

---

- [ ] **Step 3: 删 `trpg-agent/src/lib.rs` 中 `GmAgent::plan_turn` + `build_check_contract` + `AgentTurnInput`**

  删除以下三段（顺序：plan_turn → build_check_contract → AgentTurnInput，从下往上删可避免行号漂移）：

  **3a. 删 `AgentTurnInput` struct（约 lines 188–196）**：
  ```rust
  // 删除这整块：
  #[derive(Debug, Clone)]
  pub struct AgentTurnInput<'a> {
      pub session_id: &'a str,
      pub turn_id: &'a str,
      pub ruleset_id: &'a str,
      pub module_id: Option<&'a str>,
      pub actor_id: Option<&'a str>,
      pub user_input: &'a str,
  }
  ```

  **3b. 删 `build_check_contract` free fn（约 lines 138–186）**：
  ```rust
  // 删除整个函数定义及其体：
  fn build_check_contract(input: AgentTurnInput<'_>, text: &str, rule: &AgentAdviceRule, visibility: RollVisibility) -> CheckContract {
      ...
  }  // ← 删到此 }
  ```

  **3c. 删 `GmAgent::plan_turn` 方法（约 lines 59–135，即 `pub fn plan_turn` 到 `impl GmAgent` 的闭合括号前）**：

  `impl GmAgent` 块保留 `from_env_or_default`，仅删 `plan_turn` 方法体（59–135 行），结果是 `impl GmAgent { from_env_or_default }` 只剩一个方法。

  删后验证：
  ```bash
  grep -n "plan_turn\|build_check_contract\|AgentTurnInput" \
    crates/trpg-agent/src/lib.rs
  # 预期：0 行
  ```

---

- [ ] **Step 4: 删 `trpg-agent/src/lib.rs` 中 `AgentPolicyPack::matching_rules` 和 `fn matches_conditions`**

  **4a. 删 `fn matches_conditions`（约 lines 324–334，私有 fn）**：
  ```rust
  // 删除：
  fn matches_conditions(lowered_input: &str, conditions: &MatchConditions) -> bool {
      if !conditions.not_terms.is_empty() && conditions.not_terms.iter().any(|term| lowered_input.contains(&term.to_lowercase())) {
          return false;
      }
      if !conditions.all_terms.is_empty() && !conditions.all_terms.iter().all(|term| lowered_input.contains(&term.to_lowercase())) {
          return false;
      }
      let any_ok = conditions.any_terms.is_empty() || conditions.any_terms.iter().any(|term| lowered_input.contains(&term.to_lowercase()));
      let regex_ok = conditions.regex_any.is_empty() || conditions.regex_any.iter().any(|pat| Regex::new(pat).map(|re| re.is_match(lowered_input)).unwrap_or(false));
      any_ok && regex_ok
  }
  ```

  **4b. 删 `AgentPolicyPack::matching_rules`（约 lines 223–236）**：
  ```rust
  // 删除：
  pub fn matching_rules(&self, input: &str) -> Vec<AgentAdviceRule> {
      let lowered = input.to_lowercase();
      let mut out = Vec::new();
      for layer in &self.advice_layers {
          if !layer.enabled { continue; }
          for rule in &layer.rules {
              if !rule.enabled { continue; }
              if matches_conditions(&lowered, &rule.match_conditions) {
                  out.push(rule.clone());
              }
          }
      }
      out
  }
  ```

  删后验证：
  ```bash
  grep -n "matching_rules\|matches_conditions" crates/trpg-agent/src/lib.rs
  # 预期：0 行

  # 确认 regex import 现在变成 dead import，若 Regex 无其他用途则一并删除
  grep -n "use regex::Regex" crates/trpg-agent/src/lib.rs
  grep -n "Regex" crates/trpg-agent/src/lib.rs | grep -v "^.*use regex"
  # 若 Regex 只在 matches_conditions 内使用（已删），删 `use regex::Regex;` 并从 Cargo.toml 的
  # trpg-agent 中移除 regex 依赖（如有）：
  grep "regex" crates/trpg-agent/Cargo.toml
  ```

---

- [ ] **Step 5: 删 `trpg-runtime/src/lib.rs` 中 `plan_agent_turn` 方法 + 清理 import**

  **5a. 删 `plan_agent_turn`（约 lines 339–358，含注释）**：
  ```rust
  // 删除以下整块（包含 doc 注释）：

      /// Rust-owned GM Agent planning step. The policy/advice content is loaded
      /// from JSON advice layers and is not mixed into the LLM system prompt.
      pub async fn plan_agent_turn(
          &self,
          request: &ContextRequest,
          state: &RuntimeState,
          user_input: &str,
      ) -> Result<AgentTurnPlan> {
          let agent = GmAgent::from_env_or_default();
          let actor_id = request.viewer.actor_id.as_deref().or(Some("pc.current"));
          let plan = agent.plan_turn(trpg_agent::AgentTurnInput {
              session_id: &request.session_id,
              turn_id: &request.turn_id,
              ruleset_id: &request.ruleset_id,
              module_id: request.module_id.as_deref().or(state.module_id.as_deref()),
              actor_id,
              user_input,
          });
          Ok(plan)
      }
  ```

  **5b. 从 line 7 的 import 中移除 `GmAgent`（其他 6 项保留）**：

  将：
  ```rust
  use trpg_agent::{contract_block, looks_like_new_action_or_abandon, make_pending_check, parse_roll_text, GmAgent, ParsedRollText};
  ```
  改为：
  ```rust
  use trpg_agent::{contract_block, looks_like_new_action_or_abandon, make_pending_check, parse_roll_text, ParsedRollText};
  ```

  删后验证：
  ```bash
  grep -n "plan_agent_turn\|GmAgent" crates/trpg-runtime/src/lib.rs
  # 预期：0 行
  ```

---

- [ ] **Step 6: 删 `trpg-api/src/lib.rs` 中 `/api/agent/plan` 路由及其 handler**

  **6a. 删 route 注册（line 55）**：
  ```rust
  // 删除：
          .route("/api/agent/plan", post(agent_plan_api))
  ```

  **6b. 删 OpenAPI 文档条目（约 line 109）**：
  ```rust
  // 删除：
              "/api/agent/plan": {"post": {"summary": "Run Rust GM Agent planning without streaming narration"}},
  ```

  **6c. 删 `AgentPlanRequest` struct + `agent_plan_api` handler（约 lines 928–942）**：
  ```rust
  // 删除（含 derive、struct、impl fn）：
  #[derive(Debug, Deserialize)]
  pub struct AgentPlanRequest {
      pub context_request: ContextRequest,
      pub runtime_state: RuntimeState,
      pub user_input: String,
  }

  async fn agent_plan_api(State(state): State<AppState>, Json(req): Json<AgentPlanRequest>) -> Result<Json<Value>, ApiError> {
      let plan = state.runtime.plan_agent_turn(&req.context_request, &req.runtime_state, &req.user_input).await?;
      state.runtime.persist_agent_plan(&plan).await?;
      let prompt_public = plan.check.as_ref()
          .filter(|_| matches!(plan.kind, TurnPlanKind::AskPlayerRoll))
          .map(pending_check_prompt);
      Ok(Json(json!({"plan": plan, "prompt_public": prompt_public})))
  }
  ```

  删后验证：
  ```bash
  grep -n "agent_plan_api\|AgentPlanRequest\|/api/agent/plan" crates/trpg-api/src/lib.rs
  # 预期：0 行
  ```

---

- [ ] **Step 7: `cargo check --workspace` 验证零 error**

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
  cargo check --workspace 2>&1 | grep -E "^error" | head -20
  # 预期：0 行（无 error 行）
  ```

  若出现 `error[E0425]: cannot find function` 或 `error[E0412]: cannot find type`，定位残留引用并按照前述步骤补删。常见误报：
  - `Regex` unused import in trpg-agent（Step 4 已处理）
  - `use post` for `agent_plan_api` in trpg-api 已被 Step 6 route 删除后无 `post` unused warning（`post` 仍被其他路由使用，不会产生 error）

---

- [ ] **Step 8: 全仓词法路径 grep 零残留验证（验收门）**

  ```bash
  # 验收命令 1：词法 advice 执行路径全消失
  grep -rn \
    "plan_agent_turn\|\.plan_turn\b\|matches_conditions\|matching_rules\|build_check_contract\|agent_plan_api\|AgentPlanRequest\|AgentTurnInput" \
    crates/ --include="*.rs" | grep -v "target/"
  # 预期：0 行

  # 验收命令 2：GmAgent 引用只剩 trpg-agent/src/lib.rs 内部定义（struct + from_env_or_default）
  grep -rn "GmAgent" crates/ --include="*.rs" | grep -v "target/" | grep -v "trpg-agent/src/lib.rs"
  # 预期：0 行（runtime import 已删，无其他 crate 引用）

  # 验收命令 3：cargo check 无 error
  cargo check --workspace 2>&1 | grep "^error" | wc -l
  # 预期：0
  ```

  **三条命令全部通过（0 行 / 0），T7 完成。**

---

**不删除的内容（T7 范围外）**

- `GmAgent` struct 本身 + `from_env_or_default`：壳保留，`AgentPolicyPack` 数据模型可能在未来 M1 扩展中被 execute_turn 读取，不做预防性删除。
- `persist_agent_plan`（runtime）：仍被 `named_or_forced_assessment_plan` 调用链使用，在 gate/named_check phase 中有意义，保留。
- `forced_technical_assessment_plan` / `named_or_forced_assessment_plan`：数据驱动、从 kernel 读骰种，非词法 advice 路径，保留。
- `AgentPolicyPack`、`AgentAdviceLayer`、`AgentAdviceRule`、`MatchConditions`、`AgentPlanAdviceKind`：JSON schema 结构体，删 `matching_rules`/`plan_turn` 后它们成为数据容器，`AgentPolicyPack::load_dir` 被 `GmAgent::from_env_or_default` 保留——如将来无新用途可在后续独立清理任务中删除。


---

### Task 8: decisive-cut 验证闸（harness + 真库三链 CLI&API + 零回归）

**Files:**
- Read-only: `crates/trpg-harness/src/main.rs`, `harness/cases/`, `harness/scenarios/`
- Create: `harness/scenarios/r1_coc_blood_road_2turn.json`, `harness/scenarios/r1_triangle_vault_2turn.json`
- Modify: none（验证步骤不改产品代码）

---

- [ ] **Step 1: cargo check --workspace（零编译错误基线）**

  在 workspace 根跑：

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
  cargo check --workspace 2>&1 | tail -5
  ```

  预期：末行为 `Finished` 或 `warning: ...`，无 `error[E...]`。若有 error 则 Task 8 挡在 Tasks 1-7 之前，不能继续。

---

- [ ] **Step 2: 构建 debug 二进制**

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
  cargo build -p trpg-cli -p trpg-harness 2>&1 | tail -5
  ```

  预期：`Finished dev [unoptimized + debuginfo]` 。两个二进制：`./target/debug/trpg`（来自 trpg-cli）、`./target/debug/trpg-harness`。

---

- [ ] **Step 3: 全 crate 单元测试零回归（分 crate 跑，跳过 DB live 测试）**

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
  cargo test --workspace --lib --bins 2>&1 | tail -20
  ```

  SKIP 条件：live DB 测试（需 `DATABASE_URL` 连接，此步不要求）；若有以 `live_` 为前缀或打了 `#[ignore]` 的测试被跳过属正常。
  预期输出末行：`test result: ok. N passed; 0 failed; ...`（每个 crate 一行）；汇总无 `FAILED`。

  若某 crate 失败，检查是否为 R1 引入的回归（与一期基线 diff），记录 crate 名和测试名，立即停止——这是 Task 8 的硬性 BLOCK。

---

- [ ] **Step 4: trpg-gm + trpg-formula 单独跑（含新增 execute_turn 测试）**

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm
  cargo test 2>&1 | tail -10

  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-formula
  cargo test 2>&1 | tail -10
  ```

  预期：`test result: ok. N passed; 0 failed`（N >= 一期 N，Tasks 1-7 应已增加测试数）。

---

- [ ] **Step 5: harness suite 全跑（73 个 cases，DB :54346，cyberpunk_red 默认）**

  harness suite 使用 `trpg turn` 子命令，通过 `DATABASE_URL` 连接当前 `.env` 配置的 DB（:54346 为 `.env` 默认，cyberpunk_red 场景用此库）。

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
  DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54346/chatrpg \
  cargo run -p trpg-harness -- suite \
    --dir harness/cases \
    --bin ./target/debug/trpg \
    --cwd . \
    --output text \
    --timeout-secs 120 \
  2>&1 | tee /tmp/r1_harness_suite.txt
  grep -E "^PASS|^FAIL|suite complete" /tmp/r1_harness_suite.txt | tail -10
  ```

  SKIP 条件：`harness/cases/` 中含 `module_id: "call_of_cthulhu_7e.document"` 或 `"triangle_agency.the_vault"` 的 case（如 `triangle_anomaly_frame_starts.json`、`parameter_facet_coc_sanity_track.json`）需把 `DATABASE_URL` 换成 `:54347`。可先跑全套，失败的 DB-wrong-port case 单独补跑 Step 6。

  预期：`suite complete: N passed, 0 failed`（N 应 >= 73 一期基线）。若出现 FAIL，需判定是 R1 引入还是一期既有——diff 验证方法见 Step 10。

---

- [ ] **Step 6: CoC + Triangle harness cases 补跑（DB :54347）**

  CoC 和 Triangle 规则集 session 在 `chatrpg-postgres-rulesets`（:54347）：

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula

  # CoC sanity track case
  DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
  cargo run -p trpg-harness -- run \
    --case harness/cases/parameter_facet_coc_sanity_track.json \
    --bin ./target/debug/trpg \
    --cwd . \
    --output text \
    --verbose

  # Triangle chaos track case
  DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
  cargo run -p trpg-harness -- run \
    --case harness/cases/parameter_facet_triangle_chaos_track.json \
    --bin ./target/debug/trpg \
    --cwd . \
    --output text \
    --verbose

  # Triangle anomaly frame (uses module_id triangle_agency.the_vault)
  DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
  cargo run -p trpg-harness -- run \
    --case harness/cases/triangle_anomaly_frame_starts.json \
    --bin ./target/debug/trpg \
    --cwd . \
    --output text \
    --verbose
  ```

  预期：三个 `PASS`。

---

- [ ] **Step 7: 新建 CoC 血色公路 2-turn CLI playtest scenario**

  创建 `harness/scenarios/r1_coc_blood_road_2turn.json`：

  ```json
  {
    "name": "r1_coc_blood_road_2turn",
    "ruleset_id": "call_of_cthulhu_7e",
    "module_id": "call_of_cthulhu_7e.document",
    "stream_format": "jsonl",
    "forbidden_player_text": [
      "请你掷", "请告诉我", "你来掷", "roll result", "total is"
    ],
    "turns": [
      {
        "user_input": "我们刚抵达小镇，停好车后四处张望，想了解一下当地的氛围和现在的时刻。",
        "notes": "入场观察。断言：GM 叙事非空（delta chars > 0），phase:done 出现，落库 turns +1，session current_scene_id 非 NULL。",
        "expect_phases": ["done"],
        "require_db_deltas": {
          "turns": 1,
          "memory_events": 1
        }
      },
      {
        "user_input": "我走进最近的酒吧，打算和酒保搭话，打探打探这儿发生的怪事。",
        "notes": "触发场景语义跳转意图。断言：GM 叙事非空，phase:done 出现，turns +1，若场景切换则 current_scene_id 变化（DB 查询断言在 Step 9 手动跑）。",
        "expect_phases": ["done"],
        "require_db_deltas": {
          "turns": 1
        }
      }
    ]
  }
  ```

---

- [ ] **Step 8: 新建 Triangle The Vault 2-turn CLI playtest scenario**

  创建 `harness/scenarios/r1_triangle_vault_2turn.json`：

  ```json
  {
    "name": "r1_triangle_vault_2turn",
    "ruleset_id": "triangle_agency",
    "module_id": "triangle_agency.the_vault",
    "stream_format": "jsonl",
    "forbidden_player_text": [
      "请你掷", "请告诉我", "roll result"
    ],
    "turns": [
      {
        "user_input": "我们的小队抵达了集合点，准备开始这次调查任务，我打算先和队友确认分工。",
        "notes": "任务开场。断言：GM 叙事非空，phase:done，turns +1，memory_events +1。",
        "expect_phases": ["done"],
        "require_db_deltas": {
          "turns": 1,
          "memory_events": 1
        }
      },
      {
        "user_input": "我负责侦察入口，靠近大楼正门，观察警卫的巡逻规律和监控摄像头位置。",
        "notes": "侦察动作，可能触发检定或进入下一场景。断言：GM 叙事非空，turns +1。",
        "expect_phases": ["done"],
        "require_db_deltas": {
          "turns": 1
        }
      }
    ]
  }
  ```

---

- [ ] **Step 9: 真库三链复测——CoC 链（CLI transport，DB :54347）**

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula

  DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
  cargo run -p trpg-harness -- playtest \
    --scenario harness/scenarios/r1_coc_blood_road_2turn.json \
    --bin ./target/debug/trpg \
    --cwd . \
    --output text \
    --output-dir /tmp/r1_coc_playtest \
    --timeout-secs 180 \
  2>&1 | tee /tmp/r1_coc_cli.txt
  ```

  手动断言（读 `/tmp/r1_coc_playtest/`）：

  ```bash
  # 1. 叙事非空：每回合 player_visible_output.txt 有实质内容
  wc -c /tmp/r1_coc_playtest/turn_001/player_visible_output.txt
  wc -c /tmp/r1_coc_playtest/turn_002/player_visible_output.txt
  # 预期：均 > 100 bytes

  # 2. 流式事件含 delta（非批量拼接）：events.jsonl 中含多行 {"event":"delta"...}
  grep -c '"event":"delta"' /tmp/r1_coc_playtest/turn_001/events.jsonl
  # 预期：>= 5（多个 delta chunk，证明非一次性缓冲输出）

  # 3. turns 落库
  jq '.db_diff.count_delta.turns' /tmp/r1_coc_playtest/turn_001/db_diff.json
  jq '.db_diff.count_delta.turns' /tmp/r1_coc_playtest/turn_002/db_diff.json
  # 预期：均为 1

  # 4. memory_events 落库（至少有 1 条 per turn）
  jq '.db_diff.count_delta.memory_events // 0' /tmp/r1_coc_playtest/turn_001/db_diff.json
  # 预期：>= 1
  ```

  Session ID 从 playtest_result.json 取：
  ```bash
  SESSION=$(jq -r '.session_id' /tmp/r1_coc_playtest/playtest_result.json)
  echo "CoC session: $SESSION"

  # 5. current_scene_id 非 NULL（入口激活）
  docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg \
    -c "SELECT current_scene_id FROM sessions WHERE session_id='$SESSION';"
  # 预期：current_scene_id 非空字符串

  # 6. world_events 含 player_action（record_player_action phase 落库）
  docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg \
    -c "SELECT event_kind, count(*) FROM world_events WHERE session_id='$SESSION' GROUP BY event_kind ORDER BY count(*) DESC;"
  # 预期：player_action >= 2（两回合），若有场景切换则 scene_changed >= 1

  # 7. 场景切换断言（第 2 回合若 scene_navigate 触发）
  SCENE_BEFORE=$(docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg -t \
    -c "SELECT current_scene_id FROM sessions WHERE session_id='$SESSION';" | tr -d ' ')
  echo "scene after turn 2: $SCENE_BEFORE"
  # 若 scene_changed world event 存在则 SceneTransition 发生，current_scene_id 应与 turn_001 后不同
  ```

  预期总体：`PASS r1_coc_blood_road_2turn` 输出。

---

- [ ] **Step 10: 真库三链复测——CPR Homecoming 链（CLI transport，DB :54346）**

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula

  DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54346/chatrpg \
  cargo run -p trpg-harness -- playtest \
    --scenario harness/scenarios/homecoming_external_playtest_8turn.json \
    --bin ./target/debug/trpg \
    --cwd . \
    --output text \
    --output-dir /tmp/r1_homecoming_playtest \
    --timeout-secs 240 \
  2>&1 | tee /tmp/r1_homecoming_cli.txt
  ```

  手动断言：

  ```bash
  # 叙事非空（Turn 1）
  wc -c /tmp/r1_homecoming_playtest/turn_001/player_visible_output.txt
  # 预期：> 100 bytes

  # delta 多行（流式）
  grep -c '"event":"delta"' /tmp/r1_homecoming_playtest/turn_001/events.jsonl
  # 预期：>= 5

  # turns 落库 per turn
  for i in $(seq -w 1 8); do
    echo "turn $i turns delta: $(jq '.db_diff.count_delta.turns' /tmp/r1_homecoming_playtest/turn_$(printf '%03d' $i)/db_diff.json)"
  done
  # 预期：每回合均为 1

  SESSION=$(jq -r '.session_id' /tmp/r1_homecoming_playtest/playtest_result.json)

  # scene_navigate 触发断言（Homecoming 有显式场景结构）
  docker exec chatrpg-postgres-v1162 psql -U chatrpg -d chatrpg \
    -c "SELECT event_kind, count(*) FROM world_events WHERE session_id='$SESSION' GROUP BY event_kind;"
  # 预期：scene_changed >= 1（场景导航在 8 回合内至少触发 1 次）

  # postprocess parity 断言：scene_navigator 已并入 execute_turn，CLI 路径也触发
  grep -l "scene_navigator_error" /tmp/r1_homecoming_playtest/turn_*/events.jsonl || echo "no scene_navigator_error (OK)"
  # 预期：无 scene_navigator_error（或有也只是 warn 不阻塞）
  ```

  SKIP 条件：若 `.env` 中 `DATABASE_URL` 已是 `:54346`，可不传 `DATABASE_URL=`前缀（直接继承）。

  预期：`PASS homecoming_external_playtest_8turn`（8 回合全 PASS）。

---

- [ ] **Step 11: 真库三链复测——Triangle The Vault 链（CLI transport，DB :54347）**

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula

  DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
  cargo run -p trpg-harness -- playtest \
    --scenario harness/scenarios/r1_triangle_vault_2turn.json \
    --bin ./target/debug/trpg \
    --cwd . \
    --output text \
    --output-dir /tmp/r1_triangle_playtest \
    --timeout-secs 180 \
  2>&1 | tee /tmp/r1_triangle_cli.txt

  # 断言：叙事非空
  wc -c /tmp/r1_triangle_playtest/turn_001/player_visible_output.txt
  # 预期：> 100 bytes

  # delta 多行
  grep -c '"event":"delta"' /tmp/r1_triangle_playtest/turn_001/events.jsonl
  # 预期：>= 5

  SESSION=$(jq -r '.session_id' /tmp/r1_triangle_playtest/playtest_result.json)

  # current_scene_id 激活
  docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg \
    -c "SELECT current_scene_id FROM sessions WHERE session_id='$SESSION';"
  # 预期：非空（入口场景已激活）

  # world_events
  docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg \
    -c "SELECT event_kind, count(*) FROM world_events WHERE session_id='$SESSION' GROUP BY event_kind;"
  # 预期：player_action >= 2
  ```

  预期：`PASS r1_triangle_vault_2turn`。

---

- [ ] **Step 12: API transport 复测——启动 API server + curl SSE（CoC 链，DB :54347）**

  终端 A 启动服务器（背景运行）：

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
  DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
  ./target/debug/trpg api --addr 127.0.0.1:8787 2>/tmp/r1_api_server.log &
  API_PID=$!
  sleep 2
  echo "API server pid: $API_PID"
  ```

  创建 CoC 新 session（从首回合 SSE 响应取 session_id）：

  ```bash
  # Turn 1: 首次请求（不带 session_id，API 自建）
  TURN1_EVENTS=$(curl -s -N -X POST http://127.0.0.1:8787/api/sessions/auto/turn \
    -H "Content-Type: application/json" \
    -d '{
      "ruleset_id": "call_of_cthulhu_7e",
      "module_id": "call_of_cthulhu_7e.document",
      "user_input": "我们刚抵达小镇，停好车后四处张望，想了解一下当地的氛围和现在的时刻。"
    }' 2>&1 | tee /tmp/r1_api_turn1.jsonl)
  ```

  注意：API route 是 `/api/sessions/{session_id}/turn`（POST）。需要先有一个 session_id。从 playtest 结果取或先调 start_session 端点。检查实际 API 端点：

  ```bash
  # 检查是否有 start_session 端点
  curl -s http://127.0.0.1:8787/api/openapi.json 2>/dev/null | python3 -m json.tool | grep -A2 '"path"' | head -40
  ```

  若无 start_session 端点，复用 Step 9 的 `$SESSION`（CoC session 已存在于 :54347 DB）：

  ```bash
  # 复用已有 session（Step 9 拿到的 SESSION）
  SESSION_COC="$SESSION"  # 从 Step 9 复制

  # Turn 1 via API SSE
  curl -s -N -X POST "http://127.0.0.1:8787/api/sessions/${SESSION_COC}/turn" \
    -H "Content-Type: application/json" \
    -d '{
      "ruleset_id": "call_of_cthulhu_7e",
      "module_id": "call_of_cthulhu_7e.document",
      "user_input": "我继续在酒吧里观察，注意到角落有一个人一直盯着我们。"
    }' 2>&1 | tee /tmp/r1_api_coc_turn1.jsonl
  ```

  断言（逐 token 流式）：

  ```bash
  # delta 事件多行（流式验证）
  grep -c '"event":"delta"' /tmp/r1_api_coc_turn1.jsonl
  # 预期：>= 5（证明非整体缓冲后输出）

  # 无 error event
  grep '"event":"error"' /tmp/r1_api_coc_turn1.jsonl && echo "ERROR FOUND" || echo "no error (OK)"

  # phase:done 出现
  grep '"phase":"done"' /tmp/r1_api_coc_turn1.jsonl
  # 预期：至少 1 行

  # postprocess_scheduled 出现（API 后台尾部语义）
  grep '"phase":"postprocess_scheduled"' /tmp/r1_api_coc_turn1.jsonl
  # 预期：1 行（API 后台 drain 尾部时会先发此事件）

  # Turn 2
  curl -s -N -X POST "http://127.0.0.1:8787/api/sessions/${SESSION_COC}/turn" \
    -H "Content-Type: application/json" \
    -d '{
      "ruleset_id": "call_of_cthulhu_7e",
      "module_id": "call_of_cthulhu_7e.document",
      "user_input": "我走向那个神秘的人，尝试和他搭话，看看他知道什么。"
    }' 2>&1 | tee /tmp/r1_api_coc_turn2.jsonl

  grep -c '"event":"delta"' /tmp/r1_api_coc_turn2.jsonl
  grep '"phase":"done"' /tmp/r1_api_coc_turn2.jsonl
  ```

  落库断言（给 postprocess 2 秒完成）：

  ```bash
  sleep 2
  docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg \
    -c "SELECT count(*) FROM turns WHERE session_id='${SESSION_COC}';"
  # 预期：相对 Step 9 结束时增加 2

  docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg \
    -c "SELECT current_scene_id FROM sessions WHERE session_id='${SESSION_COC}';"
  # 预期：current_scene_id 非空

  # scene_changed world event（如果第 2 回合触发场景导航）
  docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg \
    -c "SELECT event_kind, count(*) FROM world_events WHERE session_id='${SESSION_COC}' GROUP BY event_kind;"
  ```

  关闭 API server：

  ```bash
  kill $API_PID 2>/dev/null || true
  ```

---

- [ ] **Step 13: API transport 复测——CPR Homecoming（DB :54346）**

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula

  # 启动 API server（:54346）
  DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54346/chatrpg \
  ./target/debug/trpg api --addr 127.0.0.1:8787 2>/tmp/r1_api_cpr_server.log &
  API_PID=$!
  sleep 2

  # 取已有 CPR session（从 Step 10 playtest_result.json 或已知 session）
  SESSION_CPR=$(jq -r '.session_id' /tmp/r1_homecoming_playtest/playtest_result.json)
  echo "CPR session: $SESSION_CPR"

  # Turn 1 via API SSE
  curl -s -N -X POST "http://127.0.0.1:8787/api/sessions/${SESSION_CPR}/turn" \
    -H "Content-Type: application/json" \
    -d '{
      "ruleset_id": "cyberpunk_red",
      "module_id": "cyberpunk_red.homecoming",
      "user_input": "我用手势示意队友保持位置，然后悄悄绕到无人机左侧，准备切断电缆。"
    }' 2>&1 | tee /tmp/r1_api_cpr_turn1.jsonl

  # 断言
  grep -c '"event":"delta"' /tmp/r1_api_cpr_turn1.jsonl
  # 预期：>= 5

  grep '"phase":"postprocess_scheduled"' /tmp/r1_api_cpr_turn1.jsonl
  # 预期：1 行（API 后台模式）

  grep '"phase":"done"' /tmp/r1_api_cpr_turn1.jsonl
  # 预期：1 行

  # Turn 2
  curl -s -N -X POST "http://127.0.0.1:8787/api/sessions/${SESSION_CPR}/turn" \
    -H "Content-Type: application/json" \
    -d '{
      "ruleset_id": "cyberpunk_red",
      "module_id": "cyberpunk_red.homecoming",
      "user_input": "电缆切断了，无人机开始失控，我迅速撤回掩体后面。"
    }' 2>&1 | tee /tmp/r1_api_cpr_turn2.jsonl

  grep -c '"event":"delta"' /tmp/r1_api_cpr_turn2.jsonl
  grep '"phase":"done"' /tmp/r1_api_cpr_turn2.jsonl

  # 落库断言
  sleep 2
  docker exec chatrpg-postgres-v1162 psql -U chatrpg -d chatrpg \
    -c "SELECT count(*) FROM turns WHERE session_id='${SESSION_CPR}';"
  # 预期：比 Step 10 结束时多 2

  docker exec chatrpg-postgres-v1162 psql -U chatrpg -d chatrpg \
    -c "SELECT current_scene_id FROM sessions WHERE session_id='${SESSION_CPR}';"
  # 预期：非空

  # scene_navigator parity 断言（API 路径也触发了 scene_navigate）
  docker exec chatrpg-postgres-v1162 psql -U chatrpg -d chatrpg \
    -c "SELECT event_kind, count(*) FROM world_events WHERE session_id='${SESSION_CPR}' GROUP BY event_kind ORDER BY count(*) DESC;"
  # 预期：player_action >= 2；若场景切换则 scene_changed >= 1

  kill $API_PID 2>/dev/null || true
  ```

---

- [ ] **Step 14: TTFT 不劣化验证（对比一期基线中位 5s）**

  TTFT 定义：从 `trpg turn` 进程启动到第一个 `{"event":"delta"...}` 出现的墙钟时间。以 CPR Homecoming session 的简单输入为例（该模组已有解析缓存，前置确定性阶段较快）：

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula

  SESSION_CPR=$(jq -r '.session_id' /tmp/r1_homecoming_playtest/playtest_result.json 2>/dev/null || echo "MISSING_SESSION")

  for i in 1 2 3; do
    echo "=== TTFT sample $i ==="
    REQUEST=$(python3 -c "import json,sys; print(json.dumps({'ruleset_id':'cyberpunk_red','module_id':'cyberpunk_red.homecoming','session_id':'$SESSION_CPR','user_input':'我观察四周，保持警惕。'}))")
    time (DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54346/chatrpg \
      echo "$REQUEST" | ./target/debug/trpg turn --request-json - --stream-format jsonl \
      | awk 'BEGIN{start=0} /\"event\":\"delta\"/{if(start==0){print "FIRST DELTA"; start=1; exit}}' )
  done
  ```

  判读：`time` 输出的 `real` 时间即为 TTFT 代理值（包括进程启动开销约 0.2-0.5s，实际 LLM TTFT = real - 约 0.3s）。
  一期基线中位约 5s（MEMORY.md 记录）。
  可接受范围：p50 <= 7s（允许 R1 增加 2s 内的确定性阶段开销）；p50 > 10s 为劣化，需回查 phase_context_assembly 是否同步阻塞了 LLM 调用。

  若 TTFT 测量值可接受（<= 7s p50），记录三次样本取中位数，写进此步验证结论。

---

- [ ] **Step 15: postprocess parity 显式断言（scene_navigate 在 CLI + API 两路径都触发）**

  此步通过检查 world_events 表来确认 scene_navigate phase 在两 transport 均落库：

  ```bash
  # CoC：CLI 路径（Step 9 session）
  SESSION_COC_CLI=$(jq -r '.session_id' /tmp/r1_coc_playtest/playtest_result.json 2>/dev/null)
  docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg \
    -c "SELECT count(*) FROM world_events WHERE session_id='${SESSION_COC_CLI}' AND event_kind='scene_changed';"
  # 若 scene_navigate 在 execute_turn agent 路径首次触发，预期 >= 0（取决于叙事内容）；
  # 关键是不为 NULL/error——scene_navigate 被调用（即使不切换）不应报错。

  # CoC：API 路径（Step 12 session）
  docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg \
    -c "SELECT count(*) FROM world_events WHERE session_id='${SESSION_COC}' AND event_kind='scene_changed';"

  # CPR：CLI 路径（Step 10 session）
  SESSION_CPR_CLI=$(jq -r '.session_id' /tmp/r1_homecoming_playtest/playtest_result.json 2>/dev/null)
  docker exec chatrpg-postgres-v1162 psql -U chatrpg -d chatrpg \
    -c "SELECT count(*) FROM world_events WHERE session_id='${SESSION_CPR_CLI}' AND event_kind='scene_changed';"
  # 预期：>= 1（Homecoming 8 回合内场景切换可观测）

  # CPR：API 路径（Step 13 session）
  docker exec chatrpg-postgres-v1162 psql -U chatrpg -d chatrpg \
    -c "SELECT count(*) FROM world_events WHERE session_id='${SESSION_CPR}' AND event_kind='scene_changed';"
  ```

  如果 CLI session 有 scene_changed 而 API session 没有（在相同意图输入下），这是 postprocess parity 失败——scene_navigate 未在 API 路径触发。需回到 Task 6（execute_turn scene_navigate phase）检查 API transport 是否跳过了 conditional scene_navigate 的调用。

---

- [ ] **Step 16: 最终 cargo check + 全 crate test 零回归确认**

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula

  # 1. 编译
  cargo check --workspace 2>&1 | grep -E "^error|Finished"
  # 预期：只有 Finished，无 error

  # 2. 全 lib + bin 测试（跳过 live DB tests）
  cargo test --workspace --lib --bins 2>&1 | grep -E "^test result:|FAILED" | tail -30
  # 预期：全部 "test result: ok"，无 FAILED

  # 3. trpg-gm 完整测试（含 integration tests）
  cd crates/trpg-gm && cargo test 2>&1 | tail -5
  cd ../..

  # 4. trpg-rule-agent 完整测试（60 lib tests）
  cd crates/trpg-rule-agent && cargo test 2>&1 | tail -5
  cd ../..
  ```

  预期：全部绿，无回归。任何 FAILED 必须 triage——若与 R1 无关（一期既有 flaky test）记录为 pre-existing；若为 R1 引入则阻止合并。

---

- [ ] **Step 17: 回滚预案记录（git revert 整体退）**

  R1 所有变更在同一分支/PR 内。回滚操作：

  ```bash
  # 查看 R1 相关 commit range（以 Task 1 第一个 commit 到最新为例）
  git log --oneline --no-walk $(git merge-base HEAD main)..HEAD
  # 输出 R1 所有 commit SHA

  # 若验证闸中任何步骤 BLOCK：整体 revert 所有 R1 commits
  # 方法 A：revert merge commit（若 PR 已 squash merge）
  # git revert <merge-commit-sha>

  # 方法 B：revert 最近 N 个 commits（N = R1 commit 数，保持顺序倒序）
  # git revert --no-commit HEAD~N..HEAD
  # git commit -m "revert: R1 agent path default rollback — validation gate failed at Task 8 Step K"

  # 方法 C：直接 reset 到 R1 起始点（仅限未推远端分支时）
  # WARN: 破坏性，需确认无其他人依赖此分支
  # git reset --hard <pre-r1-sha>
  ```

  回滚触发条件：
  - Step 16 中有任何 `FAILED`（且确认为 R1 引入）
  - Step 14 TTFT p50 > 10s
  - Step 15 postprocess parity 失败（API 路径无 scene_navigate）
  - Steps 9-13 任何 `FAIL playtest` 输出

  回滚后恢复一期基线验证：
  ```bash
  cargo build -p trpg-cli -p trpg-harness
  DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54346/chatrpg \
  cargo run -p trpg-harness -- suite --dir harness/cases --bin ./target/debug/trpg --cwd . --output text
  # 预期恢复到一期 73 cases 全 PASS
  ```
