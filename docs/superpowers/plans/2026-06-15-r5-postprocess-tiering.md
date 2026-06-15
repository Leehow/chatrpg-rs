# R5 实施计划：postprocess 临界/重活分层 + turn 高水位守卫

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 R1 execute_turn 的 postprocess 尾段拆「critical(同步,TurnComplete 前落账) / heavy(后台,TurnComplete 后另 spawn)」，加 turn 高水位守卫消除玩家连发回合的陈旧态竞态；重活(深抽/audit/memory)不再挂 SSE 流、不阻塞下一回合。

**Architecture:** execute.rs run_pipeline 尾段拆两组：critical(save_turn + 切场景 set_session_scene/SceneChanged，同步 await→发 TurnComplete) / heavy(audit/memory 写/到场深抽/frontier/carryover/errata，tokio::spawn 另起任务，末写 pp_lifecycle=complete)。turns 加 pp_lifecycle 列(streaming→critical_done→complete)；回合入口 await_prev_turn_critical bounded 短等(fail-closed 放行不硬拒)。两 transport：API SSE 结束于 TurnComplete；CLI turn 等 heavy(确定性)、play 不等(守卫兜)。前置 R1(8c69ba9)+R2(6825891) 已合 main。

**Tech Stack:** Rust, tokio (spawn/select), trpg-gm execute_turn/turn_loop, trpg-runtime scene_navigation, trpg-db turns 迁移。前置 spec：`docs/superpowers/specs/2026-06-15-r5-postprocess-tiering-design.md`。

---

## 共享契约（精确类型/命名）

- postprocess 生命周期：streaming → critical_done → complete；**新增 turns 列 `pp_lifecycle`**(迁移，保 postprocess_status 现 ready/awaiting 义)；常量 PP_STREAMING/PP_CRITICAL_DONE/PP_COMPLETE；db 原语 `set_turn_pp_lifecycle(turn_id,phase)` + `load_last_turn_pp_lifecycle(session_id)->Option<String>`。
- execute.rs run_pipeline：critical 组(phase_finalize 的 save_turn[只 save_turn+status+pp_lifecycle=critical_done] + scene_navigate_critical[决策+set_session_scene+SceneChanged]) 同步 await → 发 TurnComplete → heavy 组(audit + memory_event 写 + scene_navigate_heavy[深抽+frontier] + carryover + errata 记忆，tokio::spawn，末写 pp_lifecycle=complete)。gm 按 value move 进 heavy spawn(RuntimeEngine/Arc 基自带，errata/obligations 连续性不破)。heavy 失败 warn 不影响 critical/已发事件。
- scene_navigation.rs 拆：`scene_navigate_critical(...)->Option<SceneNavCommit{from,to,reason}>` + `scene_navigate_heavy(db,llm,module_id,target,data_dir)`；`scene_navigator` 保留为薄 wrapper(critical→heavy 串联) 字节等价旧逻辑。
- 高水位守卫：`await_prev_turn_critical(db,session_id,timeout_ms=2000,interval_ms=100)` 接回合入口，<critical_done 轮询等到或超时 warn 放行(fail-closed 不硬拒)；>=critical_done 立即继续；heavy 未完(critical_done 非 complete)直接放行。
- 两 transport：API play_turn_sse drain 到 TurnComplete 结束 SSE；CLI turn 末 bounded 等 pp_lifecycle=complete 再退；CLI play 到 TurnComplete 不等。

## 文件结构总览

| 文件 | 动作 | 责任 | Task |
|---|---|---|---|
| `crates/trpg-gm/src/execute.rs` | Modify | run_pipeline 尾段拆 critical/heavy + heavy spawn | T1 |
| `crates/trpg-gm/src/turn_loop.rs` | Modify | finalize 拆 memory、phase_scene_navigate 拆 critical/heavy、phase_finalize 只 save_turn | T1 |
| `crates/trpg-runtime/src/scene_navigation.rs` | Modify | scene_navigate_critical/heavy + SceneNavCommit | T1 |
| `crates/trpg-db/src/lib.rs` + migrations | Modify | pp_lifecycle 列 + set/load 原语 | T2 |
| `crates/trpg-runtime/src/lib.rs` | Modify | await_prev_turn_critical 接 prepare_turn_context | T2 |
| `crates/trpg-api/src/lib.rs` + `crates/trpg-cli/src/{main,agent_play}.rs` | Modify | 两 transport 时序接线 | T3 |
| harness + 真库两 transport | Verify | R5 验证闸 | T4 |

---

### Task 1: execute.rs `run_pipeline` 尾段拆 critical/heavy + finalize 拆 memory + scene_navigate 拆切场景/深抽

R5 核心段。把 R1 的「单 spawn 跑全 postprocess 尾段 → TurnComplete」改成 **critical 组（同步，TurnComplete 前 await）→ 发 TurnComplete → heavy 组（owned `gm` move 进新 `tokio::spawn`，TurnComplete 后台跑）**。三处刀口：① `phase_finalize` 把 memory_event 写移出到 heavy（finalize 只 save_turn + status）；② `phase_scene_navigate` 拆 `navigate_critical`（决策 + `set_session_scene` + SceneChanged）/ `navigate_heavy`（深抽 + frontier）；③ `run_pipeline` 重排尾段时序 + heavy spawn。严守 R1 行为：critical = 旧 `finalize_turn` 的 save_turn + 旧 `scene_navigator` 的 set_session_scene/SceneChanged；heavy = 旧 audit_learning + turn memory + 旧 extract/prefetch + carryover/errata 记忆。本任务**不动** DB 列/常量/高水位守卫（Task 2）与 transport 接线（Task 3）——`pp_lifecycle` 写入留 Task 2 接管，本任务先把时序拆对、heavy 真后台、heavy 失败隔离。

**关键约束（已实读代码核对）**：
- `run_pipeline(gm: &mut GmLoop, …)`（execute.rs:85）现借用 `gm`；heavy spawn 需 `'static`，故本任务把 `execute_turn`(execute.rs:67) 的 `tokio::spawn` 里改成 **`run_pipeline` 取 `gm: GmLoop`（by value）**，critical 用 `&mut gm`，heavy 把 `gm` 整体 `move` 进新 spawn——`gm` 即 spec 要求的 "owned 句柄"（其内 `engine: RuntimeEngine`(Clone)/`llm: Arc<dyn LlmClient>`/`data_dir: PathBuf`/`scene_extractor`/`errata`/`obligations` 全随 `gm` 自带，无需逐个 clone，且 errata/obligations 的跨回合连续性不被破坏）。`OwnedTurnRequest` 的字段（module_id/data_dir/state/request/user_input）也 `move` 进 heavy。
- heavy 需读 `ctx` 的 agent_loop 产物（`visible_text`/`ledger`/`compiled`/`awaiting_gate`/`obligations`），故 `TurnContext` 也整体 `move` 进 heavy spawn（它是 `pub(crate)` 本地结构，move 安全）。
- critical 之后 heavy 之前必须先发 `TurnComplete`（contract：TurnComplete = critical 已落账可继续）。
- heavy spawn 内部不再持 `tx`（SSE/CLI drain 在 TurnComplete 后即可结束；heavy 事件本任务不发，纯后台——`HeavyPostprocessDone` 留后续可选）。
- fail-closed：heavy 任务 panic/Err 只 warn，绝不影响已 save 的 turn / 已发的 TurnComplete（D2）。各 `gm.phase_*` 内部已 `let _ =`/`warn` 自吞错，本任务额外在 heavy spawn 外层不 `.unwrap()`、不 `panic`。

**Files:**
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/execute.rs:67`（`execute_turn` 的 spawn）、`:85-127`（`run_pipeline`）、`:165-195`（`dispatch_postprocess`）— 重写尾段为 critical/heavy 两段
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/turn_loop.rs:564-594`（`finalize_turn`）— 拆出 memory_event/audit 到独立方法
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/turn_loop.rs:612-618`（`phase_finalize`）— 只留 save_turn 路径
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/turn_loop.rs:620-633`（`phase_scene_navigate`）— 拆 critical/heavy 两半
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-runtime/src/scene_navigation.rs:183-231`（`scene_navigator`）— 拆 `scene_navigate_critical` + `scene_navigate_heavy`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/execute_tests.rs:1`（测试模块）、`:92-107`（`exec_fixture`）、`:78-90`（`MockLlm`）— 加慢/失败 heavy 时序测试

---

- [ ] **Step 1 — 失败测试：scene_navigation 拆 critical/heavy 的纯函数边界**
  在 `scene_navigation.rs` 的 `#[cfg(test)] mod tests` 末尾（`:379` 后）加一个**不需 DB/LLM 的纯函数测试**，断言新拆出的 `scene_navigate_critical` 的「决策→target」纯逻辑（复用现有 `validate_transition`）与 heavy 的 frontier 选择互不耦合。先只加测试占位断言新函数存在的签名编译失败：
  ```rust
  #[tokio::test]
  async fn navigate_critical_and_heavy_have_separate_entrypoints() {
      // 编译级断言：两个新公开异步函数的存在与签名（critical 返回 Option<切换详情>，heavy 接受 target）。
      // 真 DB 行为由 e2e 验证闸覆盖；此处仅锁定 API 边界，防止合回一体。
      fn _assert_critical(f: fn(&trpg_db::Db, &dyn trpg_llm::LlmClient, &str, &str, &std::path::Path, &str, &str)
          -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Option<SceneNavCommit>>> + Send>>) { let _ = f; }
      fn _assert_heavy(f: fn(&trpg_db::Db, &dyn trpg_llm::LlmClient, &str, &str, &std::path::Path)
          -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>) { let _ = f; }
      // 不调用——仅靠类型推断在编译期校验签名稳定。
      let _ = (_assert_critical, _assert_heavy);
  }
  ```
  并在 tests 模块外（同文件顶层）引用尚不存在的 `SceneNavCommit` 类型 → 触发编译失败。

- [ ] **Step 2 — 验失败（编译错）**
  `cargo test -p trpg-runtime --no-run` → 预期 **编译失败**：`cannot find type SceneNavCommit`、`cannot find function scene_navigate_critical / scene_navigate_heavy`。记录失败输出确认是缺类型/缺函数，而非语法错。

- [ ] **Step 3 — 最小实现：scene_navigation.rs 拆 critical/heavy（完整代码）**
  在 `scene_navigation.rs` 把 `scene_navigator`（:183-231）拆成两个公开函数 + 一个返回类型。`scene_navigate_critical` 做「决策 + `set_session_scene` + SceneChanged world event」并返回切换详情（含 target，供 heavy 用）；`scene_navigate_heavy` 做「到场深抽 + frontier 前探」。保留 `scene_navigator` 为薄 wrapper（critical 后串 heavy）供任何旧调用方与单 transport 临时复用，行为字节等价旧逻辑。
  ```rust
  /// critical 切场景的产出：成功切换才 Some，携带新 target 供 heavy 深抽。
  /// from/reason 供调用方折成 SceneTransition 事件（execute.rs）。
  #[derive(Debug, Clone)]
  pub struct SceneNavCommit {
      pub from: String,
      pub to: String,
      pub reason: String,
  }

  /// R5 critical 半：语义判定 + set_session_scene + SceneChanged。**不**深抽/前探
  /// （那是 heavy，下一回合 prepare_turn_context 不强依赖——SkeletonOnly 降级块兜底）。
  /// 全程 fail-closed：无图/空图/LLM 失败/校验不过 → Ok(None)（留原场景）。
  pub async fn scene_navigate_critical(
      db: &Db,
      llm: &dyn LlmClient,
      session_id: &str,
      module_id: &str,
      _data_dir: &std::path::Path,
      player_input: &str,
      narration: &str,
  ) -> anyhow::Result<Option<SceneNavCommit>> {
      let Some(graph) = db.load_module_graph(module_id).await? else { return Ok(None); };
      if graph.scenes.is_empty() { return Ok(None); }
      let current = db.load_session_scene(session_id).await?.unwrap_or_default();
      let list = graph.scenes.iter()
          .map(|s| format!("{} | {} | {}", s.node_id, s.node_type, s.title))
          .collect::<Vec<_>>().join("\n");
      let cur_title = graph.scenes.iter()
          .find(|s| s.node_id == current)
          .map(|s| s.title.as_str())
          .unwrap_or("(未定)");
      let usr = build_nav_prompt(&current, cur_title, &list, player_input, narration);
      let decision = match llm.complete_json(vec![trpg_llm::system(SCENE_NAV_SYS), trpg_llm::user(&usr)], 0.0).await {
          Ok(v) => v,
          Err(err) => { tracing::warn!(error = %err, "scene_navigate_critical llm failed; stay"); return Ok(None); }
      };
      let Some(target) = validate_transition(&decision, &graph.scenes, &current) else { return Ok(None); };
      db.set_session_scene(session_id, &target).await?;
      let reason = decision.get("reason").and_then(|v| v.as_str()).unwrap_or("").to_string();
      info!(session_id, from = %current, to = %target, %reason, "scene transition (critical)");
      let event_data = json!({"kind": "scene_transition", "from": current, "to": target, "reason": reason, "module_id": module_id});
      if let Err(err) = WorldTimeService::new(db.clone())
          .record_event(session_id, None, None, WorldEventKind::SceneChanged, event_data, Visibility::GmOnly)
          .await
      {
          tracing::warn!(error = %err, "scene_navigate_critical: world event write failed; scene already switched");
      }
      Ok(Some(SceneNavCommit { from: current, to: target, reason }))
  }

  /// R5 heavy 半：到场深抽 + frontier 前探（best-effort，后台跑，下一回合不强依赖）。
  /// 全程 fail-closed：深抽/前探失败只 warn，绝不回滚已切换的场景。
  pub async fn scene_navigate_heavy(
      db: &Db,
      llm: &dyn LlmClient,
      module_id: &str,
      target: &str,
      data_dir: &std::path::Path,
  ) {
      if let Err(err) = extract_module_scenes(db, llm, module_id, None, None, data_dir, 12, Some(target)).await {
          tracing::warn!(error = %err, %target, "on-arrival deep-extract failed; scene already switched");
      }
      prefetch_frontier(db, llm, module_id, target, data_dir).await;
  }
  ```
  把旧 `scene_navigator`(:183-231) 改写成 wrapper：
  ```rust
  /// 旧入口（critical→heavy 串行一体）。保留供尚未拆分的调用方；新执行器走拆分路径。
  pub async fn scene_navigator(
      db: &Db, llm: &dyn LlmClient, session_id: &str, module_id: &str,
      data_dir: &std::path::Path, player_input: &str, narration: &str,
  ) -> anyhow::Result<()> {
      if let Some(commit) = scene_navigate_critical(db, llm, session_id, module_id, data_dir, player_input, narration).await? {
          scene_navigate_heavy(db, llm, module_id, &commit.to, data_dir).await;
      }
      Ok(())
  }
  ```

