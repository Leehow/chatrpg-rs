# 分层运行时长任务 · 治理章程 (Charter)

> 单一事实源:本章程 + `2026-06-19-layered-runtime-architecture.md`(大计划 P0–P7)+ `design/设计4.md`(架构) 三者构成长任务的全部决策依据。
> **理念(用户 2026-06-19 拍板)**:长任务**只在开头集中拍板 + 立这套规则/理念**;开跑后 AI **据本章程自决一切**,**绝不中途回来要人**,逐阶段跑到整条计划结尾。只有 §3 的硬停项才停。

---

## §1 自决规则(最高指令)

1. AI 对**一切**设计 / 架构 / 实现 / 取舍**自主决断**,依据 = 本章程 §2 原则 + §5 每阶段既定立场 + design4 + 代码现状。
2. **不因"需要架构决策"而停**。本章程已把全计划 36 个原本逐阶段冒出的决策预解成既定立场(§5),全部 `needs_human_signoff=false`。
3. 遇到章程未覆盖的新决策:据 §2 原则(保守 / 加性 / fail-closed / 镜像既有)自决并在 Progress.md 记录依据,**继续推进,不停**。
4. `BlockedByHuman` **仅**用于 §3 硬停项;不再用于"实现/架构拿不准"。拿不准 → 按 §2 选最保守的加性选项 + 文档化 + 继续。
5. 逐阶段 P1→P7 跑到底;每阶段完成即滚动集成(§4)并自动进入下一阶段,直到全 plan 的 `FACTORY_DONE`。

## §2 全局决策原则(AI 自决时的判据)

**数据 / 语义**
- 零硬编码:ruleset/模组特定数据进 kernel/override(`embedded_config/rules/*.override.json`),引擎只按数据匹配 + GENERIC 兜底;引擎 .rs 无 ruleset 字面量。
- fail-closed:取不到 / 不确定 → 按最严 / 最保守(漏报优于错报);source-backed,不编造。
- 镜像既有 pattern,不发明平行类型 / 第二套真相源(§10.5 核心数据不分叉)。
- player-safe 投影只 **allowlist** 机械事实摘要,GM-only / secret / 规则原文 / tool-JSON 一律不进。

**迁移 / 等价**
- 加性优先;behavior-preserving 是铁律——新功能挂 env flag 默认 OFF,**OFF==基线逐字节(hash 字节)等价**作为零行为变更铁证(沿用 R1/R2/BP1-BP2 手法)。
- 不破坏既有绿测试;replace → **coexist-then-deprecate**,不一把删。
- 翻默认 ON 的判据 = **客观验收门全绿**(等价回归 + ON 路径专项 + 两 transport live e2e 逐字段一致 + §19 对应门),由 AI 据门自决,不靠人拍。

**结构**
- **行为切分先行,结构重构(rename / crate 抽取 / ports / 新 PhaseId)殿后到 P7**;module-first,耦合过密就停在 module 边界写 handoff,不强抽 crate。
- `CANONICAL_TURN_PLAN` 严格 15-phase 不破(`canonical_plan_has_fifteen_phases` 等纯函数测试);新 phase 仅 P6/P7 显式批准。
- 不提前过度建模(design3 教训:别铺影子层宽度,逐条把影子变 load-bearing)。

**质量 / 范围**
- Superpowers 按任务判断(非强制):行为代码默认 TDD + 失败时 systematic-debugging;有测试/门就在收口前 verification-before-completion(真跑真读输出);大重构加一轮真库 live e2e。纯文档/配置/琐碎活跳过仪式。
- 严守阶段边界,不把未来阶段的活提前做;单文件 ≤400 行。
- scoped commit:只 stage 该阶段 code paths,**绝不 stage `.claude/`**;每阶段独立分支/worktree。

## §3 硬停项清单(唯一会停下要人的;其余全预授权)

**会停(标 BlockedByHuman + 写清原因,继续做其他 row)**
- `git push` / 合并到**远程或 shared** / 部署。
- 破坏性迁移 / 数据丢失 / DB schema 破坏 / public API 破坏。
- 外部发布(发数据到第三方)/ 触碰安全·凭据·密钥。
- **预算 / 时间上限超**(见 §6,由用户设定)。
- design4 自相矛盾且无原则可解 / 与用户既有需求冲突。
- 聚焦调查后仍**无法归因**的验证失败。

