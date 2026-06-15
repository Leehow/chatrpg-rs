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
use trpg_model::{ChatMessage, CompiledContext, ContextRequest, MechanicDue, MemoryEvent, MemoryKind, RuntimeState, StateFrame, Visibility, WorldEventKind};
use trpg_runtime::RuntimeEngine;
use uuid::Uuid;

/// 上下文准备注入 seam（单测绕开真 DB；生产装配恒 None）。
pub type CtxProviderFn = Arc<dyn Fn(&ContextRequest, &RuntimeState) -> CompiledContext + Send + Sync>;

#[derive(Debug, Clone)]
pub struct LoopConfig { pub max_tool_rounds: u8, pub repeat_finding_threshold: u8 }
impl Default for LoopConfig { fn default() -> Self { Self { max_tool_rounds: 8, repeat_finding_threshold: 3 } } }
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome { Narration(String), AwaitingPlayerRoll { check_id: String, prompt_public: String } }
/// 生产 GmLoop。R5 Task1c 测试构建额外带一个 heavy-only 探针字段（见 `#[cfg(test)]`
/// 变体）；生产构建无此字段、零开销。
#[cfg(not(test))]
pub struct GmLoop { pub engine: RuntimeEngine, pub llm: Arc<dyn LlmClient>, pub tools: ToolRegistry, pub cfg: LoopConfig, pub data_dir: PathBuf, pub scene_extractor: Option<SceneDeepExtractFn>, pub ctx_provider: Option<CtxProviderFn>, pub gate_resolver: Option<GateResolverFn>, pub errata: ErrataMemory, pub obligations: ObligationLedger }
/// 测试 GmLoop：多一个 `heavy_probe` seam——execute.rs spawn_heavy 入口调
/// `heavy_probe_enter()`，注入时序记录 / 慢 / panic 以验证 heavy 在 TurnComplete
/// 之后、且失败隔离。
#[cfg(test)]
pub struct GmLoop { pub engine: RuntimeEngine, pub llm: Arc<dyn LlmClient>, pub tools: ToolRegistry, pub cfg: LoopConfig, pub data_dir: PathBuf, pub scene_extractor: Option<SceneDeepExtractFn>, pub ctx_provider: Option<CtxProviderFn>, pub gate_resolver: Option<GateResolverFn>, pub errata: ErrataMemory, pub obligations: ObligationLedger, pub(crate) heavy_probe: Option<HeavyProbe> }
pub struct GmTurnInput<'a> { pub request: &'a ContextRequest, pub state: &'a RuntimeState, pub user_input: &'a str, pub history: &'a [ChatMessage], pub recent_transcript: Option<&'a str> }

/// R5 Task1c heavy-only 测试探针：spawn_heavy 入口经 `heavy_probe_enter` 调 `enter()`。
/// `order` 记录 heavy 进入时序（与 TurnComplete 到达时序对照）；`sleep_ms` 让 heavy
/// 入口慢（验 TurnComplete 不被挂住）；`panic` 让 heavy 入口炸（验失败隔离）。
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct HeavyProbe {
    pub order: Option<std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>>,
    pub sleep_ms: u64,
    pub panic: bool,
}
#[cfg(test)]
impl HeavyProbe {
    async fn enter(&self) {
        // 先 sleep 再记录"heavy_enter"：即便 runtime 先调度 heavy 任务，sleep 处的 yield
        // 也让主测试 drain 循环有机会先消费 TurnComplete → 时序断言确定（heavy 工作慢于
        // 已发的 TurnComplete，不挂住流）。
        if self.sleep_ms > 0 { tokio::time::sleep(std::time::Duration::from_millis(self.sleep_ms)).await; }
        if let Some(order) = &self.order { order.lock().unwrap_or_else(|p| p.into_inner()).push("heavy_enter"); }
        if self.panic { panic!("heavy probe induced panic (isolation test)"); }
    }
}