- [ ] **Step 4 — 验通过（runtime）**
  `cargo test -p trpg-runtime` → 预期：Step 1 的边界测试 + 原有 `validate_transition_is_fail_closed` / `build_nav_prompt_*` 全绿，零回归。

- [ ] **Step 5 — commit**
  `git checkout -b claude/r5-postprocess-tiering-execute`（若尚未在 R5 分支）；`git add crates/trpg-runtime/src/scene_navigation.rs` → `git commit`（信息：`R5 Task1a: split scene_navigator into critical(set_scene+event) / heavy(deep-extract+frontier)`，末尾加 `Co-Authored-By` 行）。

- [ ] **Step 6 — 失败测试：finalize 拆 memory（turn_loop 单测）**
  现 `finalize_turn`(:568-594) 把 save_turn + memory + audit 三件事绑一起。要拆出 memory/audit。在 `turn_loop_tests.rs`（`#[path = "turn_loop_tests.rs"]` mod，:690）加一个**不需真 DB 的方法存在性测试**（lazy pool，MockLlm，断言新方法签名编译）：
  ```rust
  #[test]
  fn finalize_save_and_heavy_memory_are_separable() {
      // 编译级断言：save_turn-only 的 phase_finalize 与 heavy memory 写各有独立入口。
      fn _assert<'a>(
          f1: fn(&'a mut GmLoop, &'a mut TurnContext, &'a GmTurnInput<'a>, &'a str)
              -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>,
          f2: fn(&'a GmLoop, &'a GmTurnInput<'a>, &'a trpg_model::RuntimeState, &'a trpg_model::CompiledContext, &'a str)
              -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>,
      ) { let _ = (f1, f2); }
      let _ = _assert;
  }
  ```
  （`f1` = 现 `phase_finalize` 签名不变但语义改为只 save；`f2` = 新 `heavy_finalize_memory`，参数为 turn 写记忆 + audit 所需 owned 视图。）

- [ ] **Step 7 — 验失败（编译错）**
  `cargo test -p trpg-gm --no-run` → 预期编译失败：`no method named heavy_finalize_memory`。

- [ ] **Step 8 — 最小实现：turn_loop.rs 拆 finalize（完整代码）**
  把 `finalize_turn`(:568-594) 拆成两块：`finalize_save_turn`（只 save_turn + status，critical）与 `heavy_finalize_memory`（turn 摘要 memory_event + audit_learning，heavy）。改 `phase_finalize`(:612-618) 调 `finalize_save_turn`。新增 `phase_finalize_heavy_memory` 供 execute.rs heavy 段调用（从 `ctx` 读 compiled、从 input 读 state，复用旧 memory/audit 逻辑逐字搬）。
  ```rust
  /// R5 critical：只持久化回合记录 + status（save_turn）。memory/audit 移到 heavy。
  /// 叙事已流出不可回收，save 失败 warn 不 panic（MockLlm lazy pool 下保持绿）。
  pub(crate) async fn finalize_save_turn(&self, request: &ContextRequest, compiled: &CompiledContext, user_input: &str, assistant_output: &str, status: &str) {
      let hashes = json!({"prefix": compiled.prefix_hash, "pinned": compiled.pinned_hash, "dynamic": compiled.dynamic_hash});
      if let Err(err) = self.engine.db.save_turn(&request.session_id, &request.turn_id, user_input, assistant_output, hashes, status).await {
          tracing::warn!(error = %err, "agent path save_turn failed");
      }
  }

  /// R5 heavy：富版回合记忆 + learning audit（下一回合不强依赖；后台跑）。逐字搬自
  /// 旧 finalize_turn 的 memory/audit 半边。失败只 warn，绝不影响已 save 的 turn。
  pub(crate) async fn heavy_finalize_memory(&self, request: &ContextRequest, state: &RuntimeState, user_input: &str, assistant_output: &str) {
      if assistant_output.trim().is_empty() { return; }
      let summary: String = assistant_output.chars().take(280).collect();
      let transcript_excerpt = Some(format!(
          "Player: {}\nGM: {}",
          user_input,
          assistant_output.chars().take(2000).collect::<String>()
      ));
      let event = MemoryEvent { event_id: format!("mem_turn_{}", Uuid::new_v4().simple()), session_id: request.session_id.clone(), turn_id: Some(request.turn_id.clone()), ruleset_id: request.ruleset_id.clone(), module_id: request.module_id.clone(), scene_id: state.scene_id.clone(), location_id: state.location_id.clone(), actor_ids: state.active_npc_ids.clone(), visibility: Visibility::GmOnly, event_kind: MemoryKind::Event, summary, transcript_excerpt, source: json!({"source":"gm_agent.turn_summary"}), tags: vec!["turn".to_string(), "session_memory".to_string(), "gm_turn".to_string()], importance: 50, occurred_at: Utc::now() };
      if let Err(err) = self.engine.db.save_memory_event(&event).await {
          tracing::warn!(error = %err, "agent path turn summary memory event failed");
      }
      if let Err(err) = self.engine.audit_learning_for_turn(&request.session_id, &request.ruleset_id, request.module_id.as_deref(), &request.turn_id, user_input, assistant_output).await {
          tracing::warn!(error = %err, "agent path learning audit failed");
      }
  }
  ```
  `phase_finalize`(:612-618) 改为调 `finalize_save_turn`（注意 awaiting 终态的 assistant_output 兜底逻辑保留）：
  ```rust
  pub(crate) async fn phase_finalize(&mut self, ctx: &mut TurnContext, input: &GmTurnInput<'_>, status: &str) {
      let assistant_output = match &ctx.awaiting_gate {
          Some(gate) if ctx.visible_text.trim().is_empty() => gate.prompt_public.clone(),
          _ => ctx.visible_text.clone(),
      };
      self.finalize_save_turn(input.request, &ctx.compiled, input.user_input, &assistant_output, status).await;
  }

  /// R5 heavy：phase_finalize 的 memory/audit 半边（execute.rs heavy 段调）。
  pub(crate) async fn phase_finalize_heavy_memory(&self, ctx: &TurnContext, input: &GmTurnInput<'_>) {
      let assistant_output = match &ctx.awaiting_gate {
          Some(gate) if ctx.visible_text.trim().is_empty() => gate.prompt_public.clone(),
          _ => ctx.visible_text.clone(),
      };
      self.heavy_finalize_memory(input.request, input.state, input.user_input, &assistant_output).await;
  }
  ```
  保留旧 `finalize_turn` 为 wrapper（`run_gm_turn` 仍调它，R1 未退役）：`self.finalize_save_turn(...).await; self.heavy_finalize_memory(...).await;`。

- [ ] **Step 9 — 拆 phase_scene_navigate（turn_loop.rs）**
  把 `phase_scene_navigate`(:624-633) 拆成 critical/heavy 两个 wrapper，桥到 Step 3 的 runtime 函数。critical 返回 `SceneTransitionInfo`（execute.rs 折成 `SceneTransition` 事件，**修复 R1 现状恒 None 的 TODO**）；heavy 读 critical 产出的 target 深抽。
  ```rust
  /// R5 critical：切场景决策 + set_session_scene + SceneChanged。返回切换详情供
  /// execute.rs 发 SceneTransition 事件（R1 旧实现恒 None，本期修复）。
  pub(crate) async fn phase_scene_navigate_critical(&mut self, ctx: &TurnContext, input: &GmTurnInput<'_>) -> Option<SceneTransitionInfo> {
      let module_id = input.request.module_id.as_deref()?;
      match trpg_runtime::scene_navigation::scene_navigate_critical(
          &self.engine.db, self.llm.as_ref(), &input.request.session_id, module_id,
          self.data_dir.as_path(), input.user_input, &ctx.visible_text,
      ).await {
          Ok(Some(c)) => Some(SceneTransitionInfo { from: c.from, to: c.to, reason: c.reason }),
          Ok(None) => None,
          Err(err) => { tracing::warn!(error = %err, "agent path scene_navigate_critical failed"); None }
      }
  }

  /// R5 heavy：到场深抽 + frontier（后台）。target 来自 critical 的 SceneTransitionInfo.to。
  pub(crate) async fn phase_scene_navigate_heavy(&self, target: &str, module_id: &str) {
      trpg_runtime::scene_navigation::scene_navigate_heavy(
          &self.engine.db, self.llm.as_ref(), module_id, target, self.data_dir.as_path(),
      ).await;
  }
  ```
  删除旧 `phase_scene_navigate`(:624-633)（execute.rs 不再调它）。

- [ ] **Step 10 — 验通过（gm 单测）**
  `cargo test -p trpg-gm` → 预期 Step 6 边界测试 + 既有 turn_loop/execute/mode 单测全绿（execute.rs 尚未改 → 仍用旧路径但已有新方法可编译）。`SKIP_DB_TESTS=1 cargo test -p trpg-gm` 跑无 DB 子集确认非 DB 测试全过（:54347 DB 测试标 SKIP）。

- [ ] **Step 11 — commit**
  `git add crates/trpg-gm/src/turn_loop.rs` → `git commit`（`R5 Task1b: split finalize(save vs memory/audit) and scene_navigate(critical vs heavy) wrappers`，加 `Co-Authored-By`）。

- [ ] **Step 12 — 失败测试①：heavy 慢不挂 TurnComplete（execute_tests.rs）**
  在 `execute_tests.rs` 加测试：注入一个**会让 heavy 段 sleep 的 seam**，断言 `TurnComplete` 在 heavy 完成**之前**就到达流。由于 heavy 段调用 `scene_navigate_heavy`（需 module + 真图）与 memory（需真 DB），纯 mock 无法直接慢 heavy；改用**可观测时序断言**：在 heavy spawn 入口与 TurnComplete 发送点各记一个时间戳到共享 `Arc<Mutex<Vec<&str>>>` order log（通过 `GmLoop` 新增一个 `#[cfg(test)]` 可选 `heavy_probe: Option<Arc<...>>` seam，或更简洁——用 `MockLlm` 在 heavy 唯一会调的 `complete_json`（scene nav）里 sleep）。最小可行：让 `MockLlm.complete_json` 在被调用时 `tokio::time::sleep(200ms)` 并 push `"heavy_llm"` 到 order log；在 stream drain 处收到 `TurnComplete` 时 push `"turn_complete"`。断言 `order` 中 `turn_complete` 在 `heavy_llm` **之前**：
  ```rust
  #[tokio::test]
  async fn turn_complete_precedes_heavy_work() {
      if std::env::var("SKIP_DB_TESTS").is_ok() { return; }
      let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));
      // MockLlm 变体：complete_json（scene nav，仅 heavy 用）sleep+记录；module_id=Some 触发 scene_navigate。
      let (gm, mut req) = exec_fixture_with_order(vec![vec![
          StreamEvent::ContentDelta("叙事。".into()),
          StreamEvent::Done { finish_reason: Some("stop".into()) },
      ]], order.clone());
      req.module_id = Some("mod1".into());
      req.request.module_id = Some("mod1".into());
      let mut stream = execute_turn(gm, req, CANONICAL_TURN_PLAN);
      while let Some(ev) = stream.next().await {
          if let TurnEvent::TurnComplete { .. } = ev { order.lock().unwrap().push("turn_complete"); }
      }
      // drain 结束（SSE/CLI 在 TurnComplete 即可结束）后给 heavy spawn 一点时间跑完。
      tokio::time::sleep(std::time::Duration::from_millis(400)).await;
      let log = order.lock().unwrap().clone();
      let tc = log.iter().position(|x| *x == "turn_complete");
      let heavy = log.iter().position(|x| *x == "heavy_llm");
      assert!(tc.is_some(), "must see TurnComplete");
      if let (Some(tc), Some(heavy)) = (tc, heavy) {
          assert!(tc < heavy, "TurnComplete 必须先于 heavy LLM 工作（heavy 不得挂住 TurnComplete），order={log:?}");
      }
  }
  ```
  （`exec_fixture_with_order` = `exec_fixture` 的变体，`MockLlm.complete_json` 改为 `{ order.lock().push("heavy_llm"); sleep(200ms); Ok(json!({"hits":[],"moved":false})) }`。注：`moved:false` 让 scene critical 不切场景 → 但 scene critical 的 `load_module_graph`/`complete_json` 调用本身就只在 critical 段；要把 sleep 放在**仅 heavy 调用的点**。若 critical 也调 `complete_json`（scene_navigate_critical 会调），则改用 `heavy_finalize_memory` 唯一调的 `save_memory_event`/`audit_learning_for_turn` 作为 heavy 探针——见 Step 13 备注，drafter 落地时按真实调用图二选一，确保探针只在 heavy 段触发。）