**预授权(不停,直接做)**
- 本地分支 / worktree / 本地 scoped commit / 重构 / 数据移动 / 新类型 / **新表(加性迁移,if-not-exists 幂等、不改既有迁移号)**。
- behavior-verified(arch_gates + 阶段验收 + live e2e 绿)后**本地 main 的 FF-merge**(不 push)。
- 新功能挂 env flag 默认 OFF 落地。

## §4 每阶段执行流程 + 滚动集成

```
重锚大计划该阶段 + 本章程 §5 立场
  → TDD / spec(行为代码)
  → 实现(据 §2 自决,scoped commit)
  → arch_gates.sh 必 8/8 绿 + 该阶段验收门绿
  → 受影响 crate cargo test 绿 + 大改动 live e2e(CoC :54347 / Cyberpunk :54346)
  → 本地 FF-merge 进 main(不 push)
  → 更新 ledger / Progress / TaskList
  → 自动进入下一阶段
```
跑到 P7 完成 → 写 `FACTORY_DONE.sentinel` + `<promise>FACTORY_DONE</promise>`。

## §5 每阶段既定立场速查(全部 AI 自决,无需人拍)

**P1 拆 Adjudicator/Narrator + 修实时流(根因切口)**
- NarrationPacket 只透传【已 committed 机械事实摘要 + player_input + 玩家可感知/已揭示事实 + forbidden_reveals】;`adjudicator_prose` 一律不喂 Narrator(仅 fail-soft 直发 fallback)。投影复用 `TurnLedger.snapshot` / `list_player_known_fact_ids`(trpg-db:3583)/ `harvest_session_secret_terms`(turn_loop:1676),fail-closed allowlist。
- Narrator 复用同一 `self.llm`,不引第二模型句柄(独立 cache key / 更强模型留 P7)。
- 不新增 PhaseId,Narrator 作 AgentLoop body 内 ON 分支(buffer 模式产 AdjudicationPacket→投 NarrationPacket→流出),不破 15-phase。
- flag `TRPG_NARRATOR_SPLIT` 默认 OFF;翻 ON 判据=等价回归绿 + ON 路径"adjudicator Delta 不出 tx"绿 + 两 transport live 逐字段一致 + §19-#1/#5 绿;不发明 per-ruleset 灰度(要分段走 plugins.rs `applies_when` 数据)。
- Narrator system prompt 走 data/agent 插件系统(`plugins.rs` + `applies_when`),不内联;StyleProfile 从 `gm_skill` 取轻量串或中性默认。
- P1 只以"Narrator 无工具(空集/ToolChoice::None)"落 commit 边界;显式能力标注留 P2。

**P2 commit 边界标注 + 校验可阻断**
- `PresentationGate::Block` 严格收窄 = {SecretLeak, InventedEffect, ManualRollRequest} 且 severity==Blocker;其余 Allow + 现有 errata/RetroDebt 补账不变。白名单钉成纯函数穷举测试常量(扩需改测试=显式人审点)。
- Block 修复阶梯:首选重生一次(带 forbidden_reveals 收窄,bounded retry=1)→ 仍 Block/LLM 失败则降级确定性 committed-facts 模板叙事(§18,绝不空白)。
- `ask_clarification` 只给 Narrator(`narrator_safe()` 追加),不进 `standard()`(否则破 15-工具 schema 字节稳定)。
- 能力严格二分 ReadOnly/Mutating,默认 Mutating(fail-closed);commit-tier 细分留 P6。
- `TRPG_PRESENTATION_GATE` 默认 OFF;真阻断硬前置 = P1 buffered 就位;未就位时即便 ON 也退化为只记 trace 不阻断(文字已外流,扣留无意义)。两 flag 正交。

