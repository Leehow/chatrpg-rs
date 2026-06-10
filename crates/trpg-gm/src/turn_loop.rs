use crate::errata::ErrataMemory;
use crate::gate::GateResolverFn;
use crate::ledger::TurnLedger;
use crate::prompts::{load_gm_skill, DynamicTailInput, TurnMessages};
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
pub struct GmLoop { pub engine: RuntimeEngine, pub llm: Arc<dyn LlmClient>, pub tools: ToolRegistry, pub cfg: LoopConfig, pub data_dir: PathBuf, pub scene_extractor: Option<SceneDeepExtractFn>, pub ctx_provider: Option<CtxProviderFn>, pub gate_resolver: Option<GateResolverFn>, pub errata: ErrataMemory }
pub struct GmTurnInput<'a> { pub request: &'a ContextRequest, pub state: &'a RuntimeState, pub user_input: &'a str, pub history: &'a [ChatMessage], pub recent_transcript: Option<&'a str> }
impl GmLoop {
    pub fn new(engine: RuntimeEngine, llm: Arc<dyn LlmClient>, tools: ToolRegistry, cfg: LoopConfig, data_dir: PathBuf) -> Self { let errata = ErrataMemory::new(cfg.repeat_finding_threshold); Self { engine, llm, tools, cfg, data_dir, scene_extractor: None, ctx_provider: None, gate_resolver: None, errata } }
    pub async fn run_gm_turn(&mut self, input: GmTurnInput<'_>, on_delta: &mut (dyn FnMut(&str) + Send)) -> Result<TurnOutcome> {
        // —— 1. 确定性头部（spec §4 四项顺序：record → refresh → reconcile → gate 结算）——
        let _ = self.engine.record_world_event(&input.request.session_id, Some(&input.request.turn_id), None, WorldEventKind::PlayerAction, json!({"input": input.user_input}), Visibility::GmOnly).await;
        let _ = self.engine.refresh_actor_live_derived(&input.request.session_id, input.request.viewer.actor_id.as_deref().unwrap_or("pc.current")).await;
        // prepare_turn_context 内部也会 reconcile，但那发生在 gate 结算之后，
        // 不满足 spec 顺序——此处显式调用（幂等，与既有调用方同款 let _ =）。
        let _ = InteractionLifecycleKernel::new(self.engine.db.clone()).reconcile_session(&input.request.session_id).await;
        let mut ledger = TurnLedger::new();
        let mut resolved_gate_facts = Vec::new();
        // gate 结算（含裸 "roll" 兜底与 Err 折叠两个 e2e must-fix）收口在 gate.rs 单点。
        crate::gate::resolve_pending_gate(&self.engine, self.gate_resolver.as_ref(), &input.request.session_id, &input.request.turn_id, input.user_input, &mut ledger, &mut resolved_gate_facts).await;
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
        let gm_skill = load_gm_skill(&self.data_dir, &input.request.ruleset_id)?;
        let mut errata_blocks = Vec::new();
        if let Some(block) = self.errata.errata_block() { errata_blocks.push(block); }
        if let Some(block) = self.errata.standing_reminder_block() { errata_blocks.push(block); }
        let tail = DynamicTailInput { user_input: input.user_input, resolved_gate_facts: &resolved_gate_facts, errata_blocks: &errata_blocks };
        let mut messages = TurnMessages::assemble(&compiled, &gm_skill, input.history, &tail);
        let schemas = self.tools.schemas();
        let mut visible_text = String::new();
        let mut awaiting: Option<AwaitingPlayerRoll> = None;
        let mut narrated = false;
        // —— 3. 工具轮（循环内恒 ToolChoice::Auto；ctx 限定块级作用域，块结束
        //     即释放对 self 的借用，收尾 &self 方法才可调用）——
        {
            let ctx = ToolCtx { engine: &self.engine, request: input.request, state: &state_agent, scene_extractor: self.scene_extractor.as_ref() };
            'rounds: for round in 0..self.cfg.max_tool_rounds {
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
                let mut stream = self.llm.stream_chat_with_tools(messages.to_request_messages(), schemas.clone(), ToolChoice::Auto).await?;
                let mut saw_tool = false;
                let mut redactor = RedactingBuffer::new(ledger.private_roll_tokens());
                while let Some(event) = stream.next().await {
                    match event? {
                        StreamEvent::ContentDelta(delta) => {
                            let safe = redactor.push(&delta);
                            if !safe.is_empty() { on_delta(&safe); visible_text.push_str(&safe); }
                        }
                        StreamEvent::Usage(usage) => {
                            // §6.1 第 5 条可观测：relay 透传则记，缺省无此事件。
                            tracing::info!(target: "gm_cache", cached_tokens = ?usage.pointer("/prompt_tokens_details/cached_tokens"), "upstream usage reported");
                        }
                        StreamEvent::ToolCalls(calls) => {
                            saw_tool = true;
                            messages.push_assistant_tool_calls(&calls);
                            for call in calls {
                                let outcome = self.tools.dispatch(&ctx, &mut ledger, &call).await;
                                messages.push_tool_result(&outcome.tool_call_id, &outcome.name, &outcome.content);
                                if let Some(gate) = outcome.awaiting_player_roll {
                                    let rest = redactor.finish();
                                    if !rest.is_empty() { on_delta(&rest); visible_text.push_str(&rest); }
                                    awaiting = Some(gate);
                                    break 'rounds;
                                }
                            }
                            // 工具产物可能新增私骰 token：排空旧缓冲后按最新账本重建（混合轮防漏）。
                            let rest = redactor.finish();
                            if !rest.is_empty() { on_delta(&rest); visible_text.push_str(&rest); }
                            redactor = RedactingBuffer::new(ledger.private_roll_tokens());
                        }
                        StreamEvent::Done { .. } => {}
                    }
                }
                let rest = redactor.finish();
                if !rest.is_empty() { on_delta(&rest); visible_text.push_str(&rest); }
                if !saw_tool { narrated = true; break 'rounds; }
            }
        }
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
        let submission = trpg_agent::FinalNarrationSubmission { player_visible_text: visible_text.to_string(), mechanical_claims: vec![], referenced_ledger_ids: vec![] };
        let result = verifier.verify(ledger.snapshot(), &submission);
        let entries = self.errata.record(&request.turn_id, &result.findings);
        if !entries.is_empty() {
            let event = self.errata.to_memory_event(request, &entries);
            let _ = self.engine.db.save_memory_event(&event).await;
        }
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


#[cfg(test)]
#[path = "turn_loop_tests.rs"]
mod tests;