- [ ] **Step 13 — 失败测试②③：heavy 失败不影响 critical；事件序 Delta→TurnComplete**
  同文件加两测试。② heavy panic 隔离：让 heavy 段唯一调用点返回 Err/panic（如 `MockLlm` 在 heavy 探针处 `panic!`），断言 stream 仍正常发出 `TurnComplete`（critical 已落账、流不被毒化）——因 heavy 在独立 `tokio::spawn`，panic 只杀该任务不波及主 drain：
  ```rust
  #[tokio::test]
  async fn heavy_panic_does_not_break_turn_complete() {
      if std::env::var("SKIP_DB_TESTS").is_ok() { return; }
      // heavy 探针处 panic 的 MockLlm 变体（仅 heavy 段触发）。
      let (gm, mut req) = exec_fixture_panicking_heavy(vec![vec![
          StreamEvent::ContentDelta("叙事。".into()),
          StreamEvent::Done { finish_reason: Some("stop".into()) },
      ]]);
      req.module_id = Some("mod1".into()); req.request.module_id = Some("mod1".into());
      let mut stream = execute_turn(gm, req, CANONICAL_TURN_PLAN);
      let mut completed = false;
      while let Some(ev) = stream.next().await {
          if let TurnEvent::TurnComplete { .. } = ev { completed = true; }
      }
      assert!(completed, "heavy panic 后 TurnComplete 仍必须到达（critical/已发事件不受影响）");
  }
  ```
  ③ 事件序：复用现有 `execute_turn_streams_deltas_and_completes` 强化为断言**全序**——记录所有事件 kind，断言所有 `Delta` 严格早于 `TurnComplete`，且 `TurnComplete` 是流的**最后一个 transport 可见事件**（heavy 在其后台、不经 tx）：
  ```rust
  #[tokio::test]
  async fn event_order_deltas_then_turn_complete_last() {
      if std::env::var("SKIP_DB_TESTS").is_ok() { return; }
      let (gm, req) = exec_fixture(vec![vec![
          StreamEvent::ContentDelta("A".into()), StreamEvent::ContentDelta("B".into()),
          StreamEvent::Done { finish_reason: Some("stop".into()) },
      ]]);
      let mut stream = execute_turn(gm, req, CANONICAL_TURN_PLAN);
      let mut kinds: Vec<&str> = vec![];
      while let Some(ev) = stream.next().await {
          kinds.push(match ev { TurnEvent::Delta(_) => "delta", TurnEvent::TurnComplete { .. } => "complete", TurnEvent::PostprocessScheduled => "pp_sched", _ => "other" });
      }
      let last = kinds.last().copied();
      assert_eq!(last, Some("complete"), "TurnComplete 必须是 transport 末事件（heavy 不经 tx）");
      let c = kinds.iter().position(|k| *k == "complete").unwrap();
      assert!(kinds[..c].iter().all(|k| *k != "complete"), "TurnComplete 之前不得有第二个 complete");
      assert!(kinds[..c].iter().any(|k| *k == "delta"), "TurnComplete 前必须有 Delta");
  }
  ```

- [ ] **Step 14 — 验失败**
  `cargo test -p trpg-gm --no-run` → 预期编译失败（缺 `exec_fixture_with_order` / `exec_fixture_panicking_heavy` 辅助函数 + execute.rs 尚未拆 heavy → 时序断言会逻辑失败）。先确认编译错来自缺辅助函数，补辅助函数后 `cargo test -p trpg-gm` 应在 `turn_complete_precedes_heavy_work` / `event_order_*` 上**逻辑失败**（现 R1 把 heavy 全在 TurnComplete 前跑 → TurnComplete 不是末事件 / heavy 早于 complete），记录失败输出。

- [ ] **Step 15 — 最小实现：execute.rs run_pipeline 拆 critical/heavy + heavy spawn（完整代码）**
  改 `execute_turn`(execute.rs:67) 把 `run_pipeline(&mut gm, …)` 改成 `run_pipeline(gm, …)`（by value），并改 `run_pipeline` 签名为 `gm: GmLoop`。重写 `run_pipeline` 尾段(:113-127)：
  ```rust
  async fn run_pipeline(
      mut gm: GmLoop,
      req: OwnedTurnRequest,
      plan: &'static [TurnPhasePlan],
      tx: &tokio::sync::mpsc::Sender<TurnEvent>,
  ) {
      let mut ctx = TurnContext::new();
      // —— 1. 确定性头部（不变）——
      {
          let input = GmTurnInput { request: &req.request, state: &req.state, user_input: &req.user_input, history: &req.history, recent_transcript: req.recent_transcript.as_deref() };
          for phase in plan.iter().filter(|p| p.kind == PhaseKind::Deterministic) {
              if !dispatch_deterministic(&mut gm, &mut ctx, &input, phase.id).await {
                  let _ = tx.send(TurnEvent::TurnComplete { outcome: gm.take_outcome(&mut ctx) }).await;
                  return;
              }
          }
          // —— 2. AgentLoop body（不变）——
          let signal = gm.run_agent_loop(&mut ctx, &input, tx).await;

          // —— 3a. CRITICAL 尾段（同步，TurnComplete 前 await）——
          // 选 phase（与 R1 同），但只跑 critical 部分：VerifyAfterStream（critical：
          // 勘误判定要在 finalize 前注入 ledger 真相）→ Finalize 的 save_turn →
          // SceneNavigate 的切场景决策+set_session_scene+SceneChanged 事件。
          let module_present = req.module_id.is_some();
          let has_pending = gm.has_pending_obligations();
          let selected = select_phases(plan, signal, module_present, has_pending);
          let mut scene_commit: Option<crate::turn_loop::SceneTransitionInfo> = None;
          for phase in &selected {
              if phase.kind != PhaseKind::Postprocess { continue; }
              match phase.id {
                  PhaseId::VerifyAfterStream => {
                      let _ = tx.send(TurnEvent::PostprocessScheduled).await;
                      gm.phase_verify_after_stream(&mut ctx, &input).await;
                  }
                  PhaseId::Finalize => {
                      let status = match signal { AgentSignal::AwaitingPlayerRoll => "awaiting_player_roll", AgentSignal::Narration => "ready" };
                      gm.phase_finalize(&mut ctx, &input, status).await; // 现只 save_turn
                  }
                  PhaseId::SceneNavigate => {
                      if let Some(t) = gm.phase_scene_navigate_critical(&ctx, &input).await {
                          let _ = tx.send(TurnEvent::SceneTransition { from: t.from.clone(), to: t.to.clone(), reason: t.reason.clone() }).await;
                          scene_commit = Some(t);
                      }
                  }
                  _ => {} // AuditLearning/CarryoverDebt → heavy
              }
          }
          // —— 4. TurnComplete（critical 已落账，可继续下一回合）——
          let outcome = gm.take_outcome(&mut ctx);
          let _ = tx.send(TurnEvent::TurnComplete { outcome }).await;
          drop(input); // 释放对 req 的借用，下面 move req 进 heavy
          // —— 3b. HEAVY 尾段（TurnComplete 后另起 spawn，gm/ctx/req 整体 move）——
          spawn_heavy(gm, ctx, req, selected, scene_commit);
          return;
      }
  }
  ```
  注意：`input` 借用 `req`，heavy 需 own `req` → 把 head/critical 放进一个块、块结束前 `drop(input)`，再 `spawn_heavy` move `gm/ctx/req`。`selected` 是 `Vec<TurnPhasePlan>`（Copy 元素），move 安全。新增 `spawn_heavy`：
  ```rust
  /// HEAVY 尾段：TurnComplete 后台跑——audit/memory 写、到场深抽+frontier、carryover、
  /// errata 记忆。owned gm/ctx/req（spec：heavy 需 owned 句柄，沿用 R1 spawn 模式）。
  /// 失败隔离：整个任务包在 spawn 里，panic 只杀该任务，绝不影响已发 TurnComplete（D2）。
  fn spawn_heavy(
      gm: GmLoop,
      ctx: TurnContext,
      req: OwnedTurnRequest,
      selected: Vec<TurnPhasePlan>,
      scene_commit: Option<crate::turn_loop::SceneTransitionInfo>,
  ) {
      tokio::spawn(async move {
          let gm = gm; let ctx = ctx; // own
          let input = GmTurnInput { request: &req.request, state: &req.state, user_input: &req.user_input, history: &req.history, recent_transcript: req.recent_transcript.as_deref() };
          // turn 摘要 memory + learning audit（原 finalize_turn 的 memory/audit 半边）。
          gm.phase_finalize_heavy_memory(&ctx, &input).await;
          // 到场深抽 + frontier（仅 critical 真切了场景时）。
          if let (Some(commit), Some(module_id)) = (&scene_commit, req.module_id.as_deref()) {
              gm.phase_scene_navigate_heavy(&commit.to, module_id).await;
          }
          // carryover 债务记忆（select_phases 已据 has_pending 决定是否在 selected 里）。
          if selected.iter().any(|p| p.id == PhaseId::CarryoverDebt && p.kind == PhaseKind::Postprocess) {
              let mut gm = gm; let mut ctx = ctx; // carryover/errata 走 &mut self
              gm.phase_carryover_debt(&mut ctx, &input).await;
              // T2 将在此处写 pp_lifecycle=complete（本任务暂不写）。
              let _ = (&mut gm, &mut ctx);
          }
          // errata 记忆已在 critical 的 phase_verify_after_stream 内落账（save_memory_event）——
          // R1 行为：verify 在 finalize 前。本任务保持 verify 在 critical（勘误是 finalize 前注入
          // ledger 真相的依赖），其 memory 写本就同步。heavy 不重复 errata 写。
      });
  }
  ```
  删除/收编旧 `dispatch_postprocess`(:165-195)（其逻辑已并入 critical 循环 + spawn_heavy；保留 `dispatch_deterministic`）。
  **R1 行为保真核对**：critical 跑 VerifyAfterStream（同 R1 顺序，verify 在 finalize 前）+ Finalize 的 save_turn（原 finalize_turn 的 save_turn 半边）+ SceneNavigate 的 set_session_scene/SceneChanged（原 scene_navigator 的 critical 半边）；heavy 跑 memory/audit（原 finalize_turn 后半）+ 深抽/frontier（原 scene_navigator 后半）+ carryover（原 CarryoverDebt phase）。phase 集合与 `select_phases` 完全一致 → AwaitingPlayerRoll 早返语义不变（无 SceneNavigate/CarryoverDebt → scene_commit=None、heavy 只跑 memory）。

- [ ] **Step 16 — 验通过（gm 全测，:54347 在线）**
  `cargo test -p trpg-gm` → 预期：Step 12/13 的三个时序/隔离测试转绿（TurnComplete 末事件、heavy 后台、panic 隔离），原有 4 个 `select_phases` 纯测试 + `execute_turn_streams_deltas_and_completes` + turn_loop/mode 单测全绿。`SKIP_DB_TESTS=1 cargo test -p trpg-gm` 确认无 DB 子集全过。

- [ ] **Step 17 — 验通过（workspace 编译 + runtime 回归）**
  `cargo build --workspace` 确认 trpg-api/trpg-cli 仍编译（它们经 `execute_turn` 入口，签名未变——只是 `run_pipeline` 内部，对外 `execute_turn(gm, req, plan)` 签名不动）。`cargo test -p trpg-runtime` 复跑 scene_navigation 测试零回归。

- [ ] **Step 18 — commit**
  `git add crates/trpg-gm/src/execute.rs crates/trpg-gm/src/execute_tests.rs` → `git commit`（`R5 Task1c: split run_pipeline into critical(save+set_scene+event) then TurnComplete then heavy(memory/audit/deep-extract/frontier/carryover) spawn`，加 `Co-Authored-By`）。

**Task 1 完成判据**：critical 段（save_turn + set_session_scene + SceneChanged）在 `TurnComplete` 前同步完成；heavy 段（memory/audit/深抽/frontier/carryover）在独立 `tokio::spawn` 里 TurnComplete 后跑；heavy panic 不影响已发 TurnComplete（独立任务隔离）；事件序 Delta…→PostprocessScheduled→(critical)→TurnComplete 为末 transport 事件；`select_phases` 集合与 AwaitingPlayerRoll 早返语义保持 R1 不变；`SceneTransition` 事件在真切场景时发出（修 R1 恒 None）；文件 ≤400 行（execute.rs 拆后约 200 行、turn_loop.rs 新增方法控制在原文件、scene_navigation.rs 约 380 行需复核拆分后行数，若超 400 把 critical/heavy 函数移至同 crate 新 `scene_navigation_tiered.rs` 子模块）；零规则集硬编码；fail-closed（heavy 失败仅 warn）。**留给 Task 2**：critical 末写 `pp_lifecycle=critical_done`、heavy 末写 `pp_lifecycle=complete`、高水位守卫（Step 15 已在 spawn_heavy carryover 后留注释锚点）。


---

### Task 2: postprocess 生命周期状态机（pp_lifecycle 列 + 常量 + db 原语）+ turn 高水位守卫