**P3 强化 Knowledge/Policy(可与 P1 并行早做)**
- WorldFact 一等化 = 新建独立 `world_facts` 表(0039,if-not-exists 幂等;同时补漏接的 0038 进 migrate() 数组尾,不改既有号);WorldFact 落 world_facts,memory_facts 仅留三元组。不抽 trpg-world crate。
- WorldFact↔KnowledgeEdge 引用契约本阶段**弱**(孤儿 warn+trace 不阻断),**不加 DB 外键**(存量 player reveal 边引用尚无 world_fact 身份);纯函数 seam 可参数化 enforce,默认 warn,翻强留后续 + 回填后。
- VerifierPrivateView 取 ledger committed 事件 **id 集**(不拉正文流);NPC 子视图 speech+action 都纳入;fail-closed;gm_truth 永不进 PlayerNarrationProjection。
- before-commit / before-narration 两 checkpoint 本阶段纯 advisory(只 trace,不阻断不删块);真阻断/真删留 P2 后。PluginHook 加性新增两 variant。
- NoSpoiler ContextFilter 由 **runtime 投影时**按 SpoilerMeta(parse 产)+ KnowledgeProjection(回合态)现派生标注,不在 parse 产带标注 block(reveal 是回合级动态)。

**P4 World Simulation 层(按需 packet)**
- NpcBehaviorPlan→WorldReactionCandidate 确定性纯映射(additive 方法,不改 derive()):urgency=interaction_desire/100、feasibility=min(willingness,risk_tolerance)/100、risk=risk_tolerance/100;knowledge_basis 只取 `facts_can_reveal`(绝不含 withhold/全集);稳定 candidate_id。
- clock 迁层用 coexist-then-deprecate:World(`world/clock.rs`)成单一源,director `maybe_tick_clocks` 改薄 shim(签名/4 测试逐字节不变);`prepare_actionable_situation` 已证是孤儿(不落库),迁层无既有落账可破。
- NpcActionIntent 回送 Rules **只接 Attack 一条竖切**(复用 `opposed_prepass` 现成对偶,新增 `prepare_npc_attack_binding`,防御键走 `check_param_need` 数据映射);其余 kind 只产草案不结算;World 内禁掷骰/结算。
- WorldDelta **不进 NeedBus**(World 是产候选非取数,NeedBus 是 read-model 总线;塞入会破 BP1/BP2 hash 等价);World 内部取 NPC 卡仍走既有 EntityNeedResolver。
- faction/information_propagation 最小起步:纯类型占位 + 从既有 module graph faction 派生(无则 vec![]),不接回合流、不建表、不引 LLM;显式 TODO。
- 不新增 phase,World 投影内联 `phase_context_assembly`,`render_world_reaction_block` 与 `to_guidance_block` 逐字节相同。

