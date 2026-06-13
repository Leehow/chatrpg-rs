use crate::errata::ErrataMemory;
use crate::gate::GateResolverFn;
use crate::ledger::TurnLedger;
use crate::obligations::{carryover_memory_event, ObligationLedger, RetroDebtKind, RetroactiveEffectDebt};
use crate::prompts::{load_gm_skill_with_mode, DynamicTailInput, TurnMessages};
use crate::stream::RedactingBuffer;
use crate::tools::{AwaitingPlayerRoll, SceneDeepExtractFn, ToolCtx, ToolRegistry};
use anyhow::Result;
use chrono::Utc;
use futures_util::StreamExt;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use trpg_interaction::InteractionLifecycleKernel;
use trpg_llm::{LlmClient, StreamEvent, ToolChoice};
use trpg_model::{ChatMessage, CompiledContext, ContextRequest, MemoryEvent, MemoryKind, RuntimeState, Visibility, WorldEventKind};
use trpg_runtime::RuntimeEngine;
use uuid::Uuid;

/// 上下文准备注入 seam（单测绕开真 DB；生产装配恒 None）。
pub type CtxProviderFn = Arc<dyn Fn(&ContextRequest, &RuntimeState) -> CompiledContext + Send + Sync>;

#[derive(Debug, Clone)]
pub struct LoopConfig { pub max_tool_rounds: u8, pub repeat_finding_threshold: u8 }
impl Default for LoopConfig { fn default() -> Self { Self { max_tool_rounds: 8, repeat_finding_threshold: 3 } } }
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome { Narration(String), AwaitingPlayerRoll { check_id: String, prompt_public: String } }
pub struct GmLoop { pub engine: RuntimeEngine, pub llm: Arc<dyn LlmClient>, pub tools: ToolRegistry, pub cfg: LoopConfig, pub data_dir: PathBuf, pub scene_extractor: Option<SceneDeepExtractFn>, pub ctx_provider: Option<CtxProviderFn>, pub gate_resolver: Option<GateResolverFn>, pub errata: ErrataMemory, pub obligations: ObligationLedger }
pub struct GmTurnInput<'a> { pub request: &'a ContextRequest, pub state: &'a RuntimeState, pub user_input: &'a str, pub history: &'a [ChatMessage], pub recent_transcript: Option<&'a str> }
impl GmLoop {
    pub fn new(engine: RuntimeEngine, llm: Arc<dyn LlmClient>, tools: ToolRegistry, cfg: LoopConfig, data_dir: PathBuf) -> Self { let errata = ErrataMemory::new(cfg.repeat_finding_threshold); Self { engine, llm, tools, cfg, data_dir, scene_extractor: None, ctx_provider: None, gate_resolver: None, errata, obligations: ObligationLedger::default() } }
    pub async fn run_gm_turn(&mut self, input: GmTurnInput<'_>, on_delta: &mut (dyn FnMut(&str) + Send)) -> Result<TurnOutcome> {
        // —— 1. 确定性头部（spec §4 四项顺序：record → refresh → reconcile → gate 结算）——
        let _ = self.engine.record_world_event(&input.request.session_id, Some(&input.request.turn_id), None, WorldEventKind::PlayerAction, json!({"input": input.user_input}), Visibility::GmOnly).await;
        let _ = self.engine.refresh_actor_live_derived(&input.request.session_id, input.request.viewer.actor_id.as_deref().unwrap_or("pc.current")).await;
        // prepare_turn_context 内部也会 reconcile，但那发生在 gate 结算之后，
        // 不满足 spec 顺序——此处显式调用（幂等，与既有调用方同款 let _ =）。
        let _ = InteractionLifecycleKernel::new(self.engine.db.clone()).reconcile_session(&input.request.session_id).await;
        let mut ledger = TurnLedger::new();
        let mut resolved_gate_facts = Vec::new();
        // gate 结算（含裸 "roll" 兜底与 Err 折叠两个 e2e must-fix + C7 盖章契约的
        // effect_policy 强制执行）收口在 gate.rs 单点。
        crate::gate::resolve_pending_gate(&self.engine, self.gate_resolver.as_ref(), input.request, input.state.scene_id.as_deref(), input.user_input, &mut ledger, &mut resolved_gate_facts).await;
        // B5 第三触发通路：TurnStart hook dues 在 prepare_turn_context 之前落库
        // （B3 BP3 投影 / B6 债务装载同回合可见）。失败 unwrap_or_default()
        // ——头部任何 watcher 故障不得阻断回合（与既有 `let _ =` 风格一致）。
        let hook_dues = trpg_mechanics::RefereeCombatService::new(self.engine.db.clone())
            .dues_for_hook(&input.request.session_id, &input.request.turn_id, &input.request.ruleset_id, &trpg_mechanics::watcher::HookEvent::TurnStart)
            .await
            .unwrap_or_default();
        // —— 刺激驱动检定预 pass（J2 SAN 修复）：目录 when_to_use × 本回合虚构
        //    内容（玩家输入+上回合叙事尾段）语义命中 → 阻塞债务候选，经 watcher
        //    admit_dues（与 hook 通路同套抑制+落库）收编。fail-closed：门关/
        //    无目录/LLM 失败 → 空，头部绝不因它阻断回合。
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
        // —— 三期姿态推导（spec §4.1）：active state_frame → mode；无 frame /
        //    frame 种类无 mode 包 → None（默认叙事姿态，与二期字节级一致）。db
        //    失败 unwrap_or_default（与头部 watcher 同款绝不阻断回合）；mode 包
        //    存在但 manifest 损坏 → Err fail-closed（配置错误终止回合，与
        //    gm_skill 装载同级）。
        let active_frames = self.engine.db.list_active_state_frames(&input.request.session_id, 8).await.unwrap_or_default();
        let mode_manifest = crate::mode::active_mode_manifest(&self.data_dir, &active_frames)?;
        let mode_id = mode_manifest.as_ref().map(|m| m.mode_id.clone());
        // —— B6 债务装载（spec §5.3）：turn-scope 豁免过期 → 跨回合遗留（持久
        //    字段 + db open dues）与 TurnStart hook dues 统一进 ObligationLedger
        //    （按 due_id 去重——watcher 产出即落库，两路可能是同一条）。db 失败
        //    unwrap_or_default：债务装载绝不阻断回合，in-memory 清单仍兜底。
        self.obligations.begin_turn(&input.request.turn_id);
        // mode 退出结算义务（spec §4.4）：回合头部凭 manifest 幂等 re-seed（跨
        // 进程重启不丢；只门 exit_mode，绝不门叙事轮——B6 blocking() 不含它）。
        if let Some(m) = &mode_manifest {
            self.obligations.ensure_mode_exit_obligations(&m.mode_id, &m.exit_obligations);
        }
        let leftover_dues = self.engine.db.list_open_mechanic_dues(&input.request.session_id).await.unwrap_or_default();
        self.obligations.absorb_dues(leftover_dues);
        self.obligations.absorb_dues(hook_dues);
        self.obligations.absorb_dues(stimulus_dues);
        // 上回合遗留债务 → 本回合 BP3 尾段（errata 之后、Player Input 之前）。
        let obligations_block = self.obligations.carryover_block();
        // —— 2. 上下文（agent_loop_protocol 强制置位 → BP1 出 agent-loop 版 engine protocol）——
        let mut state_agent = input.state.clone();
        state_agent.agent_loop_protocol = true;
        let compiled = match &self.ctx_provider {
            Some(provider) => provider(input.request, &state_agent),
            None => self.engine.prepare_turn_context(input.request, &state_agent, Some(input.user_input), input.recent_transcript).await?,
        };
        // §6.1 第 4 条 fail-closed：prefix/pinned 超预算是配置错误，终止回合而非静默裁剪。
        crate::prompts::validate_compiled_budget(&compiled, input.request)?;
        // gm_skill 是 agent 路径硬依赖：fail-closed，Err 终止回合（契约第 7 节，
        // 绝不 unwrap_or_default；data_dir 由装配方传 default_data_dir()）。
        // 三期 §4.2：四级合并（mode=None 退化两级，字节不变——缓存稳定硬回归）。
        let mut gm_skill = load_gm_skill_with_mode(&self.data_dir, &input.request.ruleset_id, mode_id.as_deref())?;
        // —— 三期 §4.5 目录联动（终审返工）：mode 激活 → kernel mechanics_catalog
        //    经 manifest.catalog_filter 过滤，渲染为紧凑节拼进 mode 提示层尾部
        //    （与 mode 提示同生命周期、同有因失效；BP1 目录索引保持 mode 无感
        //    不动——缓存设计 §4.2 RarelyChanged 不随 mode 失效）。无 kernel/
        //    空过滤结果/db 失败 → 不注入（fail-closed 不阻断回合，warn 可观测）。
        if let Some(manifest) = &mode_manifest {
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
        let mut errata_blocks = Vec::new();
        if let Some(block) = self.errata.errata_block() { errata_blocks.push(block); }
        if let Some(block) = self.errata.standing_reminder_block() { errata_blocks.push(block); }
        // —— 三期 §5 novelty 复用（批2）：姿态激活时把 frame 状态里的已用战术
        //    渲染为 BP3 尾段事实块（一期 novelty director 数据，零额外 LLM 调用）；
        //    mode=None 绝不注入（二期行为字节级一致）。
        if mode_id.is_some() {
            if let Some(block) = crate::tools::frame::novelty_block(&active_frames) { errata_blocks.push(block); }
        }
        let tail = DynamicTailInput { user_input: input.user_input, resolved_gate_facts: &resolved_gate_facts, errata_blocks: &errata_blocks, obligations_block: obligations_block.as_deref() };
        let mut messages = TurnMessages::assemble(&compiled, &gm_skill, input.history, &tail);
        // —— 三期 §4.3 工具按 mode 组装：mode 激活 → for_mode（基础 14 +
        //    manifest.extra_tools；schema 变化 = 前缀缓存有因失效豁免项）；
        //    mode=None → 沿用装配方注入的 self.tools（二期行为字节级一致 +
        //    单测替身不被覆盖）。未知工具名 fail-closed Err 终止回合。
        let mode_tools = match mode_id.as_deref() {
            Some(mode) => Some(ToolRegistry::for_mode(&self.data_dir, Some(mode))?),
            None => None,
        };
        let schemas = mode_tools.as_ref().unwrap_or(&self.tools).schemas();
        // —— 三期 §4.6 节拍参数：tempo.max_tool_rounds per-mode 覆盖 LoopConfig
        //    （None = 沿用默认）；effect_closure_per_cluster 收紧交锋簇节拍
        //    （批2：轮前债务判定时按账本重算簇闭合状态）。
        let max_tool_rounds = mode_manifest.as_ref().and_then(|m| m.tempo.max_tool_rounds).unwrap_or(self.cfg.max_tool_rounds);
        let effect_closure_per_cluster = mode_manifest.as_ref().and_then(|m| m.tempo.effect_closure_per_cluster).unwrap_or(false);
        let mut visible_text = String::new();
        let mut awaiting: Option<AwaitingPlayerRoll> = None;
        let mut narrated = false;
        // —— 3. 工具轮（循环内恒 ToolChoice::Auto；ctx 限定块级作用域，块结束
        //     即释放对 self 的借用，收尾 &self 方法才可调用）——
        // B6：obligations 暂入互斥单元——dispatch 链上 ToolCtx 是共享引用，
        // waive_obligation 需要点改清单；块结束取回持久字段。
        let obligations_cell = std::sync::Mutex::new(std::mem::take(&mut self.obligations));
        {
            let tools = mode_tools.as_ref().unwrap_or(&self.tools);
            let ctx = ToolCtx { engine: &self.engine, request: input.request, state: &state_agent, scene_extractor: self.scene_extractor.as_ref(), obligations: Some(&obligations_cell), data_dir: Some(&self.data_dir), current_mode: mode_id.as_deref() };
            'rounds: for round in 0..max_tool_rounds {
                // §6.1 第 5 条可观测链前半：每轮请求前记缓存锚点（后半 cached_tokens
                // 由下方 Usage 分支在同一 gm_cache target 下记录，relay 不透传则缺省）。
                tracing::info!(
                    target: "gm_cache",
                    session_id = %input.request.session_id,
                    turn_id = %input.request.turn_id,
                    prefix_hash = %compiled.prefix_hash,
                    pinned_hash = %compiled.pinned_hash,
                    request_prefix_hash = %messages.prefix_byte_hash(2),
                    tool_round = round,
                    "gm agent prompt cache anchors"
                );
                // —— B6 债务门控前置（spec §5.3；D2 真流式终审修复）：债务状态只能
                //    被工具调用改变、纯叙事轮内不可能新增债务 ⇒ 轮前判定一次即够。
                //    blocking 为空 ⇒ 本轮纯叙事终态必然合法 ⇒ ContentDelta 经
                //    RedactingBuffer 后立即 on_delta 直通（恢复一期逐 token 流式）；
                //    blocking 非空 ⇒ 本轮绝不能成为交付的叙事终态 ⇒ content 实时
                //    丢弃（不缓冲不流出不进消息历史），回填债务观察让 agent 用工具
                //    处理或 waive 后才放行。round 0 不回填：同一清单刚由
                //    obligations_block 进了本回合 dynamic tail，相邻重复无信息增量。
                let blocked = {
                    let mut obligations = obligations_cell.lock().unwrap_or_else(|p| p.into_inner());
                    // 账本同步：有结果的契约 settle，未结算的升格 open-check 债务
                    // （request_player_roll gate 开着走 awaiting 终态不经此处，不算债）。
                    let settled: std::collections::BTreeSet<&str> = ledger.snapshot().check_results.iter().map(|r| r.check_id.as_str()).collect();
                    for contract in &ledger.snapshot().check_contracts {
                        if settled.contains(contract.check_id.as_str()) { obligations.mark_check_settled(&contract.check_id); } else { obligations.record_open_check(&contract.check_id); }
                    }
                    // 追溯债务清偿（spec §5.3"补 apply_effect 落账"半边）：前轮新落账
                    // effect 按 FIFO 结清追溯债务（effect_id 只消费一次，逐轮重扫安全）。
                    let effect_ids: Vec<String> = ledger.snapshot().effect_contracts.iter().map(|e| e.effect_id.clone()).collect();
                    obligations.settle_retro_debts_with_effects(&effect_ids);
                    // Check 类追溯债务（MissingCheck"补 roll_check"半边）：本回合新
                    // 结算的检定按 FIFO 结清（同款逐轮重扫安全）。
                    let settled_check_ids: Vec<String> = settled.iter().map(|c| c.to_string()).collect();
                    obligations.settle_retro_debts_with_checks(&settled_check_ids);
                    // 三期 §4.6 交锋簇节拍收紧（批2）：紧节拍下"已结算检定但零效果
                    // 落账"（effect 契约 / track 落账 / 检定自带 committed patches
                    // 皆无）⇒ 簇未闭合 ⇒ 本轮不得成为叙事终态。tight=false（mode=None
                    // /幕间）恒清空——二期行为字节级一致。
                    let snap = ledger.snapshot();
                    let effects_booked = snap.effect_contracts.len()
                        + snap.parameter_impacts.len()
                        + snap.check_results.iter().filter(|r| !r.committed_patches.is_empty()).count();
                    obligations.update_cluster_closure(effect_closure_per_cluster, snap.check_results.len(), effects_booked);
                    match obligations.block_text() {
                        Some(block) => { if round > 0 { messages.push_system_observation(&block); } true }
                        None => false,
                    }
                };
                let mut stream = self.llm.stream_chat_with_tools(messages.to_request_messages(), schemas.clone(), ToolChoice::Auto).await?;
                let mut saw_tool = false;
                let mut redactor = RedactingBuffer::new(ledger.private_roll_tokens());
                while let Some(event) = stream.next().await {
                    match event? {
                        StreamEvent::ContentDelta(delta) => {
                            // 被门轮 content 实时丢弃（绝不缓冲、绝不流出）。
                            if !blocked {
                                let safe = redactor.push(&delta);
                                if !safe.is_empty() { on_delta(&safe); visible_text.push_str(&safe); }
                            }
                        }
                        StreamEvent::Usage(usage) => {
                            // §6.1 第 5 条可观测：relay 透传则记，缺省无此事件。
                            tracing::info!(target: "gm_cache", cached_tokens = ?usage.pointer("/prompt_tokens_details/cached_tokens"), "upstream usage reported");
                        }
                        StreamEvent::ToolCalls(calls) => {
                            saw_tool = true;
                            messages.push_assistant_tool_calls(&calls);
                            for call in calls {
                                let outcome = tools.dispatch(&ctx, &mut ledger, &call).await;
                                // 可观测性（J2 修复）：每次 dispatch 落 agent_tool_calls
                                // ——此前 GM loop 工具活动除骰子外全程不可见，评测
                                // 无法对账。落库失败 `let _ =` 吞，绝不阻断回合。
                                let output_value = serde_json::from_str::<serde_json::Value>(&outcome.content)
                                    .unwrap_or_else(|_| json!({"raw": outcome.content.as_str()}));
                                let status = if output_value.get("error").is_some() { "error" } else { "done" };
                                let _ = ctx.engine.db.insert_agent_tool_call(&trpg_model::AgentToolCallRecord {
                                    tool_call_id: outcome.tool_call_id.clone(),
                                    session_id: input.request.session_id.clone(),
                                    turn_id: input.request.turn_id.clone(),
                                    tool_name: outcome.name.clone(),
                                    visibility: Visibility::GmOnly,
                                    input_json: serde_json::from_str(&call.arguments).unwrap_or_else(|_| json!({"raw": call.arguments.as_str()})),
                                    output_json: Some(output_value),
                                    status: status.to_string(),
                                    error: None,
                                    created_at: Utc::now(),
                                }).await;
                                messages.push_tool_result(&outcome.tool_call_id, &outcome.name, &outcome.content);
                                if let Some(gate) = outcome.awaiting_player_roll {
                                    drain_redactor(&mut redactor, blocked, on_delta, &mut visible_text);
                                    awaiting = Some(gate);
                                    break 'rounds;
                                }
                            }
                            // 工具产物可能新增私骰 token：排空旧缓冲后按最新账本重建（混合轮防漏）。
                            // 混合轮 content 已实时直通（不是叙事终态，不受债务门控）。
                            drain_redactor(&mut redactor, blocked, on_delta, &mut visible_text);
                            redactor = RedactingBuffer::new(ledger.private_roll_tokens());
                        }
                        StreamEvent::Done { .. } => {}
                    }
                }
                drain_redactor(&mut redactor, blocked, on_delta, &mut visible_text);
                // 被门轮（blocked）无论纯叙事还是混合轮都不得成为终态：继续工具轮，
                // 下一轮轮前重新判定（工具可能已清债）。
                if saw_tool || blocked { continue 'rounds; }
                narrated = true;
                break 'rounds;
            }
        }
        self.obligations = obligations_cell.into_inner().unwrap_or_else(|p| p.into_inner());
        // —— 终态 A：request_player_roll gate ——
        if let Some(gate) = awaiting {
            // 流后校验是 loop 之后的无条件阶段（spec §4）：awaiting 终态前可能已
            // 流出含可见掷骰结果的叙事，仅在 visible_text 非空时跑（空文本无可对账内容）。
            if !visible_text.trim().is_empty() {
                self.verify_after_stream(input.request, &ledger, &visible_text).await;
            }
            let assistant_output = if visible_text.trim().is_empty() { gate.prompt_public.clone() } else { visible_text.clone() };
            self.finalize_turn(input.request, &compiled, input.user_input, &assistant_output, "awaiting_player_roll").await;
            return Ok(TurnOutcome::AwaitingPlayerRoll { check_id: gate.check_id, prompt_public: gate.prompt_public });
        }
        // —— 4. 仅当轮数自然耗尽且模型仍要工具：追加唯一一轮 ToolChoice::None 逼散文 ——
        if !narrated {
            let mut stream = self.llm.stream_chat_with_tools(messages.to_request_messages(), schemas, ToolChoice::None).await?;
            let mut redactor = RedactingBuffer::new(ledger.private_roll_tokens());
            while let Some(event) = stream.next().await {
                if let StreamEvent::ContentDelta(delta) = event? {
                    let safe = redactor.push(&delta);
                    if !safe.is_empty() { on_delta(&safe); visible_text.push_str(&safe); }
                }
            }
            let rest = redactor.finish();
            if !rest.is_empty() { on_delta(&rest); visible_text.push_str(&rest); }
            // B6：轮耗尽带债强制叙事 → 债务落勘误记忆（绝不静默丢失；BP3 注入
            // 由下回合 carryover_block 完成）。落库失败 `let _ =` 吞错。
            if let Some(block) = self.obligations.carryover_block() {
                let event = carryover_memory_event(input.request, &block);
                let _ = self.engine.db.save_memory_event(&event).await;
            }
        }
        // —— 5. 流后校验（不阻塞交付：叙事已全部流出）——
        self.verify_after_stream(input.request, &ledger, &visible_text).await;
        // —— 6. 确定性收尾 ——
        self.finalize_turn(input.request, &compiled, input.user_input, &visible_text, "ready").await;
        Ok(TurnOutcome::Narration(visible_text))
    }

    /// 流后校验（spec §4 第 5 步）：NarrationVerifier 对账已流出全文 → 勘误记忆
    /// （注入下一轮 dynamic tail）→ 新增勘误折 MemoryEvent 持久化（tags 含
    /// "gm_errata"；落库失败 `let _ =` 吞错——叙事已交付，校验绝不反向中断回合）。
    async fn verify_after_stream(&mut self, request: &ContextRequest, ledger: &TurnLedger, visible_text: &str) {
        let verifier = trpg_agent::NarrationVerifier;
        // B7 语义决策：agent 不显式声明引用，引擎代填"已落账事实全集"
        // （ledger_id_set 物化，排序保证确定性）——结构化核对退化为"声称的
        // id 必在账本"恒真 + 子串回退被关闭；`fallback:substring_scan: `
        // 标注路径只在账本为空（id 全集为空 → refs 为空）时出现。
        let referenced_ledger_ids = { let mut ids: Vec<String> = trpg_agent::ledger_id_set(ledger.snapshot()).into_iter().collect(); ids.sort(); ids };
        let submission = trpg_agent::FinalNarrationSubmission { player_visible_text: visible_text.to_string(), mechanical_claims: vec![], referenced_ledger_ids };
        let result = verifier.verify(ledger.snapshot(), &submission);
        let entries = self.errata.record(&request.turn_id, &result.findings);
        if !entries.is_empty() {
            let event = self.errata.to_memory_event(request, &entries);
            let _ = self.engine.db.save_memory_event(&event).await;
        }
        // B6：InventedEffect → Effect 追溯债务（补 apply_effect 落账）；J2 修复：
        // MissingCheck → Check 追溯债务（补 roll_check）。叙事已流出不可回收，
        // 债务进下回合清单——处理或 waive_obligation 带理由，债务必清。
        let debts = result.findings.iter()
            .filter_map(|f| {
                let kind = match f.kind {
                    trpg_agent::VerifierFindingKind::InventedEffect => Some(RetroDebtKind::Effect),
                    trpg_agent::VerifierFindingKind::MissingCheck => Some(RetroDebtKind::Check),
                    _ => None,
                };
                kind.map(|kind| RetroactiveEffectDebt { debt_id: format!("debt_{}", Uuid::new_v4().simple()), turn_id: request.turn_id.clone(), finding_detail: f.detail.clone(), kind, created_at: Utc::now() })
            })
            .collect::<Vec<_>>();
        self.obligations.absorb_retro_debts(debts);
    }

    /// 确定性收尾（spec §4：save_turn / memory event / learning audit）：持久化
    /// 本回合 + 回合摘要 + 学习审计。叙事已流出不可回收，收尾失败 tracing::warn
    /// 不 panic（也让 MockLlm 单测在 lazy pool 下保持绿）；真实落库由 Task 11 的
    /// turns 表 SQL 验证。
    async fn finalize_turn(&self, request: &ContextRequest, compiled: &CompiledContext, user_input: &str, assistant_output: &str, status: &str) {
        let hashes = json!({"prefix": compiled.prefix_hash, "pinned": compiled.pinned_hash, "dynamic": compiled.dynamic_hash});
        if let Err(err) = self.engine.db.save_turn(&request.session_id, &request.turn_id, user_input, assistant_output, hashes, status).await {
            tracing::warn!(error = %err, "agent path save_turn failed");
        }
        if !assistant_output.trim().is_empty() {
            let summary: String = assistant_output.chars().take(280).collect();
            let event = MemoryEvent { event_id: format!("mem_turn_{}", Uuid::new_v4().simple()), session_id: request.session_id.clone(), turn_id: Some(request.turn_id.clone()), ruleset_id: request.ruleset_id.clone(), module_id: request.module_id.clone(), scene_id: None, location_id: None, actor_ids: vec![], visibility: Visibility::GmOnly, event_kind: MemoryKind::Event, summary, transcript_excerpt: None, source: json!({"source":"gm_agent.turn_summary"}), tags: vec!["gm_turn".to_string()], importance: 1, occurred_at: Utc::now() };
            if let Err(err) = self.engine.db.save_memory_event(&event).await {
                tracing::warn!(error = %err, "agent path turn summary memory event failed");
            }
            // spec §4 收尾第三项：learning audit（保留）——沿用旧路径原语
            // RuntimeEngine::audit_learning_for_turn（内部自带 learning_audit_enabled()
            // 门控与自吞错，门关时为 no-op；与旧 run_turn_once L1481 行为对齐）。
            if let Err(err) = self.engine.audit_learning_for_turn(&request.session_id, &request.ruleset_id, request.module_id.as_deref(), &request.turn_id, user_input, assistant_output).await {
                tracing::warn!(error = %err, "agent path learning audit failed");
            }
        }
    }
}

/// 排空 RedactingBuffer 尾窗并直通 on_delta（混合轮重建 / 轮收尾 / awaiting
/// 终态共用）；被门轮（blocked）丢弃——B6 门控下该轮任何 content 不得流出。
fn drain_redactor(redactor: &mut RedactingBuffer, blocked: bool, on_delta: &mut (dyn FnMut(&str) + Send), visible_text: &mut String) {
    let rest = redactor.finish();
    if blocked || rest.is_empty() { return; }
    on_delta(&rest);
    visible_text.push_str(&rest);
}

#[cfg(test)]
#[path = "turn_loop_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "turn_loop_mode_tests.rs"]
mod mode_tempo_tests;