落地 spec §4.2 的「`turns.postprocess_status` 升级为完成追踪 `streaming`→`critical_done`→`complete`」与「回合入口 bounded 短等」。**关键决策**：不复用/覆写现有 `postprocess_status`（它的 `ready`/`awaiting_player_roll`/`draft_needs_rules_source` 语义被 `save_character`、`save_turn`、chargen、API 多处消费，覆写会破坏建卡与 awaiting-roll 终态），而是新增独立列 `pp_lifecycle` 专做生命周期追踪。`save_turn` 现有签名与 `ready`/`awaiting` 写入**完全不动**。本 Task 只交付 db 层与守卫；T1（execute.rs 分层）在 critical 末调 `set_turn_pp_lifecycle(turn_id, PP_CRITICAL_DONE)`、heavy 末调 `set_turn_pp_lifecycle(turn_id, PP_COMPLETE)`——接缝是这三个 db 原语 + 三个常量。

**Files:**
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/migrations/0028_turn_pp_lifecycle_v120.sql`（新建：迁移 SQL）
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-db/src/lib.rs:24-52`（migrate() 的 `include_str!` 数组——手动追加 0028 行）
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-db/src/lib.rs:987-1010`（save_turn 区——其后新增 `set_turn_pp_lifecycle` + `load_last_turn_pp_lifecycle` 两原语）
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-model/src/lib.rs:16-21`（pub const 区——新增 `PP_STREAMING`/`PP_CRITICAL_DONE`/`PP_COMPLETE` + `pp_lifecycle_rank` 辅助）
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-runtime/src/lib.rs:187-202`（prepare_turn_context 顶部——插入 `await_prev_turn_critical` 守卫调用 + 新增私有 async fn）
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-db/tests/live_turn_pp_lifecycle.rs`（新建：状态机 round-trip live 测试，:54347 + SKIP）
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-runtime/tests/turn_high_water_guard.rs`（新建：守卫三分支 live 测试，:54347 + SKIP）

---

#### 子任务 2A：常量 + 排序辅助（trpg-model，纯函数 TDD）

- [ ] **Step 1** — 失败测试先行（纯单测，无 DB）。在 `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-model/src/lib.rs` 文件**末尾**追加（或追加进已有 `#[cfg(test)] mod tests`，若无则新建）：

  ```rust
  #[cfg(test)]
  mod pp_lifecycle_tests {
      use super::{pp_lifecycle_rank, PP_STREAMING, PP_CRITICAL_DONE, PP_COMPLETE};

      #[test]
      fn lifecycle_ranks_are_strictly_monotonic() {
          // streaming < critical_done < complete —— 守卫据此判断「是否已达 critical」。
          assert!(pp_lifecycle_rank(PP_STREAMING) < pp_lifecycle_rank(PP_CRITICAL_DONE));
          assert!(pp_lifecycle_rank(PP_CRITICAL_DONE) < pp_lifecycle_rank(PP_COMPLETE));
      }

      #[test]
      fn unknown_lifecycle_ranks_lowest_fail_closed() {
          // 未知/旧值（如历史 'ready' 或脏数据）排最低 = 守卫视作「未达 critical」→
          // 触发短等而非误判已落账（fail-closed：宁可多等也不读陈旧）。
          assert!(pp_lifecycle_rank("ready") < pp_lifecycle_rank(PP_STREAMING));
          assert!(pp_lifecycle_rank("") < pp_lifecycle_rank(PP_STREAMING));
          assert!(pp_lifecycle_rank("garbage") < pp_lifecycle_rank(PP_STREAMING));
      }

      #[test]
      fn const_values_are_the_canonical_strings() {
          assert_eq!(PP_STREAMING, "streaming");
          assert_eq!(PP_CRITICAL_DONE, "critical_done");
          assert_eq!(PP_COMPLETE, "complete");
      }
  }
  ```

- [ ] **Step 2** — 验证失败（符号未定义，编译错）。预期 `cannot find ... PP_STREAMING / pp_lifecycle_rank`：

  ```bash
  cargo test -p trpg-model pp_lifecycle 2>&1 | tail -20
  ```

  应见 `error[E0425]`/`error[E0432]` unresolved。

- [ ] **Step 3** — 最小实现。在 `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-model/src/lib.rs` 的 pub const 区（紧跟第 21 行 `CHARACTER_ONBOARDING_SCHEMA_VERSION` 之后）插入：

  ```rust
  // R5 postprocess 生命周期状态机（turns.pp_lifecycle 列的取值，集中常量）：
  // streaming → critical_done → complete。与 turns.postprocess_status（ready/awaiting）
  // 正交：后者是 finalize 终态，前者是回合后处理的临界/重活落账进度（高水位守卫据此）。
  pub const PP_STREAMING: &str = "streaming";
  pub const PP_CRITICAL_DONE: &str = "critical_done";
  pub const PP_COMPLETE: &str = "complete";

  /// 生命周期阶段的有序秩（守卫用：>= critical_done 即可继续，未知值排最低 fail-closed）。
  pub fn pp_lifecycle_rank(phase: &str) -> u8 {
      match phase {
          PP_STREAMING => 1,
          PP_CRITICAL_DONE => 2,
          PP_COMPLETE => 3,
          _ => 0, // 未知/旧值/空 → 最低，守卫视作未达 critical（fail-closed 多等不误读）
      }
  }
  ```

- [ ] **Step 4** — 验证通过：

  ```bash
  cargo test -p trpg-model pp_lifecycle 2>&1 | tail -15
  ```

  3 个测试 PASS。

- [ ] **Step 5** — commit：

  ```bash
  git add crates/trpg-model/src/lib.rs && \
  git commit -m "R5 T2: pp_lifecycle 状态机常量 + 有序秩辅助（fail-closed 未知排最低）

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
  ```

---

#### 子任务 2B：迁移 + db 原语（pp_lifecycle 列 / set / load_last）

- [ ] **Step 6** — 失败测试先行（live，:54347 + SKIP，参照 `live_mechanic_dues.rs` 的就地自施 + SKIP 模式）。新建 `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-db/tests/live_turn_pp_lifecycle.rs`：

  ```rust
  //! R5 T2: turns.pp_lifecycle 生命周期状态机 round-trip。
  //! Run: DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
  //!      cargo test -p trpg-db --test live_turn_pp_lifecycle -- --nocapture
  //! 无 DATABASE_URL 时 SKIP（fail-closed，不卡 CI）。
  use serde_json::json;
  use trpg_db::Db;
  use trpg_model::{PP_STREAMING, PP_CRITICAL_DONE, PP_COMPLETE};

  const SESSION: &str = "sess_r5_pp_lifecycle_test";

  /// 0028 就地自施（幂等 add column if not exists）——不跑整条迁移链（链上有非幂等老迁移）。
  async fn ensure_column(db: &Db) {
      for stmt in include_str!("../../../migrations/0028_turn_pp_lifecycle_v120.sql").split(';') {
          let s = stmt.trim();
          if s.is_empty() { continue; }
          sqlx::query(s).execute(&db.pool).await.expect("0028 statement must apply");
      }
  }

  async fn seed_session(db: &Db) {
      // turns.session_id 外键 → sessions(session_id)；先建会话再写 turn。
      db.create_session(SESSION, "call_of_cthulhu_7e", None).await.unwrap();
  }

  #[tokio::test]
  async fn pp_lifecycle_streaming_critical_done_complete_roundtrip() {
      let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
      let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect: {e}"); return; } };
      ensure_column(&db).await;
      seed_session(&db).await;
      sqlx::query("delete from turns where session_id=$1").bind(SESSION).execute(&db.pool).await.unwrap();

      // 无 turn 时 load_last 返 None（fail-closed：守卫据此立即放行）。
      assert_eq!(db.load_last_turn_pp_lifecycle(SESSION).await.unwrap(), None);

      // save_turn 写入后默认 pp_lifecycle = streaming（迁移 default 值）。
      db.save_turn(SESSION, "turn_1", "look around", "you see fog", json!({}), "ready").await.unwrap();
      assert_eq!(db.load_last_turn_pp_lifecycle(SESSION).await.unwrap().as_deref(), Some(PP_STREAMING));

      // critical 末：set → critical_done。
      db.set_turn_pp_lifecycle("turn_1", PP_CRITICAL_DONE).await.unwrap();
      assert_eq!(db.load_last_turn_pp_lifecycle(SESSION).await.unwrap().as_deref(), Some(PP_CRITICAL_DONE));

      // heavy 末：set → complete。
      db.set_turn_pp_lifecycle("turn_1", PP_COMPLETE).await.unwrap();
      assert_eq!(db.load_last_turn_pp_lifecycle(SESSION).await.unwrap().as_deref(), Some(PP_COMPLETE));

      // postprocess_status（ready）未被 pp_lifecycle 流转污染——两列正交。
      let st: (String,) = sqlx::query_as("select postprocess_status from turns where session_id=$1 and turn_id='turn_1'")
          .bind(SESSION).fetch_one(&db.pool).await.unwrap();
      assert_eq!(st.0, "ready", "pp_lifecycle 流转不得改写 postprocess_status");

      sqlx::query("delete from turns where session_id=$1").bind(SESSION).execute(&db.pool).await.unwrap();
  }

  #[tokio::test]
  async fn load_last_returns_most_recent_turn_by_created_at() {
      let url = match std::env::var("DATABASE_URL") { Ok(u) => u, Err(_) => { eprintln!("SKIP: DATABASE_URL unset"); return; } };
      let db = match Db::connect(&url).await { Ok(d) => d, Err(e) => { eprintln!("SKIP: connect: {e}"); return; } };
      ensure_column(&db).await;
      seed_session(&db).await;
      sqlx::query("delete from turns where session_id=$1").bind(SESSION).execute(&db.pool).await.unwrap();

      db.save_turn(SESSION, "turn_a", "i", "o", json!({}), "ready").await.unwrap();
      db.set_turn_pp_lifecycle("turn_a", PP_COMPLETE).await.unwrap();
      // turn_b 后写 → created_at 更新 → load_last 必取 turn_b（仍 streaming）。
      db.save_turn(SESSION, "turn_b", "i2", "o2", json!({}), "ready").await.unwrap();
      assert_eq!(db.load_last_turn_pp_lifecycle(SESSION).await.unwrap().as_deref(), Some(PP_STREAMING),
          "load_last 必返最近一回合（turn_b）的 lifecycle，而非任意 turn");

      sqlx::query("delete from turns where session_id=$1").bind(SESSION).execute(&db.pool).await.unwrap();
  }
  ```

- [ ] **Step 7** — 验证失败（迁移文件缺失 + 原语未定义）。预期 `include_str!` 找不到 0028、`no method named set_turn_pp_lifecycle / load_last_turn_pp_lifecycle`：

  ```bash
  cargo test -p trpg-db --test live_turn_pp_lifecycle 2>&1 | tail -20
  ```

  应见编译错（缺文件 / 缺方法）。

- [ ] **Step 8** — 写迁移 SQL。新建 `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/migrations/0028_turn_pp_lifecycle_v120.sql`（幂等 add-column 模式，对齐 0026）：

  ```sql
  -- migrations/0028_turn_pp_lifecycle_v120.sql
  -- R5 postprocess 生命周期：streaming → critical_done → complete。
  -- 独立于 turns.postprocess_status（ready/awaiting/draft，finalize 终态保持原义不变），
  -- 本列专做高水位守卫的后处理落账进度追踪。默认 streaming（save_turn 时即此态）。
  alter table turns add column if not exists pp_lifecycle text not null default 'streaming';
  create index if not exists idx_turns_session_created_at on turns (session_id, created_at desc);
  ```

- [ ] **Step 9** — 在 migrate() 数组手动追加（记忆：include_str! 数组需手动加行）。在 `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-db/src/lib.rs` 第 51 行 `0027_mechanic_dues_v120.sql` 之后插入：

  ```rust
              include_str!("../../../migrations/0028_turn_pp_lifecycle_v120.sql"),
  ```

- [ ] **Step 10** — 实现两 db 原语。在 `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-db/src/lib.rs` 的 `save_turn`（结束于第 1010 行 `}`）之后插入：

  ```rust
      /// R5：流转某回合的 postprocess 生命周期阶段（critical 末写 critical_done、heavy 末写 complete）。
      /// 只触 pp_lifecycle 列，绝不动 postprocess_status（两列正交）。
      pub async fn set_turn_pp_lifecycle(&self, turn_id: &str, phase: &str) -> Result<()> {
          sqlx::query("update turns set pp_lifecycle = $2, updated_at = now() where turn_id = $1")
              .bind(turn_id)
              .bind(phase)
              .execute(&self.pool)
              .await?;
          Ok(())
      }

      /// R5：取该会话最近一回合（created_at desc）的 pp_lifecycle；无回合返 None（守卫据此立即放行）。
      pub async fn load_last_turn_pp_lifecycle(&self, session_id: &str) -> Result<Option<String>> {
          let row: Option<(String,)> = sqlx::query_as(
              "select pp_lifecycle from turns where session_id = $1 order by created_at desc limit 1",
          )
          .bind(session_id)
          .fetch_optional(&self.pool)
          .await?;
          Ok(row.map(|r| r.0))
      }
  ```

  > 注：`set_turn_pp_lifecycle` 按 `turn_id`（非 `session_id+turn_id`）更新——`turns.turn_id` 在单会话内唯一（`unique(session_id, turn_id)`），但跨会话可能重名；T1 调用处恒持本回合 turn_id 且同会话流转，实际无歧义。若 drafter 后续发现跨会话碰撞风险，升级签名为 `(session_id, turn_id, phase)` 并同步 T1 调用点（此为 T1/T2 接缝，T2 提供原语、T1 持有 session_id）。

