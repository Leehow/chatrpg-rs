# GM Agent Loop 设计（方案 A：混合 agent loop）

日期：2026-06-10
状态：设计已获用户逐项确认（范围/预算/掷骰体验/架构/分节设计）
范围：第一期 —— 主游戏回合 loop（CLI 路径），建卡/解析 skill 化属二期

## 1. 背景与痛点取证

用户痛点："编排器和 LLM 链接不上导致 LLM 没办法决定什么时候让玩家投骰子；投的骰子没有效果。" 经三路并行代码取证，两个痛点均成立：

### 痛点 1：叙事 LLM 对掷骰零话语权（实锤）

- 最终叙事调用 `stream_chat` 无 tools 参数（`crates/trpg-llm/src/lib.rs:57` trait 签名；请求体 `base_body` 只有 model/messages/stream/temperature）。
- 叙事 prompt 明令禁止叙事者碰机制：`player_facing_output_contract`（`crates/trpg-runtime/src/lib.rs:1915`）"Do not ask the player to roll dice… Rust tools own checks, dice"。
- 掷骰决策全部在叙事之前由上游做出：语义分类器（`crates/trpg-orchestrator/src/lib.rs:142`，每次调用 `visible_scene_objects` 和 `actor_parameter_summary` 传**空 JSON**，看不到场景与角色数值）→ 路由 → advice 关键词 contains/regex 匹配（`crates/trpg-agent/src/lib.rs:229-242,330-339`）→ 各内核。
- 结论：看得到全部上下文的组件（叙事者）无权决定掷骰；有权决定掷骰的组件（分类器/关键词路由）看不到上下文。脑手分离。
- 唯一有 function-calling 的 `gm_retrieve_phase`（`crates/trpg-cli/src/main.rs:1976`）只有 retrieve / apply_track_change 两个工具，不能创建检定；API 路径连这个阶段都没有。

### 痛点 2：掷了骰却没有效果（系统性失效，默认在线）

最致命链条（generic/forced 检定路线）：

1. 契约 `tested_parameter=None`、`target=UnknownUntilLookup`、骰式缺省 "1d10"（`crates/trpg-agent/src/lib.rs:148,166`）。
2. 结算时 `derive_tested_source` 靠"角色卡键名以子串出现在检定文本里"撞绑定 —— 中文输入 + 英文键名几乎必不命中。
3. 撞不上 → outcome 落 Provisional（success=null）→ on_success/on_failure 资源触发全部跳过（零状态变化）；CLI 给叙事注入 [system] 指令"不要陈述或暗示成功/失败"。
4. **全代码库没有任何消费 ProvisionalNeedsBinding 的再结算路径** —— "掷骰→渲染张力→什么都没发生"是该路线的必然终点。

帮凶：攻击类无 source 直接拒掷（blocked_missing_source 默认 fail）；命中但找不到武器伤害公式 → 仅记 blocked 事实、HP 永不变；`NarrationVerifier`（`crates/trpg-agent/src/gm_loop.rs:175`，防叙事无视/编造掷骰结果）**整个未接线**（全仓库零消费方）；SAN/Chaos 类 StatePatch::CreateFact 变化连 CLI 事件都不发。

### 原语盘点结论（重构可行性）

- 确定性原语层质量很高：掷骰/检定结算/对抗/效果应用（`after_check_resolved` 单一公共边界）/成长改值/规则检索/场景图/世界时间/记忆/NPC 合成，基本都是 `(db, ids, args) → Result` 自包含签名。`apply_track_change` 注释自称 "CLI command + GM-agent tool both call this"。
- `chargen_compile` 与 `module_reader` 内部已是 `complete_with_tools` 驱动的 agentic 子循环 —— 二期"解析 skill"已有两份样板。
- BP1/BP2/BP3 三段缓存带（`prepare_turn_context` → CompiledContext）可直接当 agent loop 的 prompt builder。
- 真正耦合在控制流层：`run_turn_once`（~900 行）、orchestrator 路由、advice 关键词匹配、每回合 4-5 次薄上下文小 LLM 调用 —— 即本设计的拆解对象。
- 已知缺口：trpg-llm 流式与 tools 互斥（`complete_with_tools` 写死 stream:false）；`extract_module_scenes(only)` 为 crate 私有需提 pub。

## 2. 已拍板决策

