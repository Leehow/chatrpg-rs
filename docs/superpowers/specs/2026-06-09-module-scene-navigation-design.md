# 模组场景导航设计

> Date: 2026-06-09 · Status: design approved
> 护栏：语义优先非关键词 · 数据驱动零硬编码 · fail-closed 永不乱跳 · 复用现有 turn_postprocess/load_module_graph/state_frame · 文件 ≤400 行。

## 1. 问题

P4 投影机制(`module_scene_blocks_for_turn`)已建，但**没人激活/切换"当前模组场景"**：
- `start_session` 只 `create_session`，不设场景；CLI `run_turn_once` 与 API run_turn 的 `RuntimeState.scene_id` 恒为 `None`。
- 结果：进游戏看不到模组入口场景；越过入口后无法推进到后续场景；P5 后台预抽的场景不可达。

## 2. 目标 / 非目标

**目标**：进游戏即看到模组入口场景；GM 叙事推进时**语义地**把当前场景切到党实际走到的模组场景；整本模组可玩；P5 预抽场景可达。
**非目标**：把 state_frame 改造成场景层(它是交互级 Combat/Chase/…，粒度不对，不动)；重写 turn 编排；多人/多党分支场景(单一 current scene)。

## 3. Grounding（直接读码）

- `state_frames` = 交互级帧(FrameKind: Combat/Chase/Netrun/InvestigationNode/…)，**非叙事场景层** → 当前场景用轻量持久化，不塞 frame。
- `turn_postprocess`(trpg-api:1398，每回合后台 job)+ `load_module_graph(module_id)`(trpg-db:454，seam-4)现成 → 语义切换的落点 + 场景图来源。
- P4 `module_scene_blocks_for_turn`(runtime:1423)已有**入口回退**(scene_id 缺失 → 投影模组入口场景)、`scene_node_to_blocks`(runtime:1952)。
- `deep_extract_scene_in_place`(module_reader_loop，Phase 5)可复用做到场深抽。

## 4. 设计（5 组件）

### 4.1 持久化当前场景
`sessions` 加列 `current_scene_id text`(可空，新增迁移；app 启动跑 sqlx migrate)。
db：`set_session_scene(session_id, scene_id)` + `load_session_scene(session_id) -> Option<String>`。

### 4.2 入口激活
`start_session(ruleset, module_id)`：若有 module → `load_module_graph(module_id)` → 取入口场景 id
(优先 `module_graph.spine.entry_node_id`；否则首个 `extraction_status==DeepExtracted`；再否则 `scenes[0]`)
→ `set_session_scene(session_id, entry_id)`。fail-closed：取不到图/无场景 → 不设(保持 None，P4 入口回退兜底)。

### 4.3 每回合载入（单一点，DRY）
runtime 构建上下文时(prepare_turn_context / compile_context 入口，有 session_id)：
若 `state.scene_id.is_none()` 且 `state.module_id.is_some()` → `load_session_scene` → 设 `state.scene_id`。
CLI/API 均自动受益，无需各自改 RuntimeState 构造。

### 4.4 P4 投影当前场景 + 出口
`scene_node_to_blocks` 在 read_aloud/gm_notes/在场 NPC 之外，**附加当前场景 `links` 的出口清单**
(每条 `to_node_id` 查 `scenes` 取 title + link_type)，让 GM 知道党可去的相邻场景。仍 `expires_at_scene`。

### 4.5 语义切换（核心）+ 到场深抽
`turn_postprocess` 新增 `scene_navigator(db, llm, session_id, module_id, current_scene_id, transcript)`：
1. `load_module_graph` → 场景列表(id+title+kind) + 当前场景的 links。
2. 一次小 LLM 调用，输入 当前场景 + 全场景列表 + 本回合叙事 → 语义判定：党是否移动？移到哪个 **真实存在的** node_id？
   submit schema `{moved: bool, target_node_id: string|null, reason: string}`。
   **fail-closed**：`moved=false` 或 target 不在场景列表 → 留原场景，不更新。
3. 移动且 target 有效 → `set_session_scene(session_id, target)`；若 target 场景 `SkeletonOnly` →
   `deep_extract_scene_in_place`(复用 Phase 5) + upsert module bundle，使下回合可投影。target 已 Deep(P5 预抽) → 直接就绪。
零硬编码(target 从模组真实场景图选，无规则/语言专名)；语义优先(LLM 判定，非标题关键词匹配)。

## 5. 数据流

```
进游戏 start_session(module) → 入口激活 current_scene_id=entry
  turn1: runtime 载 current_scene_id → P4 投影入口场景(念白+出口) → 玩家秒看到开场
  GM 叙事「党驱车前往镇中心」
  turn_postprocess: scene_navigator 语义判定 moved=true target=镇中心 → set current_scene_id + 深抽镇中心
  turn2: runtime 载新 current_scene_id → P4 投影镇中心 deep 内容
```

## 6. 错误处理（全程 fail-closed）

- scene_navigator 拿不准/无匹配/LLM 失败 → 留原场景(`moved=false` 语义默认)，绝不乱跳。
- 入口激活失败 → current_scene_id 留 None → P4 入口回退兜底。
- 目标 SkeletonOnly 且未及深抽 → P4 跳过 → materializer 文本检索降级(不崩、不编造)。
- 到场深抽/upsert 失败 → 记 warn，current_scene_id 已更新(场景仍可走文本降级)。

## 7. 测试

- `scene_navigator` 纯判定单测：给定场景列表+当前场景+叙事 → moved/target；叙事无移动→moved=false；target 不在列表→fail-closed 不动。
- 入口激活：start_session(module) 后 current_scene_id == 入口 id；无图→None。
- runtime 载入点：state.scene_id None+module → 载 current_scene_id；已有 scene_id→不覆盖。
- P4 出口投影：deep 场景块含 links 出口标题。
- 到场深抽：target SkeletonOnly → 深抽后 Deep。
- 跨结构通用：线性(Homecoming)/sandbox(血色公路)/任务集(Vault) 语义切换不靠关键词。
- in-game e2e：CoC 真模组(已 37 场景)进游戏跑 2-3 回合，看入口投影 + 一次语义切换 + 切换后场景投影。

## 8. 已决策

| 项 | 决策 |
|---|---|
| 切换驱动 | turn_postprocess 语义后处理(解耦 GM 状态机) |
| 当前场景持久化 | `sessions.current_scene_id` 列(非 state_frame，粒度对) |
| 载入点 | runtime 单一点(DRY，CLI/API 共享) |
| 切换目标范围 | 模组全场景列表语义选(非仅 links，兼容 sandbox)，fail-closed 默认不动 |
| 到场深抽 | 复用 deep_extract_scene_in_place；P5 预抽则直接就绪 |