- [ ] **Step 11** — 验证通过（需 :54347 起 CoC 库；无则 SKIP）：

  ```bash
  DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
    cargo test -p trpg-db --test live_turn_pp_lifecycle -- --nocapture 2>&1 | tail -20
  # 期望：2 passed（或 "SKIP: DATABASE_URL unset" 若库未起）
  ```

  同时确认全库编译不回归：`cargo build -p trpg-db 2>&1 | tail -5`。

- [ ] **Step 12** — commit：

  ```bash
  git add migrations/0028_turn_pp_lifecycle_v120.sql crates/trpg-db/src/lib.rs crates/trpg-db/tests/live_turn_pp_lifecycle.rs && \
  git commit -m "R5 T2: turns.pp_lifecycle 列 + set/load_last db 原语（与 postprocess_status 正交）

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
  ```

---

#### 子任务 2C：高水位守卫 await_prev_turn_critical（接 prepare_turn_context 顶部）

守卫语义（spec §4.2，fail-closed 放行不硬拒）：取上一回合 `pp_lifecycle`，
- `None`（无上一回合）→ 立即放行；
- `rank >= critical_done`（含 `complete`）→ 立即放行（heavy 未完不阻塞）；
- `rank < critical_done`（`streaming` 或未知/旧值）→ 每 `interval_ms=100` 轮询，直到达 `critical_done` 或累计 `timeout_ms=2000` 超时（`warn` 放行）。

> ⚠️ 测试避坑：守卫"等待→达到→继续"分支若靠真并发回合驱动会非确定性。采用**种 turn 行直接设 pp_lifecycle + 后台 spawn 延时翻列**的确定性手段（mock/seeded），不依赖真 execute_turn。"超时放行"分支种一行恒 `streaming`、断言守卫在 ~timeout 后返回 Ok 且耗时 ≥timeout（用墙钟 elapsed 断言 bounded）。

- [ ] **Step 13** — 失败测试先行。新建 `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-runtime/tests/turn_high_water_guard.rs`（:54347 + SKIP，对齐 runtime live 测试用 `TRPG_TEST_DATABASE_URL`）：

  ```rust
  //! R5 T2: turn 高水位守卫三分支（fail-closed 放行，从不硬拒玩家）。
  //! Run: TRPG_TEST_DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
  //!      cargo test -p trpg-runtime --test turn_high_water_guard -- --nocapture
  //! 无 TRPG_TEST_DATABASE_URL 时 SKIP（fail-closed，不卡 CI）。
  use std::time::Instant;
  use serde_json::json;
  use trpg_db::Db;
  use trpg_model::{PP_STREAMING, PP_CRITICAL_DONE};
  use trpg_runtime::await_prev_turn_critical;

  const SESSION: &str = "sess_r5_high_water_test";

  async fn setup(url: &str) -> Db {
      let db = Db::connect(url).await.expect("connect");
      for stmt in include_str!("../../../migrations/0028_turn_pp_lifecycle_v120.sql").split(';') {
          let s = stmt.trim();
          if !s.is_empty() { sqlx::query(s).execute(&db.pool).await.expect("0028"); }
      }
      db.create_session(SESSION, "call_of_cthulhu_7e", None).await.unwrap();
      sqlx::query("delete from turns where session_id=$1").bind(SESSION).execute(&db.pool).await.unwrap();
      db
  }

  fn skip() -> Option<String> {
      match std::env::var("TRPG_TEST_DATABASE_URL") {
          Ok(u) => Some(u),
          Err(_) => { eprintln!("SKIP: set TRPG_TEST_DATABASE_URL to CoC DB on :54347"); None }
      }
  }

  #[tokio::test]
  async fn no_prev_turn_returns_immediately() {
      let Some(url) = skip() else { return; };
      let db = setup(&url).await;
      let t = Instant::now();
      await_prev_turn_critical(&db, SESSION, 2000, 100).await; // 无 turn → 立即放行
      assert!(t.elapsed().as_millis() < 200, "无上一回合必须立即放行");
  }

  #[tokio::test]
  async fn critical_already_reached_returns_immediately() {
      let Some(url) = skip() else { return; };
      let db = setup(&url).await;
      db.save_turn(SESSION, "turn_1", "i", "o", json!({}), "ready").await.unwrap();
      db.set_turn_pp_lifecycle("turn_1", PP_CRITICAL_DONE).await.unwrap();
      let t = Instant::now();
      await_prev_turn_critical(&db, SESSION, 2000, 100).await; // >= critical → 立即
      assert!(t.elapsed().as_millis() < 200, "critical_done 必须立即放行");
  }

  #[tokio::test]
  async fn waits_then_proceeds_when_critical_arrives_late() {
      let Some(url) = skip() else { return; };
      let db = setup(&url).await;
      db.save_turn(SESSION, "turn_1", "i", "o", json!({}), "ready").await.unwrap(); // streaming
      // 后台 ~300ms 后翻 critical_done（模拟前一回合 critical 组延迟落账）。
      let db2 = db.clone();
      tokio::spawn(async move {
          tokio::time::sleep(std::time::Duration::from_millis(300)).await;
          let _ = db2.set_turn_pp_lifecycle("turn_1", PP_CRITICAL_DONE).await;
      });
      let t = Instant::now();
      await_prev_turn_critical(&db, SESSION, 2000, 100).await;
      let ms = t.elapsed().as_millis();
      assert!(ms >= 250, "应等到 critical 到达（≥~300ms），实测 {ms}ms");
      assert!(ms < 1500, "到达后必须立刻继续，远早于 2000ms 超时，实测 {ms}ms");
  }

  #[tokio::test]
  async fn timeout_proceeds_fail_closed_when_critical_never_arrives() {
      let Some(url) = skip() else { return; };
      let db = setup(&url).await;
      db.save_turn(SESSION, "turn_1", "i", "o", json!({}), "ready").await.unwrap(); // 恒 streaming
      let t = Instant::now();
      await_prev_turn_critical(&db, SESSION, 600, 100).await; // 永不到达 → 超时放行
      let ms = t.elapsed().as_millis();
      assert!(ms >= 600, "超时前必须 bounded 等满 timeout，实测 {ms}ms");
      assert!(ms < 1200, "超时后必须 fail-closed 放行（不死等），实测 {ms}ms");
  }
  ```

- [ ] **Step 14** — 验证失败（守卫未导出）。预期 `unresolved import trpg_runtime::await_prev_turn_critical`：

  ```bash
  cargo test -p trpg-runtime --test turn_high_water_guard 2>&1 | tail -15
  ```

- [ ] **Step 15** — 实现守卫（独立 pub async fn，便于测试直调）。在 `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-runtime/src/lib.rs` 顶层（`impl RuntimeEngine` 块**之外**，与其它 free fn 同级）新增。先确认 model 常量已 use：文件已有 `use trpg_model::*;`（若无则补 `use trpg_model::{pp_lifecycle_rank, PP_CRITICAL_DONE};`）。

  ```rust
  /// R5 turn 高水位守卫：回合入口处等上一回合的 critical 组落账（pp_lifecycle >= critical_done）。
  /// fail-closed——从不硬拒玩家：无上一回合 / 已达 critical / heavy 未完（critical_done 非 complete）
  /// 均立即放行；仅当上一回合仍 < critical_done 时 bounded 轮询，超时 warn 放行。
  pub async fn await_prev_turn_critical(db: &Db, session_id: &str, timeout_ms: u64, interval_ms: u64) {
      let reached = |phase: &Option<String>| -> bool {
          match phase {
              None => true, // 无上一回合 → 放行
              Some(p) => pp_lifecycle_rank(p) >= pp_lifecycle_rank(PP_CRITICAL_DONE),
          }
      };
      // 首查：常见路径（critical 已落账 / 无上一回合）零等待。
      match db.load_last_turn_pp_lifecycle(session_id).await {
          Ok(ref phase) if reached(phase) => return,
          Err(err) => { tracing::warn!(error = %err, session_id, "high-water guard load failed; proceeding (fail-closed)"); return; }
          _ => {}
      }
      let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
      let step = std::time::Duration::from_millis(interval_ms);
      loop {
          if tokio::time::Instant::now() >= deadline {
              tracing::warn!(session_id, timeout_ms, "high-water guard timed out waiting for prev-turn critical; proceeding (fail-closed, possibly slightly stale)");
              return;
          }
          tokio::time::sleep(step).await;
          match db.load_last_turn_pp_lifecycle(session_id).await {
              Ok(ref phase) if reached(phase) => return,
              Err(err) => { tracing::warn!(error = %err, session_id, "high-water guard re-poll failed; proceeding (fail-closed)"); return; }
              _ => {}
          }
      }
  }
  ```

- [ ] **Step 16** — 接入回合入口。在 `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-runtime/src/lib.rs` 的 `prepare_turn_context` 函数体**最顶部**（现第 194-195 行注释/`state.clone()` 之前）插入：

  ```rust
          // R5 turn 高水位守卫：等上一回合 critical 组落账，消除连发回合读到陈旧
          // current_scene_id/记忆的竞态。fail-closed——超时/无上一回合/已达均放行，不卡玩家。
          await_prev_turn_critical(&self.db, &request.session_id, 2000, 100).await;
  ```

  > 注：守卫看的是「最近一回合」的 lifecycle。本回合的 turn 行尚未 `save_turn`（save 发生在 finalize，回合尾），故 `load_last_turn_pp_lifecycle` 此刻返回的恰是**上一回合**，无自指。timeout/interval 此处硬写 `2000/100`——非规则集硬编码（是引擎级时序常量，spec §4.2 钦定）；若 drafter 倾向集中，可提为 `RuntimeEngine` 常量或 env 覆盖，但不强制。

- [ ] **Step 17** — 验证通过：

  ```bash
  cargo build -p trpg-runtime 2>&1 | tail -5 && \
  TRPG_TEST_DATABASE_URL=postgres://chatrpg:chatrpg@127.0.0.1:54347/chatrpg \
    cargo test -p trpg-runtime --test turn_high_water_guard -- --nocapture 2>&1 | tail -25
  # 期望：4 passed（或全 SKIP 若库未起）。重点看 waits_then_proceeds / timeout 两条墙钟断言。
  ```

- [ ] **Step 18** — 回归确认（守卫接入未破现有 runtime 测试）：

  ```bash
  cargo test -p trpg-runtime 2>&1 | tail -15
  # 期望：原有单测全绿；live 测试缺库时 SKIP。
  ```

- [ ] **Step 19** — commit：

  ```bash
  git add crates/trpg-runtime/src/lib.rs crates/trpg-runtime/tests/turn_high_water_guard.rs && \
  git commit -m "R5 T2: await_prev_turn_critical 高水位守卫（bounded 轮询/fail-closed 放行）接 prepare_turn_context 顶部

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
  ```

---

**Task 2 接缝交付给 T1（execute.rs 分层）**：
- 常量 `trpg_model::{PP_STREAMING, PP_CRITICAL_DONE, PP_COMPLETE}` + `pp_lifecycle_rank`。
- db 原语 `Db::set_turn_pp_lifecycle(turn_id, phase)`（critical 组末调 `PP_CRITICAL_DONE`、heavy 组末调 `PP_COMPLETE`）+ `Db::load_last_turn_pp_lifecycle(session_id)`。
- `turns.pp_lifecycle` 列默认 `streaming`（`save_turn` 即此态，无需 T1 显式写 streaming）；`postprocess_status` 现义（`ready`/`awaiting_player_roll`）保持不变，T1 的 `phase_finalize` 仍照常写它。
- 守卫已在 `prepare_turn_context` 顶部接好，T1 无需再接；T1 只需在 critical/heavy 两组末尾各调一次 `set_turn_pp_lifecycle`。

**未决留给 drafter/T1 协调**：`set_turn_pp_lifecycle` 当前按 `turn_id` 单键更新（见 Step 10 注）；若 T1 spawn 的 heavy 任务跨会话复用风险出现，升级为 `(session_id, turn_id, phase)` 三参签名。


---

### Task 3: 两 transport 时序接线（API SSE 结束于 TurnComplete / CLI turn 等 heavy、play 不等）

**Files:**

- `crates/trpg-api/src/lib.rs:949-1057` — `play_turn_sse`，当前 drain 全部事件含 TurnComplete
- `crates/trpg-cli/src/main.rs:789-904` — `turn_cli`，当前 drain 全部事件到底
- `crates/trpg-cli/src/agent_play.rs:28-143` — `play_cli_agent`，交互循环每回合 drain
- `crates/trpg-gm/src/turn_event.rs:9-22` — `TurnEvent` 枚举（新增 `HeavyPostprocessDone` variant）
- `crates/trpg-db/src/lib.rs:987-1006` — `save_turn` / `load_last_turn_pp_lifecycle`（Task 2 加入）
- `crates/trpg-cli/src/transport_policy.rs`（新建）— `WaitMode` 判定函数 + 单测

---

**前提**：Task 1（execute.rs critical/heavy 拆分 + `TurnEvent::HeavyPostprocessDone`）和 Task 2（`pp_lifecycle` 状态机 + `load_last_turn_pp_lifecycle` db 原语）已合入同一分支。Task 3 在其之上接线。

---