| 决策点 | 结论 |
|---|---|
| 第一期范围 | 主游戏 loop 先行；建卡/解析维持现有入口，二期 skill 化 |
| 回合预算 | 质量优先：主 loop 全程 gpt-5.5，工具轮上限 8，接受 20-40s/回合 |
| 掷骰体验 | agent 自主决定：roll_check（系统掷）与 request_player_roll（暂停等玩家亲手掷）双工具，按戏剧张力裁量 |
| 架构 | 方案 A 混合 agent loop：LLM 拥有控制流，Rust 原语做工具 |
| 流式（硬性原则） | **最终输出必须 SSE 真流式**（D2：终轮 content delta 直接流给玩家，非缓冲回放）。校验后置：NarrationVerifier 不做出口闸门，事后跑、findings 写入下一轮记忆，由 GM 在后续叙事中自洽勘误；只盯"同类错误不高频复发"，不做可见修补/撤回 |
| 缓存稳定（硬性原则） | loop 的 prompt 前缀必须字节级稳定：回合内各轮只在尾部追加；跨回合稳定段只随场景/规则变化；禁一切 cache-buster（详见 §6.1） |

## 3. 目标 / 非目标

**目标**

- 叙事 LLM（上下文最全的组件）直接持有"是否/何时/如何掷骰"的决策权（痛点 1 根治）。
- 检定的 tested_parameter 由 agent 语义绑定，Provisional 静默死路消失；fail-closed 错误变为 agent 可见、可修复（痛点 2 根治）。
- 防幻觉护栏换形态不减弱：状态唯一写入通道 = 工具；NarrationVerifier 首次接线为出口闸门。
- 零硬编码不变量保持：GM 裁量准则、政策文本全为数据文件；kernel 仍是 kernel。
- 旧路径完整保留，可 A/B 对照。

**非目标（第一期）**

- 不做建卡/解析 skill 化（现有入口照用）。
- 不做 API 路径接入（CLI 验证后二期）。
- 不搬 conflict kernel 的 frame/reaction 深度（战斗走通用 roll_check + apply_effect，深度回退为已知 trade-off）。
- 不做出口阻断式校验（用户拍板 D2：校验后置为勘误记忆，不阻塞流式交付）。
- 不删旧代码。

## 4. 架构与回合数据流

新建 crate `crates/trpg-gm`（依赖 trpg-runtime / trpg-db / trpg-llm / trpg-agent / trpg-contest / trpg-mechanics / trpg-model）。

```
玩家输入
  → [确定性头部，无 LLM]
      record_world_event(PlayerAction)
      refresh_actor_live_derived("pc.current")
      reconcile_session
      若有 open gate 且输入可解析为骰值回复:
        resolve_check_with_input → 结果作为「已发生事实」注入 loop 初始上下文
        （gate 结算不再是早退控制流，而是 loop 的输入事实）
  → [上下文] prepare_turn_context → CompiledContext(BP1/BP2/BP3)（复用现有）
  → [LOOP，轮数 ≤ max_tool_rounds=8]
      stream_chat_with_tools(messages, tools)   ← SSE delta 解析
      ├─ tool_calls delta → 聚合完整调用 → Rust 执行 → 结果回填 messages，
      │     事实记入 TurnLedgerSnapshot（本轮对玩家零输出）
      ├─ content delta → 这就是最终叙事：经防漏滑动缓冲后 SSE 直通玩家（终态）
      │     ※ 防漏缓冲：仅过滤本回合账本中的私骰 token（小集合、流中可查）
      └─ request_player_roll → CheckContract + PendingCheck + InteractionGate 落库
           → 返回提示语（终态，回合结束等玩家）
      轮数耗尽仍要工具 → 末轮强制 tool_choice:"none" 逼出散文
  → [流后校验，不阻塞交付] NarrationVerifier(ledger, 已流出文本)
      findings → 勘误记忆（下一轮 BP3 注入，GM 在后续叙事中自洽）
      同类 finding 在会话内 ≥N 次 → 升级为持续提醒块（防同错高频复发）
  → [确定性收尾] save_turn / memory event / learning audit(保留) 
```

核心类型（示意）：

```rust
pub struct GmLoop { engine: RuntimeEngine, llm: Arc<dyn LlmClient>, tools: ToolRegistry, cfg: LoopConfig }
pub struct LoopConfig { max_tool_rounds: u8 /*8*/, repeat_finding_threshold: u8 /*N，同类勘误升级阈值*/ }
pub enum TurnOutcome {
    Narration(String),
    AwaitingPlayerRoll { check_id: String, prompt_public: String },
}
pub async fn run_gm_turn(&self, session, ruleset, module, user_input, transcript) -> Result<TurnOutcome>
```