/// 头部确定性 phase 的累积上下文（原 run_gm_turn 局部变量集中到此，供 phase
/// handler 顺序填充）。`messages` 用 Option（TurnMessages 未实现 Default——
/// assemble 一次性产出，context_assembly 之前为 None）。T4：execute.rs 解释器
/// 需在兄弟模块构建+驱动它，故 `pub(crate)`；agent_loop 产物字段供尾部 phase 读。
pub(crate) struct TurnContext {
    ledger: TurnLedger,
    resolved_gate_facts: Vec<String>,
    active_frames: Vec<StateFrame>,
    mode_id: Option<String>,
    mode_manifest: Option<crate::mode::ModeManifest>,
    obligations_block: Option<String>,
    pending_dues: Vec<MechanicDue>,
    opposed_binding: Option<crate::opposed_prepass::OpposedBinding>,
    state_agent: RuntimeState,
    compiled: CompiledContext,
    gm_skill: String,
    errata_blocks: Vec<String>,
    mode_tools: Option<ToolRegistry>,
    max_tool_rounds: u8,
    effect_closure_per_cluster: bool,
    messages: Option<TurnMessages>,
    // —— T4 agent_loop 产物（run_agent_loop 填，尾部 phase 读）——
    visible_text: String,
    awaiting_gate: Option<AwaitingPlayerRoll>,
}
impl TurnContext {
    pub(crate) fn new() -> Self {
        Self {
            ledger: TurnLedger::new(),
            resolved_gate_facts: Vec::new(),
            active_frames: Vec::new(),
            mode_id: None,
            mode_manifest: None,
            obligations_block: None,
            pending_dues: Vec::new(),
            opposed_binding: None,
            state_agent: RuntimeState::default(),
            compiled: CompiledContext::default(),
            gm_skill: String::new(),
            errata_blocks: Vec::new(),
            mode_tools: None,
            max_tool_rounds: 0,
            effect_closure_per_cluster: false,
            messages: None,
            visible_text: String::new(),
            awaiting_gate: None,
        }
    }
}
impl GmLoop {
    #[cfg(not(test))]
    pub fn new(engine: RuntimeEngine, llm: Arc<dyn LlmClient>, tools: ToolRegistry, cfg: LoopConfig, data_dir: PathBuf) -> Self { let errata = ErrataMemory::new(cfg.repeat_finding_threshold); Self { engine, llm, tools, cfg, data_dir, scene_extractor: None, ctx_provider: None, gate_resolver: None, errata, obligations: ObligationLedger::default() } }
    #[cfg(test)]
    pub fn new(engine: RuntimeEngine, llm: Arc<dyn LlmClient>, tools: ToolRegistry, cfg: LoopConfig, data_dir: PathBuf) -> Self { let errata = ErrataMemory::new(cfg.repeat_finding_threshold); Self { engine, llm, tools, cfg, data_dir, scene_extractor: None, ctx_provider: None, gate_resolver: None, errata, obligations: ObligationLedger::default(), heavy_probe: None } }

    /// R5 Task1c heavy-only 测试 seam：execute.rs spawn_heavy 入口无条件调此方法。
    /// 生产为零开销 no-op（`#[cfg(not(test))]`）；测试构建据 `heavy_probe` 注入
    /// 时序记录 / 慢 / panic（仅 heavy 段触发，critical 不经此路径）。
    #[cfg(not(test))]
    pub(crate) async fn heavy_probe_enter(&self) {}
    #[cfg(test)]
    pub(crate) async fn heavy_probe_enter(&self) {
        if let Some(probe) = &self.heavy_probe { probe.enter().await; }
    }
    pub async fn run_gm_turn(&mut self, input: GmTurnInput<'_>, on_delta: &mut (dyn FnMut(&str) + Send)) -> Result<TurnOutcome> {
        // —— 1+2. 确定性头部（spec §4）：9 个 phase handler 按序填充 TurnContext。
        //    record → refresh → reconcile → gate → stimulus_pass → opposed_prepass
        //    → mode_inference → debt_load → context_assembly。本任务纯重构：方法
        //    体逐字搬自原头部，调用顺序与字节产物不变（T4 将由 plan 解释器调度）。
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
        // 头部产物拆回局部（工具轮~尾部保持原作用域结构、字节不变；messages
        // 由 context_assembly 必产出，take 出 Some 值——绝不为 None）。
        let TurnContext { mut ledger, mode_id, opposed_binding, mode_tools, state_agent, compiled, max_tool_rounds, effect_closure_per_cluster, messages, .. } = ctx;
        let mut messages = messages.expect("context_assembly assembles messages");
        let schemas = mode_tools.as_ref().unwrap_or(&self.tools).schemas();
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
            let ctx = ToolCtx { engine: &self.engine, request: input.request, state: &state_agent, scene_extractor: self.scene_extractor.as_ref(), obligations: Some(&obligations_cell), data_dir: Some(&self.data_dir), current_mode: mode_id.as_deref(), opposed_binding: opposed_binding.as_ref() };
            // R1 follow-up: run_agent_loop (execute.rs) duplicates this loop; dedupe when run_gm_turn is retired.
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
            self.finalize_turn(input.request, input.state, &compiled, input.user_input, &assistant_output, "awaiting_player_roll").await;
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
        self.finalize_turn(input.request, input.state, &compiled, input.user_input, &visible_text, "ready").await;
        Ok(TurnOutcome::Narration(visible_text))
    }