- [ ] **Step 1：新建 `crates/trpg-cli/src/transport_policy.rs`，定义 `WaitMode` 枚举 + 纯函数 + 单测**

  目标：将「turn 模式等 heavy、play 模式不等」的判定抽成一个**零 IO 纯函数**，便于单测与跨 transport 复用。

  ```rust
  // crates/trpg-cli/src/transport_policy.rs
  
  /// CLI transport 决策：一次性 turn 需等 heavy 落账，交互 play 不等（由下一轮守卫兜）。
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum WaitMode {
      /// 等 heavy 完成（`pp_lifecycle = complete`）再退出进程。
      WaitHeavy,
      /// 不等：TurnComplete 后即可开始下一回合（high-water guard 兜）。
      NoWait,
  }
  
  /// 根据 CLI 使用场景选择 `WaitMode`。
  /// `is_one_shot` = true  → `trpg turn`（脚本/测试，需确定性落账）。
  /// `is_one_shot` = false → `trpg play`（交互循环，下一轮守卫兜）。
  pub fn cli_wait_mode(is_one_shot: bool) -> WaitMode {
      if is_one_shot { WaitMode::WaitHeavy } else { WaitMode::NoWait }
  }
  
  #[cfg(test)]
  mod tests {
      use super::*;
  
      #[test]
      fn one_shot_turn_waits_heavy() {
          assert_eq!(cli_wait_mode(true), WaitMode::WaitHeavy);
      }
  
      #[test]
      fn interactive_play_no_wait() {
          assert_eq!(cli_wait_mode(false), WaitMode::NoWait);
      }
  
      #[test]
      fn wait_heavy_ne_no_wait() {
          assert_ne!(WaitMode::WaitHeavy, WaitMode::NoWait);
      }
  }
  ```

  验收：
  ```
  cargo test -p trpg-cli transport_policy
  ```
  三个单测全绿，无编译错误。

---

- [ ] **Step 2：在 `TurnEvent` 中新增 `HeavyPostprocessDone` variant（Task 1 依赖，若未加则在此补）**

  检查 `crates/trpg-gm/src/turn_event.rs`，若 `HeavyPostprocessDone` 尚不存在，添加：

  ```rust
  // turn_event.rs，在 TurnComplete 前插入
  /// heavy 组后台任务完成信号（CLI turn 模式等此事件再退出；API/play 不等）。
  HeavyPostprocessDone,
  ```

  同步更新 `turn_event.rs` 的测试（`all_variants_construct_and_clone_and_debug`）加入该 variant：
  ```rust
  TurnEvent::HeavyPostprocessDone,
  ```

  验收：
  ```
  cargo test -p trpg-gm turn_event
  ```
  全绿。

---

- [ ] **Step 3：`turn_cli` 接线——drain 到 `TurnComplete` 后轮询 `pp_lifecycle = complete` 再退**

  **目标**：`trpg turn`（一次性）在 `TurnComplete` 之后，轮询 DB `load_last_turn_pp_lifecycle(session_id)`，直到返回 `Some("complete")` 或超时（上限 30s，间隔 500ms），再退出进程。

  修改 `crates/trpg-cli/src/main.rs`，`turn_cli` 函数（当前行 789-904）：

  在文件顶部 `mod agent_play;` 之后加：
  ```rust
  mod transport_policy;
  ```

  在 `use` 块补：
  ```rust
  use transport_policy::{cli_wait_mode, WaitMode};
  ```

  在 `turn_cli` drain 循环结尾（当前约 880-904 行，drain 完毕后紧接 `Ok(())`）前插入：

  ```rust
  // Task 3：一次性 turn 等 heavy 落账（确定性退出），轮询 pp_lifecycle=complete。
  if cli_wait_mode(true) == WaitMode::WaitHeavy {
      await_heavy_complete(&db, &session_id).await;
  }
  ```

  在文件底部（`emit_sse` 之后）新增辅助函数：

  ```rust
  /// 等待 session 最近 turn 的 pp_lifecycle 到达 "complete"，上限 30s，间隔 500ms。
  /// fail-closed 超时放行（warn），绝不阻死进程。仅 `trpg turn` 一次性模式调用。
  async fn await_heavy_complete(db: &Db, session_id: &str) {
      use std::time::{Duration, Instant};
      const TIMEOUT: Duration = Duration::from_secs(30);
      const INTERVAL: Duration = Duration::from_millis(500);
      const COMPLETE: &str = trpg_db::PP_COMPLETE; // Task 2 加入的常量
      let start = Instant::now();
      loop {
          match db.load_last_turn_pp_lifecycle(session_id).await {
              Ok(Some(ref s)) if s == COMPLETE => return,
              Ok(_) => {}
              Err(err) => {
                  tracing::warn!(error = %err, "await_heavy_complete: db error; giving up");
                  return;
              }
          }
          if start.elapsed() >= TIMEOUT {
              tracing::warn!(
                  session_id,
                  "await_heavy_complete: timeout ({}s) waiting for pp_lifecycle=complete; proceeding",
                  TIMEOUT.as_secs()
              );
              return;
          }
          tokio::time::sleep(INTERVAL).await;
      }
  }
  ```

  验收：`cargo build -p trpg-cli` 编译通过（DB 单测标注 `:54347 + SKIP`，此处无 DB 单测）。

---

- [ ] **Step 4：`play_cli_agent` 接线——drain 到 `TurnComplete` 即开始下一回合（不等 heavy）**

  `crates/trpg-cli/src/agent_play.rs` 中，`play_cli_agent` 当前的 drain 循环（行 97-140）已经是同步 drain 到底（`stream.next()` 直到 `None`）。

  **需要做的修改**：在 `HeavyPostprocessDone` 事件到达时，直接 `break` 跳出 drain（不等剩余事件），以便立刻开始下一轮。但更安全的做法——因为 `TurnComplete` 已在 Task 1 中先于 `HeavyPostprocessDone` 到达——是在 `TurnComplete` 处理完后 `break`。

  在 `agent_play.rs` 的 `while let Some(event) = stream.next().await` 循环中，修改 `TurnComplete` 分支为：

  ```rust
  TurnEvent::TurnComplete { outcome } => {
      println!();
      match outcome {
          TurnOutcome::Narration(text) => {
              history.push(ChatMessage { role: "user".to_string(), content: input.clone() });
              history.push(ChatMessage { role: "assistant".to_string(), content: text.clone() });
              let tail = format!("\nPlayer: {input}\nGM: {text}\n");
              let r = recent.get_or_insert_with(String::new);
              r.push_str(&tail);
              *r = take_tail_chars(r, 12_000);
          }
          TurnOutcome::AwaitingPlayerRoll { prompt_public, .. } => {
              history.push(ChatMessage { role: "user".to_string(), content: input.clone() });
              history.push(ChatMessage {
                  role: "assistant".to_string(),
                  content: if streamed.trim().is_empty() { prompt_public } else { streamed.clone() },
              });
          }
      }
      // play 模式：TurnComplete 后不等 heavy（下一轮高水位守卫兜）。
      break;
  }
  TurnEvent::HeavyPostprocessDone => {
      // play 模式：heavy 后台完成信号，已 break（上面处理过 TurnComplete 后 break）。
      // 若因竞态先到此，同样忽略继续。
  }
  ```

  在 `use trpg_gm::{...}` 导入中补 `TurnEvent::HeavyPostprocessDone`（variant 无需单独导入，match 即可）。

  验收：`cargo build -p trpg-cli` 通过。

---

- [ ] **Step 5：`play_turn_sse` 接线——drain 到 `TurnComplete` 即结束 SSE，heavy 自走**

  `crates/trpg-api/src/lib.rs`，`play_turn_sse` 的 spawn 内 drain 循环（行 1026-1054）。

  当前 `TurnComplete` 分支发 `done` phase event 后**继续循环**（`while let` 继续等 `stream.next()`，直到 `None`）。Task 1 之后，`HeavyPostprocessDone` 会在 `TurnComplete` 之后到来——SSE handler 应在 `TurnComplete` 后关闭 `tx`（或 break），不挂等 heavy。

  修改 `TurnComplete` 分支为：

  ```rust
  TurnEvent::TurnComplete { outcome } => {
      // critical 已落账，发 done 关闭 SSE（heavy 自走与 SSE 生命周期解耦）。
      if let TurnOutcome::Narration(_) = outcome {
          send_phase(&tx, "done", json!({})).await;
      }
      break; // SSE 结束于 TurnComplete；heavy 在 execute_turn 的后台 spawn 继续。
  }
  ```

  在 `HeavyPostprocessDone` 分支（若 Task 1 已加入 `TurnEvent::HeavyPostprocessDone` variant，此处需 exhaustive match）：

  ```rust
  TurnEvent::HeavyPostprocessDone => {
      // API SSE：已在 TurnComplete 处 break，此分支不可达；保留以满足 exhaustive match。
  }
  ```

  验收：`cargo build -p trpg-api` 通过。

---

- [ ] **Step 6：`turn_cli` drain 循环同步添加 `HeavyPostprocessDone` arm（exhaustive match）**

  `crates/trpg-cli/src/main.rs`，`turn_cli` drain 循环（行 879-903）的 `match event` 加：

  ```rust
  TurnEvent::HeavyPostprocessDone => {
      // turn 模式：通过 await_heavy_complete 轮询 DB 等 complete，此事件可选提前退出。
      // 这里两种选择等价：轮询已超快收敛；收到此事件也可直接 break 加速。
      // 保守做法：不 break（轮询负责等 complete，此事件仅日志），保单一等待路径。
      emit_phase(args.stream_format, "heavy_done", json!({}))?;
  }
  ```

  验收：`cargo build -p trpg-cli` 通过。

---

- [ ] **Step 7：`transport_policy` 单测扩展——验证判定函数覆盖所有决策路径**

  在 `transport_policy.rs` 的测试模块中补充文档测试：

  ```rust
  #[test]
  fn wait_mode_debug_str_non_empty() {
      // WaitMode 实现 Debug，用于 tracing 日志。
      assert!(!format!("{:?}", WaitMode::WaitHeavy).is_empty());
      assert!(!format!("{:?}", WaitMode::NoWait).is_empty());
  }
  
  #[test]
  fn cli_wait_mode_is_pure_deterministic() {
      // 同参数反复调用返回同值（纯函数）。
      for _ in 0..5 {
          assert_eq!(cli_wait_mode(true), WaitMode::WaitHeavy);
          assert_eq!(cli_wait_mode(false), WaitMode::NoWait);
      }
  }
  ```

  验收：
  ```
  cargo test -p trpg-cli transport_policy
  ```
  全 5 个单测绿。

---

- [ ] **Step 8：编译全 workspace 验收 + 文件行数检查**

  ```
  cargo build --manifest-path /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/Cargo.toml
  ```

  检查新文件行数：
  ```
  wc -l crates/trpg-cli/src/transport_policy.rs
  ```
  须 ≤ 400 行。

  `crates/trpg-api/src/lib.rs`、`crates/trpg-cli/src/main.rs`、`crates/trpg-cli/src/agent_play.rs` 各自行数变化：`lib.rs` 加约 3 行（`break` + `HeavyPostprocessDone` arm）、`main.rs` 加约 30 行（`await_heavy_complete` + `mod transport_policy` + 调用点）、`agent_play.rs` 加约 5 行（`break` + `HeavyPostprocessDone` arm）——均在 400 行安全范围内。

---

**时序验收摘要（无 DB 依赖的可测部分）：**

| 场景 | 实现 | 单测覆盖 |
|---|---|---|
| `trpg turn`（one-shot）等 heavy | `await_heavy_complete` 轮询 `pp_lifecycle=complete`，上限 30s | `cli_wait_mode(true) == WaitHeavy`（transport_policy 单测） |
| `trpg play`（交互）不等 heavy | `TurnComplete` 处 `break`，heavy 自走 | `cli_wait_mode(false) == NoWait`（transport_policy 单测） |
| API SSE 结束于 TurnComplete | `TurnComplete` 分支 `break`，`tx` drop → SSE 关闭 | exhaustive match 编译证明 |
| heavy 失败不影响 SSE/CLI | heavy spawn 独立任务（Task 1 保证），warn 只写 lifecycle | 由 Task 1 单测覆盖（heavy panic 不影响已发事件） |

**DB 依赖测试（需 `:54347`，标 `#[ignore]`）：**

完整 e2e（连发两回合验证无陈旧 + TurnComplete 在叙事后快速到达 + heavy 最终落账 `status=complete`）归入 Task 4 验证闸，本 Task 3 不直接运行 DB 测试。


---

### Task 4: R5 验证闸（连发回合无陈旧 + heavy 不挂流 + 零回归 + 缓存稳定）

**Files:**
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/execute.rs` — pipeline 分层实现（Tasks 1-3 落地处）
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-db/src/lib.rs` — `set_turn_pp_lifecycle` / `load_last_turn_pp_lifecycle` / `save_turn` 签名（Tasks 2 落地处）
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/turn_loop.rs:568-617` — `finalize_turn` / `phase_finalize` / `phase_scene_navigate`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-cli/src/main.rs:789-904` — `turn_cli` drain 逻辑
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-api/src/lib.rs:949-1057` — `play_turn_sse` handler
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-runtime/src/scene_navigation.rs:183-231` — `scene_navigator`（含 `set_session_scene` + deep-extract + frontier）
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/migrations/` — 最新迁移为 `0027_mechanic_dues_v120.sql`；R5 pp_lifecycle 迁移编号为 `0028_*`

**环境约定（全 Task 4 共享）**

```bash
# DB：CoC 在 :54347（.env 默认 :54323，须显式 swap）
export DATABASE_URL="postgres://chatrpg:chatrpg@localhost:54347/chatrpg"

# LLM：本地 codex-relay（OpenAI-compatible，拒 temperature）
export TRPG_LLM_PROVIDER=openai
export TRPG_LLM_BASE_URL="http://localhost:18888/v1"
export TRPG_LLM_API_KEY=relay
export TRPG_LLM_MODEL=gpt-5.4           # 或 gpt-5.5；relay 升级后按实际可用
export TRPG_LLM_SEND_TEMPERATURE=false