**P5 扩展 Director(故事结构 + DirectorBriefPacket)**
- thread/promise 推进判定语义**整体推迟出 P5**;本阶段 director 纯函数只【读】StoryState 选焦点 + 产最保守确定性 propose 候选(仅明确结构信号 SceneTransitioned/Resolved 才提,不携"已推进"断言),fail-closed 宁漏不错。
- 候选池由 runtime 在 `prepare_director_packet` 装配 = WorldFactCandidate 提案 id ∪ 活跃 scene links;`world_candidate_ids` 作单一 `&[String]` 入参(P4 落地后 runtime 并入即可,director 零改);`retain` 过滤池外 id(§19-#3),交接靠不变量非协调文档。
- 不升第 16 phase;DirectorBriefPacket 经 ContextAssembly 内联 upsert 成 GmOnly+DynamicTail ContextBlock;关停靠现有 `DirectorMode::Disabled`。
- director 全确定性 Rust 零 LLM(beat 从 conflict.intent/frame/scene_plan 推导);LLM situation-planning 留 World+Director 合并点统一引,P5 不开口。
- StoryState 写穿 = propose-only + runtime-owned commit(`apply_story_proposals`);回合内只读,真写穿放 heavy-postprocess/scene-transition;新表 0039 只承载当前无表的故事结构数据,**不收编** spotlight_states/state_frames(读 spotlight 单一源,不复制),不造并行真相源。

**P6 事件溯源成熟化 + 同种子确定性 + 两提交点**
- ResourceChanged 本阶段不带 turn_id(用 world_tick 确定性键,不改 ~6 处签名,透传留 P7);NpcActionResolved 挂现有 `resolve_outcome_with_opposition`(~2312)唯一 NPC 结算点(P4 落地后迁语义拥有者);DialogueSpoken 占位不真 emit,真 emit 在 PresentationCommit。
- 种子公式 = `stable_u64(sha256(session:turn:check:roller_id:expression))`,**不纳入 attempt/retry**(否则破可复现 §19-#9);防御骰保留现有 'defender' 盐;`seed_commitment`/`DiceRollRecord` schema 不变。
- 两提交点本阶段**纯逻辑边界**(事件分组+提交时机),不加 `commit_phase` 列、不开 0038;kind 已编码语义分组。
- PresentationCommit 前置 = "verifier 未阻断(Allow)"即足够,**不**要求念白文本核验钩子(防太严有害;文本核验留 prompt 插件式可选 follow-up)。
- event-fold/replay 放 `trpg-runtime::event_fold`(同 crate,不提前建 kernel/ 布局,P7 纯 rename 整体迁);replay parity 首批只覆盖有 SQL 真值锚的 3 个(PlayerExposure/ContextSurfaced/PlayerKnowledge),其余随 P3/P4/P5 各自落地按需补 impl。

**P7 抽层间 Ports + crate 边界(结构重构,殿后)**
- 控制平面命名 `trpg-gm::TurnConductor`(避开同名异职的 `trpg-orchestrator::TurnOrchestrator`);本阶段 struct 形态留 trpg-gm 内,不独立 crate(除非硬约束)。
- trpg-world crate:迁 World-pure 模块(npc_mind/relationship/behavior/profile + P4 反应);`npc_synth` 留 trpg-runtime(跨 Rules/World,迁走会让 Rules 反依赖 World);module-first + 回桥,耦合过密则停 module 边界写 handoff。
- kernel/(权威状态)vs context/(只读视图)按 §17 判据切;scene_projection 按函数语义拆(导航进 kernel,检定契约视图留 Rules 临近);回桥不可破则不切(保守留原位)。
- PolicyPort 单 trait + `PolicyCheckpoint` enum 入参(镜像现有 PluginHook enum),不拆多 hook trait;conductor 多 checkpoint 回调=横切非第六层。
- 插件 Host 暂留 trpg-gm(不迁 trpg-runtime::plugin / 不建 trpg-policy);判据待 trpg-world 抽出后据可观察耦合面自决,写 handoff。
- 保留 GmLoop 作过渡聚合体,**一次一层一 PR** 渐进换 port(先 KernelPort 最稳→Narrator→Rules→Policy),每切跑 test + 两库 live ≥2 回合断言 §19 不退化;绝不中途一把拆五部件;trpg-gm 收薄壳放最后(前 6 步全绿才做)。

## §6 长跑 · 资源 · 检查点(用户 2026-06-19 已定)

- relauncher 跨会话续命(每轮新 `claude -p` 重武装 ralph);tmux 进程组 kill 收子进程,串行不堆积;连续 N 会话无新提交=卡住自停;`FACTORY_DONE.sentinel` 跨会话判全局完成。
- **§6.1 集成策略 = 滚动 FF-merge 进本地 main(不 push)**。运行进程在专属 worktree/分支 `claude/layered-runtime/autonomous-run`(off main a308526)做 P1→P7 逐阶段 scoped commit,分支始终保持绿+集成态(线性、每阶段都在前阶段提交上推进)。**每阶段 arch_gates 8/8 + 验收 + 受影响 crate test(+大改动 live e2e)全绿并提交后,写 `.claude/autonomous/PHASE_<id>_DONE.sentinel`(一行总结)再进下一阶段;运行进程本身绝不碰 main checkout(不 merge/checkout/reset/commit 到 main)**。集成者(主会话)收到该 phase 信号后把本地 main `merge --ff-only` 推进到分支 tip(**FF-only、不 push、race-free、过一眼已验证产出**)。main 始终健康、不叠未集成债。
- **§6.2 节奏 = 一路跑到 P7 不中停**。每阶段完成发**非阻塞**进度通知(proactive),但**不停**继续下一阶段;只有 §3 硬停项才停。用户随时可介入打断。
- **§6.3 预算/时间上限 = 不设硬上限,以任务完成为止**(几十小时亦可)。安全网仅:连续 N 会话无新提交=卡住自停 + `STOP.sentinel` + §3 硬停项。