    /// T4 AgentLoop body：run_gm_turn 工具轮循环体的 channel 变体。头部 phase
    /// 已由解释器经 `ctx` 跑完——本方法接管「工具轮 + gate 终态 + 轮耗尽逼散文」
    /// 三段，把 Delta 经 `tx.send(TurnEvent::Delta)` 实时直通（字节级等价
    /// on_delta），命中 gate 时 `tx.send(TurnEvent::AwaitingPlayerRoll)` 并返回
    /// `AgentSignal::AwaitingPlayerRoll`；正常散文返回 `AgentSignal::Narration`。
    /// 产物（visible_text / awaiting_gate）写回 `ctx` 供尾部 phase 读。verify /
    /// finalize 不在此跑——解释器作为 Postprocess phase 跑。run_gm_turn 与本方法
    /// 暂时共存（重复可接受、临时——run_gm_turn 在 Task 5/7 删除）。
    pub(crate) async fn run_agent_loop(
        &mut self,
        ctx: &mut TurnContext,
        input: &GmTurnInput<'_>,
        tx: &tokio::sync::mpsc::Sender<crate::turn_event::TurnEvent>,
    ) -> crate::execute::AgentSignal {
        use crate::turn_event::TurnEvent;
        let messages = ctx.messages.as_mut().expect("context_assembly assembles messages");
        let schemas = ctx.mode_tools.as_ref().unwrap_or(&self.tools).schemas();
        let max_tool_rounds = ctx.max_tool_rounds;
        let effect_closure_per_cluster = ctx.effect_closure_per_cluster;
        let mode_id = ctx.mode_id.clone();
        let opposed_binding = ctx.opposed_binding.clone();
        let ledger = &mut ctx.ledger;
        let mut visible_text = String::new();
        let mut awaiting: Option<AwaitingPlayerRoll> = None;
        let mut narrated = false;
        // B6：obligations 暂入互斥单元——与 run_gm_turn 同款（dispatch 链上 ToolCtx
        // 是共享引用，waive_obligation 需点改清单；块结束取回持久字段）。
        let obligations_cell = std::sync::Mutex::new(std::mem::take(&mut self.obligations));
        {
            let tools = ctx.mode_tools.as_ref().unwrap_or(&self.tools);
            let tool_ctx = ToolCtx { engine: &self.engine, request: input.request, state: &ctx.state_agent, scene_extractor: self.scene_extractor.as_ref(), obligations: Some(&obligations_cell), data_dir: Some(&self.data_dir), current_mode: mode_id.as_deref(), opposed_binding: opposed_binding.as_ref() };
            'rounds: for round in 0..max_tool_rounds {
                tracing::info!(
                    target: "gm_cache",
                    session_id = %input.request.session_id,
                    turn_id = %input.request.turn_id,
                    prefix_hash = %ctx.compiled.prefix_hash,
                    pinned_hash = %ctx.compiled.pinned_hash,
                    request_prefix_hash = %messages.prefix_byte_hash(2),
                    tool_round = round,
                    "gm agent prompt cache anchors"
                );
                let blocked = {
                    let mut obligations = obligations_cell.lock().unwrap_or_else(|p| p.into_inner());
                    let settled: std::collections::BTreeSet<&str> = ledger.snapshot().check_results.iter().map(|r| r.check_id.as_str()).collect();
                    for contract in &ledger.snapshot().check_contracts {
                        if settled.contains(contract.check_id.as_str()) { obligations.mark_check_settled(&contract.check_id); } else { obligations.record_open_check(&contract.check_id); }
                    }
                    let effect_ids: Vec<String> = ledger.snapshot().effect_contracts.iter().map(|e| e.effect_id.clone()).collect();
                    obligations.settle_retro_debts_with_effects(&effect_ids);
                    let settled_check_ids: Vec<String> = settled.iter().map(|c| c.to_string()).collect();
                    obligations.settle_retro_debts_with_checks(&settled_check_ids);
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
                let mut stream = match self.llm.stream_chat_with_tools(messages.to_request_messages(), schemas.clone(), ToolChoice::Auto).await {
                    Ok(s) => s,
                    Err(err) => { tracing::warn!(error = %err, "agent loop stream failed; ending turn"); break 'rounds; }
                };
                let mut saw_tool = false;
                let mut redactor = RedactingBuffer::new(ledger.private_roll_tokens());
                while let Some(event) = stream.next().await {
                    let event = match event { Ok(e) => e, Err(err) => { tracing::warn!(error = %err, "agent loop stream event error"); break; } };
                    match event {
                        StreamEvent::ContentDelta(delta) => {
                            if !blocked {
                                let safe = redactor.push(&delta);
                                if !safe.is_empty() { let _ = tx.send(TurnEvent::Delta(safe.clone())).await; visible_text.push_str(&safe); }
                            }
                        }
                        StreamEvent::Usage(usage) => {
                            tracing::info!(target: "gm_cache", cached_tokens = ?usage.pointer("/prompt_tokens_details/cached_tokens"), "upstream usage reported");
                        }
                        StreamEvent::ToolCalls(calls) => {
                            saw_tool = true;
                            messages.push_assistant_tool_calls(&calls);
                            for call in calls {
                                let outcome = tools.dispatch(&tool_ctx, ledger, &call).await;
                                let output_value = serde_json::from_str::<serde_json::Value>(&outcome.content)
                                    .unwrap_or_else(|_| json!({"raw": outcome.content.as_str()}));
                                let status = if output_value.get("error").is_some() { "error" } else { "done" };
                                let _ = tool_ctx.engine.db.insert_agent_tool_call(&trpg_model::AgentToolCallRecord {
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
                                    drain_redactor_tx(&mut redactor, blocked, tx, &mut visible_text).await;
                                    awaiting = Some(gate);
                                    break 'rounds;
                                }
                            }
                            drain_redactor_tx(&mut redactor, blocked, tx, &mut visible_text).await;
                            redactor = RedactingBuffer::new(ledger.private_roll_tokens());
                        }
                        StreamEvent::Done { .. } => {}
                    }
                }
                drain_redactor_tx(&mut redactor, blocked, tx, &mut visible_text).await;
                if saw_tool || blocked { continue 'rounds; }
                narrated = true;
                break 'rounds;
            }
        }
        self.obligations = obligations_cell.into_inner().unwrap_or_else(|p| p.into_inner());
        // —— 终态 A：request_player_roll gate ——
        if let Some(gate) = awaiting {
            ctx.visible_text = visible_text;
            let _ = tx.send(TurnEvent::AwaitingPlayerRoll { check_id: gate.check_id.clone(), prompt_public: gate.prompt_public.clone() }).await;
            ctx.awaiting_gate = Some(gate);
            return crate::execute::AgentSignal::AwaitingPlayerRoll;
        }
        // —— 轮数耗尽且模型仍要工具：追加唯一一轮 ToolChoice::None 逼散文 ——
        if !narrated {
            match self.llm.stream_chat_with_tools(messages.to_request_messages(), schemas, ToolChoice::None).await {
                Ok(mut stream) => {
                    let mut redactor = RedactingBuffer::new(ledger.private_roll_tokens());
                    while let Some(event) = stream.next().await {
                        let event = match event { Ok(e) => e, Err(err) => { tracing::warn!(error = %err, "agent loop forced-prose stream error"); break; } };
                        if let StreamEvent::ContentDelta(delta) = event {
                            let safe = redactor.push(&delta);
                            if !safe.is_empty() { let _ = tx.send(TurnEvent::Delta(safe.clone())).await; visible_text.push_str(&safe); }
                        }
                    }
                    let rest = redactor.finish();
                    if !rest.is_empty() { let _ = tx.send(TurnEvent::Delta(rest.clone())).await; visible_text.push_str(&rest); }
                }
                Err(err) => tracing::warn!(error = %err, "agent loop forced-prose stream failed"),
            }
        }
        ctx.visible_text = visible_text;
        crate::execute::AgentSignal::Narration
    }

    /// PhaseId::RecordPlayerAction — world event PlayerAction（spec §4 头部第 1）。
    pub(crate) async fn phase_record_player_action(&self, _ctx: &mut TurnContext, input: &GmTurnInput<'_>) {
        let _ = self.engine.record_world_event(&input.request.session_id, Some(&input.request.turn_id), None, WorldEventKind::PlayerAction, json!({"input": input.user_input}), Visibility::GmOnly).await;
    }

    /// PhaseId::RefreshLiveDerived — refresh_actor_live_derived（spec §4 头部第 2）。
    pub(crate) async fn phase_refresh_live_derived(&self, _ctx: &mut TurnContext, input: &GmTurnInput<'_>) {
        let _ = self.engine.refresh_actor_live_derived(&input.request.session_id, input.request.viewer.actor_id.as_deref().unwrap_or("pc.current")).await;
    }

    /// PhaseId::Reconcile — 机制对账（spec §4 顺序：gate 结算之前显式调，幂等；
    /// prepare_turn_context 内部也会 reconcile，但那发生在 gate 结算之后）。
    pub(crate) async fn phase_reconcile(&self, _ctx: &mut TurnContext, input: &GmTurnInput<'_>) {
        let _ = InteractionLifecycleKernel::new(self.engine.db.clone()).reconcile_session(&input.request.session_id).await;
    }

    /// PhaseId::Gate — request_player_roll 闸门结算（gate.rs 单点；含裸 "roll"
    /// 兜底 + Err 折叠 + C7 effect_policy 强制；spec §4 头部第 4）。
    pub(crate) async fn phase_gate(&self, ctx: &mut TurnContext, input: &GmTurnInput<'_>) {
        crate::gate::resolve_pending_gate(&self.engine, self.gate_resolver.as_ref(), input.request, input.state.scene_id.as_deref(), input.user_input, &mut ctx.ledger, &mut ctx.resolved_gate_facts).await;
    }

    /// PhaseId::StimulusPass — TurnStart hook dues + 语义被动刺激预 pass（J2 SAN）。
    /// hook 先、stimulus 后并入 ctx.pending_dues，供 debt_load 按序吸收。
    /// fail-closed：门关/无目录/LLM 失败 → 空，绝不阻断回合（unwrap_or_default）。
    pub(crate) async fn phase_stimulus_pass(&self, ctx: &mut TurnContext, input: &GmTurnInput<'_>) {
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

    /// PhaseId::OpposedPrepass — 回合头部对抗绑定（现搓防御方 NPC + 备 OpposedBinding
    /// 供 roll_check 注入）。fail-closed：门关/无 NPC/无攻击意图 → None。
    pub(crate) async fn phase_opposed_prepass(&self, ctx: &mut TurnContext, input: &GmTurnInput<'_>) {
        ctx.opposed_binding = crate::opposed_prepass::prepare_binding(&self.engine, &self.llm, input.request, input.state, input.user_input, input.recent_transcript, input.history).await;
    }

    /// PhaseId::ModeInference — active state_frame → mode 推导（三期 §4.1）。manifest
    /// 损坏 → Err fail-closed 终止回合；db 失败 unwrap_or_default 绝不阻断。
    pub(crate) async fn phase_mode_inference(&self, ctx: &mut TurnContext, input: &GmTurnInput<'_>) -> Result<()> {
        ctx.active_frames = self.engine.db.list_active_state_frames(&input.request.session_id, 8).await.unwrap_or_default();
        ctx.mode_manifest = crate::mode::active_mode_manifest(&self.data_dir, &ctx.active_frames)?;
        ctx.mode_id = ctx.mode_manifest.as_ref().map(|m| m.mode_id.clone());
        Ok(())
    }

    /// PhaseId::DebtLoad — B6 债务装载（spec §5.3）：begin_turn + mode 退出义务
    /// re-seed + leftover/hook/stimulus dues 吸收（按 due_id 去重）→ carryover block。
    pub(crate) async fn phase_debt_load(&mut self, ctx: &mut TurnContext, input: &GmTurnInput<'_>) {
        self.obligations.begin_turn(&input.request.turn_id);
        if let Some(m) = &ctx.mode_manifest {
            self.obligations.ensure_mode_exit_obligations(&m.mode_id, &m.exit_obligations);
        }
        let leftover_dues = self.engine.db.list_open_mechanic_dues(&input.request.session_id).await.unwrap_or_default();
        self.obligations.absorb_dues(leftover_dues);
        self.obligations.absorb_dues(std::mem::take(&mut ctx.pending_dues));
        ctx.obligations_block = self.obligations.carryover_block();
    }

    /// PhaseId::ContextAssembly — prepare_turn_context + 四级 gm_skill 合并 + mode
    /// 目录联动 + errata/novelty BP3 块 + TurnMessages 组装 + mode 工具/节拍参数。
    /// fail-closed：budget 超限 / gm_skill 缺 / 未知工具名 → Err 终止回合。
    pub(crate) async fn phase_context_assembly(&self, ctx: &mut TurnContext, input: &GmTurnInput<'_>) -> Result<()> {
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
        ctx.messages = Some(TurnMessages::assemble(&ctx.compiled, &ctx.gm_skill, input.history, &tail));
        ctx.mode_tools = match ctx.mode_id.as_deref() {
            Some(mode) => Some(ToolRegistry::for_mode(&self.data_dir, Some(mode))?),
            None => None,
        };
        ctx.max_tool_rounds = ctx.mode_manifest.as_ref().and_then(|m| m.tempo.max_tool_rounds).unwrap_or(self.cfg.max_tool_rounds);
        ctx.effect_closure_per_cluster = ctx.mode_manifest.as_ref().and_then(|m| m.tempo.effect_closure_per_cluster).unwrap_or(false);
        Ok(())
    }

    /// 流后校验（spec §4 第 5 步）：NarrationVerifier 对账已流出全文 → 勘误记忆
    /// （注入下一轮 dynamic tail）→ 新增勘误折 MemoryEvent 持久化（tags 含
    /// "gm_errata"；落库失败 `let _ =` 吞错——叙事已交付，校验绝不反向中断回合）。
    pub(crate) async fn verify_after_stream(&mut self, request: &ContextRequest, ledger: &TurnLedger, visible_text: &str) {
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

    /// R5 critical：只持久化回合记录 + status（save_turn）。memory/audit 移到 heavy
    /// （heavy_finalize_memory）。叙事已流出不可回收，save 失败 warn 不 panic（MockLlm
    /// lazy pool 下保持绿）；真实落库由 turns 表 SQL 验证。
    pub(crate) async fn finalize_save_turn(&self, request: &ContextRequest, compiled: &CompiledContext, user_input: &str, assistant_output: &str, status: &str) {
        let hashes = json!({"prefix": compiled.prefix_hash, "pinned": compiled.pinned_hash, "dynamic": compiled.dynamic_hash});
        if let Err(err) = self.engine.db.save_turn(&request.session_id, &request.turn_id, user_input, assistant_output, hashes, status).await {
            tracing::warn!(error = %err, "agent path save_turn failed");
        }
    }

    /// R5 heavy：富版回合记忆 + learning audit（下一回合不强依赖；后台跑）。逐字搬自
    /// 旧 finalize_turn 的 memory/audit 半边。失败只 warn，绝不影响已 save 的 turn（D2）。
    pub(crate) async fn heavy_finalize_memory(&self, request: &ContextRequest, state: &RuntimeState, user_input: &str, assistant_output: &str) {
        if assistant_output.trim().is_empty() { return; }
        let summary: String = assistant_output.chars().take(280).collect();
        // 富版回合记忆（与退役 API turn_postprocess 对齐，spec §4.3）：importance 50、
        // scene/location/actor 取自本回合 RuntimeState、含转录摘录，tags 用
        // ["turn","session_memory"] 并保留 "gm_turn" 便于沿用旧检索。
        let transcript_excerpt = Some(format!(
            "Player: {}\nGM: {}",
            user_input,
            assistant_output.chars().take(2000).collect::<String>()
        ));
        let event = MemoryEvent { event_id: format!("mem_turn_{}", Uuid::new_v4().simple()), session_id: request.session_id.clone(), turn_id: Some(request.turn_id.clone()), ruleset_id: request.ruleset_id.clone(), module_id: request.module_id.clone(), scene_id: state.scene_id.clone(), location_id: state.location_id.clone(), actor_ids: state.active_npc_ids.clone(), visibility: Visibility::GmOnly, event_kind: MemoryKind::Event, summary, transcript_excerpt, source: json!({"source":"gm_agent.turn_summary"}), tags: vec!["turn".to_string(), "session_memory".to_string(), "gm_turn".to_string()], importance: 50, occurred_at: Utc::now() };
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

    /// 确定性收尾（spec §4：save_turn / memory event / learning audit）。R5 拆分后保留
    /// 为薄 wrapper（save→memory 串行），供 run_gm_turn（R1 未退役）复用；新执行器
    /// （execute.rs）走拆分路径（critical save、heavy memory/audit 后台）。
    pub(crate) async fn finalize_turn(&self, request: &ContextRequest, state: &RuntimeState, compiled: &CompiledContext, user_input: &str, assistant_output: &str, status: &str) {
        self.finalize_save_turn(request, compiled, user_input, assistant_output, status).await;
        self.heavy_finalize_memory(request, state, user_input, assistant_output).await;
    }

    // ====================== T4 解释器尾部 phase wrappers ======================
    // execute.rs 解释器经这些 pub(crate) 方法跑后置 phase，全部从 `ctx` 读
    // run_agent_loop 写回的产物（visible_text / ledger / compiled / awaiting_gate）。

    /// PhaseId::VerifyAfterStream — 流后校验（与 run_gm_turn 同语义：visible_text
    /// 非空才跑；空文本无可对账内容）。
    pub(crate) async fn phase_verify_after_stream(&mut self, ctx: &mut TurnContext, input: &GmTurnInput<'_>) {
        if ctx.visible_text.trim().is_empty() && ctx.awaiting_gate.is_some() { return; }
        let visible = std::mem::take(&mut ctx.visible_text);
        self.verify_after_stream(input.request, &ctx.ledger, &visible).await;
        ctx.visible_text = visible;
    }

    /// PhaseId::Finalize（R5 critical）— 只持久化回合记录 + status（save_turn）。
    /// memory/audit 由 phase_finalize_heavy_memory 在 heavy 段后台跑。awaiting 终态用
    /// gate prompt_public 兜底空 visible_text（与 run_gm_turn assistant_output 选择一致）。
    pub(crate) async fn phase_finalize(&mut self, ctx: &mut TurnContext, input: &GmTurnInput<'_>, status: &str) {
        let assistant_output = match &ctx.awaiting_gate {
            Some(gate) if ctx.visible_text.trim().is_empty() => gate.prompt_public.clone(),
            _ => ctx.visible_text.clone(),
        };
        self.finalize_save_turn(input.request, &ctx.compiled, input.user_input, &assistant_output, status).await;
    }

    /// R5 heavy：phase_finalize 的 memory/audit 半边（execute.rs heavy 段调）。从 ctx
    /// 读 awaiting_gate/visible_text 兜底 assistant_output（与 critical 同口径），从
    /// input 读 state 投 RuntimeState 给富版回合记忆。失败只 warn，绝不影响已 save 的 turn。
    pub(crate) async fn phase_finalize_heavy_memory(&self, ctx: &TurnContext, input: &GmTurnInput<'_>) {
        let assistant_output = match &ctx.awaiting_gate {
            Some(gate) if ctx.visible_text.trim().is_empty() => gate.prompt_public.clone(),
            _ => ctx.visible_text.clone(),
        };
        self.heavy_finalize_memory(input.request, input.state, input.user_input, &assistant_output).await;
    }

    /// PhaseId::SceneNavigate（R5 critical）— 切场景决策 + set_session_scene +
    /// SceneChanged world event。返回切换详情供 execute.rs 发 SceneTransition 事件
    /// （R1 旧实现恒 None，本期修复）。**不**深抽/前探（那是 heavy）。
    /// fail-closed：无 module / scene_navigate_critical Err / 未切换 → None（仅 warn）。
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

    /// PhaseId::SceneNavigate（R5 heavy）— 到场深抽 + frontier 前探（后台，best-effort）。
    /// target 来自 critical 的 SceneTransitionInfo.to；module_id 由调用方校验非空后传入。
    pub(crate) async fn phase_scene_navigate_heavy(&self, target: &str, module_id: &str) {
        trpg_runtime::scene_navigation::scene_navigate_heavy(
            &self.engine.db, self.llm.as_ref(), module_id, target, self.data_dir.as_path(),
        ).await;
    }

    /// PhaseId::CarryoverDebt — 轮耗尽带债的债务跨回合落账（吸收原 run_gm_turn
    /// `!narrated` 分支的 carryover_block → carryover_memory_event → save_memory_event；
    /// fail-closed：落库失败仅 warn，BP3 注入由下回合 carryover_block 完成）。
    pub(crate) async fn phase_carryover_debt(&mut self, _ctx: &mut TurnContext, input: &GmTurnInput<'_>) {
        if let Some(block) = self.obligations.carryover_block() {
            let event = carryover_memory_event(input.request, &block);
            if let Err(err) = self.engine.db.save_memory_event(&event).await {
                tracing::warn!(error = %err, "agent path debt carryover memory event failed");
            }
        }
    }

    /// 是否有未决义务（CarryoverDebt phase 的运行期条件位；select_phases 用）。
    pub(crate) fn has_pending_obligations(&self) -> bool {
        self.obligations.carryover_block().is_some()
    }

    /// 把 agent 终态折成 TurnOutcome（TurnComplete 事件载荷）。awaiting_gate 优先。
    pub(crate) fn take_outcome(&self, ctx: &mut TurnContext) -> TurnOutcome {
        match ctx.awaiting_gate.take() {
            Some(gate) => TurnOutcome::AwaitingPlayerRoll { check_id: gate.check_id, prompt_public: gate.prompt_public },
            None => TurnOutcome::Narration(std::mem::take(&mut ctx.visible_text)),
        }
    }
}

/// run_agent_loop 私骰尾窗排空：经 mpsc 发 Delta（与 drain_redactor 同语义，
/// 但走 tx.send 而非 on_delta；被门轮 blocked 丢弃）。
async fn drain_redactor_tx(redactor: &mut RedactingBuffer, blocked: bool, tx: &tokio::sync::mpsc::Sender<crate::turn_event::TurnEvent>, visible_text: &mut String) {
    let rest = redactor.finish();
    if blocked || rest.is_empty() { return; }
    let _ = tx.send(crate::turn_event::TurnEvent::Delta(rest.clone())).await;
    visible_text.push_str(&rest);
}

/// scene_navigate 切换详情（execute.rs 折成 TurnEvent::SceneTransition + 排 heavy 深抽）。
/// R5：phase_scene_navigate_critical 在真切场景时返回 Some（来自 scene_navigate_critical
/// 的 SceneNavCommit），修复 R1 恒 None 的 TODO。
#[derive(Debug, Clone)]
pub(crate) struct SceneTransitionInfo {
    pub from: String,
    pub to: String,
    pub reason: String,
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