# Data dir：CoC 规则/模组 units 所在
export TRPG_DATA_DIR="/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/data"

# 模组开关（scene_navigator 需要 ModuleGraph）
export TRPG_MODULE_READER=1

# 单测 SKIP 守卫（含 :54347 lazy pool 的 execute_turn_streams_deltas_and_completes 等）
# 真库步骤须手动移除此变量或确认库在线
export SKIP_DB_TESTS=1   # 纯单测阶段设；live 步骤时 unset
```

---

- [ ] **Step 1: 前置确认——Tasks 1-3 均已落地、全工作区 check 零错误**

  在开始验证闸前，确认 R5 前三个 Task 的代码已提交或存于工作区。检查编译状态：

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula

  cargo check --workspace 2>&1 | grep -E "^error\[|^warning:.*unused import|Finished"
  ```

  预期：末行输出 `Finished`，无 `error[E...]`。若有编译错误，验证闸在此挡住——必须先修复 Tasks 1-3，不得继续。

  附加：确认 R5 关键符号已存在（Tasks 1/2 产物）：

  ```bash
  # PP 生命周期常量（trpg-model 或 trpg-db）
  grep -rn "PP_STREAMING\|PP_CRITICAL_DONE\|PP_COMPLETE" \
    crates/trpg-model/src/ crates/trpg-db/src/ 2>/dev/null | head -5
  # 预期：至少 3 行常量定义

  # db 原语
  grep -n "set_turn_pp_lifecycle\|load_last_turn_pp_lifecycle" crates/trpg-db/src/lib.rs
  # 预期：各至少 1 行函数定义

  # 高水位守卫入口
  grep -n "await_prev_turn_critical" crates/trpg-db/src/lib.rs crates/trpg-gm/src/execute.rs 2>/dev/null
  # 预期：至少 1 行定义
  ```

  若上述 grep 无输出，说明对应 Task 未完成，验证闸在此挡住。记录缺失 Task 编号后停止。

---

- [ ] **Step 2: 全工作区单测零回归（SKIP_DB_TESTS，不含 live DB）**

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula

  SKIP_DB_TESTS=1 cargo test --workspace --lib --bins 2>&1 \
    | grep -E "^test result:|FAILED|error\[" | tail -40
  ```

  预期：每个 crate 末行均为 `test result: ok`，无 `FAILED`，无编译 `error[E...]`。

  如有失败，triage：若为 R5 前既存 flaky test（与 execute.rs/db lifecycle 无关），记录为 pre-existing 允许继续；若为 R5 引入回归（涉及 `select_phases` / `PP_*` 常量 / `await_prev_turn_critical`），**验证闸在此挡住**，不得继续。

  单独跑 trpg-gm（含 execute_tests）：

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
  SKIP_DB_TESTS=1 cargo test -p trpg-gm --lib 2>&1 | tail -10
  ```

  预期：`test result: ok. N passed; 0 failed`，N 须 >= 原有测试数（Step 2 验收前先 `grep -c "#\[test\]" crates/trpg-gm/src/execute_tests.rs` 数一下原有数量，确认 R5 新增单测也在其中）。

---

- [ ] **Step 3: R5 专项单测显式验证（TDD 红绿证明）**

  验证 Tasks 1-3 新增的三类 R5 专项单测全部绿（参照 Tasks 1-3 TDD 步骤产生的测试）。直接运行并用 `--nocapture` 看详细输出：

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula

  # R5 分层单测（Task 1）：critical 在 TurnComplete 前、heavy 在其后、heavy 失败隔离
  SKIP_DB_TESTS=1 cargo test -p trpg-gm --lib \
    r5_critical_before_turn_complete \
    r5_heavy_after_turn_complete \
    r5_heavy_failure_does_not_block_critical \
    -- --nocapture 2>&1 | tail -20
  # 预期：3 个测试全 ok

  # R5 高水位守卫单测（Task 2）：短等/超时放行/heavy 未完放行
  SKIP_DB_TESTS=1 cargo test -p trpg-gm --lib \
    r5_guard_waits_until_critical_done \
    r5_guard_timeout_failopen \
    r5_guard_heavy_not_complete_passthrough \
    -- --nocapture 2>&1 | tail -20
  # 预期：3 个测试全 ok

  # R5 状态机单测（Task 2）：streaming→critical_done→complete 状态转移
  SKIP_DB_TESTS=1 cargo test -p trpg-gm --lib \
    r5_pp_lifecycle_transitions \
    -- --nocapture 2>&1 | tail -10
  # 预期：ok
  ```

  若 Task 1-3 命名与上方不一致，用实际测试函数名替换（运行 `grep -rn "#\[test\]" crates/trpg-gm/src/ | grep r5` 确认）。

---

- [ ] **Step 4: 构建 release 候选 + 确认 trpg binary 可运行**

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula

  # debug build 足够（不必 release，节省时间）
  cargo build -p trpg-cli 2>&1 | grep -E "^error\[|Finished"
  # 预期：Finished

  # smoke：版本输出不 panic
  ./target/debug/trpg --version
  # 预期：输出 trpg x.y.z
  ```

---

- [ ] **Step 5: DB 迁移验证——pp_lifecycle 列存在于 :54347**

  ```bash
  # 确认 DB 在线
  docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg \
    -c "SELECT 1;" 2>&1
  # 预期：1 行结果

  # 运行迁移（migrate 命令幂等）
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
  DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
    ./target/debug/trpg migrate 2>&1
  # 预期：无 error；若报"migration statement failed"则检查 0028_*.sql 语法

  # 验证 pp_lifecycle 列存在
  docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg \
    -c "\d turns" 2>&1 | grep -E "pp_lifecycle|postprocess_status"
  # 预期：pp_lifecycle text 列存在；postprocess_status 列保持原义（ready/awaiting_player_roll）
  ```

  SKIP 条件：若 Docker 容器不在线，此步 SKIP（标注），但真库 live 步骤（Steps 6-10）也将全部 SKIP——需先起容器。

---

