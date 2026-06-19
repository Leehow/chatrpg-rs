# 分层运行时架构 — 实施大计划 (2026-06-19)

> 目标架构见 `design/设计4.md`(含附录 A 现状对照 / B codex 评审 / C 修正)。本计划由 8 个并行分段起草、grounded 进真实代码;序经 grounding+codex 修正为 **行为切分先行、结构重构殿后**。
>
> **主线程纪律(用户 2026-06-19 明确)**:大工程,主线程必须**每阶段先重锚本计划 + TaskList(P0-P7),动手标 in_progress,完成才 completed**,绝不"做一点就忘主线跑偏"。
>
> 推进序:P0 护栏 → **P1 拆 Adjudicator/Narrator + 修流(根因切口,最高优先)** → P2 commit边界+校验可阻断 → P3 Knowledge/Policy(并行早做) → P4 World Sim → P5 Director故事结构 → P6 事件溯源+确定性+两提交点 → P7 Ports+crate边界(殿后)。

---

## P0 P0 守则与护栏(基线)— 把 设计4 §19 转成持续 CI gate,锁定现有行为基线,固化 §14/§17 映射纠正

**目标**:不改任何运行时行为,只立护栏与基线。具体三件事:(1) 把 设计4 §19 的 10 条架构验收逐条转成可在 CI/本地反复跑的 gate——能立即落地的写成可执行测试/脚本,暂不可落地的(replay parity #8、同种子 #9)写成 #[ignore] 占位测试 + 文档化的"红线断言",作为后续 P6 阶段的目标锚;(2) 用一个汇总脚本(scripts/arch_gates.sh)把现有零硬编码守卫 + 关键回归测试 + 新建架构门收口成单一可调用入口,并在 AGENTS.md 登记为"层化迁移期间每次改动必跑";(3) 把 附录A/C 的映射纠正(§14 Ports 尚零落地、§17 trpg-gm 是业务总汇而非纯 Orchestrator、TurnOrchestrator 同名异职、commit 边界=runtime-owned typed services 非大 Kernel、Policy 是横切 hook、五 compiler 暂缓)固化成代码内 ANCHOR 注释 + 一份 docs/architecture/layered-runtime-invariants.md 事实约束清单,让后续每个 phase 的自动 agent 不会基于 设计4 正文(已被附录修正)的过时假设动手。这一阶段产物是"地基绳",P1-P7 都在它约束下施工。

**步骤**:
1. 开专属 worktree 施工(共享 checkout 多 session 抢 HEAD;遵全局 git 纪律):`git worktree add .worktrees/layered-runtime/p0-guardrails -b claude/layered-runtime-p0-guardrails`,本阶段所有改动只落该 worktree;不 push、不动他人 .worktrees/* 与 dirty 文件。
2. 新建 crates/trpg-gm/src/arch_gates_tests.rs(经 #[cfg(test)] #[path] 挂进 lib.rs,沿用仓库 *_tests.rs 约定),作为 §19 验收的 Rust 侧门。先写 6 条**现可落地**的断言:(a) §19-#1/#5 现状基线——断言 run_agent_loop 用的 ToolRegistry::standard()(tools/mod.rs:209)当前含 15 工具且其中 mutation 工具(RollCheckTool/ApplyEffectTool/ChangeTrackTool/RememberTool/RevealFactTool)在册,把这条作为『P1 前 narrator 尚未隔离』的可见基线(P1 会令此断言演化为『Narrator registry 不含 mutation 工具』);(b) §19-#10——断言 DomainEventKind 含 ContextSurfaced/PlayerExposed/PlayerLearnedFact 三分(domain_event.rs:41-48 已在),并断言三者 as_str token 稳定(防误改破坏知识账本幂等键);(c) §19 末段——断言 trpg_model::asset::ExecutionTier 枚举存在且 AssetEnvelope 带 source_refs/visibility/execution_tier 字段(asset.rs:89-98 契约不回退)。
3. 在 arch_gates_tests.rs 追加 4 条**暂不可落地**的 #[ignore="P6: 待 event-fold/replay 引擎"] / #[ignore="P6: 待 seedable 掷骰"] 占位测试,把 §19-#8 与 §19-#9 的目标断言**先写成代码**(replay 同序事件→重建 projection 逐字段相等;同 (state, seed, action)→roll() 返回相同 rolls/total)。这些 ignore 测试是 P6 的『可执行验收锚』:P6 实现时去掉 #[ignore] 即变绿门。测试体内注释引 设计4 §19-#8/#9 与本计划 P6。
4. 把现有两支零硬编码守卫 + 关键回归测试收口成单一可调用门 scripts/arch_gates.sh(set -euo pipefail):顺序跑 ① cargo fmt --check ② bash scripts/no_engine_ruleset_hardcode.sh ③ bash scripts/no_parser_seed_hardcode.sh ④ `cargo test -p trpg-gm --lib arch_gates`(只跑本阶段新门,快)⑤ `cargo test -p trpg-gm turn_plan`(锁 CANONICAL_TURN_PLAN 15-phase)⑥ `cargo test -p trpg-gm prompts`(锁 BP1/BP2 prefix hash 稳定)⑦ `cargo test -p trpg-gm schema`(锁工具 schema 字节稳定)。任一非零退出即 FAIL。脚本顶部注释写明『层化迁移期每次改动必跑,P0 立、后续阶段往里加门』。
5. 新增第三支零硬编码守卫 scripts/no_layer_boundary_violation.sh(对标 no_engine_ruleset_hardcode.sh 的 python-in-bash 结构):扫 trpg-gm/src,禁止后续阶段在 Narrator 路径直接出现状态变更调用——本阶段先把禁字面量集设为占位(仅扫 `apply_damage`/`apply_effect_roll`/直写 KnowledgeEdge 的裸调用模式)且**对当前代码全绿**(因 narrator 尚未拆,现无违例),作为 P1/P2 拆分后『Narrator 无状态工具』(§9.5/§14 禁止项)的预置门;命中→退出码 1。把它加进 arch_gates.sh ⑧。注意:本步只立门不改现有代码,确保对基线 0 命中(先 grep 验证当前 trpg-gm 无裸 apply_damage 调用再定字面量集,避免假阳)。
6. 建 docs/architecture/layered-runtime-invariants.md,把附录A/C 的 6 条映射纠正固化为『事实约束清单』(后续每个 phase 的 spec 与自动 agent 必读):①§14 Ports/独立 Narrator/NarrationPacket/五 ContextCompiler 当前=零落地(rg 证据);②控制平面真身=trpg-gm execute.rs:107 run_pipeline(经 execute_turn 驱动),**不是** trpg-orchestrator::TurnOrchestrator(后者 reduce_turn 干 gate/lifecycle);③§17『trpg-gm 只留 Orchestrator/Narrator』读反了——trpg-gm 现为业务总汇,迁移是把业务**移出**而非确认现状;④commit 边界=只有 runtime-owned typed services 能 commit,Narrator 不能(非新造大 Kernel);⑤Policy=横切 hook(ContextAssembly/AfterLlmStream/HeavyPostprocess 三 checkpoint),非顺序第六运行层;⑥五套 compiler 暂缓——先复用 prepare_turn_context 投分层 packet,验证收益后再拆。每条带 file:line 证据 + 『为何不能按 设计4 正文字面执行』。
7. 在三处关键源码顶部加 // ARCHITECTURE-ANCHOR 注释(只加注释,零行为变更),把上条不变量钉在代码现场,防自动 agent 误读:(a) trpg-orchestrator/src/lib.rs:137 TurnOrchestrator 上方注释『此非 设计4 §4 控制平面;同名异职;控制平面见 trpg-gm execute.rs run_pipeline』;(b) trpg-gm/src/execute.rs:107 run_pipeline 上方注释『这是事实上的 Turn Orchestrator/控制平面(设计4 §4),目标态控制平面概念落于此,勿投到 trpg-orchestrator 同名类型』;(c) trpg-runtime/src/lib.rs:4552 DiceRollerPlugin::roll 上方注释『§19-#9 同种子确定性目标:roll 当前无 seed 入参、用 thread_rng;P6 将加 seed 通道,改签名前先看 arch_gates_tests 的 #[ignore] seedable 测试』。
8. 更新 AGENTS.md『Rust code-affecting work』段(行 118-131):在 cargo 子集命令后追加一行 `bash scripts/arch_gates.sh  # 层化迁移护栏:每次改动必跑(P0 立)`,把汇总门登记为迁移期标准验证步,使后续 worker 的 validation_profile 自然包含它。
9. 本阶段验证:在 worktree 跑 `bash scripts/arch_gates.sh` 全绿(注意 cargo target-dir 重定向陷阱——用 ~/.cache/cargo-target 而非 worktree/target 的陈旧副本);确认 ignore 测试被正确 skip 而非 fail;确认三支守卫脚本对当前基线 0 命中;确认未触碰任何运行时行为(git diff 仅含新增 test 文件/脚本/docs/注释,无逻辑文件的非注释改动)。
10. 做 scoped local commit(commit_policy 默认允许 worker scoped commit;仅 stage 本阶段 write_set:arch_gates_tests.rs、scripts/arch_gates.sh、scripts/no_layer_boundary_violation.sh、docs/architecture/layered-runtime-invariants.md、三处 ANCHOR 注释所在文件、AGENTS.md、lib.rs 的 mod 挂载),commit message 收尾带 Co-Authored-By: Claude。不 push、不合并。在 handoff 报 commit SHA 并确认只含 write_set。

**关键文件**:
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/arch_gates_tests.rs (新建:§19 验收 Rust 门 + #[ignore] P6 锚)`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/lib.rs (加 #[cfg(test)] #[path] mod 挂载 arch_gates_tests)`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/scripts/arch_gates.sh (新建:汇总门单一入口)`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/scripts/no_layer_boundary_violation.sh (新建:Narrator 无状态工具预置门)`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/scripts/no_engine_ruleset_hardcode.sh (现有门,被 arch_gates.sh 收口调用)`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/scripts/no_parser_seed_hardcode.sh (现有门,被收口)`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/docs/architecture/layered-runtime-invariants.md (新建:映射纠正事实约束清单)`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-orchestrator/src/lib.rs (ANCHOR 注释 @137)`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/execute.rs (ANCHOR 注释 @107 run_pipeline)`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-runtime/src/lib.rs (ANCHOR 注释 @4552 DiceRollerPlugin::roll)`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-model/src/domain_event.rs (只读引用:§19-#10 三分事件断言依据)`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-model/src/asset.rs (只读引用:ExecutionTier/AssetEnvelope §19 末段断言依据)`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/AGENTS.md (登记 arch_gates.sh 为迁移期标准验证步)`

**验收**:
- `bash scripts/arch_gates.sh` 在干净 worktree 退出码 0:fmt 通过 + 三支零硬编码守卫 0 命中 + arch_gates/turn_plan/prompts/schema 测试全绿(映射 §19 的可观测门已收口为单一入口)
- arch_gates_tests.rs 含 6 条现可落地断言全绿 + 4 条 #[ignore] 占位测试被 skip(非 fail):对应 §19-#1/#5(narrator 隔离基线)、#8(replay parity 锚)、#9(同种子确定性锚)、#10(PlayerLearnedFact 三分)
- §19-#10 可立即验证:测试断言 DomainEventKind 含 ContextSurfaced/PlayerExposed/PlayerLearnedFact 且 as_str token 稳定(知识账本幂等键不回退)
- §19 末段可立即验证:测试断言 ExecutionTier 枚举 + AssetEnvelope{source_refs,visibility,execution_tier} 契约在册(parser 资产 source/visibility/tier 不回退)
- docs/architecture/layered-runtime-invariants.md 6 条映射纠正全部带 file:line 证据,且三处 ARCHITECTURE-ANCHOR 注释与文档交叉引用一致(后续 phase agent 读任一即得正确事实)
- git diff 证明零运行时行为变更:逻辑文件(execute.rs/lib.rs×2/orchestrator lib.rs)的改动仅为注释行,无非注释逻辑改动;现有绿测试集(turn_plan/prompts/schema/tools)无一回归
- AGENTS.md 已登记 `bash scripts/arch_gates.sh` 为层化迁移期必跑验证步

**不可破坏**:
- CANONICAL_TURN_PLAN 15-phase 结构与 7 个 turn_plan 测试(turn_plan.rs:87-169)——本阶段不碰回合管线
- ToolRegistry::standard() 15 工具的注册顺序与 schema 字节稳定(tools/mod.rs:209,447-527)——schema 字节回归测试是缓存稳定铁律,P0 绝不改工具集
- BP1/BP2 prefix_byte_hash 跨回合字节级稳定(prompts.rs:307-545)——P0 不改 context 装配
- 现有两支零硬编码守卫对当前代码的 0 命中状态——新增 no_layer_boundary_violation.sh 必须对基线同样 0 命中(先 grep 验证再定字面量集,防假阳卡死后续 worker)
- run_agent_loop / verify_after_stream / execute.rs run_pipeline 的现有运行时行为逐字节不变(仅加注释)——P0 是护栏阶段,根因切口留 P1
- DiceRollerPlugin::roll 签名不变(P0 只加注释;改 seed 签名是 P6 的活,提前改会破 ~多处 roll 调用点)

**依赖**:无

**⚠️ 需架构师拍板(不让自动 agent 猜)**:
- 架构师拍板:§19-#1/#5『Narrator 替换不改状态 / Rules 不被文本覆盖』在 P0 只能立『现状基线 + #[ignore] 目标测试』(独立 Narrator 尚不存在)。确认这个『先红线占位、P1 拆分后转绿门』的策略可接受,而非要求 P0 就交可真验的门(那需要先做 P1 的行为切分,违背本阶段『不改行为』范围)。
- crate 边界归属语义:layered-runtime-invariants.md 要把『控制平面=trpg-gm run_pipeline 而非 trpg-orchestrator::TurnOrchestrator』写成事实约束。请确认目标态命名方向——是 (A) 后续把控制平面概念正式建在 trpg-gm 内并给 trpg-orchestrator::TurnOrchestrator 改名消歧,还是 (B) 反过来把控制平面迁回 trpg-orchestrator。design3 教训:crate-boundary 调用是人类决策,不可让自动 agent 猜。本 P0 只记录现状事实,不替你定方向。
- §19-#8 replay parity 与 #9 同种子确定性的『相等』判定语义(逐字段 vs 规范化后相等、骰子 seed 的注入边界是 per-turn 还是 per-roll)是 P6 的实现决策。P0 写 #[ignore] 锚测试时需要一个**占位**断言形状;请确认 P0 仅写最弱形状(存在性/签名)即可,精确的 parity 等价语义留到 P6 spec 由架构师定,不在 P0 锁死。
- no_layer_boundary_violation.sh 的『Narrator 禁止的状态变更调用』黑名单字面量集(apply_damage/apply_effect_roll/直写 KnowledgeEdge…)的精确边界——哪些是真禁、哪些是 runtime-owned typed service 的合法路径(附录C:commit 边界=typed services 可、Narrator 不可)——是提取语义决策。P0 先立对基线 0 命中的最小集,完整黑名单待 P1/P2 拆出 Narrator 路径后由架构师确认。

---

## P1 Adjudicator / Narrator 两层切分 + 修实时流(根治机械朗读)

**目标**:把当前"单 agent 既裁定又叙事 + 边裁定边把念白流给玩家"的结构,沿 codex 与附录 A/B/C 共同认定的根因切口,拆成两层:(1) Adjudicator——沿用现有 agent loop + 15 工具,完成机械裁定/检定/落账,产一个结构化只读 AdjudicationPacket(从 TurnLedger/TurnContext/TurnOutcome 投影,零新存储);(2) Narrator——无工具、无状态写,只接收从 AdjudicationPacket + player-safe 视图投出的 NarrationPacket,产玩家可见散文。关键修复:agent loop 不再把 adjudicator 的 ContentDelta 直接流给玩家(turn_loop.rs:800/915/924),改为 buffer adjudicator prose(供 packet 投影与 verifier 用,不直发),只把 Narrator 的最终输出经 TurnEvent::Delta 流出——让校验能在交付前介入。本阶段以 env flag 守门、默认 OFF==当前逐字节行为,ON 时两 transport(CLI/API)走新 Narrator 流。commit 边界(只有 runtime-owned typed services/工具能 commit、Narrator 不能)在本阶段以"Narrator 无工具"自然落地。不引入 trpg-world / Director 故事结构 / 五套 compiler / Ports——那些是后续 phase。

**现状(grounded)**:单 agent + 实时直流是 load-bearing 的根因,已逐行核实:
- 一个 GmLoop 一个 llm:crates/trpg-gm/src/turn_loop.rs:55-66 `struct GmLoop{ engine, llm: Arc<dyn LlmClient>, tools: ToolRegistry, cfg, ... }`;ToolRegistry::standard() 把 15 个工具(含 mutation: roll_check/request_player_roll/apply_effect/change_track/advance_time/remember/reveal_fact)全给这同一个 agent(tools/mod.rs:209-229)。
- run_agent_loop(turn_loop.rs:665-934)是 CANONICAL_TURN_PLAN 唯一 AgentLoop body(turn_plan.rs:26,45-65)。它在工具轮里边裁定边把 ContentDelta 直接 send 给玩家:StreamEvent::ContentDelta 分支 `let _ = tx.send(TurnEvent::Delta(safe.clone())).await; visible_text.push_str(&safe)`(turn_loop.rs:796-803);轮数耗尽逼散文段同样直发(turn_loop.rs:912-918 与 finish 残窗 922-926);drain_redactor_tx 也直发(turn_loop.rs:848-859,865)。
- 校验只能事后记勘误、收不回已流出念白:verify_after_stream(turn_loop.rs:1436-1506)经 NarrationVerifier.verify(ledger.snapshot, FinalNarrationSubmission) 算 findings → errata 记忆 + absorb_retro_debts(下一回合补账),全是"叙事已流出不可回收,债务进下回合"语义(注释 turn_loop.rs:1466-1488)。phase_verify_after_stream(turn_loop.rs:1811-1835)在 AgentLoop 之后才作为 Postprocess phase 跑(execute.rs:241-245)。
- 单一大 GM prompt:TurnMessages::assemble(prompts.rs:35-80)把 `{compiled.prefix_text}\n\n{gm_skill}` 塞 system、pinned_text 塞 BP2、dynamic_text+尾段塞 BP3——规则原文/模组正文/状态/NPC 信息/errata/义务全在一个 prompt(正是 §11 第一句"不要再用"的形态)。
- 可投影的裁定事实已结构化在手:TurnLedger(ledger.rs:13-75,snapshot 暴露 check_contracts/check_results/effect_contracts/parameter_impacts/committed_patches、private_roll_tokens())、TurnContext.{visible_text,awaiting_gate,ledger,resolved_gate_facts}(turn_loop.rs:164-190)、TurnOutcome(turn_loop.rs:44-51)。NarrationVerifier 已吃 (TurnLedgerSnapshot, FinalNarrationSubmission)(trpg-agent/src/gm_loop.rs:170-186),证明 AdjudicationPacket 天然可投。
- 两 transport 消费 Delta 完全同构,是重路由的唯一接缝:CLI agent_play.rs:139 与 main.rs:1175、API play_turn_sse(trpg-api/src/lib.rs:1475-1521 经 execute_turn 的 TurnEvent::Delta);turn_event.rs:9-42 TurnEvent{Delta/AwaitingPlayerRoll/SceneTransition/PostprocessScheduled/TurnFailed/TurnComplete}。
- 已存在的复用资产:RedactingBuffer(私骰泄漏)、AwaitingPlayerRoll 早退终态(turn_loop.rs:847-857,880-891)、插件 host AfterLlmStream hook(plugin/host.rs)。

**步骤**:
1. 1. 定义 AdjudicationPacket(纯 Rust 结构,新文件 crates/trpg-gm/src/packet.rs,≤400 行)——Adjudicator 的结构化只读产物。字段从现成数据投影、零新存储:player_input;mechanical_outcomes(从 TurnLedger.snapshot() 的 check_results/effect_contracts/parameter_impacts/committed_patches 折成玩家无关的机械事实摘要,含 roll/检定成败/伤害/资源/状态/clock);adjudicator_prose(buffer 下来的 adjudicator 自由文本,供 verifier/fallback,不直发);resolved_gate_facts(复用 ctx.resolved_gate_facts);awaiting(Option<AwaitingPlayerRoll>);referenced_ledger_ids(复用 trpg_agent::ledger_id_set)。给一个 from_turn_context(ctx: &TurnContext, input) -> AdjudicationPacket 投影构造器。不含规则原文/GM-only secret/未来场景/tool JSON。
2. 2. 定义 NarrationPacket(同 packet.rs)——Narrator 唯一输入,是 AdjudicationPacket 的 player-safe 收窄投影。字段对齐 设计4 §9.1 的可落地子集:player_input、what_happened(committed 机械事实的玩家可感知摘要)、what_changed(状态变化摘要)、player_perceivable_facts、style_profile(本阶段从 gm_skill 取一个轻量风格串或留默认)、forbidden_reveals(本阶段复用 verify 路径已采集的 secret_terms/player_known_fact_ids 投影,为空也合法)。给 NarrationPacket::project(adj: &AdjudicationPacket, player_safe_view) -> NarrationPacket;绝不透传 adjudicator_prose 原文中可能含的 GM-only 推理(本阶段先只透传机械事实摘要 + player_input,prose 仅作为 fallback,不喂 Narrator)。
3. 3. 在 GmLoop 上加 Narrator 能力但不给工具:复用现有 self.llm(同一 Arc<dyn LlmClient>,本阶段不引第二模型句柄,降风险),新增 async fn run_narrator(&self, packet: &NarrationPacket, tx, cancel) -> String,用一个专属的最小 narrator system prompt(只含风格 + 输出契约 + 机械事实摘要,无规则原文/无 schema/ToolChoice::None)逐 token 流式产出玩家散文,经 tx.send(TurnEvent::Delta) 实时发。沿用 RedactingBuffer 防私骰泄漏(复用 ledger.private_roll_tokens(),从 packet 透传)。cancel 语义沿用 run_agent_loop 现有 next_stream_event/cancelled 处理(turn_loop.rs:786,902)。
4. 4. 改造 run_agent_loop 让 adjudicator prose 可 buffer 不直发:加一个内部布尔 stream_adjudicator_prose(默认 true=现行为)。当 Narrator 路径开启时置 false——三处 ContentDelta 直发点(turn_loop.rs:800、912-916、922-924)与 drain_redactor_tx(859/865)在 false 时只 push_str 进 visible_text、不 tx.send。AwaitingPlayerRoll 早退(847-891)与 awaiting prompt_public 行为完全不变(掷骰桌 gate 散文本就是确定性、不经 Narrator)。非开启路径逐字节零变更(等价铁律,沿用 P1-3 同款 OFF==ON 证明)。
5. 5. 在 execute.rs run_pipeline 的 AgentLoop body 之后、TurnComplete 之前插入 Narrator 阶段(env flag TRPG_NARRATOR_SPLIT 守门,默认 OFF)。OFF:走现行 run_agent_loop 直流路径,逐字节不变。ON:run_agent_loop 以 buffer 模式跑(adjudicator prose 进 ctx.visible_text 但不发 Delta)→ 用 AdjudicationPacket::from_turn_context 投影 → NarrationPacket::project → gm.run_narrator 流出玩家散文(真正的 TurnEvent::Delta 来源)→ Narrator 输出回写 ctx.visible_text(供 verify/finalize/记忆用同一单一事实源)。AwaitingPlayerRoll 终态短路:不跑 Narrator(已有 prompt_public 即玩家可见文本),与 OFF 一致。
6. 6. 保持 verify_after_stream 语义不变但接缝正确:ON 路径下 phase_verify_after_stream(execute.rs:241-245)校验的是 Narrator 的最终输出(ctx.visible_text 现已是 Narrator 产物),不再是 adjudicator prose——这正是'让校验在交付前/对交付物介入'的兑现。本阶段不把 verify 升级为可阻断(那是 P2);仅确认 findings/errata/retro-debt 对 Narrator 输出同样工作、ledger 仍是机械事实唯一源。
7. 7. 两 transport ON 路径验证:CLI(agent_play.rs:139、main.rs:1175)与 API(play_turn_sse,lib.rs:1475)消费 TurnEvent::Delta 的代码完全不用改——它们只看见 Delta 流,ON 时 Delta 来自 Narrator 而非 adjudicator。确认 SSE channel 容量 128 背压(execute.rs:89)与 keep-alive 不受影响。
8. 8. fail-soft/降级(对齐 §9.6 + §18 Narrator 行):Narrator LLM 失败或 ON 但 packet 投影异常 → fail-soft 回退到 adjudicator_prose 直发(即退化为 OFF 行为),绝不让回合空白或 panic;flag OFF 永远是安全基线。降级路径打 warn + 记 TurnTrace,不入控制流。
9. 9. 测试:① 纯函数级——AdjudicationPacket::from_turn_context 与 NarrationPacket::project 的纯单测(给定 ledger/ctx fixture → 断言机械事实摘要正确、绝无 secret/规则原文字段);② OFF==当前逐字节等价回归(同 prompt 同 ledger,ON=false 时 TurnEvent::Delta 序列与现状字节一致,沿用 prefix_byte_hash 思路对 Delta 拼接做 hash 等价);③ ON 路径:断言 adjudicator ContentDelta 不出现在 tx,只有 Narrator 输出出现在 tx;④ Narrator 无工具(schemas 为空/ToolChoice::None)断言;⑤ AwaitingPlayerRoll 在 ON/OFF 两路径行为一致。最后跑一轮真库 live e2e(CoC :54347 / Cyberpunk :54346)走遍 OFF 与 ON 两路径,确认 ON 输出更像散文、机械状态(HP/检定/场景)两路径一致(对齐 §19-#1/#5)。

**关键文件**:
- `crates/trpg-gm/src/turn_loop.rs`
- `crates/trpg-gm/src/execute.rs`
- `crates/trpg-gm/src/turn_plan.rs`
- `crates/trpg-gm/src/tools/mod.rs`
- `crates/trpg-gm/src/prompts.rs`
- `crates/trpg-gm/src/ledger.rs`
- `crates/trpg-gm/src/turn_event.rs`
- `crates/trpg-agent/src/gm_loop.rs`
- `crates/trpg-cli/src/agent_play.rs`
- `crates/trpg-cli/src/main.rs`
- `crates/trpg-api/src/lib.rs`
- `crates/trpg-gm/src/packet.rs (新增)`

**验收**:
- §19-#1(替换 Narrator 模型不应改变 HP/场景/知识/NPC 状态):ON 路径下换 Narrator prompt/模型,机械状态 = ledger 投影,断言两次不同 Narrator 产出对应同一 committed ledger/HP/scene。
- §19-#5(Rules 结果不能被 Narrator 文本覆盖):Narrator 无状态写工具(schemas 空/ToolChoice::None),所有机械事实只来自 AdjudicationPacket 的 ledger 投影,Narrator 文本无法改 HP/检定/资源。
- 根因切口验证(附录 A/B-#2):ON 路径下 adjudicator 的 ContentDelta 不再经 TurnEvent::Delta 流给玩家——测试断言 tx 中只出现 Narrator 输出;校验对'最终交付给玩家的文本'介入,而非事后对已流出 adjudicator prose 记勘误。
- 等价铁律:flag OFF(默认)逐字节零行为变更——TurnEvent::Delta 序列与改造前完全一致(Delta 拼接 hash 等价回归 + 现有 turn_loop_tests/execute_tests 全绿)。
- 两 transport(CLI agent_play/main、API play_turn_sse)无需修改即在 ON 路径正确流出 Narrator 散文;AwaitingPlayerRoll 早退在 ON/OFF 两路径行为一致。
- fail-soft:Narrator 失败 → 回退 adjudicator_prose 直发(退化为 OFF),回合不空白、不 panic,降级打 warn + 记 TurnTrace。
- 真库 live e2e 双路径通过(CoC :54347 / Cyberpunk :54346):ON 输出更像散文、机械状态两路径一致。

**不可破坏**:
- CANONICAL_TURN_PLAN 15 phase 顺序与 turn_plan.rs 全部纯函数测试(canonical_plan_has_fifteen_phases / agent_loop_is_the_single_middle_body 等)——AgentLoop 仍是唯一 body,Narrator 作为 ON 路径在 body 内或紧随,不新增 PhaseId 破坏现有计数测试(若需新 phase 须同步改全部 plan 顺序断言,本阶段倾向不加 phase)。
- OFF 默认路径与所有缓存稳定性回归:prompts.rs 的 assemble_order_is_stable/prefix_byte_hash、tools/mod.rs schema_serialization_is_stable 与 15 工具 schema 字节/顺序(ToolRegistry::standard 注册顺序绝不动)。
- AwaitingPlayerRoll 早退终态语义(turn_loop.rs:847-891)与 select_phases 的'AwaitingPlayerRoll 只保留 VerifyAfterStream+Finalize'契约(execute.rs:57-60)。
- 私骰泄漏防护:RedactingBuffer/private_roll_tokens 在 Narrator 流路径同样生效(UX 红线:私骰泄漏是唯一不可后置勘误项)。
- verify_after_stream/errata/absorb_retro_debts/ObligationLedger carryover 现有行为(turn_loop.rs:1436-1506、execute.rs should_run_carryover)——本阶段不改其语义,只改它校验的输入来源(ON 时=Narrator 输出)。
- P1-3 取消语义(cancel token 在 LLM 流边界 break、不持久化半截回合,execute.rs:208-226)——Narrator 流必须沿用同款 cancel 处理。
- ctx.visible_text 作为 assistant_output / 记忆 / finalize 的单一事实源(heavy_assistant_output 派生,turn_loop.rs:221-226):ON 路径 Narrator 输出必须正确回写它,否则记忆/审计/save_turn 读到空。
- 两 transport 现有 TurnEvent 消费(CLI/API)与 SSE channel 容量/keep-alive。

**依赖**:P0

**⚠️ 需架构师拍板(不让自动 agent 猜)**:
- NarrationPacket 的 player-safe 投影 SEMANTICS:哪些 adjudicator 机械事实/推理算'玩家可感知'、哪些必须收窄(forbidden_reveals 的来源与默认严格度)。design3 教训=投影/抽取语义是人类决策,不能让 auto-agent 猜——尤其'adjudicator_prose 是否完全不喂 Narrator(只喂机械事实摘要)还是允许透传部分玩家可见叙事意图'需架构师拍板。
- Narrator 是否复用同一 self.llm 还是引第二个 LLM 句柄/模型:本阶段建议复用(降风险、省改动),但若架构师要 Narrator 走更强叙事模型(对齐 §12 narrator_prefix_hash 独立 cache key 的长期目标),是 crate/装配边界决策,需拍板。
- 是否新增一个 PhaseId(如 NarratorStream)进 CANONICAL_TURN_PLAN,还是把 Narrator 作为 AgentLoop body 内的 ON 分支:前者更显式但要改全部 plan 顺序断言与 select_phases,后者改动小但 plan 可观测性弱——这是回合管线结构决策,需架构师定。
- env flag 命名与默认/灰度策略(本计划用 TRPG_NARRATOR_SPLIT 默认 OFF):何时把默认翻 ON、是否按 ruleset/module 分段灰度,属产品/发布节奏决策。
- Narrator 的最小 system prompt 内容与风格契约(StyleProfile 来源):放 data/agent 下成可组合 prompt 插件(对接已拍板的提示词插件系统),还是先内联——属 prompt 资产归属决策。
- commit 边界在本阶段以'Narrator 无工具'落地,但附录 C 的'只有 runtime-owned typed services 能 commit'若要在本阶段就显式标注(而非留 P2),需架构师确认范围,避免与 P2'commit 边界 + 校验可阻断'职责重叠。

---

## P2 Commit 边界显式标注 + verify_after_stream 升级为可阻断校验

**目标**:把"谁能 commit"从隐性约定变成显式、可机读、可测试的能力标注:在现有 15 个 runtime-owned typed GM 工具上标注 mutation/commit 能力(对应 设计4 附录C-#2 "只有 runtime-owned typed services 能 commit,Narrator 不能"),并据此派生一个 Narrator-safe 工具子集(移除全部 mutation 工具,最多保留只读 + 新增的无状态 ask_clarification)。同时把 verify_after_stream 从"叙事流出后只记勘误/落债"升级成一个在 PresentationCommit 前能对 Blocker 级 finding(剧透 SecretLeak / 机械编造 InventedEffect / 越权 ManualRollRequest)阻断或要求修复的 gate。本阶段是 P1(独立 Narrator + 收口实时流,让文字在校验前不外流)的能力底座与安全闸,但本阶段自身保持 additive:不动 P1 尚未落地时的默认行为路径,只新增标注 + 新增 gate 判定 + feature-gate 切换。

**现状(grounded)**:ROOT CAUSE 印证(附录B-#2):run_agent_loop(crates/trpg-gm/src/turn_loop.rs:665) 在工具轮内把 LLM 的 ContentDelta 直接 `tx.send(TurnEvent::Delta(...))` 流给玩家(turn_loop.rs:800),而 verify_after_stream 作为独立 Postprocess phase VerifyAfterStream 在 AgentLoop 之后才跑(turn_plan.rs:60 / execute.rs:244 phase_verify_after_stream)——物理上无法在文字外流前拦截。当前 verify_after_stream(turn_loop.rs:1436) 拿 NarrationVerifier(trpg-agent/src/gm_loop.rs:179) verify 出 NarrationVerifierResult{accepted, findings, next_required_action}(gm_loop.rs:315);findings 有 severity=Blocker/Warning(gm_loop.rs:351)和 kind 含 SecretLeak/InventedEffect/MissingCheck/ManualRollRequest 等(gm_loop.rs:341),next_required_action 已能给出 ReviseText/CallApplyEffect 等(gm_loop.rs:360),但调用方完全忽略 accepted/severity——只把 findings 折成 PluginContributionTrace(turn_loop.rs:1492)、转 RetroactiveEffectDebt 入下回合 obligations(turn_loop.rs:1469-1487)、落 errata MemoryEvent(turn_loop.rs:1461)。即"可阻断"的数据结构已存在,但无阻断接线。工具侧:ToolRegistry::standard() 注册全部 15 个工具(tools/mod.rs:209)无能力分级;经核对哪些 mutate——roll_check(check.rs:669 settle_system_check + check.rs:765 insert_check_contract/insert_pending_check/insert_interaction_gate)、request_player_roll(同表写)、apply_effect(effect.rs:128 RefereeCombatService + 143 ledger.record_effect)、change_track(effect.rs:174 engine.apply_track_change)、navigate_scene(world.rs:299 set_session_scene)、advance_time(world.rs RefereeCombatService)、remember(world.rs:237 save_memory_event)、reveal_fact(world.rs:413 engine.reveal_fact)、ensure_npc_param(npc.rs 经 engine 写 sheet_json)、waive_obligation(mechanic.rs:196 update_mechanic_due_status + 209 save_memory_event)、enter_mode/exit_mode(mode.rs 改 frame 状态)=共 12 个有副作用;纯只读仅 retrieve_rules(world.rs:84)、get_actor(npc.rs:27)、lookup_mechanic(mechanic.rs:48)=3 个。GmTool trait(tools/mod.rs:91) 只有 spec()+call(),无能力位。无 ask_clarification 工具(grep 零命中)。插件 host(plugin/host.rs)是 propose-not-commit、hook 只 ContextAssembly/AfterLlmStream/HeavyPostprocess(plugin/types.rs:18),无 PreCommit/PresentationCommit checkpoint。

**步骤**:
1. 步骤1(能力标注,additive):在 GmTool trait(crates/trpg-gm/src/tools/mod.rs:91)上新增带默认实现的 `fn capability(&self) -> ToolCapability { ToolCapability::Mutating }`(默认 Mutating = fail-closed:未显式声明只读的工具一律按可变,杜绝漏标。)。在 mod.rs 同文件定义 `pub enum ToolCapability { ReadOnly, Mutating }`。仅对已核实零副作用的三个工具 override 为 ReadOnly:world::RetrieveRulesTool(world.rs:84)、npc::GetActorTool(npc.rs:27)、mechanic::LookupMechanicTool(mechanic.rs:48)。其余 12 个保持默认 Mutating,无需改它们的代码。
2. 步骤2(守卫测试先行,TDD):新增 mod.rs 内 `#[test]` 断言:standard() 注册的 15 个工具里恰好 3 个 ReadOnly、12 个 Mutating,并逐名锁定 ReadOnly 集合={retrieve_rules,get_actor,lookup_mechanic}。这是防回归闸——日后新增 mutation 工具若误标 ReadOnly 立即红。再加一个测试:对 12 个 Mutating 工具逐一断言其 name 在 ReadOnly 白名单之外(用 name 字符串比对,不跑 DB)。
3. 步骤3(派生 Narrator-safe 工具集,additive):在 ToolRegistry(mod.rs:197)新增 `pub fn narrator_safe(&self) -> Self`——过滤掉所有 capability()==Mutating 的工具,只留 ReadOnly 子集(对应 设计4 §9.5 Narrator 无状态工具)。注:本阶段产出的是 *能力*,Narrator 是否真用此 registry 是 P1 的接线(execute_turn 拆 Adjudicator/Narrator 时调 narrator_safe());本阶段只让该方法存在 + 被测试覆盖,不改 run_agent_loop 现行调用(仍 self.tools / mode_tools,见 turn_loop.rs:693)。
4. 步骤4(新增无状态 ask_clarification 工具,additive):在 tools/world.rs(或新 tools/clarify.rs ≤80 行)新增 `AskClarificationTool`,capability()=ReadOnly,call() 不碰 DB/ledger——只把 question 透传回 ToolOutput::ok(json!({"clarification":question}))(对应 §9.5 "最多保留 ask_clarification")。**不**把它加进 standard()(避免污染现 Adjudicator 的 15 工具 schema 字节稳定性,schema_stability_tests 见 mod.rs:447),只让 narrator_safe() 在过滤后追加它。给该工具加单测:call() 返回结构正确且无副作用。
5. 步骤5(校验结果分级抽纯函数,TDD):在 verify_after_stream(turn_loop.rs:1436)算出 NarrationVerifierResult 后(turn_loop.rs:1460 `let result = verifier.verify(...)`),新增一个纯函数 `fn presentation_gate_decision(result: &NarrationVerifierResult) -> PresentationGate`(放 trpg-gm 内,签名只吃已有类型),把 findings 按 severity+kind 折成 `enum PresentationGate { Allow, Block(Vec<VerifierFinding>) }`:仅当存在 severity==Blocker 且 kind∈{SecretLeak,InventedEffect,ManualRollRequest} 时 Block,其余(Warning / MissingCheck 等只补债的)→ Allow(保持现有 errata+RetroDebt 不变。**人决:确切阻断 kind 白名单见 human_decisions。**)。对此纯函数写穷举测试(每个 kind×severity 组合)。
6. 步骤6(把 gate 挂到 phase 上但默认旁路,additive + feature-gate):verify_after_stream 在算出 result 后,额外算 `let gate = presentation_gate_decision(&result);`,把 gate 结果写进 ctx(新增 ctx 字段 `presentation_gate: PresentationGate`,默认 Allow)供 execute.rs 的 PresentationCommit 阶段读。**关键 additive 约束**:本阶段 *不* 让 Block 真的扣回已 send 的 Delta(那要 P1 先收口实时流,否则文字已外流无法回收)。在 env flag `TRPG_PRESENTATION_GATE`(默认 off)下:off ⇒ 仅把 gate 决策记入 Flight Recorder / plugin trace(观测,零行为变更);on 且 P1 buffered-narration 已就位 ⇒ Block 时令 PresentationCommit phase 跳过对玩家提交 Presentation 事件并触发 repair。flag off 时逐字节零行为变更(对齐 R1/R2 等价铁证手法)。
7. 步骤7(PresentationCommit phase 占位,与 §13 对齐):在 turn_plan.rs CANONICAL_TURN_PLAN 不改变现有 15 phase 的前提下,在 execute.rs 的 VerifyAfterStream 处理(execute.rs:244 附近)读取 ctx.presentation_gate:flag off ⇒ 只 emit 一条 TurnEvent/trace 表明 gate 决策(Allow/Block + findings 摘要),不阻断;为 P1 预留 Block 分支(TODO 注释引用 §13 PresentationCommit + 本 phase_id)。本步不新增 phase 枚举值(那是 P1/后期结构重构),避免破坏 turn_plan 的 15-phase 守卫测试(turn_plan.rs:87 canonical_plan_has_fifteen_phases)。
8. 步骤8(可观测性接线):在 verify_after_stream 现有 PluginContributionTrace 折叠(turn_loop.rs:1492)旁,把 presentation_gate_decision 的 Allow/Block + 命中 kind 一并记入 trace(复用 surface_verifier_finding_trace 同款路径,turn_loop.rs:1495),使 `trpg explain --plugins`(对应 设计4 §20 第六阶段 explain-turn)能显示 "Policy 本回合是否会阻断、阻断了哪些 finding"。零行为变更(仅多记一条 advisory trace)。
9. 步骤9(验证):跑 `cargo test -p trpg-gm`(工具能力/守卫/narrator_safe/gate 纯函数)+ `cargo test -p trpg-agent`(NarrationVerifier 现有测试不回归)。再跑一轮真库 live e2e(CoC :54347 + Cyberpunk :54346,按 MEMORY 大重构必跑 e2e 铁律):flag off 下确认逐回合 Delta 字节、HP/scene/知识状态与基线完全一致(对应 §19-#1/#5/#7);手工注入一条含 secret_terms 的模组会话,确认 gate 在 flag off 下 *记录* 了 Block 决策但未改变交付(证明闸已就位、待 P1 收口实时流后翻 on)。

**关键文件**:
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/tools/mod.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/tools/world.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/tools/effect.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/tools/check.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/tools/npc.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/tools/mechanic.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/turn_loop.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/execute.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/turn_plan.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-agent/src/gm_loop.rs`

**验收**:
- §19-#5 印证(Rules 结果不能被 Narrator 文本覆盖):narrator_safe() registry 不含任何 capability()==Mutating 的工具;守卫测试断言 Narrator 工具集 ∩ {roll_check,request_player_roll,apply_effect,change_track,navigate_scene,advance_time,remember,reveal_fact,ensure_npc_param,waive_obligation,enter_mode,exit_mode} = ∅。
- §19-#6 印证(Narrator 不得泄露未来场景秘密)的执行底座:presentation_gate_decision 对 SecretLeak Blocker finding 返回 Block;穷举纯函数测试覆盖(SecretLeak,Blocker)→Block。
- §19-#7 印证(Policy 阻断输出时不破坏已提交机械状态):flag off 路径下 gate=Block 仅记 trace,不回滚任何 ledger/DB;live e2e 证明注入 secret 的会话其 HP/scene/check 落账与基线逐字段一致。
- §19-#1 印证(替换 Narrator 模型不改机械状态)的前提:工具能力标注使 Narrator 物理上无 mutation 工具可调,机械状态只能来自 Adjudicator 已 commit 的 ledger。
- 能力标注守卫测试绿:standard() 恰好 3 ReadOnly + 12 Mutating,ReadOnly 集合锁定={retrieve_rules,get_actor,lookup_mechanic};新增 mutation 工具误标 ReadOnly 会红。
- additive 等价铁证:TRPG_PRESENTATION_GATE=off 时,两 transport(CLI/API)逐回合 Delta 字节 + 终态 + 落账 hash 与 P2 前基线完全等价(沿用 R1/R2 BP1/BP2 hash 等价手法)。
- schema 稳定性不破:ToolRegistry::standard().schemas() 字节与顺序不变(mod.rs:447 schema_serialization_is_stable / mode_switch_changes_tool_schema_bytes 仍绿;ask_clarification 不进 standard())。
- cargo test -p trpg-gm 与 -p trpg-agent 全绿,无既有测试回归。

**不可破坏**:
- ToolRegistry::standard() 的 15 工具注册顺序与每个工具的 OpenAI function schema 字节绝不变(mod.rs:202 注释明令 + schema_stability_tests mod.rs:441 守卫;缓存稳定依赖它)。
- CANONICAL_TURN_PLAN 仍是 15 phase、顺序不变(turn_plan.rs:45;canonical_plan_has_fifteen_phases / plan_order_matches_phase_id_declaration 测试)。本阶段不新增 phase 枚举值。
- 现有 verify_after_stream 的 errata 落账(turn_loop.rs:1461)+ RetroactiveEffectDebt 入 obligations(turn_loop.rs:1469-1487)+ AfterLlmStream hook(turn_loop.rs:1502)行为不变——gate 只新增旁路决策,不替换这些路径。
- request_player_roll 桌面骰权 gate 终态语义(ToolOutput::awaiting / AwaitingPlayerRoll,mod.rs:74)不受能力标注影响:它是 Mutating,但早退路径(turn_loop.rs:847 break 'rounds)照常。
- redactor 私骰泄漏防护(turn_loop.rs:785 RedactingBuffer)与 drain_redactor_tx 路径不动。
- TRPG_PRESENTATION_GATE 默认 off 时整条回合路径逐字节等价(不得因新增 gate 计算改变任何对玩家可见输出或落账)。

**依赖**:P1

**⚠️ 需架构师拍板(不让自动 agent 猜)**:
- 阻断 kind 白名单(设计3 教训:enforcement 语义是人决,非 auto-agent 可猜):确认 PresentationGate::Block 只触发于 {SecretLeak, InventedEffect, ManualRollRequest} 且 severity==Blocker 吗?MissingCheck/MissingRollExecution/MissingEffect/OmittedVisibleResult 目前走 RetroDebt 补账(下回合处理),是否维持 Allow+补债、还是也升级为可阻断要求当回合 repair?(过严会拦正常叙事,呼应 MEMORY '防剧透别太严'。)
- Block 后的修复语义(待 P1 收口实时流后才真正可执行):Block 时是 (a) 整段重生 narration(再调一次 Narrator,带 forbidden_reveals 约束),还是 (b) 确定性 fallback narration(设计4 §9.6/§18 用 committed facts 模板化),还是 (c) 仅扣留该 Presentation 事件、把已流出文字标记勘误?三者对玩家体验与 token 成本权衡不同,需架构师拍板。
- ask_clarification 是否应让 Adjudicator 也能用(纳入 standard()),还是严格只给 Narrator?纳入 standard() 会改 schema 字节(破缓存稳定),需评估收益;本计划默认只给 narrator_safe()。
- TRPG_PRESENTATION_GATE 默认值与翻 on 的前置条件:确认 'P1 buffered-narration(文字在校验前不外流)落地' 是翻 on 的硬前置;在 P1 未完成前 on 是否允许(只会记录不阻断)还是 flag 直接拒绝开启。
- 能力分级是否需要比 ReadOnly/Mutating 更细(如区分 'commit 世界事实' vs 'commit 表现事实',对应 §13 两提交点 ResolutionCommit/PresentationCommit)——本阶段先二分,是否够用由架构师确认,以免提前过度建模。

---

## P3 Knowledge / Policy 强化:WorldFact 一等化 + VerifierPrivateView 类型化 + Policy 横切多 checkpoint

**目标**:把已有的"半成品"知识/策略边界从隐式、零散、advisory 的状态强化成显式、类型化、可被 §19 验收的边界,不引入新层、不改回合行为契约。三条主线:(1) 让 WorldFact 成为一等实体——给世界事实身份建独立 world_facts 存储并与 KnowledgeEdge.fact_id 建立显式引用契约,使 gm_truth_view / player_knowledge / NPC 投影都指向同一个事实身份源(呼应设计3 收尾的 R4-D2"WorldFact 是否独立成表"决策);(2) 把今天分散在 NarrationVerifier(只见 ledger)+ project_for_npc_speech + verify_player_narration_leak 的校验输入,收敛成一个类型化、fail-closed 的 VerifierPrivateView(完整秘密 / 玩家知识 / 每 NPC 知识 / committed 事件),它只喂校验器、绝不进玩家可见 prompt(§11.5);(3) 把 Policy 从今天只有 ContextAssembly(before-context)+ AfterLlmStream(after-narration)两个真实接线点,形式化成横切多 checkpoint hook,补 before-commit / before-narration 两个枚举点位与最小接线骨架,并明确 NoSpoiler 完全经 KnowledgeProjection、NPC speech 完全经 NpcKnowledgeView(二者已大体落地,本阶段补缺口 + 加守卫测试钉死)。全程 additive、零规则集硬编码、fail-closed,可与 P1(Adjudicator/Narrator 拆分)并行,不依赖其落地。

**现状(grounded)**:Knowledge runtime 已相当厚但 WorldFact 未一等化:WorldFactCandidate 提案存在(trpg-model/src/memory_proposal.rs:170),但 review_and_commit_proposals 把它落进 memory_facts 表(打 tag "world_fact",memory_proposal.rs:324-355,CommitAction::WorldFact→memory_facts.upsert),既无独立 world_facts 表也不自动建 GM-holder knows_true 边——所以 gm_truth_view(trpg-db/src/lib.rs:3570,只查 knowledge_edges holder='gm')反映不出已提交的世界事实,WorldFact 身份与"谁知道"靠裸 fact_id 字符串隐式对齐。knowledge_edges.fact_id 是无 FK 的 text 列(migrations/0032_knowledge_edges.sql:9,无 REFERENCES),KnowledgeUpdateCandidate.fact_id 注释明说"identity ref, not the fact"(memory_proposal.rs:218-222)但无任何引用契约校验。
KnowledgeProjection 已有四个 typed viewer/speaker 投影(trpg-runtime/src/knowledge_projection.rs:48-266:PlayerNarrationProjection / GmAdjudicationProjection / NpcSpeechProjection / NpcActionProjection,结构上把 GM 隐藏真相与玩家已知集分离,gm_truth_fact_ids 永不复用为玩家可见集)。但 Verifier 侧无统一 VerifierPrivateView:NarrationVerifier 只见 ledger(turn_loop.rs:1460);泄漏校验分三处独立装载(verify_after_stream→run_after_llm_stream_hook 取 player_known_fact_ids+harvest_session_secret_terms,turn_loop.rs:1525-1551;npc_consistency_after_stream 逐 NPC project_for_npc_speech,turn_loop.rs:1622-1670;NoSpoilerGuard.secret_leak_findings 在 builtin_no_spoiler.rs:178)。三者各自取数、无单一 fail-closed 私有视图、不含 GM 完整秘密(§11.5 要求 Verifier 可看完整秘密但该视图不得进玩家 prompt)。
Policy 已是 propose-not-commit 横切 host(plugin/host.rs:run_hook 只收集+排序)但 hook 只 3 个枚举(plugin/types.rs:18 PluginHook = ContextAssembly / AfterLlmStream / HeavyPostprocess),真实接线只两点:ContextAssembly(turn_loop.rs:1309 before-context,产 PromptBlock+ContextFilter)、AfterLlmStream(turn_loop.rs:1538 after-narration,产 SecretLeak finding)。§10.2 的 before-commit / before-narration 两个 checkpoint 无对应枚举、无接线。NoSpoiler 已半经 KnowledgeProjection:ContextAssembly 侧用 player_known_fact_ids 放行已揭示块(builtin_no_spoiler.rs:107 should_drop),runtime spoiler_guard.rs 用 revealed 账本做源实体级裁剪;但块级 ContextFilter 依赖 block 产出方打 tag(spoiler_secret/future_scene/fact:<id>),注释承认"生产里恒 no-op"(builtin_no_spoiler.rs:44-46),即 NoSpoiler 的块级裁剪未真正经 KnowledgeProjection 驱动。NPC speech 一致性已真经 NpcSpeechProjection(turn_loop.rs:1649),NpcKnowledgeView 路径健康。migrations 最新 0038(但 0038 文件存在却未进 trpg-db/src/lib.rs:51-86 的 migrate() include 数组,是预存在缺口);下一可用迁移号 0039。

**步骤**:
1. 步骤1(架构师决策前置 — 见 human_decisions #1):锁定 WorldFact 一等化的存储形态。两选项:(A) 新建独立 world_facts 表(fact_id PK + subject/predicate/object/summary/truth_status/source_event_ids/turn_id/confidence),WorldFact 提案改落此表,memory_facts 保留为记忆三元组用途;(B) 保留 memory_facts 作为物理存储但加一个 world_facts 视图/逻辑契约。本步只产出决策记录(写进 plan 的 decision ledger),不动代码。默认倾向 (A) 独立表(呼应 R4-D2,使 WorldFact≠记忆三元组),但这是 crate-boundary + 数据语义决策,必须架构师拍板。
2. 步骤2(WorldFact 存储,additive):按步骤1 决策落地。若 (A):新增 migrations/0039_world_facts.sql 建 world_facts 表(列同 WorldFactCandidate 字段 + created_at/updated_at,fact_id PRIMARY KEY,session_id NOT NULL,索引 (session_id))。务必同时把 0039 加进 trpg-db/src/lib.rs migrate() 的 include_str! 数组(并顺手把已存在却漏接的 0038_memory_fact_truth_status.sql 一并补进数组——见 must_not_break,这是预存在缺口,补它是 additive 修复)。新增 trpg-db Db::upsert_world_fact + load_world_fact(session_id, fact_id),与既有 upsert_memory_fact 同风格(on conflict (fact_id) do update)。
3. 步骤3(WorldFact 提交路径改道,行为等价优先):在 trpg-runtime/src/memory_proposal.rs 的 CommitAction::WorldFact 分支(:324-355)改为写 world_facts 表(经步骤2 的 Db::upsert_world_fact)而非 memory_facts.upsert;CommitAction::describe()(:232)与 dry-run trace 同步更新为 'world_facts.upsert(world_fact)'。保持 fact_id / source_event_ids / turn_id / truth_status 全透传不变。为防回归:先加一个 dry-run 等价测试(CommitContext 走 dry-run 路径,断言 outcome.committed_ref == fact_id 不变),再切实写库。
4. 步骤4(WorldFact↔KnowledgeEdge 引用契约,fail-closed,需 human_decisions #2 决定强度):在 trpg-model 新增一个纯校验 seam(如 fn world_fact_ref_is_resolvable 或在 KnowledgeUpdateCandidate::validated 内),约束 KnowledgeUpdate 的 fact_id 必须指向一个已知 world fact 身份。强度二选一由架构师定:(弱)只在 runtime commit 时若 world_facts 查无此 fact_id 则 warn+trace 不阻断;(强)fail-closed 拒绝提交孤儿知识边。本步只实现 model 层纯函数 + runtime commit 处接线,DB 层是否加 FK(knowledge_edges.fact_id REFERENCES world_facts)留步骤2 迁移按决策决定(加 FK 是破坏性约束,务必架构师拍板)。
5. 步骤5(GM 真相边自动派生 — 让 WorldFact 进 gm_truth_view):在 review_and_commit_proposals 的 WorldFact 提交成功后,additive 地为该 fact 写一条 (holder='gm', knows_true) KnowledgeEdge(经既有 Db::upsert_knowledge_edge,holder_id=''),使新提交的世界事实立即出现在 gm_truth_view(trpg-db/src/lib.rs:3570)与 GmAdjudicationProjection.gm_truth_fact_ids(knowledge_projection.rs:84)。语义:世界里发生了=GM 确知,但玩家未知(不写 player_party 边)——精确对齐 §13'世界里发生了≠玩家知道了'。加守卫测试:提交一个 WorldFact 后 gm_truth_view 含其 fact_id、player_knowledge_view 不含。
6. 步骤6(VerifierPrivateView 类型化 — §11.5):在 trpg-runtime/src/knowledge_projection.rs(或新 verifier_view.rs)新增 pub struct VerifierPrivateView,聚合既有四投影:gm_truth_fact_ids(完整秘密,来自 GmAdjudicationProjection)+ player_known_fact_ids + 每活动 NPC 的 NpcSpeechProjection/NpcActionProjection + 本回合 committed 事件 id 集(ledger 已有)。加一个 async 装载器 build_verifier_private_view(db, session_id, active_npc_ids, player_known) 复用 project_for_gm_adjudication / project_for_npc_speech,fail-closed(任一 DB 抖动→该子集空=按最严处理)。关键不变量(加测试钉死):VerifierPrivateView 不实现/不经任何 prompt 渲染路径,且 gm_truth_fact_ids 永不被投进 PlayerNarrationProjection(复用既有 hidden_truth_fact_ids 分离语义,knowledge_projection.rs:107)。
7. 步骤7(把 verify_after_stream 改用 VerifierPrivateView,行为等价):重构 turn_loop.rs run_after_llm_stream_hook(:1512)+ npc_consistency_after_stream(:1622),让它们从单一 build_verifier_private_view 取数,替换今天三处独立装载(player_known_fact_ids / 逐 NPC project_for_npc_speech / secret_terms 仍由 harvest_session_secret_terms 提供——secret_terms 是模组图谱来源,与 VerifierPrivateView 正交,保留)。要求:重构后 findings 集合逐条等价(先快照现有 e2e 一回合的 findings,重构后 diff 为空才算过)。这是纯收敛,不新增检查、不改 finding 语义。
8. 步骤8(Policy 横切 checkpoint 形式化 — §10.2):在 plugin/types.rs PluginHook 枚举(:18)additive 新增 BeforeCommit、BeforeNarration 两个 variant(as_str 同步:'before_commit'/'before_narration'),并把现有 ContextAssembly 在文档/注释上对齐为 before-context、AfterLlmStream 对齐为 after-narration(§10.2 四阶段命名收口,不改既有 token 以免破坏序列化)。PluginHost::run_hook 无需改(已 hook-agnostic)。本步只扩枚举 + 文档,使五个内置 policy 插件(plugin/mod.rs:50-56)可声明这两个新 checkpoint。
9. 步骤9(BeforeCommit / BeforeNarration 最小接线骨架,零行为变更):在 turn_loop 回合流的两个自然位点加 host.run_hook 调用——before-commit 点(机械结果落账前,P2 标注 commit 边界后此处更清晰;本阶段先在结算后/写穿前插一个 advisory-only 调用)、before-narration 点(组装 NarrationPacket / 流前)。两点都先做成 advisory:收集 VerifierFinding/ContextFilter 贡献→只记 Flight Recorder trace(复用 to_trace,plugin/types.rs:198),不阻断、不删块。这给 §10.2 四 checkpoint 一个真实(虽 advisory)的接线,后续阶段(P2 校验可阻断)再让 before-commit 的 Deny 真正阻断。加测试:注册一个假插件声明 BeforeCommit,断言回合流确实以该 hook 调用它并把 trace 记进 TurnTrace。
10. 步骤10(NoSpoiler 完全经 KnowledgeProjection — 补块级裁剪缺口):针对 builtin_no_spoiler.rs:44-46 承认的'生产恒 no-op',让 NoSpoiler 的 ContextAssembly ContextFilter 真正由 KnowledgeProjection 驱动:在 turn_loop.rs ContextAssembly 接线处(:1269 private_blocks 构造,已从模组图谱 SpoilerMeta 派生)确保每个含 secret 的私有 block 都带 fact_id 标注且 player_known_fact_ids 来自 player_knowledge_projection——使 should_drop(builtin_no_spoiler.rs:107)的'已 player-known 放行 / 未知 secret 删'真正生效而非空转。加 live 守卫测试:一个标了 spoiler 的未揭示 fact 块在 ContextAssembly 被 NoSpoiler ContextFilter 删,reveal 后(player_party knows_true)同块放行。
11. 步骤11(收口测试 + 映射 §19 验收):新增/扩充 trpg-runtime 与 trpg-gm 测试,把本阶段不变量钉死并映射 §19:(a) §19-#4 NPC 不基于自己不知道的事实行动——NpcSpeechProjection 越界披露被 verify_npc_disclosure 抓(已有,补 VerifierPrivateView 装载版);(b) §19-#6 Narrator 不得看到未来场景完整秘密——VerifierPrivateView.gm_truth 不进任何 prompt 路径的编译期/测试期守卫;(c) §19-#10 玩家实际获知前不得建 PlayerLearnedFact——WorldFact 提交只建 GM 边不建 player_party 边(步骤5 测试);(d) NoSpoiler 经 KnowledgeProjection 的 reveal-gated 放行(步骤10)。所有测试 fail-closed 方向(取不到→按未揭示/最严)。

**关键文件**:
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-model/src/memory_proposal.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-model/src/knowledge.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-runtime/src/memory_proposal.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-runtime/src/knowledge_projection.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-runtime/src/knowledge_leak_verifier.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-runtime/src/spoiler_guard.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/turn_loop.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/plugin/types.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/plugin/host.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/plugin/builtin_no_spoiler.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/plugin/mod.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-db/src/lib.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/migrations/0032_knowledge_edges.sql`

**验收**:
- §19-#6:VerifierPrivateView.gm_truth_fact_ids(完整秘密)有编译期/测试期守卫证明它绝不进入任何玩家可见 prompt 渲染路径;且 GmAdjudicationProjection→PlayerNarrationProjection 降权后隐藏真相永不出现(复用既有 hidden_truth 分离断言,knowledge_projection.rs:321 测试扩充)。
- §19-#10:提交一个 WorldFact 后,gm_truth_view 含其 fact_id 而 player_knowledge_view 不含(步骤5 守卫测试)——玩家未实际获知前不建 player_party knows_true 边。
- §19-#4:NpcSpeechProjection 经 VerifierPrivateView 装载后,NPC 披露 facts_can_reveal 之外的 fact_id 仍被 verify_npc_disclosure 抓为 SecretLeak(turn_loop.rs:1649 路径 + 新装载版等价)。
- WorldFact 一等化:存在 world_facts 存储(或经架构师选定的等价契约),WorldFactCandidate 提交落该存储而非 memory_facts;CommitAction::describe 反映新目标;dry-run committed_ref 与改道前逐字节等价。
- WorldFact↔KnowledgeEdge 引用契约生效:一个 fact_id 查无 world fact 身份的 KnowledgeUpdate 按架构师选定强度被 warn-traced 或 fail-closed 拒绝(步骤4 测试)。
- Policy 四 checkpoint 形式化:PluginHook 含 before_context/before_commit/before_narration/after_narration 四语义点位,且 before_commit/before_narration 有真实(advisory)接线 — 注册声明该 hook 的假插件被回合流以该 hook 调用且 trace 进 TurnTrace。
- NoSpoiler 经 KnowledgeProjection 真生效:未揭示 spoiler fact 块在 ContextAssembly 被 ContextFilter 删,reveal(player_party knows_true)后同块放行(步骤10 live 守卫,非空转)。
- 全程零回归:既有 trpg-gm/trpg-runtime/trpg-model/trpg-db 测试套件全绿;一回合 live e2e(CoC 模组库,验泄漏校验+NPC 一致性路径)findings 集合与改造前逐条等价。
- 零规则集硬编码:所有新逻辑按数据(world_facts/knowledge_edges/SpoilerMeta/player_known 投影)驱动,无规则集/模组名分支;fail-closed 方向(取不到→按未揭示/最严)。

**不可破坏**:
- 既有 player reveal 写穿路径:domain_events.PlayerLearnedFact ↔ knowledge_edges (player_party, knows_true) 对齐(trpg-db record_revealed_fact)不得变;revealed-facts 兼容投影 = (player_party, knows_true) 子集语义不变(knowledge.rs:7-8)。
- actor identity 契约(TC-KNOW-00):带身份 holder(npc/pc/faction)写库前 validate_actor_id fail-closed,非法 id 不发明 holder(knowledge.rs:294,382-394)——本阶段新引用契约不得绕过它。
- 四 viewer/speaker 投影的结构分离不变量:gm_truth_fact_ids 永不复用为玩家/NPC-safe 集(knowledge_projection.rs:79-121),hidden_truth = GM真相 − 玩家已知。
- spoiler_guard 源实体级裁剪的'别太严'语义:无 spoiler 标注或已揭示 → 字节原样透传(spoiler_guard.rs:140,235 字节等价测试)不得回归。
- PluginHost propose-not-commit:host 只收集+排序贡献、绝不应用/落库(host.rs:55);新增 checkpoint 接线在本阶段保持 advisory(只 trace),不得让插件直接改状态/删块/落库。
- WorldFactCandidate 提交的 fail-closed 原子门:任一提案验证失败→整批不提交(memory_proposal.rs review_and_commit_proposals 的 all-or-nothing,:280);改道 world_facts 不得破坏该原子性。
- 迁移幂等:新增 0039 迁移须 create table if not exists + on conflict 幂等可重放;且补接 0038/0039 进 migrate() include 数组时不得改动既有迁移顺序或内容。
- 插件贡献 trace 的 secret-safe 摘要:proposal/finding 摘要只露稳定 id(fact_id/holder token),绝不回显 secret 正文(types.rs:171,host.rs:276 测试)——VerifierPrivateView/新接线的任何 trace 同样不得外溢正文。
- NarrationVerifier 既有 ledger 对账 + 勘误/retro-debt 路径(turn_loop.rs:1460-1487)语义不变:本阶段只补私有视图装载,不改 InventedEffect→Effect 债 / MissingCheck→Check 债 的折叠。

**依赖**:无

**⚠️ 需架构师拍板(不让自动 agent 猜)**:
- WorldFact 一等化的存储形态与边界(R4-D2 收尾):是新建独立 world_facts 表(WorldFact≠记忆三元组,语义最清晰,推荐),还是逻辑契约复用 memory_facts。这是 crate-boundary + 数据语义决策,且涉及是否拆 trpg-world 雏形——不可让自动 agent 猜。设计4 把 trpg-world crate 列为 P7 殿后,故本阶段即便建 world_facts 表也应留在 trpg-db/trpg-model,不提前抽 crate(需架构师确认这条边界)。
- WorldFact↔KnowledgeEdge 引用契约的强度:弱(孤儿知识边 warn+trace 不阻断)还是强(fail-closed 拒绝);以及 DB 层是否加 knowledge_edges.fact_id REFERENCES world_facts 外键(FK 是破坏性约束,且历史 player reveal 边可能引用尚无 world_fact 身份的 fact_id——加 FK 前须确认存量数据不违约或先回填)。
- VerifierPrivateView 应聚合到什么粒度:是否纳入'完整 committed 事件流'(§11.5 列了'提交事件')还是只纳入 ledger id 集;NPC 子视图是 speech+action 都进还是只 speech。这是投影 SEMANTICS 决策(设计3 教训:projection 语义是人决策),且影响 token/装载成本。
- before-commit / before-narration 两 checkpoint 在本阶段是否保持纯 advisory(只 trace),还是 before-narration 允许 ContextFilter 真删 NarrationPacket 字段。设计4 附录C 把'校验可阻断'归为 P2;若 P3 与 P1/P2 并行,需架构师确认 P3 这两点位先 advisory、阻断能力等 P2 落地后再翻——避免两阶段对同一 checkpoint 的阻断语义打架。
- NoSpoiler 块级 ContextFilter 由谁负责打 spoiler/future_scene/fact:<id> 标注:是让模组图谱 reader(parse 阶段)产出带标注的 block,还是 runtime 投影时按 SpoilerMeta 现派生标注。涉及抽取 SEMANTICS 与单一事实源归属,不可自动 agent 决定。

---

## P4 World Simulation 层(按需 packet):收拢 World 边界、产 WorldReactionSet、NPC 机械行动经 Orchestrator 回送 Rules

**目标**:把当前散在 trpg-runtime 的 NPC 心智/关系/知识/合成、以及错放在 trpg-director 的 clock 逻辑,收拢成一个逻辑 World 边界:对外只暴露一个"产候选、不裁定、不叙事"的入口,产出结构化 WorldReactionSet(立即反应候选 + 后台事件 + clock 提案 + knowledge 提案),其中需要机械裁定的 NPC 行动只输出 NpcActionIntent,由 Orchestrator 回送已有 Rules 路径(roll_check / opposed_prepass)结算——绝不另起第二套规则系统(§7.5)。沿用附录 C「行为切分先行、只在出现明确 packet 需求时才拆」的纪律:本阶段先把已存在的 NpcBehaviorPlan 升级为带 provenance 的 typed 候选并加 WorldReactionSet 聚合壳 + WorldPort trait,把 clock 从 director 迁回 World 边界,faction/information_propagation 只起最小 stub;暂不新建 trpg-world crate、暂不拆 ContextCompiler、暂不做 event-fold replay。验收对齐 §19-#3(Director 只能选 World 提供的候选)、#4(NPC 只基于自己知道的事实行动)。

**现状(grounded)**:World 层的"零件"已存在但散落、且以字符串 block 而非 typed packet 形态喂给单一 GM prompt;clock 错放在 director;无任何聚合壳/Port。具体 grounding:
- NPC 心智链已 load-bearing 但产物是 prompt 字符串:crates/trpg-runtime/src/npc_mind.rs:19/35(assemble/load_npc_mind_view,只读 NPC 自己的 knowledge/belief 边,GM 真相结构性进不来)→ crates/trpg-runtime/src/npc_behavior.rs:21/33/56(derive_npc_behavior_plan / viewer_behavior_context / load_active_npc_guidance,secret 门 fail-closed)→ 模型 crates/trpg-model/src/npc_behavior.rs:82 的 NpcBehaviorPlan(已是事实上的"NPC 行为候选":stance / willingness_help|lie|fight|reveal_secret / risk_tolerance / facts_can_reveal / facts_will_withhold / preferred_actions,确定性 derive,0 commit)。
- 这条链唯一的消费点是 crates/trpg-gm/src/turn_loop.rs:1373 build_npc_behavior_guidance(由 phase_context_assembly turn_loop.rs:1110/1191 调),把每个 active NPC 的 plan 经 NpcBehaviorPlan::to_guidance_block()(crates/trpg-model/src/npc_behavior.rs:207)拼成字符串塞进 npc_guidance_block(prompts.rs:23)——即"WorldReactionSet 提交点"今天是一坨 prompt 文本,不是结构化 set。
- NPC 机械行动→Rules 的回送通路已存在但只覆盖"玩家攻击 NPC"半边:crates/trpg-gm/src/opposed_prepass.rs(detect_attack_target opposed_prepass.rs:113 + prepare_binding:159),产 OpposedBinding 给 roll_check stamp_opposed_check。反向(NPC 主动开枪/黑入/说服当成 NpcActionIntent 回送 Rules)无代码。
- NPC 值供给(World 的"NPC 资源/能力"维度):crates/trpg-runtime/src/npc_synth.rs(resolve_param_tiered:173 T1 源>T2 原型>T3 persona-judge LLM、write_synthesized_param:104),经 RuntimeEngine::ensure_npc_parameter(crates/trpg-runtime/src/lib.rs:2941)+ EntityNeedResolver(entity_need_resolver.rs)落卡。current_check_npc_persona(lib.rs:3020)/ scene_npc_personas(lib.rs:3072)是已有的 fire-and-forget World 预 pass 取数。
- 关系状态机:crates/trpg-runtime/src/npc_relationship.rs:21/40(next_relationship / apply_npc_relationship_delta,evidence-gated、bounded clamp);heavy-postprocess 关系抽取 crates/trpg-runtime/src/relationship_extraction.rs(relationship_facts_from_inputs:293)经 RuntimeEngine::extract_relationship_facts(lib.rs:856)在 AuditLearning phase(crates/trpg-gm/src/execute.rs:402)跑。
- clock 当前错层在 director:crates/trpg-director/src/clock.rs:14 maybe_tick_clocks(从 conflict.intent 语义判停顿/观望,非关键词),由 trpg-director/src/lib.rs:106 调、result.clock_ticks 经 RuntimeEngine 在 lib.rs:1805 insert_clock_tick 落 clock_tick_events 表(crates/trpg-db/src/lib.rs:4229)。模型 ClockTick 在 crates/trpg-model/src/lib.rs:6453。
- 回合管线:crates/trpg-gm/src/turn_plan.rs 的 CANONICAL_TURN_PLAN 15 phase,World 反应当前完全寄生在 ContextAssembly(npc_guidance)与 OpposedPrepass(对抗预 pass)两处,无独立 World phase。
- 零落地(grep 全空,无 .worktrees):WorldReactionSet / WorldReactionCandidate / WorldPort / WorldSimulationRequest|View / NpcBehaviorCandidate / NpcPerformancePlan / WorldDeltaPacket / NpcActionIntent|NpcActionResolved / FactionSim / InformationPropagation / ClockAdvanceProposal。trpg-world crate 不存在。WorldEventKind(crates/trpg-model/src/lib.rs:4162)已有 NpcAction/ClockTick/NpcAttitudeChanged/ScheduledEventDue 等,但 §5.2 的 NpcActionResolved/ResourceChanged 等无。
- NeedBus 五类(crates/trpg-need/src/lib.rs:34)有 Entity/Parameter 但**无 World/WorldDelta kind**——World 反应不走 bus,是 ContextAssembly 内联。

**步骤**:
1. [前置-人类决策门] 在动手前,先取得本 phase human_decisions 列出的架构师裁定(尤其:NpcBehaviorPlan→NpcReactionCandidate 的字段映射语义、clock 迁层后 director 是否仍读 clock 提案、NpcActionIntent 回送 Rules 的触发边界、WorldDelta 是否进 NeedBus)。这些是 design3 已确认的『抽取/投影语义 + 跨 crate 边界调用 = 人类决策』,不可让自动 agent 猜。
2. [结构-最小] 在 trpg-runtime 下新建模块边界(不新建 crate,遵从 §17『先建模块边界不立刻大拆 crate』+ 附录 C):新增 crates/trpg-runtime/src/world/ 目录(mod.rs + reaction.rs + clock.rs),把现有 npc_mind.rs / npc_behavior.rs / npc_relationship.rs / npc_profile.rs / npc_synth.rs / relationship_extraction.rs **用 pub use 重导出**进 world::,不移动文件不改签名(additive,零行为变更),让 lib.rs:73-97 的现有 re-export 仍生效。world/mod.rs 写明这是『逻辑 World 边界,稳定后再决定是否独立 trpg-world crate』。
3. [模型] 在 crates/trpg-model 新增 world_reaction.rs(≤400 行),按 §7.3/§7.4 定义 typed:WorldReactionCandidate{candidate_id, npc_id, action_intent:ActionIntentDraft, goal_basis/knowledge_basis/relationship_basis/emotional_basis:Vec<String>, urgency/feasibility/risk:f32, provenance_event_ids}、NpcActionIntent(机械行动:kind=Attack/Hack/Persuade/Flee… + target + 依据 fact ids)、ClockAdvanceProposal(复用现有 ClockTick crates/trpg-model/src/lib.rs:6453 的字段语义,加 trigger_reason/source_event_ids)、KnowledgeDeltaProposal(复用现有 KnowledgeEdge/MemoryFact 提案形态,不另造)、WorldReactionSet{immediate_candidates, background_events:Vec<WorldEventProposal>, clock_proposals, knowledge_proposals}。全部 #[serde(default)] 字段、commit-nothing 标注。
4. [模型-桥] 在 crates/trpg-model/src/npc_behavior.rs 给 NpcBehaviorPlan 加一个 pure 方法 to_reaction_candidate(&self, basis_event_ids:&[String]) -> WorldReactionCandidate:把已有 stance/willingness_*/facts_can_reveal/preferred_actions 投影成 candidate 的 *_basis 与 urgency/feasibility/risk(确定性映射,基于现有 i16 分数 → f32,见 derive() 的 clamp_pct 逻辑 npc_behavior.rs:120)。**不改 derive() 现有输出**,只新增投影方法(§19-#1 等价不变量友好)。
5. [运行时-聚合] 在 world/reaction.rs 新增 pure+thin-async 两段(对标 npc_behavior.rs 的 pure derive + async load 模式):assemble_world_reaction_set(plans:&[NpcBehaviorPlan], clock_proposals, knowledge_proposals) -> WorldReactionSet(纯,把每个 plan.to_reaction_candidate 收进 immediate_candidates);load_world_reaction_set(db, session_id, active_npc_ids, targets, player_known_fact_ids) -> WorldReactionSet:复用现有 load_active_npc_guidance(npc_behavior.rs:56)逐 NPC 取 plan(secret 门照旧 fail-closed),不另起取数。fail-closed:profile 缺失/DB 抖动 → 跳过该 NPC 并 trace,绝不臆造。
6. [clock 迁层] 把 clock 从 director 迁回 World 边界:在 world/clock.rs 重实现 maybe_tick_clocks 的等价纯函数 derive_clock_proposals(从 conflict.intent 的 PauseAndObserve / WaitOrHoldAction 语义,**逐字符抄 crates/trpg-director/src/clock.rs:14-43 的判定**,产 ClockAdvanceProposal 而非直接 ClockTick)。director/src/clock.rs 改为薄 shim 调 world::derive_clock_proposals 再适配回 ClockTick(或经 human_decision 决定是否让 director 不再产 clock、改由 World phase 产)。**铁律:迁移后 clock 触发条件逐字节等价**(把 director/src/clock.rs 现有 4 个测试 fires_on_semantic_stall_paraphrase / fires_on_wait_or_hold / fail_closed_keyword / does_not_fire_on_advancing 平移到 world/clock.rs 跑绿,证明零行为变更)。
7. [Port] 在 crates/trpg-runtime 定义 WorldPort trait(对齐 §14):#[async_trait] WorldPort { async fn simulate(&self, req: WorldSimulationRequest) -> Result<WorldReactionSet>; },给 RuntimeEngine 实现一个 RuntimeWorld 适配器,内部就是 step 5/6 的 load_world_reaction_set + derive_clock_proposals 组合。WorldSimulationRequest 先用最小字段(snapshot 复用现有 RuntimeState/ContextRequest 投影、active_scene、active_actor_ids、triggering_events),**不新建五套 ContextCompiler**(附录 C #4:先复用 prepare_turn_context 出分层 packet)。
8. [回合接线-additive] 让 ContextAssembly 改走 typed set 但**保持 prompt 字节兼容**:phase_context_assembly(turn_loop.rs:1191)从 build_npc_behavior_guidance(返字符串)改为先 WorldPort::simulate 拿 WorldReactionSet,再用一个 render_world_reaction_block(set) 渲染回**与现有 to_guidance_block 逐字节相同**的 npc_guidance_block(对比测试钉死)。这样『World 反应有了 typed 中间表示』但喂给 prompt 的文本不变 → §19-#1(替换 Narrator/重渲染不改状态)与现有 e2e 全绿。
9. [NpcActionIntent 回送 Rules] 起步竖切(只接一条真实可验证的反向通路,避免过度):当 WorldReactionSet 的某 candidate 含 NpcActionIntent{kind:Attack} 且当前回合语义是『NPC 主动攻击 PC』时,经 Orchestrator 复用 opposed_prepass 的对偶——新增 prepare_npc_attack_binding(对标 opposed_prepass.rs:159 prepare_binding,但 attacker=NPC、defender=PC,防御键仍走 check_param_need lib.rs:3132 数据映射),把 NPC 攻击作为 NpcActionIntent → roll_check/stamp_opposed_check 结算。**绝不在 World 内判成败**(§7.5/§19-#3):World 只产 intent,Rules 结算,Kernel(runtime typed service)commit。其余 kind(Hack/Persuade/Flee)本阶段只产 candidate 不接结算(留 follow-up),fail-closed。
10. [faction/info-propagation 最小 stub] 按『只在出现明确 packet 需求时才拆,避免过度』:本阶段 faction/clock/information_propagation 只做起步——faction 仅定义 FactionClockProposal 类型 + 一个 derive_faction_clock_proposals(暂返 vec![] 或仅从已有 module graph faction 数据派生,无 LLM);information_propagation 仅定义 RumorProposal 类型占位,**不接回合流**。明确写 TODO + human_decision 钩子,不铺宽契约(吸取 design3『别铺影子层宽度』教训)。
11. [WorldDelta/NeedBus 决策落地] 按 human_decision 结果:若架构师裁定 World 反应进 NeedBus,则在 trpg-need 加 NeedKind::World + WorldNeed + WorldNeedResolver(对标 EntityNeedResolver entity_need_resolver.rs);若裁定暂不进 bus(更可能,World 是产候选非取数),则 WorldPort 直接由 phase 调,NeedBus 不动。**二选一由 human_decision 决定,plan 不替架构师拍**。
12. [可观测性] 把 WorldReactionSet 折进 Flight Recorder / explain-turn(对齐 §20 第六阶段『explain 显示 World 提出了哪些候选』):在 turn trace 里记 world.candidates_count / clock_proposals / 每 candidate 的 npc_id+urgency+risk(不记 secret 文本),让 trpg explain --plugins 或 explain-turn 可见 World 层输入输出。复用现有 PluginContributionTrace / TurnTrace 机制(turn_loop.rs verify_after_stream 附近),不另造。
13. [验证] 单测:world/reaction.rs assemble/load 与 to_reaction_candidate 映射(钉 §19-#4 NPC 候选只含 facts_can_reveal、绝不含 facts_will_withhold/GM 真相);world/clock.rs 4 个等价测试;render_world_reaction_block 与旧 to_guidance_block 逐字节对比测试。Live e2e(吸取 memory『大重构必跑完整 live e2e』):真库真模组真回合走 CoC(:54347)血色公路 + Cyberpunk(:54346),验 ① npc_guidance 文本不变(回归基线)② NPC 主动攻击触发 NpcActionIntent→Rules 结算落账 ③ clock 迁层后 clock_tick_events 表行为不变。用 ~/.cache/cargo-target 下的 trpg 二进制(memory:cargo-target-dir-redirect)。

**关键文件**:
- `crates/trpg-runtime/src/npc_behavior.rs`
- `crates/trpg-runtime/src/npc_mind.rs`
- `crates/trpg-runtime/src/npc_relationship.rs`
- `crates/trpg-runtime/src/npc_synth.rs`
- `crates/trpg-runtime/src/npc_profile.rs`
- `crates/trpg-runtime/src/relationship_extraction.rs`
- `crates/trpg-runtime/src/entity_need_resolver.rs`
- `crates/trpg-runtime/src/lib.rs`
- `crates/trpg-model/src/npc_behavior.rs`
- `crates/trpg-model/src/npc_mind.rs`
- `crates/trpg-model/src/lib.rs`
- `crates/trpg-gm/src/turn_loop.rs`
- `crates/trpg-gm/src/opposed_prepass.rs`
- `crates/trpg-gm/src/turn_plan.rs`
- `crates/trpg-gm/src/execute.rs`
- `crates/trpg-director/src/clock.rs`
- `crates/trpg-director/src/lib.rs`
- `crates/trpg-need/src/lib.rs`
- `crates/trpg-db/src/lib.rs`
- `/Users/haoli/leehow/code/chatrpgv2/design/设计4.md`

**验收**:
- §19-#3:新增测试证明 Director 选择只能消费 WorldReactionSet.immediate_candidates 里的 candidate_id,World 没提供的行动候选无法被选(典型反例:构造一个 candidate 集,断言 Director 选择函数对集外 id 返回拒绝/fail-closed)。
- §19-#4:WorldReactionCandidate / NpcBehaviorPlan→candidate 投影的单测断言候选的 knowledge_basis 与 facts_can_reveal 只含『该 NPC 已知为真且玩家方未知不构成 secret 或已解锁』的 fact id,facts_will_withhold 与 GM-only 真相结构性不出现(沿用 npc_behavior.rs 现有 secret 门测试,扩到 candidate 层)。
- §7.5 不另起第二套规则:NpcActionIntent{Attack} 的结算路径单测/e2e 证明 World 只产 intent、最终落账经 roll_check/stamp_opposed_check(与玩家攻击同一 Rules 通路),World 内无任何成败判定代码(grep 守卫:world/ 下不得出现 resolve_outcome/apply_damage/掷骰)。
- 零行为变更基线:render_world_reaction_block 与旧 NpcBehaviorPlan::to_guidance_block 输出在相同输入下逐字节相等(对比测试);clock 迁层后 world/clock.rs 的 4 个等价测试(paraphrase/wait-hold/fail-closed-keyword/advancing)全绿,clock_tick_events 表落库行为与迁移前一致。
- Live e2e:CoC 血色公路 + Cyberpunk 各跑 ≥2 回合,断言 ① 含 active NPC 的回合 npc_guidance 文本不变 ② 至少一个 NPC 主动攻击回合产 NpcActionIntent 并在账本落 CheckResolved ③ 停顿/观望回合仍 tick clock.scene_pressure(迁层后)。
- 文件 ≤400 行(world/reaction.rs、world/clock.rs、model/world_reaction.rs 各自独立小文件);零 per-ruleset/模组名硬编码(防御键走 check_param_need 数据映射、clock 走 conflict.intent 语义,grep 守卫规则集名不出现在 world/ 判定里)。

**不可破坏**:
- 现有 NPC 心智链的 secret 门 fail-closed 语义:crates/trpg-runtime/src/npc_behavior.rs 的 viewer_behavior_context / load_active_npc_guidance 与 crates/trpg-model 的 NpcBehaviorPlan::derive 现有输出**不得改变**(只新增 to_reaction_candidate 投影方法);现有 npc_behavior.rs / npc_mind 测试(adapter_matches_model_derivation、viewer_context_* 等)全绿。
- ContextAssembly 喂给 GM prompt 的 npc_guidance_block 文本逐字节不变(prompts.rs:23/68 的渲染契约 + token 预算/hash 稳定,BP3 DynamicTail 不漂移)。
- opposed_prepass 现有『玩家攻击 NPC』通路(opposed_prepass.rs:113/159 + stamp_opposed_check)不得被反向 NpcActionIntent 改动破坏;两条通路共用 check_param_need 数据映射不变。
- clock 迁层不得改变 clock 触发条件、ClockTick 落 clock_tick_events 表的行为、或 director DirectorOutput.clock_ticks 对下游(clue_board/consequence 渲染 director/src/lib.rs:604)的契约。
- 现有 ensure_npc_parameter / EntityNeedResolver / 现搓落卡(npc_synth.rs T1>T2>T3、write_synthesized_param 升级不降级)不得被 World 收拢破坏;NeedBus 现有五类逐 kind 点接路径不变。
- CANONICAL_TURN_PLAN 仍 15 phase 且现有 phase 顺序/语义不变(本阶段若加 World 投影只在 ContextAssembly 内联或经 Port 调,不新增独立 phase 除非 human_decision 批准)。
- additive 原则:trpg-world crate 不新建、五套 ContextCompiler 不拆、event-fold replay 不做(均属后续 phase),本阶段纯增类型 + Port + 迁层等价。

**依赖**:P0、P1、P3

**⚠️ 需架构师拍板(不让自动 agent 猜)**:
- NpcBehaviorPlan(i16 willingness/risk 分数)→ WorldReactionCandidate(urgency/feasibility/risk:f32 + *_basis:Vec<String>)的**精确投影语义**:哪个分数映 urgency、哪个映 feasibility、basis 字段填什么(goal/knowledge/relationship/emotional 各自取 plan 的哪些字段)。这是 design3 已确认的『投影语义=人类决策』,不能让 agent 猜映射公式。
- clock 迁层后的归属:clock 逻辑迁回 World 边界后,trpg-director 是否仍需读 clock 提案(用于 consequence/clue_board 渲染 director/src/lib.rs:604)?是『director 不再产 clock、改消费 World 的 ClockAdvanceProposal』,还是『World 产提案、director 与 runtime 都可读』?跨 crate 边界调用方向需架构师拍板(design3 教训:crate-boundary calls 是人类决策)。
- NpcActionIntent 回送 Rules 的**触发边界**:本阶段只接 Attack 一种竖切,还是也接 Persuade(社交检定)/Flee(脱离)?以及『NPC 主动行动该回合』如何语义判定(复用 opposed_prepass 的 LLM 语义裁决,还是新判定器)?触发太宽会让每回合多打 LLM、太窄则 World 反应不落地——ROI 边界需架构师定。
- WorldDelta 是否进 NeedBus:World 反应是『产候选』(更像 Director/Policy 的横切产物)还是『取数』(像 Entity/Parameter Need)?若进 bus 则加 NeedKind::World + WorldNeedResolver;若不进则 WorldPort 直接由 phase 调。这决定 trpg-need 是否扩 kind,是跨 crate 契约决策。
- faction/information_propagation 起步的**数据源**:faction clock 从哪里取(module graph 的 faction 字段?新表?),rumor propagation 是否本阶段就要真接回合流还是纯类型占位?吸取 design3『别铺影子层宽度、先证等价再翻默认』,铺多宽需架构师定,避免过度。
- 是否为 World 反应新增独立回合 phase:当前计划是把 World 投影内联进 ContextAssembly(零结构改动)。若架构师希望 World 成为 CANONICAL_TURN_PLAN 里一个**显式 phase**(为后续 Director selection 让位),则需在 turn_plan.rs 加 phase——这是回合管线结构决策,不由 agent 自行决定。

---

## P5 扩展 Director:故事结构层 + DirectorBriefPacket(从 World 候选选焦点,只发 WorldQuery 不直接 commit)

**目标**:在现有 `ActionableSituationDirector`(crates/trpg-director/src/lib.rs)之上叠加一层「故事结构状态」——StoryState/StoryThread/StoryPromise/ScenePlan/BeatPlan/CharacterArcState + 玩家兴趣信号/聚光灯——让导演从「把场景翻译成可行动局势」(现状,纯回合内启发式)升级为「跨回合维护焦点、铺垫、回收、角色弧光」。导演产出一个新的 GM-facing(非玩家可见)`DirectorBriefPacket`:它**从 World 提供的候选池里选焦点**(WorldFactCandidate / 未来 P4 的 WorldReactionCandidate / 当前活跃场景 links),给出 beat 意图与必保/必避约束,但**绝不直接 commit 状态**、**不在候选池外凭空造行动**——任何「池外的新想法」只能降级成一条 `WorldQuery`(声明式查询请求,如「是否存在某个知道线索 X 的 NPC 现在会联系玩家?」)交回 World/runtime 判定有无合理候选。本阶段为 additive:不动 CANONICAL_TURN_PLAN 既有 15 phase 的执行语义,不动玩家可见的 ActionableSituationBrief 路径;故事结构与 DirectorBriefPacket 作为新的 GM-only 上下文区与持久化状态并行落地。映射 设计4 §8 / §11.3 / §12(BP2 Active StoryThreads + BP3 BeatPlan)/ §16(CampaignPlan/StoryThread 维护放场景切换或 heavy postprocess)。

**现状(grounded)**:- 导演实现:`crates/trpg-director/src/lib.rs` 的 `ActionableSituationDirector`(lib.rs:22-25,DirectorMode Disabled/OnDemand/EveryTurn,lib.rs:15-20)`prepare()`(lib.rs:81-146)纯回合内启发式:build_brief(lib.rs:260-400)产 ActionableSituationBrief,辅以 clue_board / consequence / clock_ticks / spotlight / novelty。无任何跨回合「故事」概念——`needs_director_brief`(lib.rs:148-211)和 `choose_guidance_level`(lib.rs:213-258)仍是 user_input 关键词扫(中文/英文字面表)+ frame 在场判断,clock(src/clock.rs)和 spotlight(src/spotlight.rs)已经语义化/真名册化但都是**单回合**产物。
- 子模块:`crates/trpg-director/src/spotlight.rs`(SpotlightParticipant + build_spotlight,已按真实 player 名册 + prior_spotlights 跨回合累加 spotlight_count,fail-closed 无占位 id);`crates/trpg-director/src/clock.rs`(maybe_tick_clocks 消费 conflict.intent 语义,非关键词)。
- 运行期接线:`crates/trpg-runtime/src/lib.rs::prepare_actionable_situation`(lib.rs:1735-1818)是唯一调用点——但它是**孤儿方法**,grep 全仓只有 spotlight_roster.rs 注释提到它,**不在 CANONICAL_TURN_PLAN**(crates/trpg-gm/src/turn_plan.rs:45-65 的 15 phase 无 Director phase;execute.rs 无 director 调用)。它做的事:加载 module_config.director 覆盖(lib.rs:1747-1751)、prior_spotlights(lib.rs:1754-1758)、真实 player 名册(lib.rs:1766-1771)→ director.prepare → 把 brief/clue_board 经 `actionable_situation_block`/`clue_board_block`(crates/trpg-runtime/src/context_blocks.rs:62-109)upsert 成 ContextBlock(brief 是 BlockKind::ActionableSituationBrief,CacheZone::DynamicTail=BP3,Visibility::PlayerVisible),并落 consequence/clock/spotlight 行。
- 模型:`DirectorTurnResult`(crates/trpg-model/src/lib.rs:6524-6536)、`ActionableSituationBrief`(lib.rs:6500-6522)、`SpotlightState`(lib.rs:6478-6486)、`SceneFramePurpose`(lib.rs:6488-6497)、`DirectorModuleConfig`(lib.rs:4712-4740,模组导演覆盖数据:scene_facts/pressure/affordance/risk/npc_advice/known_facts/open_question)。**StoryThread / StoryState / StoryPromise / ScenePlan / BeatPlan / CharacterArcState / DirectorBriefPacket / DirectorPlan / WorldReactionCandidate / WorldQuery 全 grep 不到**(附录 A 🔴 确认零落地)。
- World 候选池(导演要从中选):①`WorldFactCandidate`(crates/trpg-model/src/memory_proposal.rs:169-215,fact_id/subject/predicate/object/summary/confidence/source_event_ids,**无 holder**、validated() fail-closed 要求 fact 身份 + 非空 evidence)是当前唯一一等「世界事实候选」;②`MemoryExtractionProposal`(memory_proposal.rs:312-319,propose-not-commit,runtime-owned 才 commit);③未来 P4 的 `WorldReactionCandidate`(任务 #5,本阶段按接口占位)。
- 知识视图(导演可读、防剧透):`Db::gm_truth_view`(crates/trpg-db/src/lib.rs:3570-3572,GM holder knows_true 的 fact_id 集=世界真相)/`player_knowledge_view`(lib.rs:3577-3579)/`list_revealed_facts`(lib.rs:3593-3595)。导演读 GM truth 摘要 + fact_id(§11.3「最好看摘要和事实 ID 而非大段幕后正文」),选 reveal 候选时受 player_knowledge 约束。
- 持久化原语已有:`insert_actionable_situation_brief`/`upsert_player_facing_clue_board`/`insert_consequence_contract`/`insert_clock_tick`/`upsert_spotlight_state`/`load_spotlight_states`(crates/trpg-db/src/lib.rs:4157-4279);spotlight 跨回合状态表已存在并被 load/upsert。最新迁移=`migrations/0038_memory_fact_truth_status.sql`(新表从 0039 起)。
- crate 依赖:`crates/trpg-director/Cargo.toml` 仅依赖 trpg-model + trpg-combat(+ serde/chrono/uuid/tracing)——WorldFactCandidate(trpg-model)可直接引用;但**不**依赖 trpg-db/trpg-llm(导演保持纯函数 + typed 输入,DB/LLM 取数留 runtime,延续现有 prepare_actionable_situation 的「runtime 取数→typed 入参→director 纯计算→runtime 落账」模式)。

**步骤**:
1. [纯模型先行] 在 trpg-model 新增故事结构 typed 数据(全部 #[serde(default)] 加性、不改既有类型)。建议新文件 crates/trpg-model/src/story.rs(参照 memory_proposal.rs 的独立文件 + lib.rs `pub mod`/`pub use` 模式,守 ≤400 行):`StoryThreadStatus{Open,Advancing,Stalled,Resolved,Abandoned}`、`StoryThread{thread_id,title,summary,status,primary_actor_ids:Vec<String>,related_fact_ids:Vec<String>(指向 WorldFactCandidate.fact_id),source_refs,opened_at_turn,last_advanced_turn}`、`StoryPromiseStatus{Planted,Reinforced,Paid,Broken}`、`StoryPromise{promise_id,thread_id:Option<String>,setup_summary,payoff_hint,status,planted_at_turn,fact_id_basis:Vec<String>}`、`ScenePlan{scene_id:Option<String>,dramatic_question,intended_beats:Vec<BeatKind>,exit_signals:Vec<String>,pacing_budget_turns:Option<u32>}`、`BeatKind{Respond,Consequence,Revelation,Choice,Pressure,Callback,Spotlight,Transition}`、`BeatPlan{beat_id,beat_kind:BeatKind,dramatic_function:String,desired_change:String,must_preserve:Vec<String>,must_avoid:Vec<String>}`、`CharacterArcState{actor_id,arc_label,current_stage,open_questions:Vec<String>,last_focus_turn:Option<String>}`、`PlayerInterestSignal{signal_id,topic,evidence_turn_ids:Vec<String>,weight:f32}`、`StoryState{session_id,threads:Vec<StoryThread>,promises:Vec<StoryPromise>,scene_plan:Option<ScenePlan>,character_arcs:Vec<CharacterArcState>,player_interest:Vec<PlayerInterestSignal>,updated_at}`。为每个有身份/证据的类型加 fail-closed `validated()`(对齐 memory_proposal.rs:191-215:空 id/空 thread_id 拒绝;related_fact_ids 允许空但若非空必须是字符串 fact_id),不引入 LLM、不引入 DB。
2. [选择产物 typed] 在同文件新增导演的两个核心产物:`WorldQuery{query_id,question:String,intent:WorldQueryIntent,referenced_fact_ids:Vec<String>,referenced_actor_ids:Vec<String>}` 与 `WorldQueryIntent{FindKnowledgeableNpc,ProbeFactionReaction,CheckClockState,SeekCallbackOpportunity,Other(String)}`(§8.4「池外新想法只能变成 WorldQuery」);`DirectorBriefPacket{packet_id,session_id,turn_id,beat_plan:BeatPlan,primary_thread_id:Option<String>,secondary_thread_ids:Vec<String>,focus_actor_ids:Vec<String>,selected_world_candidate_ids:Vec<String>(只能取自传入候选池的 id),reveal_candidate_fact_ids:Vec<String>(只能取自 GM-truth 且不在 player_knowledge 的 fact_id),callback_promise_ids:Vec<String>,world_queries:Vec<WorldQuery>,spotlight_target_actor_id:Option<String>,must_preserve:Vec<String>,must_avoid:Vec<String>,created_at}`。这是 设计4 §8.2 `DirectorPlan` 的本仓落地形(命名 DirectorBriefPacket 与现有 ActionableSituationBrief 区分:Brief=玩家可见局势,Packet=GM-only 编排意图)。packet 必须 `Serialize`(要进 ContextBlock JSON)。
3. [导演纯函数:选而非造] 在 crates/trpg-director/ 新增 story 子模块(mod story; 参照现有 mod clock/mod spotlight 的 lib.rs:8-10 声明)。核心入口 `pub fn build_director_brief_packet(input, story_state:&StoryState, world_candidate_ids:&[String], gm_truth_fact_ids:&[String], player_known_fact_ids:&[String]) -> DirectorBriefPacket`,签名延续 DirectorInput<'a>(lib.rs:27-49,已携带 request/state/compiled/user_input/conflict/module_config/participants/prior_spotlights)外加新 typed 入参。硬约束在代码层强制(不靠 prompt):①`selected_world_candidate_ids` 用 `.retain(|id| world_candidate_ids.contains(id))` 过滤——选不在池里的=丢弃(对齐 §19-#3);②`reveal_candidate_fact_ids` 必须 ∈ gm_truth 且 ∉ player_known(否则降级成 WorldQuery 或丢弃,绝不让导演「揭示」一个 GM 自己都没确知为真的事实——对齐 §8.3/§19-#10);③候选池为空但导演想要某种反应时→产 WorldQuery 不产 selection(§8.4)。beat_kind 选择从 conflict/frame/story_state.scene_plan 推导(复用现有 clock.rs 的 conflict.intent 语义消费风格,不新增关键词表)。
4. [默认 fallback 不退化空白] 实现 设计4 §18 的默认 Director fallback:当 story_state 为空(首回合/无线程)或推导失败时,build_director_brief_packet 返回一个通用 BeatPlan(beat_kind=Respond→Consequence→Choice 链:回应玩家刚做的→展示一个直接后果→留≥2 个合理行动方向、不引入未建立秘密),focus_actor_ids 取 spotlight 欠场玩家(复用 spotlight.rs 的 prior_spotlights 最少 spotlight_count 者),world_queries/selected 均空。保证 设计4 §19-#2「关闭 Director 游戏仍能跑、只是较平淡」与「Director 模型失败不退化成空白」。本阶段导演**全确定性 Rust**(沿用现状无 LLM),把可选 LLM beat 推理留作后续(§16 标注「物理上不一定调 3 个模型」)。
5. [玩家兴趣 + 聚光灯接入故事状态] 把现有 spotlight(src/spotlight.rs build_spotlight,跨回合 spotlight_count)纳入 StoryState.character_arcs / spotlight_target 决策:packet.spotlight_target_actor_id = 选 spotlight_count 最小且 last_spotlight_turn 最旧的真实 player(fail-closed:名册空→None,延续 spotlight.rs:87-103 的无占位 id 规则)。新增 `player_interest` 的轻量启发式更新(从 conflict.intent + 近期 user_input topic,纯函数,放 story 子模块),但**复杂跨回合 StoryThread/Promise 推进留 heavy postprocess**(§16:CampaignPlan/StoryThread 维护放场景切换/会话结束/heavy),本阶段只在回合内**读** StoryState 选焦点 + 提议 thread/promise 的 advance/plant 候选(propose,不 commit)。
6. [持久化:新表 + load/upsert,additive] 新增 migrations/0039_story_state.sql:`story_threads`/`story_promises`/`scene_plans`/`character_arc_states`/`player_interest_signals`(按 session_id 索引,与既有 spotlight_states 表并列;遵循 crates/trpg-db/src/lib.rs:81-86 的 include_str! 迁移数组——**手动把 0039 加进 migrate() 的 include_str! 列表**,这是已知坑,见 MEMORY「迁移0026 手动加行」)。在 trpg-db 加 `load_story_state(session_id)->StoryState`(fail-soft 空)与 propose-not-commit 的 `apply_story_proposals(session_id, &[StoryProposal])`(runtime-owned commit,导演产 proposal 但不自己写),对齐 §5/附录C-#2「只有 runtime-owned typed services 能 commit」。DirectorBriefPacket 本身是回合内 GM-only 产物,**不入** story_threads 表(它是 selection/编排,不是世界真相),只作为 ContextBlock 落 turn-scoped。
7. [运行期接线:产 GM-only packet ContextBlock] 在 crates/trpg-runtime/src 扩展 prepare_actionable_situation(或新增并列方法 prepare_director_packet,避免把 ≤400 行的 lib.rs 段落撑爆——优先新方法):runtime 负责取数(load_story_state、gm_truth_view、player_knowledge_view、当前活跃场景 links / world_candidate_ids 来源 = 本回合 WorldFactCandidate 提案 id + 活跃 scene links)→ 调 director.build_director_brief_packet(纯计算)→ 经新 `director_packet_block`(参照 context_blocks.rs:62-88 actionable_situation_block,但 **Visibility::GmOnly**、CacheZone::DynamicTail=BP3、BlockKind 新增 DirectorBriefPacket 变体)upsert 成 GM-only ContextBlock。**关键:packet 进 GM/Resolution 上下文,绝不进玩家可见 prompt**(§11.4 Narration Context 最小最干净、§19-#6 Narrator 不得见未来秘密)。story proposals 经 apply_story_proposals 由 runtime commit。
8. [把 Director 接成可关停的真 phase(可选,与 P1 协调)] 现状 prepare_actionable_situation 是孤儿;本阶段最小目标是让 DirectorBriefPacket 真正进入 GM 上下文。若 P1(Adjudicator/Narrator 拆分)已落地,则把 director packet 产出挂在 ContextAssembly 之后、AgentLoop 之前的位置(crates/trpg-gm/src/turn_plan.rs/execute.rs);若 P1 未就绪,则维持 runtime 内接线 + DirectorMode::Disabled 一键关停(lib.rs:68-79 from_env_or_default 已支持 TRPG_ACTIONABLE_DIRECTOR_ENABLE_V13),保证 §19-#2 可关停。**是否把 Director 升为 CANONICAL_TURN_PLAN 第 16 phase 是结构改动**——列入 human_decisions,本阶段默认不改 plan 的 15 phase 契约(turn_plan.rs 测试 canonical_plan_has_fifteen_phases 等不破)。
9. [测试 + 验收] 单测(crates/trpg-director,纯函数):①selected_world_candidate_ids 过滤掉池外 id(§19-#3);②reveal_candidate_fact_ids 排除 player 已知 / GM 未确知(§19-#10);③候选池空→产 WorldQuery 不产 selection;④空 StoryState→fallback BeatPlan 非空且留≥2 方向(§18);⑤spotlight_target 选欠场玩家、名册空→None(fail-closed)。trpg-model 单测:StoryThread/StoryPromise/DirectorBriefPacket validated() fail-closed(空 id/空 evidence 拒绝)。trpg-db 单测/集成:0039 迁移可跑、load_story_state 空会话 fail-soft、apply_story_proposals 幂等。runtime/集成:director_packet_block 是 GmOnly + DynamicTail。最后按 MEMORY「大重构必跑完整 live e2e」跑一轮真库真回合(CoC 模组,确认 packet 进 GM 上下文不进玩家可见、关 Director 仍可玩)。

**关键文件**:
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-director/src/lib.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-director/src/spotlight.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-director/src/clock.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-director/Cargo.toml`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-model/src/lib.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-model/src/memory_proposal.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-runtime/src/lib.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-runtime/src/context_blocks.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-db/src/lib.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/crates/trpg-gm/src/turn_plan.rs`
- `/Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula/migrations/`

**验收**:
- 设计4 §19-#3:DirectorBriefPacket.selected_world_candidate_ids 在单测中证明对任意池外 id 做 retain 过滤(导演不得选 World 没提供的候选);live 回合中 packet 的 selection 全部 ∈ 传入候选池。
- 设计4 §19-#10 / §8.3:reveal_candidate_fact_ids 单测证明必 ∈ gm_truth_view 且 ∉ player_knowledge_view(玩家实际获知前不建立揭示意图,导演不能凭 GM 知道就揭示)。
- 设计4 §8.4:候选池为空 + 导演想要某反应时,产出 WorldQuery 而非 selection;单测覆盖「池外新想法→WorldQuery」。
- 设计4 §19-#2 / §18:DirectorMode::Disabled(TRPG_ACTIONABLE_DIRECTOR_ENABLE_V13=0)下游戏仍可跑通一整回合(live e2e);StoryState 为空时 fallback BeatPlan 非空、留≥2 个合理行动方向、不引入未建立秘密。
- 设计4 §11.4 / §19-#6:director_packet_block 是 Visibility::GmOnly + CacheZone::DynamicTail,断言它不进玩家可见 prompt(集成测试 + live 检查玩家可见输出无 packet JSON)。
- Additive 不回归:turn_plan.rs 既有 15-phase 契约测试(canonical_plan_has_fifteen_phases / plan_order_matches_phase_id_declaration)与玩家可见 ActionableSituationBrief 路径不变;trpg-model/memory_proposal 既有 propose-not-commit 测试不破。
- 迁移 0039 可跑且幂等;load_story_state 对空会话 fail-soft 返回空 StoryState;apply_story_proposals 是 runtime-owned commit(导演侧无 DB 写)。
- 故事结构 typed validated() fail-closed:空 thread_id/promise_id、空 evidence 的 StoryThread/StoryPromise/DirectorBriefPacket 被拒绝(对齐 memory_proposal.rs 现有 fail-closed 模式)。

**不可破坏**:
- CANONICAL_TURN_PLAN 的 15-phase 契约(crates/trpg-gm/src/turn_plan.rs:45-65 及其单测:canonical_plan_has_fifteen_phases / agent_loop_is_the_single_middle_body / plan_order_matches_phase_id_declaration)——本阶段默认不增减 phase。
- 玩家可见 ActionableSituationBrief 路径:prepare_actionable_situation 现有产物(brief/clue_board/consequence/clock/spotlight)及其 PlayerVisible ContextBlock 行为(context_blocks.rs:62-109)逐字段不变;DirectorBriefPacket 是并列的 GM-only 新增物,不替换 Brief。
- trpg-director 纯度边界:director crate 仍只依赖 trpg-model + trpg-combat(Cargo.toml),不得新引 trpg-db/trpg-llm——DB/LLM 取数留 runtime(延续 prepare_actionable_situation 的 runtime-取数→typed-入参→director-纯计算 模式)。
- propose-not-commit 契约:导演产 story proposals / packet,绝不自己写 DB 或 append domain event(附录C-#2「只有 runtime-owned typed services 能 commit」;§5「层拥有语义,Kernel/runtime 拥有提交权」)。
- spotlight 跨回合不变量:src/spotlight.rs 的真名册 + prior_spotlights 累加 + fail-closed 无占位 id(build_spotlight 现有 4 个单测)必须仍绿。
- 防剧透:DirectorBriefPacket 永不进玩家可见上下文(§11.4/§19-#6);reveal 候选受 player_knowledge_view 约束(§19-#10),不得让 packet 成为绕过 spoiler_guard 的泄漏通道。
- 迁移数组手动登记:新增 0039 必须加进 crates/trpg-db/src/lib.rs migrate() 的 include_str! 列表(否则不执行——MEMORY 已知坑),且既有 0026-0038 顺序不动。

**依赖**:P4

**⚠️ 需架构师拍板(不让自动 agent 猜)**:
- 【抽取/投影语义】StoryThread/StoryPromise 从「回合证据」推进(plant/reinforce/advance/resolve)的判定语义——什么算一条线程「推进了」、一个承诺「回收了」?这是 design3 教训中明确的人类决策(extraction/projection SEMANTICS 不能让自动 agent 猜)。本阶段默认只读 StoryState 选焦点 + 产 propose 候选,把真正的推进规则留给架构师定。
- 【candidate 池来源边界】本阶段 World 候选池的实际构成:WorldFactCandidate 提案 id + 活跃 scene links 是否够?P4 的 WorldReactionCandidate 尚未落地(任务#5 依赖),导演选什么、池由谁在哪个 phase 装配,是跨 crate 边界决策(design3 教训:crate-boundary calls 是人类决策)。需架构师确认 P4/P5 的候选交接契约。
- 【Director 是否升为第 16 phase】把 director packet 产出接成 CANONICAL_TURN_PLAN 的正式 phase(改 15→16)是结构契约改动,涉及 turn_plan 测试与 execute 解释器;还是维持 runtime 内接线 + env 关停?默认不改 plan,但最终编排位置(ContextAssembly 后 / AgentLoop 前)需架构师拍板,且与 P1 Adjudicator/Narrator 拆分的落地顺序耦合。
- 【LLM 介入边界】导演 beat 推理本阶段全确定性 Rust(沿用现状)。是否、以及在哪一步引入 LLM 做「situation planning 一次性多 section 输出」(§16)——这关系到成本/延迟/默认开关策略,留架构师决定;本阶段不引入。
- 【StoryState 持久化粒度】story_threads/promises/scene_plans 等新表的写穿时机:回合内 propose-only 还是允许 scene-transition 时 commit?以及与既有 spotlight_states / state_frames 的关系(是否收敛进 StoryState 还是并列),涉及数据模型收敛决策,需架构师确认避免再造一套并行真相源(§10.5「核心数据不能插件化/分叉」)。

---

## P6 事件溯源成熟化 + 同种子确定性 + 两提交点(ResolutionCommit/PresentationCommit)

**目标**:把当前"write-through 即落库、projection=SQL 即席聚合、掷骰用 thread_rng、落账一气呵成"的事件层升级为可验收的事件溯源运行时:① 补齐 设计4 §5.2 缺失的 DomainEventKind(ResourceChanged/ClockAdvanced/NpcActionResolved/DialogueSpoken/ConditionApplied/ItemAcquired/ActorStateChanged/StoryThreadAdvanced)并接 write-through;② 建一个通用、纯函数、零规则集硬编码的 event-fold/replay 引擎,使关键 projection(玩家暴露集/玩家知识/资源 current/scene)可从 domain_events 重建,并加 §19-#8 "replay 后重建相同 projection" parity 测试;③ 给掷骰路径引入 seedable RNG(按 session/turn/check 确定性派生种子),实现 §19-#9 "同行动+同状态+同种子→同机械结果",并保留现有 seed_commitment 语义;④ 把"落账"切成两个语义提交点 ResolutionCommit(机械/世界事实,§13-#10)与 PresentationCommit(玩家认知:PlayerLearnedFact/FactExposed/DialogueSpoken,§13-#14),实现 §19-#10 "玩家实际获知前不得建立 PlayerLearnedFact"。全程 additive / fail-soft / 零规则集硬编码,不破坏现有 14 种事件的 write-through 与字节等价。

**现状(grounded)**:DomainEventKind 仅 14 个 variant(crates/trpg-model/src/domain_event.rs:20-67),文件头注释自承"doc 列了 ~20 种,本切片只取安全子集"。§5.2 列的 ResourceChanged/ConditionApplied/ItemAcquired/ActorStateChanged/StoryThreadAdvanced/ClockAdvanced/NpcActionResolved/DialogueSpoken 全缺。加 variant 必须同步改 as_str(:74)、from_str_token(:95) 及 token round-trip 测试(:217/:270)。事件表+写穿:migrations/0030_domain_events.sql 定义 domain_events(seq bigserial / event_id unique / on conflict do nothing 幂等);Db::append_domain_event(trpg-db/src/lib.rs:3229)、list_domain_events(:3251)、list_domain_events_for_turn(:3273)。已有 write-through:生命周期事件在 trpg-gm/src/execute.rs:135/188/266/318 经 append_lifecycle_event(:335, event_id=de_{turn_id}_{kind});DiceRolled/CheckResolved 在 trpg-db/src/lib.rs:4381/4417 的 insert_dice_roll/insert_check_result 尾部 fail-soft 写穿。projection 现状:全是 SQL 即席聚合、无 fold/replay 引擎——list_surfaced_entities(trpg-db/src/lib.rs:3329 select distinct over kind in PlayerExposed/EntitySurfaced)、list_context_surfaced_entities(:3295)、资源 current 走 generic_parameter_states 直读(crates/trpg-db/src/resource_current.rs load/write_resource_current:56/116)。grep 全仓确认无 event-fold/replay 引擎(命中均误报:trpg-harness cassette 的 LLM transcript "replay"、scene_policy.rs:72 的 fold_unexecutable 效果补丁折叠)。RNG:PseudoRandomDiceRoller::roll(crates/trpg-runtime/src/lib.rs:4581 用 rand::thread_rng().gen_range)与 roll_amount_dice(crates/trpg-mechanics/src/lib.rs:1917 同样 thread_rng)都无 seed 入参;关键:seed_commitment 字段(trpg-runtime/src/lib.rs:2276 = sha256_hex(seed_material),seed_material:2259 含 result_json)是在掷骰之后算的"对结果的承诺哈希",不驱动 RNG——§19-#9 完全未实现。提交点:execute.rs 把生命周期事件 fail-soft 顺写,无 Resolution/Presentation 之分;record_revealed_fact(trpg-db/src/lib.rs:3391)在 reveal 时一次性写 PlayerLearnedFact 事件 + upsert_knowledge_edge_player_party(:3417, knows_true),不以"玩家确已被表现/verifier 通过"为前置。explain_cli(crates/trpg-cli/src/main.rs:1256)已读 list_domain_events_for_turn 并 format_domain_events(:1278)。migrate include_str 数组止于 0037(trpg-db/src/lib.rs:78-87),磁盘上 0033/0034 有重号但数组各只注册一个,新迁移应从 0038/0039 起避让重号。

**步骤**:
1. 步骤1 (事件类型扩充, additive): 在 crates/trpg-model/src/domain_event.rs:20 的 DomainEventKind 追加 §5.2 缺失 variant:ResourceChanged / ConditionApplied / ItemAcquired / ActorStateChanged / ClockAdvanced / NpcActionResolved / DialogueSpoken / StoryThreadAdvanced。同步在 as_str(:74) 与 from_str_token(:95) 各加一行(token 与 variant 同名,钉死契约),并在测试模块加一个把全部新 variant 过一遍 as_str↔from_str_token↔serde 三方闭环的测试(对标 :270 semantic_split_kinds 测试)。绝不改动现有 14 个 variant 的 token 字符串(db 兼容铁律)。
2. 步骤2 (新事件 data schema 约定, 纯类型): 不给每种事件造新结构体(沿用 data: serde_json::Value 通用载荷),但在 domain_event.rs 用文档注释钉死每种新事件 data 必含字段:ResourceChanged={actor_id,track_id,old,new,delta,cap?};ClockAdvanced={clock_id,from,to};NpcActionResolved={npc_actor_id,action_kind,check_id?};DialogueSpoken={speaker_actor_id,addressee?,visibility};ActorStateChanged/ConditionApplied/ItemAcquired={actor_id,...}。这是 fold 引擎与 parity 测试读取的稳定契约,写在类型旁防漂移。
3. 步骤3 (ResourceChanged write-through, additive/fail-soft): 在 crates/trpg-db/src/resource_current.rs:116 write_resource_current 末尾(Ok(capped) 前)加 fail-soft append。该函数已返回 capped 实际落值,且需要 old 值——先在写前复用本文件 load_resource_current(:56) 读旧值(或在 on-conflict SQL 用 returning 拿旧值,二选一由实现层定),再 append DomainEvent(kind=ResourceChanged, 确定性幂等键如 de_res_{session}_{actor}_{track}_{world_tick},data 按步骤2)。沿用 DiceRolled write-through(lib.rs:4379)的 fail-soft 模式:append 失败仅 tracing::warn,绝不改本函数成败。注意 write_resource_current 不带 turn_id 入参,event 的 turn_id 留空串(serde default 已兼容),不在本步引入 turn_id 透传(避免改 ~6 处调用方签名,留人类决策)。
4. 步骤4 (ClockAdvanced/NpcActionResolved/DialogueSpoken write-through, additive): ClockAdvanced——定位现有 world_tick/clock 推进单点(grep set_world_tick / advance_clock / world_events 推进处),其后 fail-soft append。NpcActionResolved——挂在被选中 NPC 行动重新进 Rules 结算落账处(若 P4 World 层未落地,先挂现有 NPC opposed-roll 结算尾段 trpg-runtime resolve_outcome_with_opposition:2312 附近,data 记 npc_actor_id+check_id)。DialogueSpoken——这是 PresentationCommit 域事件(见步骤9),本步只占位 variant + data 契约,真正 emit 放步骤9。每处遵循 fail-soft、确定性 event_id、不入控制流。
5. 步骤5 (event-fold/replay 引擎骨架, 纯函数, 新模块): 新建 crates/trpg-runtime/src/event_fold.rs(<400 行)。定义 pub trait Projection { type State; fn empty()->Self::State; fn apply(state:&mut Self::State, ev:&DomainEvent); } 与 pub fn fold<P:Projection>(events:&[DomainEvent])->P::State。纯函数:输入有序事件序列、输出 projection 状态,零 DB/零 LLM/零 RNG。实现首批 3 个 impl 复刻现有 SQL 视图语义:(a) PlayerExposureProjection——fold kind in {PlayerExposed,EntitySurfaced} 收集 (entity_id,entity_kind) distinct,与 list_surfaced_entities(lib.rs:3329)逐字节对齐;(b) ContextSurfacedProjection——对齐 list_context_surfaced_entities(:3295);(c) PlayerKnowledgeProjection——fold PlayerLearnedFact 得 player_party knows_true 事实集。事件先按 seq 升序(list_domain_events 已 order by seq asc)。
6. 步骤6 (replay parity 测试, 映射 §19-#8): 新增 crates/trpg-runtime/tests/live_projection_replay_parity.rs(live, 真库)。对一个已有事件的 session:(1) 读现有 SQL 视图结果(list_surfaced_entities/list_context_surfaced_entities/玩家知识);(2) db.list_domain_events(session, large_limit) 取全量事件喂 event_fold::fold::<P>;(3) assert_eq! 两者完全一致——证明 '从事件 replay 重建的 projection == 即席 SQL projection'。再加纯函数单测:同一组事件 fold 两次字节相等(确定性/可重放)、乱序输入排序后 fold 仍一致。这是 §19-#8 的可执行验收。
7. 步骤7 (seedable RNG 注入, 改 dice 路径): 在 trpg-runtime 给 DiceRollerPlugin 增 roll_seeded(&self, expression:&str, seed:u64)->Result<DiceRoll>(default 方法回退现有 thread_rng 保兼容),PseudoRandomDiceRoller 实现用 rand::rngs::StdRng::seed_from_u64(seed)(rand 已是依赖,lib.rs:2 use rand::Rng)替换 :4581 的 thread_rng。种子派生:复用 seed_material 构造(lib.rs:2259)但去掉 result_json 分量(种子必须掷骰前确定),改为 hash(session:turn:check:roller_id:expression) 的稳定 u64(sha256_hex 取前 8 字节→u64)。在 resolve_roll_input 的 DiceExpression 分支(:2254)与 resolve_outcome_with_opposition 防御骰(:2333)两处把 roll_dice(expr) 换成 seeded 调用。
8. 步骤8 (确定性掷骰验收, 映射 §19-#9): 保留 seed_commitment 语义不变(仍是对结果的承诺哈希),但 seed 现在真驱动。加纯函数单测:同 (session,turn,check,roller,expression)→同 seed→StdRng 产同 rolls(断言 roll_seeded 两次字节相等);不同 check_id→不同 seed→期望不同序列。再加 live e2e(真库一回合两次同输入同种子):机械结果(check_results.outcome / 资源 delta)完全一致。这是 §19-#9 的可执行验收。注意:roll_amount_dice(trpg-mechanics:1917)是叶 crate 的 effect 金额掷骰,本阶段先在文档标注其确定性缺口为 follow-up(避免 trpg-mechanics→trpg-runtime 依赖环),不强行接 seed(human_decision)。
9. 步骤9 (两提交点拆分, 核心语义重构): 引入 ResolutionCommit 与 PresentationCommit 两个语义边界,落在 trpg-gm/src/execute.rs 回合流。ResolutionCommit(§13-#10)= 机械/世界事实组:DiceRolled/CheckResolved(已 db 写穿)+ ResourceChanged/NpcActionResolved/ClockAdvanced/SceneTransitioned,在 critical 段(TurnComplete 前,execute.rs:281 之前)提交,与现有顺序一致。PresentationCommit(§13-#14)= 玩家认知组:FactExposed/PlayerLearnedFact/DialogueSpoken/TranscriptSaved,改为在 verifier 通过且念白确已表现之后提交。关键改动:把 record_revealed_fact(trpg-db/src/lib.rs:3391)的调用时机从 reveal-tool 即写,改为经新的 PresentationCommit 收口点统一提交(reveal 工具改为只'提名'PlayerLearnedFact 候选,真正 append 事件 + upsert_knowledge_edge 在 PresentationCommit 处,以 verifier 未阻断为前置)。fail-closed:verifier 阻断该 reveal 时不提交 PlayerLearnedFact(实现 §19-#10)。
10. 步骤10 (§19-#10 验收 + #7 不变量保护): 加 live e2e:构造'GM 内部 context 知道某秘密但玩家本回合念白未表现且 verifier 拦截'的回合,断言回合后 db 中无该 fact 的 PlayerLearnedFact 事件、无 player_party knows_true 边(§19-#10)。再加'verifier 阻断输出后,ResolutionCommit 的机械事件(ResourceChanged/CheckResolved)仍在、未被回退'的测试(§19-#7)。两提交点拆分后跑现有 trpg-gm/trpg-runtime/trpg-db 全套 live 测试 + 反剧透/revealed-facts 测试(live_revealed_facts/live_surfaced_entities)确认零回归。
11. 步骤11 (可观测性接线 + 收尾): explain_cli(trpg-cli/src/main.rs:1256)的 domain_events 摘要让新事件种类一并显示(format_domain_events:1278 已通用按 kind.as_str 打印,验证即生效)。新迁移若需要(如给 domain_events 加 commit_phase 列区分 resolution/presentation,可选)从 0038 起编号并加入 trpg-db/src/lib.rs:78 include_str 数组(避让磁盘 0033/0034 重号)。全部新文件 <400 行。最后跑一轮真库一回合 e2e(CoC :54347 / Cyberpunk :54346)走遍掷骰→资源变更→reveal→verifier 路径,确认 ResolutionCommit/PresentationCommit 两点事件分别落账、replay parity 绿、同种子复现绿。

**关键文件**:
- `crates/trpg-model/src/domain_event.rs`
- `crates/trpg-db/src/lib.rs`
- `crates/trpg-db/src/resource_current.rs`
- `crates/trpg-runtime/src/lib.rs`
- `crates/trpg-runtime/src/event_fold.rs (新建)`
- `crates/trpg-runtime/tests/live_projection_replay_parity.rs (新建)`
- `crates/trpg-gm/src/execute.rs`
- `crates/trpg-mechanics/src/lib.rs`
- `crates/trpg-cli/src/main.rs`
- `migrations/0030_domain_events.sql`
- `crates/trpg-db/tests/live_domain_events_mechanical.rs`

**验收**:
- §19-#8: 新增 live_projection_replay_parity 测试——把一个 session 的全量 domain_events 喂 event_fold::fold,重建出的 PlayerExposure/ContextSurfaced/PlayerKnowledge projection 与现有 SQL 视图(list_surfaced_entities/list_context_surfaced_entities)逐项相等;且同事件序列 fold 两次字节相等、乱序输入排序后一致。
- §19-#9: 同 (session,turn,check,roller,expression) 派生同 u64 种子→StdRng 产相同 rolls 序列(纯函数单测);live e2e 同输入同种子两次回合机械结果(check outcome + 资源 delta)完全一致;不同 check_id 派生不同种子产不同序列。
- §19-#10: 一个 verifier 拦截 reveal 的回合后,db 中无该 fact 的 PlayerLearnedFact 事件、无 player_party knows_true 边;玩家未被表现前不建立 PlayerLearnedFact。
- §19-#7: verifier 阻断输出后,本回合 ResolutionCommit 的机械事件(CheckResolved/ResourceChanged)仍持久存在、未被回退。
- §5.2 覆盖: ResourceChanged 在 write_resource_current 真实落账(资源变更回合后能在 list_domain_events 查到带 old/new/delta 的事件);ClockAdvanced/NpcActionResolved/DialogueSpoken variant 存在且 token round-trip 测试绿。
- 零回归: trpg-model domain_event 全部 token round-trip 测试绿;现有 14 种事件 write-through(含 live_domain_events_mechanical 的 DiceRolled/CheckResolved 幂等)与 explain 输出不变;反剧透 live_revealed_facts/live_surfaced_entities 绿。
- additive/幂等: 所有新事件用确定性 event_id + on-conflict-do-nothing,回合重放不产生重复行;新 write-through 全 fail-soft(append 失败仅 warn,绝不改主函数成败)。

**不可破坏**:
- 现有 14 个 DomainEventKind variant 的 token 字符串契约(domain_event.rs as_str/from_str_token,db text 列兼容铁律,绝不改名)。
- DiceRolled/CheckResolved 现有 write-through 及其幂等键 de_roll_{roll_id}/de_check_{check_id}(live_domain_events_mechanical 测试)。
- 生命周期事件 write-through(TurnStarted/TurnFinalized/TurnFailed/SceneTransitioned)与 execute.rs 的 emit 顺序、D2 失败隔离(append 失败绝不影响已发的 Delta…TurnComplete 序列)。
- seed_commitment 字段的对外语义(对结果的承诺哈希)与 DiceRollRecord schema 不变。
- 反剧透三分语义:ContextSurfaced(进 context)≠ PlayerExposed(玩家见过)≠ PlayerLearnedFact(玩家确知),list_surfaced_entities 绝不混入 ContextSurfaced。
- 现有 NeedBus/CANONICAL_TURN_PLAN 回合两 transport(CLI/API)的逐字节等价(BP1/BP2 hash 等价铁律),本阶段为 additive 不得改变在线路径输出。
- trpg-mechanics 不得新增对 trpg-runtime 的依赖(避免依赖环);roll_amount_dice 的确定性接入留 follow-up。

**依赖**:P2

**⚠️ 需架构师拍板(不让自动 agent 猜)**:
- 新事件 data 的字段命名/语义边界(extraction/projection SEMANTICS,design3 教训=人决策):ResourceChanged 是否带 turn_id(write_resource_current 当前无 turn_id 入参,透传需改 ~6 处调用方签名)、NpcActionResolved 在 World 层(P4)未落地时挂哪个结算点、DialogueSpoken 的 speaker/addressee/visibility 取自哪——这些不能让自动 agent 猜。
- 种子派生公式的确定性边界:seed = hash(session:turn:check:roller_id:expression) 去掉 result_json 后,是否要纳入 attempt/retry 计数以区分'同回合同检定重掷';以及对抗防御骰种子是否加 'defender' 盐(现 :2334 已有)——确定性契约由架构师拍板。
- 两提交点的物理实现形态:ResolutionCommit/PresentationCommit 是仅做'事件分组+提交时机'的逻辑边界(本阶段推荐,additive),还是要在 domain_events 加 commit_phase 列(需迁移 0038)——crate-boundary/schema 决策。
- PresentationCommit 的前置条件强度:'verifier 未阻断' 是否足够,还是必须'念白文本里确含该实体/事实的可见提及'(更严但需文本核验钩子,设计4 附录提醒'防太严有害')——这是产品级 spoiler 严格度决策。
- event-fold/replay 引擎的归属 crate:本阶段放 trpg-runtime::event_fold,但 设计4 §17 目标是 trpg-runtime::kernel::{event_store,projections};是否现在就按 kernel/ 子模块布局,还是 P7 结构重构时再迁——crate 边界决策(design3 教训=crate-boundary 是人决策)。
- replay parity 的 projection 覆盖范围:首批只做 PlayerExposure/ContextSurfaced/PlayerKnowledge 3 个,§5.2 列的 WorldState/Actor/Scene/NpcMind/Relationship/Story/Obligation/Clock Projection 是否要在本阶段全部 fold 化(影响工作量与 §19-#8 验收广度)——由架构师定 parity 的最小可信集。

---

## P7 抽层间 Ports + crate 边界(结构重构,殿后)

**目标**:在 P1–P6 把行为切分(Adjudicator/Narrator)、commit 边界、Knowledge/Policy、World、Director、事件溯源都做稳之后,做最后一步纯结构重构:把已经稳定的各层抽成 §14 的 6 个 typed port(RulesPort/WorldPort/DirectorPort/NarratorPort/PolicyPort/KernelPort),由一个新建的控制平面 concept(命名 TurnConductor,故意不叫 TurnOrchestrator 以免与 trpg-orchestrator::TurnOrchestrator 同名异职混淆)在 execute_turn 这个真实单一入口处中介所有跨层调用;并按 §17 收敛模块边界(trpg-runtime 内拉出 kernel/ 与 context/ 子模块、把 npc_*/world 反应类抽成 trpg-world、trpg-gm 收敛为 Orchestrator+Narrator 的薄壳)。本阶段是 additive-then-flip:先在现有 GmLoop/execute_turn 之上引入 port trait 包装现路径(零行为变更),逐个把直接耦合替换为经 port 调用,再做物理 crate 移动。codex 明确反对先铺 ports/TurnWorkspace,故放最后做——此时各层语义已固化,trait 形状不会反复返工。

**现状(grounded)**:真实单一入口=execute_turn(gm: GmLoop, req, plan)(crates/trpg-gm/src/execute.rs:82),两 transport 各自 GmLoop::new(...) 后调它:trpg-api/src/lib.rs:1515+1634、trpg-cli/src/main.rs:1108+1169、trpg-cli/src/agent_play.rs:93+134。run_pipeline(execute.rs:107)是过程式解释器,直接持 &mut gm 调 dispatch_deterministic / run_agent_loop,无任何 port 抽象。GmLoop(crates/trpg-gm/src/turn_loop.rs:55)是业务总汇:engine: RuntimeEngine + llm: Arc<dyn LlmClient> + tools: ToolRegistry + plugin/errata/obligations,既裁定又叙事(附录 A ROOT CAUSE)。RuntimeEngine(crates/trpg-runtime/src/lib.rs:125)只裹 db: Db,持 prepare_turn_context(lib.rs:374)产 CompiledContext。名字冲突已核实:trpg-orchestrator::TurnOrchestrator(crates/trpg-orchestrator/src/lib.rs:137)是 reduce_turn(lib.rs:146)做 gate/intent 协调的 reducer,被 trpg-runtime 内部调用(lib.rs:1312、1323),不是 §4 控制平面(附录 A/C 明确警告同名异职)。依赖方向已核实适合抽取:trpg-runtime 不依赖 trpg-gm(无环);trpg-director 仅被 trpg-runtime 依赖;world 候选文件已在 trpg-runtime: npc_mind.rs(121)/npc_relationship.rs(159)/npc_behavior.rs(181)/npc_synth.rs(338)/npc_profile.rs(95)/relationship_extraction.rs(338);kernel 候选:knowledge_projection.rs(434)/scene_projection.rs(1049)/truthgraph.rs(297)/memory_proposal.rs(1032)。trpg-runtime 是扁平模块集(无 kernel/context 子目录)。插件 host(crates/trpg-gm/src/plugin/types.rs:19-27, host.rs)hook 仅 ContextAssembly/AfterLlmStream/HeavyPostprocess、贡献仅 4 类(PromptBlock/ContextFilter + 2),与 §14 PolicyPort 横切定位一致。前提:P1 已产 NarrationPacket/AdjudicationPacket(行为已切),P4 已有 World packet,P5 已有 Director 故事结构,P6 已有 KernelPort 雏形候选(两提交点),否则本阶段 port 形状无稳定语义可固化。

**步骤**:
1. 步骤 0(前置闸,不写代码):确认 P1–P6 全部 merge 且 live e2e 绿——port 抽象只能在语义固化后做。若 Narrator/Adjudicator(P1)、commit 边界(P2)、World packet(P4)、Director 故事结构(P5)、两提交点(P6)任一未稳定,本阶段不开工(写进 handoff 阻塞,不要先铺空 trait)。
2. 步骤 1(新建控制平面 concept,additive):在 trpg-gm 新建 conductor.rs,定义 pub struct TurnConductor —— 故意不复用 TurnOrchestrator 这个名字(trpg-orchestrator::TurnOrchestrator 是 gate reducer,同名异职)。第一版 TurnConductor 不改行为:它只是把 run_pipeline(execute.rs:107)的过程式编排搬进 TurnConductor::conduct(self, req, plan, tx),内部仍直接调现有 gm.phase_* / run_agent_loop。execute_turn(execute.rs:82)改为构造 TurnConductor 并调 conduct,签名对 trpg-api/trpg-cli 三处调用点(lib.rs:1634/main.rs:1169/agent_play.rs:134)保持不变。验:这是纯搬迁,字节级等价。
3. 步骤 2(定义 §14 port trait,在 trpg-gm 新建 ports.rs,先只放 trait + 现路径 adapter,不接线):按设计4 §14 逐字落 6 个 trait:RulesPort{adjudicate}/WorldPort{simulate}/DirectorPort{plan}/NarratorPort{stream}/PolicyPort{evaluate}/KernelPort{snapshot,commit,build_view},全部 #[async_trait]。请求/返回类型复用 P1–P6 已建的真实类型(AdjudicationBundle/NarrationPacket/DirectorPlan 等),不新造平行类型。每个 trait 提供一个 thin adapter impl 包住现有路径:RulesPortAdapter 包 P1 Adjudicator、NarratorPortAdapter 包 P1 Narrator、KernelPortAdapter 包 RuntimeEngine 的 snapshot/commit/build_view。此步只是声明 + 包装,TurnConductor 还不经它们调。
4. 步骤 3(TurnConductor 持有 ports,逐个 phase 切换为经 port 调用,一次一层 + 一次一 PR,每次都 live e2e 比对):把 TurnConductor 字段从直接持 GmLoop 改为持 dyn 6 个 port(由 GmLoop 各部件构造)。先切 KernelPort(snapshot/commit/build_view 包 RuntimeEngine,最底层、最稳),再切 NarratorPort,再 RulesPort,再 PolicyPort(横切 hook,evaluate 在 ContextAssembly/AfterLlmStream/HeavyPostprocess 三 checkpoint 调),World/Director 视 P4/P5 完成度切。每切一层:跑 cargo test -p trpg-gm + 受影响 crate,跑 CoC(:54347)/Cyberpunk(:54346)真库 live 回合,断言 §19-#1/#5/#7 不退化(换 Narrator 不动状态、Rules 结果不被文本覆盖、Policy 阻断不破已提交状态)。
5. 步骤 4(落 §14 禁止规则=编译期/审查期强制单向):TurnConductor 是唯一允许跨层 await 的处。强制 Director 不直接调 DB、World 不直接调 Rules executor、Narrator 不调 apply_damage、Plugin 不改 KnowledgeEdge——通过 (a) port trait 不暴露这些方法 + (b) 把 NarratorPort impl 的依赖收窄到只读 packet(无 Db/无 tools) + (c) 加一条 scripts/ 守卫脚本 grep 禁止 narrator/director 模块直接 use trpg_db::Db 或 apply_*(沿用现有 no_*_hardcode.sh 守卫模式)。这是把附录 C #2 commit 边界(只有 runtime-owned typed services 能 commit,Narrator 不能)落成结构约束。
6. 步骤 5(§17 模块边界,trpg-runtime 内拉 kernel/ 与 context/ 子目录,纯 mod 移动 + pub use 回桥,零跨 crate 变更):在 crates/trpg-runtime/src/ 下建 kernel/ 子目录,把 event/projection/knowledge/scene_time 相关模块(knowledge_projection.rs、scene_projection.rs、truthgraph.rs、memory_proposal.rs,以及 P6 落的 event_store/projections/transaction/view_builder)move 进去并按 §17 命名(kernel/event_store · projections · transaction · view_builder · knowledge · memory · scene_time · scheduler);建 context/ 子目录,把 *_need_resolver.rs + prepare_turn_context 拆出的分层 packet 投影 move 进去(context/rules_context · world_context · director_context · narration_context · verifier_context,先复用 prepare_turn_context 投出 packet,按附录 C #4 不拆五套 compiler)。lib.rs 顶部加 pub use 回桥,保持现有外部 import 路径不变(零 downstream 变更)。
7. 步骤 6(§17 新建 trpg-world crate,先 module-first 再决定独立,把 npc_*/world 反应类从 trpg-runtime 抽出):新增 crates/trpg-world,把 npc_mind.rs/npc_relationship.rs/npc_behavior.rs/npc_synth.rs/npc_profile.rs/relationship_extraction.rs 以及 P4 落的 world_reaction/faction/clock/information_propagation move 进去。依赖方向核实安全:trpg-runtime 当前依赖 trpg-director 且 trpg-runtime 不被 trpg-director 依赖、trpg-runtime 不依赖 trpg-gm——trpg-world 依赖 trpg-db/trpg-model/trpg-llm,trpg-runtime 反过来依赖 trpg-world,无环。WorldPortAdapter(步骤 2)改为包 trpg-world 的入口。先保留 pub use 回桥再逐步删 re-export。若拆出后发现耦合过密则按 §17 原话停在 module 边界(暂不独立 crate),把决定写进 handoff。
8. 步骤 7(§17 trpg-gm 收敛为 Orchestrator+Narrator 薄壳——这是终态,放最后,只有前 6 步全绿才做):把 trpg-gm 里属于 Rules/World/Director 的业务模块迁出到对应 crate(tools/check·effect·mechanic·settle → Rules 层;npc.rs → trpg-world;world.rs → trpg-world/Director),trpg-gm 最终只留 TurnConductor(原 §17 说的 TurnOrchestrator 角色)+ Narrator + SSE TurnEvent adapter(turn_event.rs/stream.rs)+ LLM invocation coordination。插件 Host 按 §17 决定迁 trpg-runtime::plugin 或独立 trpg-policy(本阶段可暂留 trpg-gm,把决定标 human_decision)。每迁一块跑全 workspace cargo test + live e2e。
9. 步骤 8(验收锚定 §19 + 守卫):补/接 §19-#1(替换 Narrator 模型不改 HP/场景/知识/NPC——由 NarratorPort 只读 packet 结构性保证)、#2(关 Director 仍能跑——DirectorPort 可注入 fallback impl)、#3(Director 不能选 World 没提供的候选——DirectorPort 输入仅 WorldReactionSet)、#5(Rules 结果不被 Narrator 文本覆盖——commit 在 NarratorPort 之外)、#7(Policy 阻断不破已提交状态——PolicyPort 横切 hook 不持 commit 权)。把这些做成 trpg-gm 集成测试 + 守卫脚本,并在 handoff 记录哪些是结构性保证、哪些靠测试。

**关键文件**:
- `crates/trpg-gm/src/execute.rs (execute_turn:82 单一入口 / run_pipeline:107 待搬进 TurnConductor)`
- `crates/trpg-gm/src/conductor.rs (新建:TurnConductor 控制平面 concept,勿用 TurnOrchestrator 名)`
- `crates/trpg-gm/src/ports.rs (新建:§14 六 port trait + 现路径 adapter)`
- `crates/trpg-gm/src/turn_loop.rs (GmLoop:55 业务总汇,逐步拆为 port 部件)`
- `crates/trpg-runtime/src/lib.rs (RuntimeEngine:125 / prepare_turn_context:374,kernel·context 子模块的拆分源)`
- `crates/trpg-runtime/src/{npc_mind,npc_relationship,npc_behavior,npc_synth,npc_profile,relationship_extraction}.rs (trpg-world 抽取源)`
- `crates/trpg-runtime/src/{knowledge_projection,scene_projection,truthgraph,memory_proposal}.rs (kernel/ 子模块抽取源)`
- `crates/trpg-orchestrator/src/lib.rs (TurnOrchestrator:137/reduce_turn:146 — 同名异职 reducer,勿复用其名)`
- `crates/trpg-director/src/lib.rs (ActionableSituationDirector → DirectorPort 包装目标)`
- `crates/trpg-gm/src/plugin/{types.rs,host.rs} (PolicyPort 横切 hook 的现有实现)`
- `crates/trpg-api/src/lib.rs:1515+1634 / crates/trpg-cli/src/main.rs:1108+1169 / crates/trpg-cli/src/agent_play.rs:93+134 (execute_turn 三调用点,签名须稳定)`
- `crates/trpg-world/Cargo.toml (新建 crate)`

**验收**:
- §19-#1:用 fake NarratorPort impl 替换叙事模型,运行一回合后 HP/scene_id/KnowledgeEdge/NPC 状态字节级不变(NarratorPort 只读 packet,无 Db/tools 句柄,结构性保证 + 集成测试断言)
- §19-#5:构造一回合让 Adjudicator 产出 mechanical_result,断言 NarratorPort 输出文本不能改写已 commit 的机械事实(commit 发生在 NarratorPort 调用之外,经 KernelPort)
- §19-#7:PolicyPort 在 AfterLlmStream 返回阻断,断言已提交的机械状态(turns/ledger/domain_events)不回滚、不损坏
- §19-#3:DirectorPort::plan 的输入仅含 WorldReactionSet 候选,测试断言 Director 无法选出 World 未提供的候选(类型层面不可达)
- §19-#2:注入默认 DirectorPort fallback(§18 Respond→Consequence→Choice 通用 Beat),关闭真实 Director,回合仍完整产出叙事不空白
- 结构:执行 scripts 守卫脚本,确认 narrator/director 模块无直接 use trpg_db::Db / 无直接调 apply_* mutation;TurnConductor 是唯一跨层 await 处
- 回归:cargo test 全 workspace 绿(与 P6 完成时基线测试数对齐),CoC(:54347)+Cyberpunk(:54346)真库各跑 ≥2 回合 live e2e 走遍改动路径无 panic、无行为退化
- 边界:execute_turn 三调用点(trpg-api/trpg-cli×2)签名未变;trpg-world 抽出后 cargo build 无依赖环(trpg-runtime→trpg-world 单向)
- 回桥:kernel//context/ 子模块化与 trpg-world 抽取后,所有 downstream import 经 pub use 回桥编译通过,零 downstream 文件改动(纯 mod 移动)

**不可破坏**:
- execute_turn(execute.rs:82)作为 CLI/API 唯一回合入口的契约不变(R1 已确立),三调用点签名稳定
- CANONICAL_TURN_PLAN 15-phase 数据化解释器语义(turn_plan.rs)+ critical/heavy 尾段拆分(execute.rs run_pipeline)不被 port 化破坏
- P1 的 Adjudicator/Narrator 行为切分 + 实时流根因修复(ContentDelta 不在校验前流出)必须保持,port 化不得让念白回退到校验前直流
- P2 commit 边界(只有 runtime-owned typed services 能 commit,Narrator 不能)+ P6 两提交点(ResolutionCommit/PresentationCommit)语义不变
- BP1/2/3(CacheZone Prefix/PinnedMiddle/DynamicTail)逐区 hash/预算 + prepare_turn_context 统一的 Need/可见性/dedup/trace 不被拆 compiler 打散(附录 C #4:暂复用 prepare_turn_context 投 packet)
- NeedBus 逐 kind 点接路径 + 插件 host propose-not-commit + 5 内置策略插件行为不变
- trpg-runtime 不依赖 trpg-gm 的现有无环约束;trpg-orchestrator::TurnOrchestrator::reduce_turn(被 trpg-runtime lib.rs:1312/1323 调)的 gate/intent 协调路径不被误改
- 现有 journey/replay/projection 测试(trpg-harness/tests journey_*、trpg-db/tests live_knowledge_edges/memory_proposal)继续绿

**依赖**:P1、P2、P3、P4、P5、P6

**⚠️ 需架构师拍板(不让自动 agent 猜)**:
- 控制平面 concept 的最终命名与归属:本计划提议 trpg-gm::TurnConductor(避开同名异职的 trpg-orchestrator::TurnOrchestrator),但 §17 原文写 trpg-gm 留 TurnOrchestrator——是否接受改名、是否最终独立成 trpg-conductor crate,需架构师拍板(命名是契约,不让 agent 猜)
- trpg-world 的 crate 边界:§17 说先 module-first、稳定后再决定是否独立 crate。哪些 npc_*/world 反应模块真正属于 World 层、哪些留 trpg-runtime(如 npc_synth 懒生成跨 Rules/World)、抽出后耦合是否过密以致停在 module 边界——这是 design3 教训点名的 crate-boundary 人工决策,不可自动 agent 决定
- kernel/ vs context/ 的归属语义:scene_projection(1049 行)/knowledge_projection(434)/truthgraph 是放 kernel/(状态权威)还是 context/(视图编译)?§17 把 projection 归 kernel、view 编译归 context,但现有文件职责混合,切分线需架构师定(投影/视图 SEMANTICS 是人工决策,design3 已点名)
- PolicyPort 作为横切 hook(附录 C #3)与 §14 顺序 trait 形状的张力:PolicyPort::evaluate 在 3 个 checkpoint 调,是否仍用单一 trait + checkpoint enum 入参,还是拆成多 hook trait?需架构师确认 port 形状不变成顺序第六层
- 插件 Host 的最终归属:§17 给两选项(trpg-runtime::plugin 或独立 trpg-policy)。插件不仅服务 Narrator 也服务 Kernel/Rules/World/Director,迁移目标影响 PolicyPort 与各层耦合方向,需架构师选定
- port adapter 是否容许保留 GmLoop 作为过渡聚合体,还是必须在本阶段彻底拆解 GmLoop:55(engine/llm/tools/plugin/obligations 五部件分别归不同 port)——彻底拆解风险高,是否分两个 PR(先 conductor+ports 接线、后拆 GmLoop)需架构师定节奏