回合账本直接复用 `trpg_agent::TurnLedgerSnapshot`：每个工具执行后把产生的 CheckContract / DiceRollRecord / CheckResultRecord / EffectContract / ParameterImpact / InteractionGate 追加进去，供流后复盘对账与私骰 token 集合提取。

## 5. 工具清单（第一期 10 个）

所有工具是现有原语的薄包装；失败一律返回结构化错误 JSON（agent 可见并自行调整），不 panic、不静默。

D2 流式终态下**没有 submit_narration 工具**：模型不再调工具、直接输出 content，即是最终叙事（pi 的自然终态）。校验后置（§7）。

| # | 工具 | 参数（草案） | 内部实现 | 要点 |
|---|---|---|---|---|
| 1 | `roll_check` | check_label, **tested_parameter(必填)**, actor_id?, opposed?{npc_id, opponent_parameter}, visibility(public/secret), intent_kind | 构造 CheckContract（骰式取 kernel.dice_core；缺失则报错）→ `execute_system_roll_bundle` → `after_check_resolved` | tested_parameter 必填是痛点 2 根治点；对抗形态走 `stamp_opposed_check` 既有通道 |
| 2 | `request_player_roll` | check_label, tested_parameter, stakes{before/success/failure}, visibility | 契约 + `insert_pending_check` + `insert_interaction_gate` | 回合终态；下回合头部结算 |
| 3 | `apply_effect` | target_actor, parameter_path 或 track_id, op, amount, reason | trpg-mechanics 效果应用路径；检定附带效果已由 after_check_resolved 覆盖，**无检定的直接效果**需把私有 apply_effect_roll 提 pub 或加薄封装（实施任务之一） | 账本记 ParameterImpact |
| 4 | `change_track` | bucket, id, op, amount\|value, kind?, category? | `RuntimeEngine::apply_track_change` | 已按 tool 设计，零改动 |
| 5 | `retrieve_rules` | query, k? | `RuntimeEngine::retrieve_rules` | 原样 |
| 6 | `get_actor` | actor_id | 角色卡 + mechanical_profile 投影 | GM 视角 |
| 7 | `ensure_npc_param` | npc_id, bucket, param, context | `ensure_npc_parameter`（T1>T2>T3 阶梯） | agent 自主决定何时调，替代 fire-and-forget 预 pass |
| 8 | `navigate_scene` | target_node_id, reason | `validate_transition` + `set_session_scene` + 到场深抽 | `extract_module_scenes(only)` 提 pub（一行改动）；独立 scene_navigator LLM 判定退役 |
| 9 | `advance_time` | amount, scale, reason | WorldTimeService | 时间权威仍在 Rust |
| 10 | `remember` | summary, importance?, tags? | `save_memory_event` | 主动记忆；回合自动摘要仍在收尾 |

第一期刻意不提供：conflict/ability/object kernel 专用工具。物品随检定的规则效果（如开枪耗弹）已在 `after_check_resolved` 公共边界内自动执行，不丢；战斗深度回退见 §3 非目标。

## 6. 提示与上下文（全部 data-driven）

- system prompt = BP1 prefix（engine protocol 出 agent-loop 版文案）+ **游戏 skill 指令**：
  - GM 裁量准则：何时掷骰 / 何时交给玩家亲手掷 / 何时免检定直接叙事 —— 从现有 22 个 advice JSON 蒸馏为语义准则（advice 层从"路由数据"变"提示数据"，内容不丢，零硬编码保持）；
  - ITEM POLICY / NPC POLICY 从 `gm_retrieve_phase` 的 system prompt 迁移；
  - 玩家可见输出契约（[system]/[roll] 标签语义、不泄 GM-only、保留）。
  - 载体：`data/agent/gm_skill/*.md`，按 ruleset 可覆盖（global → ruleset 两级合并）。