- [ ] **Step 6: CoC CLI 连发两回合——诱导场景切换 + 断言无陈旧竞态**

  这是验证闸核心步骤。先创建新 session，turn N 提供场景切换意图，立即跟发 turn N+1，断言 N+1 读到 N 切换后的 `current_scene_id`。

  **Step 6a: 创建新 CoC session（模组在场）**

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula

  SESSION_OUT=$(DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
    TRPG_LLM_PROVIDER=openai \
    TRPG_LLM_BASE_URL=http://localhost:18888/v1 \
    TRPG_LLM_API_KEY=relay \
    TRPG_LLM_MODEL=gpt-5.4 \
    TRPG_LLM_SEND_TEMPERATURE=false \
    TRPG_DATA_DIR=/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/data \
    TRPG_MODULE_READER=1 \
    ./target/debug/trpg turn \
      --ruleset call_of_cthulhu_7e \
      --module call_of_cthulhu_7e.document \
      --input "我们抵达小镇，下车后往加油站走去，打算先了解一下情况。" \
      --stream-format jsonl 2>/tmp/r5_turn1_stderr.log \
    | tee /tmp/r5_turn1.jsonl)

  # 从 jsonl 中取 session_id
  SESSION=$(grep '"phase":"session"' /tmp/r5_turn1.jsonl \
    | python3 -c "import sys,json; [print(json.loads(l)['data']['session_id']) for l in sys.stdin if 'session_id' in l]" \
    | head -1)
  echo "SESSION=$SESSION"
  # 预期：形如 "sess_xxxxxxxxxxxxxxxx"
  ```

  **Step 6b: 记录 turn N 之前的 current_scene_id（基准）**

  ```bash
  SCENE_BEFORE=$(docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg -t \
    -c "SELECT current_scene_id FROM sessions WHERE session_id='${SESSION}';" \
    | tr -d ' \n')
  echo "SCENE_BEFORE='${SCENE_BEFORE}'"
  # 预期：turn N 之前可能为 NULL 或入口场景 id（start_session 已激活入口）
  ```

  **Step 6c: turn N——包含场景切换意图的输入**

  记录 turn N 开始时间，drain 到 `TurnComplete`（done phase），记录 TurnComplete 到达的墙钟时间：

  ```bash
  T_START_N=$(date +%s%3N)   # ms

  DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
  TRPG_LLM_PROVIDER=openai \
  TRPG_LLM_BASE_URL=http://localhost:18888/v1 \
  TRPG_LLM_API_KEY=relay \
  TRPG_LLM_MODEL=gpt-5.4 \
  TRPG_LLM_SEND_TEMPERATURE=false \
  TRPG_DATA_DIR=/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/data \
  TRPG_MODULE_READER=1 \
  ./target/debug/trpg turn \
    --ruleset call_of_cthulhu_7e \
    --module call_of_cthulhu_7e.document \
    --session-id "$SESSION" \
    --input "我决定离开加油站，开车进入镇中心，去那里打探更多消息。" \
    --stream-format jsonl 2>/tmp/r5_turnN_stderr.log \
  | tee /tmp/r5_turnN.jsonl

  T_DONE_N=$(date +%s%3N)
  echo "turn N total wall time: $((T_DONE_N - T_START_N)) ms"
  ```

  **`trpg turn` 的行为（Task 3 约定）**：CLI `turn` 在 `TurnComplete` 后等 heavy 完成再退出进程。因此上方命令完成即 heavy 落账。

  **Step 6d: 断言 turn N 的 scene_changed 和 current_scene_id**

  ```bash
  # 场景是否切换（scene_navigate 决定，若叙事不含切换也可能未切）
  SCENE_AFTER_N=$(docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg -t \
    -c "SELECT current_scene_id FROM sessions WHERE session_id='${SESSION}';" \
    | tr -d ' \n')
  echo "SCENE_AFTER_N='${SCENE_AFTER_N}'"

  # pp_lifecycle 应为 complete（CLI turn 等 heavy）
  PP_AFTER_N=$(docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg -t \
    -c "SELECT pp_lifecycle FROM turns WHERE session_id='${SESSION}' ORDER BY created_at DESC LIMIT 1;" \
    | tr -d ' \n')
  echo "PP_LIFECYCLE_AFTER_N='${PP_AFTER_N}'"
  # 预期：complete

  # 若 scene_changed，则 world_events 记录存在
  docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg \
    -c "SELECT event_kind, created_at FROM world_events WHERE session_id='${SESSION}' ORDER BY created_at DESC LIMIT 5;"
  ```

  **Step 6e: turn N+1——紧接发出（无人工等待），断言读到 N 的切换结果**

  ```bash
  T_START_NP1=$(date +%s%3N)

  DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
  TRPG_LLM_PROVIDER=openai \
  TRPG_LLM_BASE_URL=http://localhost:18888/v1 \
  TRPG_LLM_API_KEY=relay \
  TRPG_LLM_MODEL=gpt-5.4 \
  TRPG_LLM_SEND_TEMPERATURE=false \
  TRPG_DATA_DIR=/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/data \
  TRPG_MODULE_READER=1 \
  ./target/debug/trpg turn \
    --ruleset call_of_cthulhu_7e \
    --module call_of_cthulhu_7e.document \
    --session-id "$SESSION" \
    --input "我在镇中心下车，打量周围的建筑，寻找可以询问的当地人。" \
    --stream-format jsonl 2>/tmp/r5_turnNp1_stderr.log \
  | tee /tmp/r5_turnNp1.jsonl

  T_DONE_NP1=$(date +%s%3N)
  echo "turn N+1 total wall time: $((T_DONE_NP1 - T_START_NP1)) ms"
  ```

  **Step 6f: 断言 N+1 读到 N 的 current_scene_id（无陈旧竞态）**

  ```bash
  SCENE_AT_NP1_START=$(grep '"phase":"context_assembled"\|"scene_id"' /tmp/r5_turnNp1.jsonl \
    | head -3)
  echo "Scene context at N+1: $SCENE_AT_NP1_START"

  # 核心：N+1 turn 记录的 pp_lifecycle 最终为 complete
  PP_NP1=$(docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg -t \
    -c "SELECT pp_lifecycle FROM turns WHERE session_id='${SESSION}' ORDER BY created_at DESC LIMIT 1;" \
    | tr -d ' \n')
  echo "PP_LIFECYCLE_NP1='${PP_NP1}'"
  # 预期：complete

  # 高水位守卫触发日志（若 N 的 critical 足够快则守卫立即放行，无明显等待）
  grep -i "await_prev_turn\|critical_done\|high.water\|guard.*wait\|guard.*pass" \
    /tmp/r5_turnNp1_stderr.log 2>/dev/null | head -5
  # 若 critical 在 N+1 发出前已完成：日志显示 "critical_done, passing immediately" 类语
  # 若守卫触发了等待：日志显示轮询等待后 "critical_done reached" 类语
  # 两种都是正确行为；不应出现 "guard timeout, proceeding anyway"（超时放行表示 critical 异常慢）

  # 无 panic
  grep -i "panic\|PANIC\|unwrap.*None\|thread.*panicked" \
    /tmp/r5_turnNp1_stderr.log /tmp/r5_turnN_stderr.log && \
    echo "PANIC DETECTED — BLOCK" || echo "zero panic (OK)"
  ```

  **关键断言汇总（Step 6 pass 条件，全部成立才算 Step 6 通过）：**
  - `PP_AFTER_N == complete`（CLI turn 等 heavy，N 的 heavy 已落账）
  - `PP_NP1 == complete`（N+1 heavy 同样落账）
  - `SCENE_AT_NP1`：若 N 发生了切换（`SCENE_AFTER_N != SCENE_BEFORE`），则 N+1 的 GM 叙事应位于新场景语境下（场景上下文块来自 `SCENE_AFTER_N` 对应的深抽内容）——通过检查 N+1 narration 引用了新场景名或 N+1 stderr log 中 `scene_id=SCENE_AFTER_N` 来确认
  - 零 panic

  若 N 的叙事未触发场景切换（LLM 决策 `moved=false`），则 `SCENE_AFTER_N == SCENE_BEFORE` 是正常结果，此时改用场景已切换的 session 重试（或接受「守卫正常放行，无陈旧」结论，因为陈旧只发生在有切换时）。

---

- [ ] **Step 7: API SSE transport——连发两回合 TurnComplete 时序验证（heavy 不挂流）**

  **Step 7a: 启动 API server**

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula

  DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg \
  TRPG_LLM_PROVIDER=openai \
  TRPG_LLM_BASE_URL=http://localhost:18888/v1 \
  TRPG_LLM_API_KEY=relay \
  TRPG_LLM_MODEL=gpt-5.4 \
  TRPG_LLM_SEND_TEMPERATURE=false \
  TRPG_DATA_DIR=/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/data \
  TRPG_MODULE_READER=1 \
  ./target/debug/trpg api --addr 127.0.0.1:8787 \
    2>/tmp/r5_api_server.log &
  API_PID=$!
  sleep 2
  curl -s http://127.0.0.1:8787/health | python3 -m json.tool
  # 预期：{"status":"ok"}
  ```

  **Step 7b: 创建 session via API**

  ```bash
  SESSION_API=$(curl -s -X POST http://127.0.0.1:8787/api/sessions/start \
    -H "Content-Type: application/json" \
    -d '{"ruleset_id":"call_of_cthulhu_7e","module_id":"call_of_cthulhu_7e.document"}' \
    | python3 -c "import sys,json; print(json.load(sys.stdin)['session_id'])")
  echo "SESSION_API=$SESSION_API"
  ```

  **Step 7c: SSE turn N（记录首 delta 时间 vs done 时间）**

  ```bash
  T_CURL_START=$(date +%s%3N)

  curl -s -N -X POST "http://127.0.0.1:8787/api/sessions/${SESSION_API}/turn" \
    -H "Content-Type: application/json" \
    -d '{
      "ruleset_id": "call_of_cthulhu_7e",
      "module_id": "call_of_cthulhu_7e.document",
      "user_input": "我们刚下车，往小镇里走，想找个地方打探消息。打算去镇中心看看。"
    }' 2>/tmp/r5_api_turnN_stderr.log \
  | tee /tmp/r5_api_turnN.jsonl \
  | awk -v start="$T_CURL_START" '
    /event: delta/ && first_delta == 0 {
      cmd = "date +%s%3N"; cmd | getline now; close(cmd);
      print "FIRST_DELTA_MS=" (now - start) " ms"
      first_delta = 1
    }
    /data:.*"phase":"done"/ {
      cmd = "date +%s%3N"; cmd | getline now; close(cmd);
      print "DONE_PHASE_MS=" (now - start) " ms"
    }
  '

  T_CURL_END=$(date +%s%3N)
  echo "SSE total wall time: $((T_CURL_END - T_CURL_START)) ms"
  ```

  **Step 7d: TurnComplete 时延断言（heavy 不挂流）**

  ```bash
  # delta 事件数（流式验证：多于 1 token 说明真流式）
  DELTA_COUNT=$(grep -c 'event: delta' /tmp/r5_api_turnN.jsonl 2>/dev/null || echo 0)
  echo "DELTA_COUNT=$DELTA_COUNT"
  # 预期：>= 3

  # postprocess_scheduled 出现（heavy spawn 前哨事件）
  grep 'postprocess_scheduled' /tmp/r5_api_turnN.jsonl && echo "postprocess_scheduled OK" || echo "MISSING postprocess_scheduled"

  # done（TurnComplete）出现
  grep '"phase":"done"' /tmp/r5_api_turnN.jsonl | head -3
  # 预期：至少 1 行

  # heavy 不挂流：TurnComplete 应在叙事 delta 完成后很快到（<= 500ms 额外延迟为合格）。
  # 对比基线：R1 时 TurnComplete 需等全 postprocess（含 deep-extract ~12s+）。
  # R5 后 TurnComplete 应在 critical（save_turn + 切场景决策）完成后立即发。
  # 观测方法：DONE_PHASE_MS 与 FIRST_DELTA_MS 之差即为"叙事后到 TurnComplete"的时间。
  # 预期：叙事完成后 TurnComplete <= 2000ms（critical 只含 save_turn + 切场景 LLM 一次调用）。
  # 若 TurnComplete 在 10s+ 之后才到（SSE 连接长时间挂），说明 heavy 仍在挂流——R5 分层未生效。
  echo "若 DONE_PHASE_MS - 最后一个 DELTA 时间 <= 2000 ms 则合格（heavy 不挂流）"
  ```

  **Step 7e: 紧接发出 SSE turn N+1，断言读到 N 切换后场景（无陈旧）**

  ```bash
  # 等 0.2s 模拟客户端轻微延迟（仍属"连发"范畴；不手工等 heavy 完成）
  sleep 0.2

  curl -s -N -X POST "http://127.0.0.1:8787/api/sessions/${SESSION_API}/turn" \
    -H "Content-Type: application/json" \
    -d '{
      "ruleset_id": "call_of_cthulhu_7e",
      "module_id": "call_of_cthulhu_7e.document",
      "user_input": "我在镇中心环顾，找到一家看起来有人的小店，推门进去。"
    }' 2>/tmp/r5_api_turnNp1_stderr.log \
  | tee /tmp/r5_api_turnNp1.jsonl

  # delta 事件数
  grep -c 'event: delta' /tmp/r5_api_turnNp1.jsonl || echo 0
  # done 事件
  grep '"phase":"done"' /tmp/r5_api_turnNp1.jsonl | head -1

  # 无 error
  grep 'event: error' /tmp/r5_api_turnNp1.jsonl && echo "ERROR IN N+1 — BLOCK" || echo "no error (OK)"

  # 守卫行为：检查 API server log 是否记录了守卫等待或立即放行
  grep -i "await_prev_turn\|critical_done\|guard.*wait\|guard.*pass\|high.water" \
    /tmp/r5_api_server.log | tail -10

  # 零 panic
  grep -i "panic\|PANIC\|thread.*panicked" /tmp/r5_api_server.log | tail -5 && \
    echo "PANIC IN SERVER — BLOCK" || echo "zero panic (OK)"
  ```

  **Step 7f: DB 断言——API path heavy 最终落账**

  ```bash
  # N+1 送出约 30s 后（heavy deep-extract 可能需要时间），查 pp_lifecycle
  sleep 15

  docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg \
    -c "SELECT turn_id, pp_lifecycle, postprocess_status, created_at
        FROM turns
        WHERE session_id='${SESSION_API}'
        ORDER BY created_at DESC
        LIMIT 2;"
  # 预期：两条 turn 的 pp_lifecycle 均为 complete（允许最近那条仍为 critical_done，
  #        即 heavy 仍在后台；但 30s 后理应已 complete）
  # 若 pp_lifecycle 仍为 streaming 表示 critical 未完成——严重 bug，BLOCK
  # 若 pp_lifecycle 为 critical_done 而不是 complete 表示 heavy 异常慢或失败——检查 server log

  # scene_changed 事件（若 N 发生了切换）
  docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg \
    -c "SELECT event_kind, count(*)
        FROM world_events
        WHERE session_id='${SESSION_API}'
        GROUP BY event_kind
        ORDER BY count(*) DESC;"
  ```

  **Step 7g: 停止 API server**

  ```bash
  kill $API_PID 2>/dev/null && echo "API server stopped" || echo "already stopped"
  ```

---

- [ ] **Step 8: 缓存锚稳定验证（仅改输入只动 dynamic_hash）**

  目标：确认 R5 分层（heavy spawn、pp_lifecycle 状态写）不影响 context_hashes 的 prefix/pinned/dynamic 三区分离语义。

  ```bash
  # 从 Step 6 的 turn N 和 N+1 的 jsonl 中取 context_hashes
  # turns 表的 context_hashes 列存 {"prefix":"...","pinned":"...","dynamic":"..."}
  docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg \
    -c "SELECT turn_id,
               context_hashes->>'prefix' AS prefix_hash,
               context_hashes->>'pinned' AS pinned_hash,
               context_hashes->>'dynamic' AS dynamic_hash
        FROM turns
        WHERE session_id='${SESSION}'
        ORDER BY created_at DESC
        LIMIT 3;"
  ```

  **断言：**
  - `prefix_hash` 在同一 session 内的连续回合间应相同（BP1 = 规则书 prefix block，不随输入变化）
  - `pinned_hash` 在同一场景内的连续回合间应相同（BP2 = 规则/模组静态块，场景切换后可变）
  - `dynamic_hash` 在 turn N 和 N+1 之间应不同（BP3 = 含玩家输入/记忆/场景动态块，每回合变化）

  若 `prefix_hash` 或 `pinned_hash` 在 R5 后变化（相同场景下），说明 R5 代码在 heavy spawn 时意外触发了 context 重组——需检查 heavy 任务是否错误调用了 `compile_context`。

  若三个 hash 全部相同（turn N 与 N+1 相同），说明 dynamic_hash 没有随输入更新——检查 `save_turn` 调用是否在 critical 组正确传入了 context_hashes。

---

- [ ] **Step 9: 完整零回归确认（全工作区）**

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula

  # 1. 编译零 error
  cargo check --workspace 2>&1 | grep -E "^error\[|Finished"
  # 预期：只有 Finished

  # 2. 全 lib + bin 测试（SKIP_DB_TESTS）
  SKIP_DB_TESTS=1 cargo test --workspace --lib --bins 2>&1 \
    | grep -E "^test result:|FAILED" | tail -40
  # 预期：全部 "test result: ok"，无 FAILED

  # 3. trpg-gm 完整（含 execute_tests、execute_turn_streams_deltas_and_completes 若
  #    SKIP_DB_TESTS=1 则跳过 lazy pool 测试）
  cd crates/trpg-gm
  SKIP_DB_TESTS=1 cargo test --lib 2>&1 | tail -5
  cd ../..

  # 4. trpg-rule-agent（模组抽取核心）
  cd crates/trpg-rule-agent
  SKIP_DB_TESTS=1 cargo test --lib 2>&1 | tail -5
  cd ../..

  # 5. trpg-formula（公式轮 trip）
  cd crates/trpg-formula
  SKIP_DB_TESTS=1 cargo test --lib 2>&1 | tail -5
  cd ../..
  ```

  预期：全部绿，无回归。任何 FAILED 须 triage；若为 R5 引入则**阻止合并**。

---

- [ ] **Step 10: 回滚预案记录**

  R5 所有变更在同一分支内。若 Steps 1-9 中任何步骤标注了 BLOCK：

  ```bash
  cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula

  # 查看 R5 分支 commit 范围
  git log --oneline $(git merge-base HEAD main)..HEAD
  # 输出所有 R5 commits（Tasks 1-3 的 commit sha）

  # 回滚选项 A（推荐）：revert 整个 R5 分支的 commits（保持历史清晰）
  # git revert --no-commit HEAD~N..HEAD  # N = R5 commit 数
  # git commit -m "revert: R5 postprocess tiering rollback — validation gate failed at Task 4 Step K"

  # 回滚选项 B：若分支未推远端，reset 到 R5 起始点
  # git reset --hard <pre-r5-sha>  # 须用户显式拍板

  # 触发条件（任一即 BLOCK）：
  # - Step 1：cargo check 有 error，或 R5 关键符号缺失
  # - Step 2/9：有 FAILED 且确认为 R5 引入回归
  # - Step 5：迁移失败，pp_lifecycle 列不存在
  # - Step 6：PP_AFTER_N != complete（CLI turn 等 heavy 语义失效），或零 panic 失败
  # - Step 7：SSE TurnComplete 在叙事后 10s+ 才到（heavy 仍挂流），或 pp_lifecycle 仍为 streaming
  # - Step 8：prefix_hash 或 pinned_hash 在相同场景内不稳定
  ```

  记录实际触发的 BLOCK 步骤编号和现象，供回滚 commit message 说明原因。

---

**Step 验收矩阵（合并前逐行打勾）**

| Step | 验收条件 | SKIP 条件 |
|---|---|---|
| 1 | `cargo check` Finished；三类 R5 符号 grep 有输出 | 无（硬性门槛） |
| 2 | 全 crate `test result: ok`，无 R5 引入 FAILED | 无（硬性门槛） |
| 3 | R5 专项 7 个单测全 ok | 无（硬性门槛） |
| 4 | `trpg --version` 不 panic | 无 |
| 5 | `pp_lifecycle` 列存在于 turns 表 | Docker 不在线则 SKIP（标注），同时 SKIP Steps 6-9 live 步骤 |
| 6 | `PP_AFTER_N=complete`；`PP_NP1=complete`；零 panic | Docker/:54347/relay 不在线则 SKIP |
| 7 | delta >= 3；`done` 出现；TurnComplete 在叙事后 <= 2000ms；N+1 无 error；零 panic | 同上 |
| 8 | `prefix_hash` 连续回合相同；`dynamic_hash` 相邻回合不同 | 同上 |
| 9 | 全工作区 `test result: ok`，无回归 | 无（硬性门槛） |
| 10 | 回滚命令已记录，触发条件已理解 | 无 |