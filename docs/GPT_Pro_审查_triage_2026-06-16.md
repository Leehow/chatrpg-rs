# GPT Pro 源码审查 triage（2026-06-16）

> 被审：`chatrpg-rs-v120-for-gptpro.tar.gz`（main = R1+R2 + R5 设计文档；GPT Pro 静态源码审查，未编译）。
> 基线现状：R1（execute_turn）+R2（Need bus）+**R5（postprocess 分层，已合并 8af38c0）**+R1 死代码清扫（c3525de）全在 main。
> 本文 = 对 GPT Pro 10 条 findings 逐条甄别 + 处理状态/计划。GPT Pro 已避开已列事项（R5/R3/R4/NeedBus 合批/大文件拆分/死代码），聚焦清单外的性能/正确性/并发/API/错误处理/可观测性。

## 已处理 ✅
- **P0-1 CarryoverDebt 时序 + retro debt 不落账**：has_pending 在 verify 前算 → 本回合新生 retro debt 触发的 carryover 被跳。**已修**（R5 #B，commit 8af38c0）：carryover 改在 heavy（verify 后）按实时 `gm.has_pending_obligations()` 经 `should_run_carryover(signal, has_pending)` 重判；should_run_carryover 单测三分支。GPT Pro 的"结构化持久化（gm_obligations 表硬执行）"= 设计演进非 bug（现机制 carryover_memory_event → 下回合 memory 块给 agent 是 agent-driven 记账的既定设计）→ 列入 R3-adjacent 候选，非本次。
- **P2-2 SceneTransition 事件发不出**：R5 T1 已顺手修（scene_navigate_critical 返 SceneNavCommit → execute.rs 真发 TurnEvent::SceneTransition，修了 R1 的 const-None TODO）。✅

## 待办（按 ROI/优先级）
- **P0-2 引擎规则集/模组硬编码（最高 ROI，最对齐核心理念）**：GPT Pro 发现比预期广——trpg-combat（inferred_homecoming_tech_dv 返 14/12、npc.scav_boss/athena_drone 别名、ruleset_id.contains 选 combat mode/profile/damage band）、trpg-referee/director/object/material/mechanics 多处。**违背"零规则集硬编码"头号理念**（引擎应按通用检定模型分支、规则差异来自 parsed kernel/data）。**计划**：迁到数据层（RuleKernel.combat_profile/referee_value_bands/search_profile/mode_selection_policy + ModulePrepPacket.scene_entity_aliases/npc_actor_bindings/technical_option_table/module_dv_overrides），runtime 只读 profile.unwrap_or(generic)；加 CI `xtask no-engine-ruleset-hardcode` grep 守卫（仅允许 tests/fixtures/parser-source-id/data-overrides 出现规则集名）。**大、跨 6 crate、需设计 RuleKernel 数据结构**→ 作下一个 spec→plan→执行 收敛（隔离 worktree）。
- **P0-3 默认 lexical fallback 稀释语义优先（中）**：`TRPG_SEMANTIC_COMBAT_LEXICAL_FALLBACK_AUDIT` 默认 true → semantic 可用且 confident 时词法仍参与 exit/assessment/object/attack/director 路由；`looks_rule_or_module_sensitive` 用关键词（含 athena/drone/cable 模组词）决定发不发 RuleNeed。**计划**：默认改 false（仅 semantic 不可用/低置信才 fallback）；引 SemanticNeedClassifier 输出 {needs_rule/material/scene, confidence} 取代关键词；实体词进 ModulePrepPacket.entity_aliases。与 P0-2 同属"数据/语义化"方向，可同期或紧随。
- **P1-1 TurnEvent 无失败语义（中，可观测性）**：仅 Delta/Awaiting/Scene/Errata/PostprocessScheduled/TurnComplete，无 TurnFailed/TurnWarning；context_assembly/mode_inference/LLM stream/forced-prose 失败 fail-closed 后仍发 TurnComplete(空 Narration) → 用户见"空白成功"。**计划**：加 TurnFailed{phase,message,recoverable}/TurnWarning；turns.status 写 failed_context/failed_llm_stream 等；API SSE 映射 event:error。（呼应"fail-closed 不应伪装成功"。）
- **P1-2 API 不从 DB 载最近历史（中，transport 一致性）**：play_turn_sse `history: vec![]` + 仅用客户端传的 recent_transcript；前端只传 user_input 时第二回合丢上下文。**计划**：默认从 DB 载最近 N turns transcript（history_policy: server_recent|client_supplied|none，默认 server_recent）。
- **P1-3 SSE 断开取消语义不清（中，成本/一致性）**：execute_turn 内外两层 spawn，tx.send 多 `let _=` 吞错；客户端断开后是否续烧 LLM/续写状态无策略。**计划**：CancellationPolicy{CancelBeforeStateMutation|ContinueAfterStateMutation|AlwaysContinue}；track state_mutated/first_delta_sent；未变更状态前断开→取消省 token，已落账→续 critical finalize + 记 client_disconnected。
- **P1-4 NeedOutcome.source_refs 被丢弃（中，可审计性）**：prepare_turn_context 多处仅 `blocks.extend(outcome.blocks)`，resolver 的 source_refs 丢失 → Need 级 acquisition trace 断。**计划**：merge_need_outcome helper（source_refs 注入 block.source_refs/metadata need_kind+reason），或低 token trace block。非 R3 字段级 verifier，只是 Need 级 trace。
- **P1-5 错误吞噬无分级（中）**：大量 unwrap_or_default/let _/warn 不分 critical。**计划**：PhaseErrorPolicy{AbortTurn|EmitWarningContinue|BackgroundWarnOnly}（context_assembly/save_turn→Abort；memory/audit→WarnContinue；learning→BackgroundWarn）+ metrics trpg_phase_errors_total/trpg_turn_failed_total。与 P1-1 配套。
- **P2-1 LLM SSE parser O(n²) + 静默 parse error（低）**：buffer = buffer[pos+1..].to_string() 反复复制；Err(_)=>continue 吞 JSON error。**计划**：BytesMut+memchr 或 buffer.drain(..pos+1)；记 llm_stream_parse_errors_total。
- **P2-3 包内 macOS AppleDouble/.DS_Store（低，卫生）**：~420 个 ._*/.DS_Store。**计划**：.gitignore 加 .DS_Store/._*；release 打包 COPYFILE_DISABLE=1 + --exclude。（快速项，可即办。）

## 推荐推进顺序
1. **P0-2 引擎去硬编码**（spec→plan→执行，最高 ROI + 头号理念，下一个大收敛）。
2. **P0-3 lexical fallback 默认关 + SemanticNeedClassifier**（与 P0-2 同方向，可并入或紧随）。
3. P1-1+P1-5（失败语义 + 错误分级，配套，可观测性）。
4. P1-2/P1-3/P1-4（API 历史 / 取消语义 / source_refs trace）。
5. P2-1/P2-3（SSE parser / 打包卫生，快速项）。
</content>