- BP2（pinned，当前可玩单元）/ BP3（dynamic，每回合状态）投影逻辑原样复用 `prepare_turn_context`。
- 工具轮 messages（assistant tool_calls / tool 结果）追加在 BP3 之后，符合 OpenAI 多轮工具会话格式（`complete_with_tools` 已支持 raw JSON tool-role 历史，chargen reader 已实战）。
- **流式策略（硬性原则，D2）**：trpg-llm 新增 `stream_chat_with_tools`——解析 SSE delta：`tool_calls` delta 聚合为完整调用（该轮零玩家输出）；`content` delta 即最终叙事，经防漏滑动缓冲后**端到端 SSE 直通玩家**（engine → emit_delta/SSE → 客户端，token 级，非缓冲回放）。首 token 延迟 = 工具轮耗时 + 单次调用 TTFT，无额外轮次。防漏缓冲只过滤本回合账本中的私骰 token（已知小集合，滑窗扫描，对玩家不可感知）。
- 末轮强制：工具轮数耗尽时以 `tool_choice:"none"` 逼出散文，保证回合必有叙事终态。

### 6.1 缓存稳定（硬性原则）

loop 每回合 3-8 次大上下文往返，缓存命中与否直接决定成本与延迟，因此前缀稳定是**设计约束**而非优化项：

1. **messages 布局按稳定性降序**：`[system: BP1+gm_skill 指令] → [BP2 pinned] → [会话历史，append-only] → [BP3 dynamic + 本回合输入] → [工具轮追加]`。稳定段在前，易变段在尾。
2. **回合内**：CompiledContext 渲染一次、整回合复用；各工具轮只在 messages 尾部追加，**绝不重渲染/重排任何前缀字节**。
3. **跨回合**：BP1 文本按 ruleset 固定；BP2 只随场景切换变化；会话历史只追加不改写（历史压缩/摘要只允许在显式 compaction 事件中发生，且视为一次有因的缓存失效）。
4. **禁 cache-buster**：prompt 文本（尤其 prefix/pinned 段）不得含时间戳、UUID、随机排序；块排序必须确定性（sort_blocks 现已确定性，dedupe 保序）；token 预算裁剪不得引起 prefix/pinned 段抖动（裁剪压力只落在 dynamic 段，prefix/pinned 超预算视为配置错误而非静默裁剪）。
5. **可观测**：每轮记录 prefix_hash/pinned_hash 与上游缓存命中指标（若 relay 透传 usage.prompt_tokens_details.cached_tokens）；hash 变化必须有因（场景切换/规则变更/显式 compaction），无因变化按 bug 处理。
6. relay 端缓存透传行为实测（见 §10 风险）。

## 7. 防幻觉护栏（换形态对照）

| 旧形态 | 新形态 |
|---|---|
| 状态写入散在内核链各处 | 状态唯一写入通道 = 工具；叙事文本零状态权威 |
| tested_parameter 靠子串撞，撞不上静默 Provisional | roll_check 必填 tested_parameter；kernel 无骰式 / 参数不存在 → 工具返回结构化错误，agent 可见可修复（去 retrieve_rules 或改 request_player_roll 或改纯叙事），fail-closed 语义保留但不再静默 |
| NarrationVerifier 写好未接线 | **首次接线，但为流后复盘而非出口闸门**（用户拍板：流式优先，小错可容忍）：流后对账（MissingCheck / MissingRollExecution / InventedEffect / OmittedVisibleResult / ManualRollRequest / SecretLeak），findings 写成**勘误记忆**进下一轮 BP3，GM 在后续叙事中自洽；同类 finding 会话内 ≥N 次升级为持续提醒块（防同错高频复发），并按 kind 聚合统计供 gm_skill 准则迭代 |
| 无预算概念，管线长度固定 | 工具轮 ≤8，耗尽强制 tool_choice:"none" 逼出散文终态 |
| 私骰泄漏靠 prompt 约束 | RollVisibility 语义保留：secret 结果进账本与 agent 上下文（它需要知道结果来叙事）；**流中滑动缓冲过滤私骰 token**（唯一在线护栏——泄密是不可事后勘误的错误类别）+ 流后 SecretLeak 复盘 |

## 8. 共存与退役

- 入口：`trpg play --agent`（CLI flag）走新 loop；默认仍旧路径。
- 第一期不删任何旧代码：orchestrator / advice 路由 / 内核链 / scene_navigator 继续服务旧路径。
- 新 loop 下退役（不被调用）的组件：TurnOrchestrator 路由、GmAgent::plan_turn 关键词匹配、forced/named assessment 构造器、gm_retrieve_phase、独立 scene_navigator 判定、NPC fire-and-forget 预 pass。
- API 路径二期接入（复用 run_gm_turn，仅替换 SSE 装配层）。

## 9. 测试与验收

**单元/集成（mock LlmClient，脚本化 SSE delta / tool_calls 序列）**

1. 正常回合：roll_check 工具轮 → content 流式终态，账本含契约+结果+补丁；content delta 逐块到达玩家侧。
2. request_player_roll 终态：gate 落库、TurnOutcome::AwaitingPlayerRoll；下回合骰值回复 → 头部结算 → 事实进上下文。
3. SSE delta 解析：tool_calls 分片聚合正确（跨 chunk 的 arguments 拼接）；content 与 tool_calls 混合序列不串台。
4. 防漏滑动缓冲：私骰 token 跨 chunk 边界出现时仍被过滤；无私骰回合缓冲透明直通。
5. 流后校验→勘误记忆：叙事漏提可见掷骰结果 → finding 落勘误记忆 → 下一回合 BP3 含该勘误块；同类 finding 达阈值 → 持续提醒块出现。
6. 轮数超限：第 8 轮后 tool_choice:"none" 强制散文终态。
7. 工具错误可修复：roll_check 缺 kernel 骰式 → 结构化错误回填 → agent 改走 retrieve_rules / request_player_roll。
8. 账本完整性：每个工具的产物正确进 TurnLedgerSnapshot。
9. 缓存稳定：同回合各轮请求体前缀字节级一致；跨回合（场景不变）prefix/pinned 段 hash 不变（§6.1 第 5 条的回归测试）。

**e2e（真 :54347 DB、真模组、真 gpt-5.5）**

- 血色公路（CoC sandbox 中文）+ The Vault（Triangle 任务集英文）各跑多回合，验收：
  1. agent 在合理时机发起检定且 tested_parameter 语义绑定正确；
  2. 资源 on_outcome 真触发（SAN/HP 实际变化，查 generic_parameter_states）；
  3. request_player_roll 暂停 → 玩家回骰 → 下回合演绎后果，全闭环；
  4. 回合总延迟 ≤ 40s，且**最终叙事为真流式**（首 content token 显著早于回合完成，目标典型 ≤20s）；
  5. 跨回合缓存命中可观测（relay 若透传 cached_tokens 则记录命中率）。
- 用 chatrpg-product-evaluator 对新旧路径做对照 playtest。

**工程约束**：文件全 ≤400 行（`trpg-gm/src/` 拆 loop.rs / tools/*.rs / ledger.rs / prompts.rs）；零 per-ruleset 硬编码。

## 10. 风险与缓解

1. **relay 兼容性（双项 smoke test，排第一个实施任务）**：① gpt-5.5 带完整 tool 历史的多轮会话（gpt-5.4 经 relay 多轮工具已实战，5.5 待验，失败则降 gpt-5.4）；② **SSE 流式下的 tool_calls delta 透传**——D2 的硬依赖，需确认 relay 原样转发分片 tool_calls 与 content delta（若 relay 把流式工具调用整块缓冲或丢字段，D2 不成立，需先修 relay 或临时回退"工具轮非流式+终轮 stream_chat"）。codex-spark 空 tool 参数不可用（已知），不选。
2. **成本**：8 轮 × 大上下文，无缓存时单回合可能数十万 input tokens。缓解：§6.1 缓存稳定为硬性原则 + 实测 relay 缓存透传；必要时压 BP3 投影预算。
3. **战斗深度回退**：通用 roll_check 不含 reaction window / frame 管理。明示为第一期 trade-off，e2e 中记录战斗体验差距，二期补战斗工具组。
4. **演绎质量漂移**：没有路由强制，agent 可能过度/不足使用检定。缓解：gm_skill 裁量准则文件可迭代（数据改不动代码）+ 对照 playtest 量化。

## 11. 二期展望（非本期承诺）

- 建卡 skill：chargen 原语 + character_creation_messages 指令载荷包装为独立 skill 会话。
- 解析 skill：parse_all / module_reader 作为后台 job 工具暴露（样板已存在）。
- 战斗工具组：frame/reaction/novelty 接入 loop。
- API 路径接入（流式 tool-call 解析第一期已做，API 侧复用 run_gm_turn + SSE 装配）。
- NarrationVerifier 的 token 对账从子串匹配升级为结构化引用（referenced_ledger_ids 强校验）；勘误记忆的聚合统计反哺 gm_skill 准则文件的自动迭代。
