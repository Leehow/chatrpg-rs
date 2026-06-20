use crate::errata::ErrataMemory;
use crate::gate::GateResolverFn;
use crate::ledger::TurnLedger;
use crate::obligations::{
    carryover_memory_event, ObligationLedger, RetroDebtKind, RetroactiveEffectDebt,
};
use crate::plugins::load_gm_skill_with_plugins;
// P7.3：控制面经 Port 适配器派发（Port trait 方法需在作用域内）。
use crate::ports::{DirectorPort, KernelPort, NarratorPort, PolicyPort, WorldPort};
use crate::prompts::{DynamicTailInput, TurnMessages};
use crate::stream::RedactingBuffer;
use crate::tools::{AwaitingPlayerRoll, SceneDeepExtractFn, ToolCtx, ToolRegistry};
use anyhow::Result;
use chrono::Utc;
use futures_util::StreamExt;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use trpg_interaction::InteractionLifecycleKernel;
use trpg_llm::{LlmClient, StreamEvent, ToolChoice};
use trpg_model::{
    ChatMessage, CompiledContext, ContextRequest, MechanicDue, MemoryEvent, MemoryKind,
    RuntimeState, StateFrame, Visibility, WorldEventKind,
};
use trpg_runtime::RuntimeEngine;
use uuid::Uuid;

/// 上下文准备注入 seam（单测绕开真 DB；生产装配恒 None）。
pub type CtxProviderFn =
    Arc<dyn Fn(&ContextRequest, &RuntimeState) -> CompiledContext + Send + Sync>;

#[derive(Debug, Clone)]
pub struct LoopConfig {
    pub max_tool_rounds: u8,
    pub repeat_finding_threshold: u8,
}
impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            max_tool_rounds: 8,
            repeat_finding_threshold: 3,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    Narration(String),
    AwaitingPlayerRoll {
        check_id: String,
        prompt_public: String,
    },
}
/// 生产 GmLoop。R5 Task1c 测试构建额外带一个 heavy-only 探针字段（见 `#[cfg(test)]`
/// 变体）；生产构建无此字段、零开销。
#[cfg(not(test))]
pub struct GmLoop {
    pub engine: RuntimeEngine,
    pub llm: Arc<dyn LlmClient>,
    pub tools: ToolRegistry,
    pub cfg: LoopConfig,
    pub data_dir: PathBuf,
    pub scene_extractor: Option<SceneDeepExtractFn>,
    pub ctx_provider: Option<CtxProviderFn>,
    pub gate_resolver: Option<GateResolverFn>,
    pub errata: ErrataMemory,
    pub obligations: ObligationLedger,
}
/// 测试 GmLoop：多一个 `heavy_probe` seam——execute.rs spawn_heavy 入口调
/// `heavy_probe_enter()`，注入时序记录 / 慢 / panic 以验证 heavy 在 TurnComplete
/// 之后、且失败隔离。
#[cfg(test)]
pub struct GmLoop {
    pub engine: RuntimeEngine,
    pub llm: Arc<dyn LlmClient>,
    pub tools: ToolRegistry,
    pub cfg: LoopConfig,
    pub data_dir: PathBuf,
    pub scene_extractor: Option<SceneDeepExtractFn>,
    pub ctx_provider: Option<CtxProviderFn>,
    pub gate_resolver: Option<GateResolverFn>,
    pub errata: ErrataMemory,
    pub obligations: ObligationLedger,
    pub(crate) heavy_probe: Option<HeavyProbe>,
}
pub struct GmTurnInput<'a> {
    pub request: &'a ContextRequest,
    pub state: &'a RuntimeState,
    pub user_input: &'a str,
    pub history: &'a [ChatMessage],
    pub recent_transcript: Option<&'a str>,
}

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
        if self.sleep_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(self.sleep_ms)).await;
        }
        if let Some(order) = &self.order {
            order
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push("heavy_enter");
        }
        if self.panic {
            panic!("heavy probe induced panic (isolation test)");
        }
    }
}

/// Synthetic plugin id for the knowledge/NPC view-load provenance entries the
/// flight recorder records BEFORE context-policy plugin contributions, so the
/// persisted `TurnTrace` proves view loading preceded plugin execution
/// (DA-PIPE-01). These entries reuse the existing `PluginContributionTrace`
/// shape — no schema change — and carry only counts/ids (never secret prose).
pub const VIEW_LOADER_PLUGIN_ID: &str = "core.view_loader";

/// Trace kind for a knowledge/NPC view-load provenance entry.
pub const VIEW_LOAD_KIND: &str = "view_load";

/// Synthetic plugin id for the P4.6 World NPC-action resolution provenance entry, so the
/// flag-ON Attack slice is OBSERVABLE in the persisted `TurnTrace` (FRAMEWORK §2: a
/// DB-mutating resolution must never be a silent `let _` drop). Reuses the existing
/// `PluginContributionTrace` shape — no schema change; carries only npc id + verdict band,
/// never secret prose.
pub const NPC_ACTION_PLUGIN_ID: &str = "core.world_npc_action";

/// Trace kind for a resolved World NPC-action (Attack) outcome.
pub const NPC_ACTION_KIND: &str = "npc_action_resolved";

/// Build one NPC-action resolution provenance entry for the flight recorder. `summary`
/// carries the npc id + the hit/miss band only (no secret content, no GM-only prose).
pub fn npc_action_resolved_trace(summary: &str) -> trpg_model::PluginContributionTrace {
    trpg_model::PluginContributionTrace {
        plugin_id: NPC_ACTION_PLUGIN_ID.to_string(),
        hook: crate::plugin::PluginHook::ContextAssembly
            .as_str()
            .to_string(),
        kind: NPC_ACTION_KIND.to_string(),
        summary: summary.to_string(),
    }
}

/// Build one view-load provenance entry (kind `view_load`) for the flight recorder.
/// `view` is the view name (e.g. `gm_context`, `player_knowledge_view`,
/// `npc_mind_views`); `detail` is a count/id summary, never secret content.
pub fn view_load_trace(view: &str, detail: &str) -> trpg_model::PluginContributionTrace {
    trpg_model::PluginContributionTrace {
        plugin_id: VIEW_LOADER_PLUGIN_ID.to_string(),
        hook: crate::plugin::PluginHook::ContextAssembly
            .as_str()
            .to_string(),
        kind: VIEW_LOAD_KIND.to_string(),
        summary: format!("{view}: {detail}"),
    }
}

/// Invariant the flight recorder maintains (DA-PIPE-01): every `view_load`
/// provenance entry must precede the first non-`view_load` contribution, proving
/// knowledge/NPC views were loaded before context-policy plugins ran. Returns true
/// when the invariant holds (vacuously true if there are no view_load entries).
pub fn flight_recorder_view_order_ok(traces: &[trpg_model::PluginContributionTrace]) -> bool {
    let first_non_view_load = traces.iter().position(|t| t.kind != VIEW_LOAD_KIND);
    let last_view_load = traces.iter().rposition(|t| t.kind == VIEW_LOAD_KIND);
    match (first_non_view_load, last_view_load) {
        (Some(first), Some(last)) => last < first,
        _ => true,
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
    // 本回合已解析的 RuleKernel（context_assembly 单点载，影子 binding 读 → 出权威 Exact 绑定）。
    // 不持久化、advisory；None = 未载到（fail-soft，影子退化为仅 need_kind 启发式）。
    rule_kernel: Option<trpg_model::RuleKernel>,
    // policy 插件本回合贡献的 Flight Recorder 折叠（context_assembly 经 PluginHost 填，
    // build_turn_trace 拷进 TurnTrace.plugin_contributions）。advisory、零行为变更。
    plugin_contributions: Vec<trpg_model::PluginContributionTrace>,
    // P2 步骤6：本回合 PresentationGate 判定（verify_after_stream 算出，execute.rs 的
    // VerifyAfterStream 处理读）。默认 Allow；TRPG_PRESENTATION_GATE OFF 时仅记 trace，
    // 零行为变更。ON + buffered-narration 就位时 Block 触发 repair ladder。
    presentation_gate: crate::presentation_gate::PresentationGate,
    // —— T4 agent_loop 产物（run_agent_loop 填，尾部 phase 读）——
    visible_text: String,
    awaiting_gate: Option<AwaitingPlayerRoll>,
    // P5.6 Director brief packet（`build_npc_behavior_guidance` 在 World 候选池仍活时填，
    // `phase_context_assembly` 读进 DynamicTailInput.director_packet_block）。flag
    // `TRPG_DIRECTOR_PACKET` 默认 OFF ⇒ 恒 None ⇒ 不写块 ⇒ 字节等价基线。仅入 [gm] BP3，
    // 绝不入玩家可见 narration。
    director_packet_block: Option<String>,
    // P6.7 reveal-gating 提名（agent_loop 内 reveal_fact 在 TRPG_REVEAL_GATING ON 时填，
    // PresentationCommit 边界在终审 Allow 后排序提交）。OFF ⇒ 恒空（reveal_fact 即时落库，基线）。
    nominated_reveals: Vec<crate::tools::RevealNomination>,
    // P6 revision (§二十四-#13 producer→commit)：note_player_rejection 在 TRPG_STORY_WRITE_LOOP ON
    // 时填本回合玩家拒绝的线索提名；PresentationCommit 边界 drain 后调 commit_story_writes 持久化。
    // OFF ⇒ 恒空（工具不注册、通道不挂 ⇒ 字节级基线）。
    rejected_nominations: Vec<crate::tools::RejectionNomination>,
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
            rule_kernel: None,
            plugin_contributions: Vec::new(),
            presentation_gate: crate::presentation_gate::PresentationGate::Allow,
            visible_text: String::new(),
            awaiting_gate: None,
            director_packet_block: None,
            nominated_reveals: Vec::new(),
            rejected_nominations: Vec::new(),
        }
    }

    /// 回合 assistant_output 的**单一事实源**派生（与 R1 旧 finalize_turn 逐字一致）：
    /// awaiting 终态且 visible_text 为空 → gate.prompt_public 兜底；否则 → visible_text。
    /// critical phase_finalize（save_turn）与 heavy phase_finalize_heavy_memory（记忆/审计）
    /// 必须用同一值；后者在 `take_outcome` 清空 ctx 后才跑，故调用方须在清空前调此快照。
    pub(crate) fn heavy_assistant_output(&self) -> String {
        match &self.awaiting_gate {
            Some(gate) if self.visible_text.trim().is_empty() => gate.prompt_public.clone(),
            _ => self.visible_text.clone(),
        }
    }

    /// T2 Flight Recorder：兄弟模块（turn_trace.rs / execute.rs）只读访问本回合
    /// 已装配的 CompiledContext，用于从 need_trace / BP1·BP2·BP3 hash 组装 TurnTrace。
    /// 字段私有（module-private to turn_loop）故经此 accessor 暴露——不开放可变写。
    pub(crate) fn compiled(&self) -> &CompiledContext {
        &self.compiled
    }

    /// MAT.M7 (D1)：本回合**消费者可见**的 active NPC 集——优先取
    /// `compiled.active_npc_ids`（prepare_turn_context 派生后填，Enforce 下 ==
    /// scene.referenced_npc_ids），它为空时退回调用方传入的 `state.active_npc_ids`。
    ///
    /// 退回保证两件事：① ctx_provider 测试 seam（生产恒 None）返回的 CompiledContext 不填
    /// 此字段 ⇒ 退回 state ⇒ 既有 gm 单测行为不变；② Off/Shadow 下派生集 == state 集
    /// （apply_npc_activation 无操作），两路同值 ⇒ 字节等价基线。空-退-空亦保 s17 不变量。
    pub(crate) fn effective_active_npc_ids<'a>(&'a self, state: &'a RuntimeState) -> &'a [String] {
        effective_active_npc_ids(&self.compiled.active_npc_ids, &state.active_npc_ids)
    }

    /// 本回合已解析的 RuleKernel（context_assembly 填）。影子 binding 读它派生权威 facet。
    pub(crate) fn rule_kernel(&self) -> Option<&trpg_model::RuleKernel> {
        self.rule_kernel.as_ref()
    }

    /// 本回合 policy 插件贡献的 Flight Recorder 折叠（context_assembly 填）。
    /// build_turn_trace 拷进 TurnTrace.plugin_contributions（advisory，零行为变更）。
    pub(crate) fn plugin_contributions(&self) -> &[trpg_model::PluginContributionTrace] {
        &self.plugin_contributions
    }

    /// P2 本回合 PresentationGate 判定（verify_after_stream 填）。execute.rs 的
    /// VerifyAfterStream 处理读它 emit gate 决策事件（OFF 时仅观测）。
    pub(crate) fn presentation_gate(&self) -> &crate::presentation_gate::PresentationGate {
        &self.presentation_gate
    }

    /// 测试 seam（kernel-facets 单测）：直接植入已解析 kernel，让 build_turn_trace 单测覆盖
    /// 影子 binding 从 kernel 出 ruleset_check 计划，无需跑真 context_assembly / DB。
    #[cfg(test)]
    pub(crate) fn set_rule_kernel_for_test(&mut self, kernel: trpg_model::RuleKernel) {
        self.rule_kernel = Some(kernel);
    }

    /// 测试 seam：直接植入 agent_loop 产物（visible_text / awaiting_gate），让纯单测
    /// 在不跑真 LLM stream 的情况下覆盖 heavy_assistant_output 的派生分支。
    #[cfg(test)]
    pub(crate) fn set_agent_products_for_test(
        &mut self,
        visible_text: String,
        awaiting_gate: Option<AwaitingPlayerRoll>,
    ) {
        self.visible_text = visible_text;
        self.awaiting_gate = awaiting_gate;
    }

    /// 测试 seam（T2 turn_trace 单测）：直接植入已装配的 CompiledContext，让纯单测在不跑
    /// 真 context_assembly 的情况下覆盖 build_turn_trace 从 need_trace / BP hash 的组装。
    #[cfg(test)]
    pub(crate) fn set_compiled_for_test(&mut self, compiled: CompiledContext) {
        self.compiled = compiled;
    }
}
impl GmLoop {
    #[cfg(not(test))]
    pub fn new(
        engine: RuntimeEngine,
        llm: Arc<dyn LlmClient>,
        tools: ToolRegistry,
        cfg: LoopConfig,
        data_dir: PathBuf,
    ) -> Self {
        let errata = ErrataMemory::new(cfg.repeat_finding_threshold);
        Self {
            engine,
            llm,
            tools,
            cfg,
            data_dir,
            scene_extractor: None,
            ctx_provider: None,
            gate_resolver: None,
            errata,
            obligations: ObligationLedger::default(),
        }
    }
    #[cfg(test)]
    pub fn new(
        engine: RuntimeEngine,
        llm: Arc<dyn LlmClient>,
        tools: ToolRegistry,
        cfg: LoopConfig,
        data_dir: PathBuf,
    ) -> Self {
        let errata = ErrataMemory::new(cfg.repeat_finding_threshold);
        Self {
            engine,
            llm,
            tools,
            cfg,
            data_dir,
            scene_extractor: None,
            ctx_provider: None,
            gate_resolver: None,
            errata,
            obligations: ObligationLedger::default(),
            heavy_probe: None,
        }
    }

    /// R5 Task1c heavy-only 测试 seam：execute.rs spawn_heavy 入口无条件调此方法。
    /// 生产为零开销 no-op（`#[cfg(not(test))]`）；测试构建据 `heavy_probe` 注入
    /// 时序记录 / 慢 / panic（仅 heavy 段触发，critical 不经此路径）。
    #[cfg(not(test))]
    pub(crate) async fn heavy_probe_enter(&self) {}
    #[cfg(test)]
    pub(crate) async fn heavy_probe_enter(&self) {
        if let Some(probe) = &self.heavy_probe {
            probe.enter().await;
        }
    }
    pub async fn run_gm_turn(
        &mut self,
        input: GmTurnInput<'_>,
        on_delta: &mut (dyn FnMut(&str) + Send),
    ) -> Result<TurnOutcome> {
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
        let TurnContext {
            mut ledger,
            mode_id,
            opposed_binding,
            mode_tools,
            state_agent,
            compiled,
            max_tool_rounds,
            effect_closure_per_cluster,
            messages,
            ..
        } = ctx;
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
        // P6.7：reveal 提名互斥单元（legacy 路径同款；commit 在下方 verify 后做）。
        let reveal_gating = reveal_gating_enabled();
        let reveals_cell: std::sync::Mutex<Vec<crate::tools::RevealNomination>> =
            std::sync::Mutex::new(Vec::new());
        {
            let tools = mode_tools.as_ref().unwrap_or(&self.tools);
            let ctx = ToolCtx {
                engine: &self.engine,
                request: input.request,
                state: &state_agent,
                scene_extractor: self.scene_extractor.as_ref(),
                obligations: Some(&obligations_cell),
                data_dir: Some(&self.data_dir),
                current_mode: mode_id.as_deref(),
                opposed_binding: opposed_binding.as_ref(),
                nominated_reveals: reveal_gating.then_some(&reveals_cell),
                // R1 legacy path has no PresentationCommit boundary to drain rejections (it commits
                // reveals inline); the story-write producer lives on the production run_agent_loop
                // path. Leave the channel unhooked here (byte-identical baseline).
                rejected_nominations: None,
            };
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
                    let mut obligations =
                        obligations_cell.lock().unwrap_or_else(|p| p.into_inner());
                    // 账本同步：有结果的契约 settle，未结算的升格 open-check 债务
                    // （request_player_roll gate 开着走 awaiting 终态不经此处，不算债）。
                    let settled: std::collections::BTreeSet<&str> = ledger
                        .snapshot()
                        .check_results
                        .iter()
                        .map(|r| r.check_id.as_str())
                        .collect();
                    for contract in &ledger.snapshot().check_contracts {
                        if settled.contains(contract.check_id.as_str()) {
                            obligations.mark_check_settled(&contract.check_id);
                        } else {
                            obligations.record_open_check(&contract.check_id);
                        }
                    }
                    // 追溯债务清偿（spec §5.3"补 apply_effect 落账"半边）：前轮新落账
                    // effect 按 FIFO 结清追溯债务（effect_id 只消费一次，逐轮重扫安全）。
                    let effect_ids: Vec<String> = ledger
                        .snapshot()
                        .effect_contracts
                        .iter()
                        .map(|e| e.effect_id.clone())
                        .collect();
                    obligations.settle_retro_debts_with_effects(&effect_ids);
                    // Check 类追溯债务（MissingCheck"补 roll_check"半边）：本回合新
                    // 结算的检定按 FIFO 结清（同款逐轮重扫安全）。
                    let settled_check_ids: Vec<String> =
                        settled.iter().map(|c| c.to_string()).collect();
                    obligations.settle_retro_debts_with_checks(&settled_check_ids);
                    // 三期 §4.6 交锋簇节拍收紧（批2）：紧节拍下"已结算检定但零效果
                    // 落账"（effect 契约 / track 落账 / 检定自带 committed patches
                    // 皆无）⇒ 簇未闭合 ⇒ 本轮不得成为叙事终态。tight=false（mode=None
                    // /幕间）恒清空——二期行为字节级一致。
                    let snap = ledger.snapshot();
                    let effects_booked = snap.effect_contracts.len()
                        + snap.parameter_impacts.len()
                        + snap
                            .check_results
                            .iter()
                            .filter(|r| !r.committed_patches.is_empty())
                            .count();
                    obligations.update_cluster_closure(
                        effect_closure_per_cluster,
                        snap.check_results.len(),
                        effects_booked,
                    );
                    match obligations.block_text() {
                        Some(block) => {
                            if round > 0 {
                                messages.push_system_observation(&block);
                            }
                            true
                        }
                        None => false,
                    }
                };
                let mut stream = self
                    .llm
                    .stream_chat_with_tools(
                        messages.to_request_messages(),
                        schemas.clone(),
                        ToolChoice::Auto,
                    )
                    .await?;
                let mut saw_tool = false;
                let mut redactor = RedactingBuffer::new(ledger.private_roll_tokens());
                while let Some(event) = stream.next().await {
                    match event? {
                        StreamEvent::ContentDelta(delta) => {
                            // 被门轮 content 实时丢弃（绝不缓冲、绝不流出）。
                            if !blocked {
                                let safe = redactor.push(&delta);
                                if !safe.is_empty() {
                                    on_delta(&safe);
                                    visible_text.push_str(&safe);
                                }
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
                                let output_value =
                                    serde_json::from_str::<serde_json::Value>(&outcome.content)
                                        .unwrap_or_else(
                                            |_| json!({"raw": outcome.content.as_str()}),
                                        );
                                let status = if output_value.get("error").is_some() {
                                    "error"
                                } else {
                                    "done"
                                };
                                let _ = ctx
                                    .engine
                                    .db
                                    .insert_agent_tool_call(&trpg_model::AgentToolCallRecord {
                                        tool_call_id: outcome.tool_call_id.clone(),
                                        session_id: input.request.session_id.clone(),
                                        turn_id: input.request.turn_id.clone(),
                                        tool_name: outcome.name.clone(),
                                        visibility: Visibility::GmOnly,
                                        input_json: serde_json::from_str(&call.arguments)
                                            .unwrap_or_else(
                                                |_| json!({"raw": call.arguments.as_str()}),
                                            ),
                                        output_json: Some(output_value),
                                        status: status.to_string(),
                                        error: None,
                                        created_at: Utc::now(),
                                    })
                                    .await;
                                messages.push_tool_result(
                                    &outcome.tool_call_id,
                                    &outcome.name,
                                    &outcome.content,
                                );
                                if let Some(gate) = outcome.awaiting_player_roll {
                                    drain_redactor(
                                        &mut redactor,
                                        blocked,
                                        on_delta,
                                        &mut visible_text,
                                    );
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
                if saw_tool || blocked {
                    continue 'rounds;
                }
                narrated = true;
                break 'rounds;
            }
        }
        self.obligations = obligations_cell
            .into_inner()
            .unwrap_or_else(|p| p.into_inner());
        // P6.7：legacy 路径 reveal 提名（gating ON 时非空）。此路径无 repair ladder/gate 保留，
        // commit 决策直接取下方 verify_after_stream 的 gate（Block⇒丢弃；Allow⇒排序提交）。
        let legacy_reveals = reveals_cell.into_inner().unwrap_or_else(|p| p.into_inner());
        // —— 终态 A：request_player_roll gate ——
        if let Some(gate) = awaiting {
            // 流后校验是 loop 之后的无条件阶段（spec §4）：awaiting 终态前可能已
            // 流出含可见掷骰结果的叙事，仅在 visible_text 非空时跑（空文本无可对账内容）。
            let mut allow = true;
            if !visible_text.trim().is_empty() {
                // R1 legacy 路径不组装 TurnTrace，丢弃返回的 plugin trace 记录（execute.rs 默认路径
                // 经 phase_verify_after_stream 收集）。
                let outcome = self
                    .verify_after_stream(
                        input.request,
                        &ledger,
                        &visible_text,
                        &input.state.active_npc_ids,
                    )
                    .await;
                allow = !outcome.gate.is_block();
            }
            self.commit_nominated_reveals(input.request, legacy_reveals, allow)
                .await;
            let assistant_output = if visible_text.trim().is_empty() {
                gate.prompt_public.clone()
            } else {
                visible_text.clone()
            };
            self.finalize_turn(
                input.request,
                input.state,
                &compiled,
                input.user_input,
                &assistant_output,
                "awaiting_player_roll",
            )
            .await;
            return Ok(TurnOutcome::AwaitingPlayerRoll {
                check_id: gate.check_id,
                prompt_public: gate.prompt_public,
            });
        }
        // —— 4. 仅当轮数自然耗尽且模型仍要工具：追加唯一一轮 ToolChoice::None 逼散文 ——
        if !narrated {
            let mut stream = self
                .llm
                .stream_chat_with_tools(messages.to_request_messages(), schemas, ToolChoice::None)
                .await?;
            let mut redactor = RedactingBuffer::new(ledger.private_roll_tokens());
            while let Some(event) = stream.next().await {
                if let StreamEvent::ContentDelta(delta) = event? {
                    let safe = redactor.push(&delta);
                    if !safe.is_empty() {
                        on_delta(&safe);
                        visible_text.push_str(&safe);
                    }
                }
            }
            let rest = redactor.finish();
            if !rest.is_empty() {
                on_delta(&rest);
                visible_text.push_str(&rest);
            }
            // B6：轮耗尽带债强制叙事 → 债务落勘误记忆（绝不静默丢失；BP3 注入
            // 由下回合 carryover_block 完成）。落库失败 `let _ =` 吞错。
            if let Some(block) = self.obligations.carryover_block() {
                let event = carryover_memory_event(input.request, &block);
                let _ = self.engine.db.save_memory_event(&event).await;
            }
        }
        // —— 5. 流后校验（不阻塞交付：叙事已全部流出）——
        let outcome = self
            .verify_after_stream(
                input.request,
                &ledger,
                &visible_text,
                &input.state.active_npc_ids,
            )
            .await;
        // P6.7：legacy narration 终态 reveal 提交（终审 Allow only，排序）。
        self.commit_nominated_reveals(input.request, legacy_reveals, !outcome.gate.is_block())
            .await;
        // —— 6. 确定性收尾 ——
        self.finalize_turn(
            input.request,
            input.state,
            &compiled,
            input.user_input,
            &visible_text,
            "ready",
        )
        .await;
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
    ///
    /// P1-3 follow-up：`cancel` 是 transport（trpg-api SSE 驱动器）的取消令牌。客户端
    /// 在状态变更前断开 → driver `cancel.cancel()`。本 loop 在**每轮起点**与 **LLM 流分块
    /// 之间**检查它，一旦 fire 就放弃在途生成（drop stream 即停 relay，不再续烧 token）、
    /// `break 'rounds`、并跳过「轮耗尽逼散文」那一轮。`cancel=None`（CLI/测试）⇒ 取消等待
    /// 分支永久 pending ⇒ 退化为裸 `stream.next().await`，非取消路径逐字节零行为变更。
    /// `stream_prose`：是否把 adjudicator 的 ContentDelta 实时发给玩家(经 tx)。
    /// - `true`(默认/OFF 路径)= 逐字节现行行为:三处 ContentDelta 直发点 + drain 都 tx.send。
    /// - `false`(P1 TRPG_NARRATOR_SPLIT ON 的 buffer 模式)= adjudicator prose 只
    ///   push_str 进 visible_text,**不** tx.send(留给 Narrator 投影后再流出);AwaitingPlayerRoll
    ///   早退与桌面骰 prompt_public 行为完全不受影响(它们是确定性文本、不经 Narrator)。
    pub(crate) async fn run_agent_loop(
        &mut self,
        ctx: &mut TurnContext,
        input: &GmTurnInput<'_>,
        tx: &tokio::sync::mpsc::Sender<crate::turn_event::TurnEvent>,
        cancel: Option<&CancellationToken>,
        stream_prose: bool,
    ) -> crate::execute::AgentSignal {
        use crate::turn_event::TurnEvent;
        let messages = ctx
            .messages
            .as_mut()
            .expect("context_assembly assembles messages");
        let schemas = ctx.mode_tools.as_ref().unwrap_or(&self.tools).schemas();
        let max_tool_rounds = ctx.max_tool_rounds;
        let effect_closure_per_cluster = ctx.effect_closure_per_cluster;
        let mode_id = ctx.mode_id.clone();
        let opposed_binding = ctx.opposed_binding.clone();
        let ledger = &mut ctx.ledger;
        let mut visible_text = String::new();
        let mut awaiting: Option<AwaitingPlayerRoll> = None;
        let mut narrated = false;
        // P1-3 follow-up：取消标志——top-of-round 检查或流分块间 select 命中取消时置 true，
        // 用于 `break 'rounds` 后跳过「轮耗尽逼散文」并指示尾段已被放弃。
        let mut cancelled = false;
        // B6：obligations 暂入互斥单元——与 run_gm_turn 同款（dispatch 链上 ToolCtx
        // 是共享引用，waive_obligation 需点改清单；块结束取回持久字段）。
        let obligations_cell = std::sync::Mutex::new(std::mem::take(&mut self.obligations));
        // P6.7：reveal 提名互斥单元（gating ON 时 reveal_fact 点改）；块结束取回进
        // ctx.nominated_reveals 供 PresentationCommit 边界提交。OFF/未挂 ⇒ 始终空。
        let reveal_gating = reveal_gating_enabled();
        let reveals_cell: std::sync::Mutex<Vec<crate::tools::RevealNomination>> =
            std::sync::Mutex::new(std::mem::take(&mut ctx.nominated_reveals));
        // P6 revision (§二十四-#13)：story-write loop ON ⇒ 注入拒绝提名通道供 note_player_rejection
        // 填；PresentationCommit 边界 drain 后 commit_story_writes 持久化。OFF ⇒ 不注入（基线）。
        let story_write_loop = trpg_runtime::story_write_loop_enabled();
        let rejections_cell: std::sync::Mutex<Vec<crate::tools::RejectionNomination>> =
            std::sync::Mutex::new(std::mem::take(&mut ctx.rejected_nominations));
        {
            let tools = ctx.mode_tools.as_ref().unwrap_or(&self.tools);
            let tool_ctx = ToolCtx {
                engine: &self.engine,
                request: input.request,
                state: &ctx.state_agent,
                scene_extractor: self.scene_extractor.as_ref(),
                obligations: Some(&obligations_cell),
                data_dir: Some(&self.data_dir),
                current_mode: mode_id.as_deref(),
                opposed_binding: opposed_binding.as_ref(),
                nominated_reveals: reveal_gating.then_some(&reveals_cell),
                rejected_nominations: story_write_loop.then_some(&rejections_cell),
            };
            'rounds: for round in 0..max_tool_rounds {
                // P1-3 follow-up：本轮起点先查取消——已 fire 则连 LLM 请求都不发（最省 token）。
                if is_cancelled(cancel) {
                    cancelled = true;
                    break 'rounds;
                }
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
                    let mut obligations =
                        obligations_cell.lock().unwrap_or_else(|p| p.into_inner());
                    let settled: std::collections::BTreeSet<&str> = ledger
                        .snapshot()
                        .check_results
                        .iter()
                        .map(|r| r.check_id.as_str())
                        .collect();
                    for contract in &ledger.snapshot().check_contracts {
                        if settled.contains(contract.check_id.as_str()) {
                            obligations.mark_check_settled(&contract.check_id);
                        } else {
                            obligations.record_open_check(&contract.check_id);
                        }
                    }
                    let effect_ids: Vec<String> = ledger
                        .snapshot()
                        .effect_contracts
                        .iter()
                        .map(|e| e.effect_id.clone())
                        .collect();
                    obligations.settle_retro_debts_with_effects(&effect_ids);
                    let settled_check_ids: Vec<String> =
                        settled.iter().map(|c| c.to_string()).collect();
                    obligations.settle_retro_debts_with_checks(&settled_check_ids);
                    let snap = ledger.snapshot();
                    let effects_booked = snap.effect_contracts.len()
                        + snap.parameter_impacts.len()
                        + snap
                            .check_results
                            .iter()
                            .filter(|r| !r.committed_patches.is_empty())
                            .count();
                    obligations.update_cluster_closure(
                        effect_closure_per_cluster,
                        snap.check_results.len(),
                        effects_booked,
                    );
                    match obligations.block_text() {
                        Some(block) => {
                            if round > 0 {
                                messages.push_system_observation(&block);
                            }
                            true
                        }
                        None => false,
                    }
                };
                let mut stream = match self
                    .llm
                    .stream_chat_with_tools(
                        messages.to_request_messages(),
                        schemas.clone(),
                        ToolChoice::Auto,
                    )
                    .await
                {
                    Ok(s) => s,
                    Err(err) => {
                        tracing::warn!(error = %err, "agent loop stream failed; ending turn");
                        break 'rounds;
                    }
                };
                let mut saw_tool = false;
                let mut redactor = RedactingBuffer::new(ledger.private_roll_tokens());
                while let Some(event) = next_stream_event(&mut stream, cancel, &mut cancelled).await
                {
                    let event = match event {
                        Ok(e) => e,
                        Err(err) => {
                            tracing::warn!(error = %err, "agent loop stream event error");
                            break;
                        }
                    };
                    match event {
                        StreamEvent::ContentDelta(delta) => {
                            if !blocked {
                                let safe = redactor.push(&delta);
                                if !safe.is_empty() {
                                    // buffer 模式(stream_prose=false)：只累积进 visible_text,
                                    // 不直发玩家——adjudicator prose 留给 Narrator 投影后再流出。
                                    if stream_prose {
                                        let _ = tx.send(TurnEvent::Delta(safe.clone())).await;
                                    }
                                    visible_text.push_str(&safe);
                                }
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
                                let output_value =
                                    serde_json::from_str::<serde_json::Value>(&outcome.content)
                                        .unwrap_or_else(
                                            |_| json!({"raw": outcome.content.as_str()}),
                                        );
                                let status = if output_value.get("error").is_some() {
                                    "error"
                                } else {
                                    "done"
                                };
                                let _ = tool_ctx
                                    .engine
                                    .db
                                    .insert_agent_tool_call(&trpg_model::AgentToolCallRecord {
                                        tool_call_id: outcome.tool_call_id.clone(),
                                        session_id: input.request.session_id.clone(),
                                        turn_id: input.request.turn_id.clone(),
                                        tool_name: outcome.name.clone(),
                                        visibility: Visibility::GmOnly,
                                        input_json: serde_json::from_str(&call.arguments)
                                            .unwrap_or_else(
                                                |_| json!({"raw": call.arguments.as_str()}),
                                            ),
                                        output_json: Some(output_value),
                                        status: status.to_string(),
                                        error: None,
                                        created_at: Utc::now(),
                                    })
                                    .await;
                                messages.push_tool_result(
                                    &outcome.tool_call_id,
                                    &outcome.name,
                                    &outcome.content,
                                );
                                if let Some(gate) = outcome.awaiting_player_roll {
                                    drain_redactor_tx(
                                        &mut redactor,
                                        blocked,
                                        tx,
                                        &mut visible_text,
                                        stream_prose,
                                    )
                                    .await;
                                    awaiting = Some(gate);
                                    break 'rounds;
                                }
                            }
                            drain_redactor_tx(
                                &mut redactor,
                                blocked,
                                tx,
                                &mut visible_text,
                                stream_prose,
                            )
                            .await;
                            redactor = RedactingBuffer::new(ledger.private_roll_tokens());
                        }
                        StreamEvent::Done { .. } => {}
                    }
                }
                drain_redactor_tx(&mut redactor, blocked, tx, &mut visible_text, stream_prose)
                    .await;
                // P1-3 follow-up：流分块间 select 命中取消 ⇒ 放弃整个 agent loop（不续轮、不逼散文）。
                if cancelled {
                    break 'rounds;
                }
                if saw_tool || blocked {
                    continue 'rounds;
                }
                narrated = true;
                break 'rounds;
            }
        }
        self.obligations = obligations_cell
            .into_inner()
            .unwrap_or_else(|p| p.into_inner());
        // P6.7：取回本回合 reveal 提名（gating ON 时非空）供 PresentationCommit 提交。
        ctx.nominated_reveals = reveals_cell.into_inner().unwrap_or_else(|p| p.into_inner());
        // P6 revision：取回本回合玩家拒绝提名（story-write loop ON 时非空）供 PresentationCommit
        // 边界 drain → commit_story_writes 持久化。
        ctx.rejected_nominations = rejections_cell
            .into_inner()
            .unwrap_or_else(|p| p.into_inner());
        // MAT.M3 axis-2：成功侦查 → 提名揭示一条 source-backed 线索 FACT（DP-3）。双闸：
        // 仅 MaterializationAffordanceMode::Enforce ∧ reveal-gating ON 才产提名（Off/Shadow/
        // gating-off ⇒ 严格无操作，字节级基线）。提名经既有 RevealNomination 通道 →
        // PresentationCommit 边界终审 Allow 后 commit（proposal-only：本路径不调 reveal_fact /
        // 不写 DB）。reveal_gating 与 reveals_cell 取回为前置（OFF 时 ctx.nominated_reveals 恒空）。
        // 传 disjoint 字段引用（非 &mut ctx 整体）：messages/ledger 借用 ctx 其它字段且贯穿本
        // 函数，故经独立字段引用避开整体可变重借（split-borrow）。
        self.apply_clue_affordance(
            &ctx.state_agent,
            input,
            ledger.snapshot(),
            &mut ctx.nominated_reveals,
            reveal_gating,
        )
        .await;
        // —— 终态 A：request_player_roll gate ——
        if let Some(gate) = awaiting {
            ctx.visible_text = visible_text;
            let _ = tx
                .send(TurnEvent::AwaitingPlayerRoll {
                    check_id: gate.check_id.clone(),
                    prompt_public: gate.prompt_public.clone(),
                })
                .await;
            ctx.awaiting_gate = Some(gate);
            return crate::execute::AgentSignal::AwaitingPlayerRoll;
        }
        // —— 轮数耗尽且模型仍要工具：追加唯一一轮 ToolChoice::None 逼散文 ——
        // P1-3 follow-up：已取消则连这轮逼散文 LLM 都不发（cancelled 在主循环置位）。
        if !narrated && !cancelled {
            match self
                .llm
                .stream_chat_with_tools(messages.to_request_messages(), schemas, ToolChoice::None)
                .await
            {
                Ok(mut stream) => {
                    let mut redactor = RedactingBuffer::new(ledger.private_roll_tokens());
                    while let Some(event) =
                        next_stream_event(&mut stream, cancel, &mut cancelled).await
                    {
                        let event = match event {
                            Ok(e) => e,
                            Err(err) => {
                                tracing::warn!(error = %err, "agent loop forced-prose stream error");
                                break;
                            }
                        };
                        if let StreamEvent::ContentDelta(delta) = event {
                            let safe = redactor.push(&delta);
                            if !safe.is_empty() {
                                if stream_prose {
                                    let _ = tx.send(TurnEvent::Delta(safe.clone())).await;
                                }
                                visible_text.push_str(&safe);
                            }
                        }
                    }
                    // 取消则放弃残余尾窗（生成已弃）；非取消路径与原逐字一致。
                    if !cancelled {
                        let rest = redactor.finish();
                        if !rest.is_empty() {
                            if stream_prose {
                                let _ = tx.send(TurnEvent::Delta(rest.clone())).await;
                            }
                            visible_text.push_str(&rest);
                        }
                    }
                }
                Err(err) => tracing::warn!(error = %err, "agent loop forced-prose stream failed"),
            }
        }
        ctx.visible_text = visible_text;
        crate::execute::AgentSignal::Narration
    }

    /// P1 Narrator 阶段（TRPG_NARRATOR_SPLIT ON）：从本回合 ctx 投影 NarrationPacket，
    /// 调无工具 Narrator 产玩家散文经 tx 流出，回写 ctx.visible_text（assistant_output 单一
    /// 事实源，供 verify/finalize/记忆）。仅 Narration 终态调用（AwaitingPlayerRoll 走桌面骰
    /// prompt_public，不经 Narrator）。fail-soft：Narrator 失败/空 → 回退直发 adjudicator prose
    /// （= OFF 基线行为，无新增泄漏），绝不让回合空白。
    pub(crate) async fn run_narrator_phase(
        &self,
        ctx: &mut TurnContext,
        input: &GmTurnInput<'_>,
        tx: &tokio::sync::mpsc::Sender<crate::turn_event::TurnEvent>,
        cancel: Option<&CancellationToken>,
    ) {
        // buffer 模式下，adjudicator 自由文本已（私骰已 redact）累积在 ctx.visible_text。
        let adjudicator_prose = ctx.visible_text.clone();
        // P6.7：BeforeNarration 已从此 split-only 切点重定位到 PresentationCommit 边界
        //（presentation_commit_boundary，对 split / 非 split 路径一致触发）——此处不再触发。
        let adj = crate::packet::AdjudicationPacket::project(
            input.user_input,
            ctx.ledger.snapshot(),
            &ctx.resolved_gate_facts,
            &adjudicator_prose,
            None,
        );
        // StyleProfile：P1 用中性默认（空串 → NarrationPacket 内置第二人称 persona 默认，A1）；
        // 不灌入完整 gm_skill（可能含规则原文）。forbidden_reveals P1 为空（P2/P3 收窄）。
        // A2(§6 大考)：注入 player-safe scene_context(= 上一回合已交付 narration / 玩家已可见)，
        // 给饿肚子的 split Narrator 补感官/连续性 grounding。来源 player-safe ⇒ 不新增泄漏面。
        let narration = crate::packet::NarrationPacket::project(&adj, "", &[])
            .with_scene_context(&player_safe_scene_context(input));
        let private_tokens = ctx.ledger.private_roll_tokens();
        // P7.3：P1 Narrator 派发经 NarratorPort 适配器（GmLoopNarratorAdapter 仅委托
        // `GmLoop::run_narrator`）——dispatch 间接，byte-identical（无行为变更）。
        let narrated = crate::ports::GmLoopNarratorAdapter(self)
            .narrate(&narration, &private_tokens, tx, cancel)
            .await;
        match narrated {
            Some(text) if !text.trim().is_empty() => {
                ctx.visible_text = text; // Narrator 输出 = 玩家可见单一事实源
            }
            _ => {
                tracing::warn!(
                    turn_id = %input.request.turn_id,
                    "narrator produced no output; fail-soft to adjudicator prose"
                );
                if !adjudicator_prose.trim().is_empty() {
                    let _ = tx
                        .send(crate::turn_event::TurnEvent::Delta(
                            adjudicator_prose.clone(),
                        ))
                        .await;
                }
                ctx.visible_text = adjudicator_prose;
            }
        }
        // Q-7-REVISED (§6 大考 v3): the API/transport emits ALL [xxx] tags RAW — narration /
        // dialogue / roll / system / choice / hide / meta. The previous v2 behavior (rebuild
        // player_text by stripping {meta,hide}) is WRONG: strip/hiding is a UI-LAYER concern for
        // later, NOT the transport. So `ctx.visible_text` (the single persisted/transported source)
        // is left RAW. The typed parser still CLASSIFIES audience (kept for an audit trace + for
        // the downstream 战报/product-evaluator to LABEL each block 玩家可见/隐藏) but does NOT
        // strip here. TRPG_GM_CRAFT ON only; OFF ⇒ untouched (byte-equal baseline).
        if crate::gm_craft::enabled() {
            let classified = crate::presentation_markup::strip_player_markup(&ctx.visible_text);
            tracing::debug!(
                turn_id = %input.request.turn_id,
                meta_blocks = classified.meta_blocks.len(),
                hide_blocks = classified.hide_blocks.len(),
                empty_rolls = classified.empty_rolls_unwrapped,
                "gm_craft typed turn-document: audience classified (emit RAW, no strip at API)"
            );
            // NOTE: ctx.visible_text intentionally NOT overwritten — emit raw (Q-7-REVISED).
        }
    }

    /// 无工具 Narrator：用最小 narrator system prompt + NarrationPacket 流式产玩家散文。
    /// 空 tools schema + ToolChoice::None（§19-#5：Narrator 物理上无 mutation 工具）；
    /// 沿用 RedactingBuffer 防私骰泄漏；cancel 沿用 run_agent_loop 同款处理。
    /// 返回 None 表 LLM 失败（交调用方 fail-soft）。
    pub(crate) async fn run_narrator(
        &self,
        packet: &crate::packet::NarrationPacket,
        private_tokens: &[String],
        tx: &tokio::sync::mpsc::Sender<crate::turn_event::TurnEvent>,
        cancel: Option<&CancellationToken>,
    ) -> Option<String> {
        // G1：env-gated「场景感兜底」。OFF（默认）字节等价旧装配；这是唯一读 env 处。
        let sensory_floor = std::env::var("TRPG_SCENE_SENSORY_FLOOR")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let messages = build_narrator_messages(packet, sensory_floor, crate::gm_craft::enabled());
        let mut stream = match self
            .llm
            .stream_chat_with_tools(messages, vec![], ToolChoice::None)
            .await
        {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(error = %err, "narrator stream failed");
                return None;
            }
        };
        let mut visible_text = String::new();
        let mut redactor = RedactingBuffer::new(private_tokens.to_vec());
        let mut cancelled = false;
        while let Some(event) = next_stream_event(&mut stream, cancel, &mut cancelled).await {
            let event = match event {
                Ok(e) => e,
                Err(err) => {
                    tracing::warn!(error = %err, "narrator stream event error");
                    break;
                }
            };
            if let StreamEvent::ContentDelta(delta) = event {
                let safe = redactor.push(&delta);
                if !safe.is_empty() {
                    let _ = tx
                        .send(crate::turn_event::TurnEvent::Delta(safe.clone()))
                        .await;
                    visible_text.push_str(&safe);
                }
            }
        }
        if !cancelled {
            let rest = redactor.finish();
            if !rest.is_empty() {
                let _ = tx
                    .send(crate::turn_event::TurnEvent::Delta(rest.clone()))
                    .await;
                visible_text.push_str(&rest);
            }
        }
        Some(visible_text)
    }

    /// PhaseId::RecordPlayerAction — world event PlayerAction（spec §4 头部第 1）。
    pub(crate) async fn phase_record_player_action(
        &self,
        _ctx: &mut TurnContext,
        input: &GmTurnInput<'_>,
    ) {
        let _ = self
            .engine
            .record_world_event(
                &input.request.session_id,
                Some(&input.request.turn_id),
                None,
                WorldEventKind::PlayerAction,
                json!({"input": input.user_input}),
                Visibility::GmOnly,
            )
            .await;
    }

    /// PhaseId::RefreshLiveDerived — refresh_actor_live_derived（spec §4 头部第 2）。
    pub(crate) async fn phase_refresh_live_derived(
        &self,
        _ctx: &mut TurnContext,
        input: &GmTurnInput<'_>,
    ) {
        let _ = self
            .engine
            .refresh_actor_live_derived(
                &input.request.session_id,
                input
                    .request
                    .viewer
                    .actor_id
                    .as_deref()
                    .unwrap_or("pc.current"),
            )
            .await;
    }

    /// PhaseId::Reconcile — 机制对账（spec §4 顺序：gate 结算之前显式调，幂等；
    /// prepare_turn_context 内部也会 reconcile，但那发生在 gate 结算之后）。
    pub(crate) async fn phase_reconcile(&self, _ctx: &mut TurnContext, input: &GmTurnInput<'_>) {
        let _ = InteractionLifecycleKernel::new(self.engine.db.clone())
            .reconcile_session(&input.request.session_id)
            .await;
    }

    /// PhaseId::Gate — request_player_roll 闸门结算（gate.rs 单点；含裸 "roll"
    /// 兜底 + Err 折叠 + C7 effect_policy 强制；spec §4 头部第 4）。
    pub(crate) async fn phase_gate(&self, ctx: &mut TurnContext, input: &GmTurnInput<'_>) {
        crate::gate::resolve_pending_gate(
            &self.engine,
            self.gate_resolver.as_ref(),
            input.request,
            input.state.scene_id.as_deref(),
            input.user_input,
            &mut ctx.ledger,
            &mut ctx.resolved_gate_facts,
        )
        .await;
    }

    /// PhaseId::StimulusPass — TurnStart hook dues + 语义被动刺激预 pass（J2 SAN）。
    /// hook 先、stimulus 后并入 ctx.pending_dues，供 debt_load 按序吸收。
    /// fail-closed：门关/无目录/LLM 失败 → 空，绝不阻断回合（unwrap_or_default）。
    pub(crate) async fn phase_stimulus_pass(&self, ctx: &mut TurnContext, input: &GmTurnInput<'_>) {
        let hook_dues = trpg_mechanics::RefereeCombatService::new(self.engine.db.clone())
            .dues_for_hook(
                &input.request.session_id,
                &input.request.turn_id,
                &input.request.ruleset_id,
                &trpg_mechanics::watcher::HookEvent::TurnStart,
            )
            .await
            .unwrap_or_default();
        ctx.pending_dues.extend(hook_dues);
        let stimulus_dues = if crate::stimulus::stimulus_pass_enabled() {
            match self
                .engine
                .db
                .load_rule_kernel(&input.request.ruleset_id)
                .await
            {
                Ok(Some(kernel)) if !kernel.mechanics_catalog.is_empty() => {
                    let recent = input.recent_transcript.or_else(|| {
                        input
                            .history
                            .iter()
                            .rev()
                            .find(|m| m.role == "assistant")
                            .map(|m| m.content.as_str())
                    });
                    let candidates = crate::stimulus::stimulus_due_candidates(
                        &self.llm,
                        &kernel.mechanics_catalog,
                        &input.request.session_id,
                        &input.request.turn_id,
                        input.user_input,
                        recent,
                    )
                    .await;
                    trpg_mechanics::RefereeCombatService::new(self.engine.db.clone())
                        .admit_dues(&input.request.session_id, candidates)
                        .await
                        .unwrap_or_default()
                }
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        };
        ctx.pending_dues.extend(stimulus_dues);
    }

    /// PhaseId::OpposedPrepass — 回合头部对抗绑定（现搓防御方 NPC + 备 OpposedBinding
    /// 供 roll_check 注入）。fail-closed：门关/无 NPC/无攻击意图 → None。
    pub(crate) async fn phase_opposed_prepass(
        &self,
        ctx: &mut TurnContext,
        input: &GmTurnInput<'_>,
    ) {
        ctx.opposed_binding = crate::opposed_prepass::prepare_binding(
            &self.engine,
            &self.llm,
            input.request,
            input.state,
            input.user_input,
            input.recent_transcript,
            input.history,
        )
        .await;
    }

    /// PhaseId::ModeInference — active state_frame → mode 推导（三期 §4.1）。manifest
    /// 损坏 → Err fail-closed 终止回合；db 失败 unwrap_or_default 绝不阻断。
    pub(crate) async fn phase_mode_inference(
        &self,
        ctx: &mut TurnContext,
        input: &GmTurnInput<'_>,
    ) -> Result<()> {
        ctx.active_frames = self
            .engine
            .db
            .list_active_state_frames(&input.request.session_id, 8)
            .await
            .unwrap_or_default();
        ctx.mode_manifest = crate::mode::active_mode_manifest(&self.data_dir, &ctx.active_frames)?;
        ctx.mode_id = ctx.mode_manifest.as_ref().map(|m| m.mode_id.clone());
        Ok(())
    }

    /// PhaseId::DebtLoad — B6 债务装载（spec §5.3）：begin_turn + mode 退出义务
    /// re-seed + leftover/hook/stimulus dues 吸收（按 due_id 去重）→ carryover block。
    pub(crate) async fn phase_debt_load(&mut self, ctx: &mut TurnContext, input: &GmTurnInput<'_>) {
        self.obligations.begin_turn(&input.request.turn_id);
        if let Some(m) = &ctx.mode_manifest {
            self.obligations
                .ensure_mode_exit_obligations(&m.mode_id, &m.exit_obligations);
        }
        let leftover_dues = self
            .engine
            .db
            .list_open_mechanic_dues(&input.request.session_id)
            .await
            .unwrap_or_default();
        self.obligations.absorb_dues(leftover_dues);
        self.obligations
            .absorb_dues(std::mem::take(&mut ctx.pending_dues));
        ctx.obligations_block = self.obligations.carryover_block();
    }

    /// PhaseId::ContextAssembly — prepare_turn_context + 四级 gm_skill 合并 + mode
    /// 目录联动 + errata/novelty BP3 块 + TurnMessages 组装 + mode 工具/节拍参数。
    /// fail-closed：budget 超限 / gm_skill 缺 / 未知工具名 → Err 终止回合。
    pub(crate) async fn phase_context_assembly(
        &self,
        ctx: &mut TurnContext,
        input: &GmTurnInput<'_>,
    ) -> Result<()> {
        ctx.state_agent = input.state.clone();
        ctx.state_agent.agent_loop_protocol = true;
        ctx.compiled = match &self.ctx_provider {
            Some(provider) => provider(input.request, &ctx.state_agent),
            None => {
                self.engine
                    .prepare_turn_context(
                        input.request,
                        &ctx.state_agent,
                        Some(input.user_input),
                        input.recent_transcript,
                    )
                    .await?
            }
        };
        crate::prompts::validate_compiled_budget(&ctx.compiled, input.request)?;
        // 单点载 RuleKernel：供 mode-catalog 子集（下方）+ 影子 binding（turn_trace 读 ctx.rule_kernel
        // 出权威 ruleset_check/ruleset_resource 绑定）。一次 DB 读、fail-soft（载不到 → None，
        // 影子退化为仅 need_kind 启发式，零行为变更）。
        match self
            .engine
            .db
            .load_rule_kernel(&input.request.ruleset_id)
            .await
        {
            Ok(k) => ctx.rule_kernel = k,
            Err(err) => {
                tracing::warn!(error = %err, "rule kernel load failed (shadow binding/mode catalog degrade)")
            }
        }
        // 四级 gm_skill 合并 + 声明式 prompt 插件（按当前 ruleset/module 过滤）。
        // 无命中插件时与 load_gm_skill_with_mode 字节级一致 → 不影响缓存稳定性。
        let mut gm_skill = load_gm_skill_with_plugins(
            &self.data_dir,
            &input.request.ruleset_id,
            ctx.mode_id.as_deref(),
            input.request.module_id.as_deref(),
        )?;
        if let Some(manifest) = &ctx.mode_manifest {
            match ctx.rule_kernel.as_ref() {
                Some(kernel) => {
                    if let Some(section) = crate::mode_catalog::mode_catalog_section(
                        &manifest.mode_id,
                        &manifest.catalog_filter,
                        &kernel.mechanics_catalog,
                    ) {
                        gm_skill.push_str("\n\n---\n\n");
                        gm_skill.push_str(&section);
                    }
                }
                None => {
                    tracing::warn!(ruleset_id = %input.request.ruleset_id, "mode catalog subset skipped: no active rule kernel")
                }
            }
        }
        // Policy 插件 host（ContextAssembly hook）：内置 policy 插件 propose PromptBlock 贡献，
        // 按 host 序（safety>priority）把其渲染文本以同 join 风格追加进 gm_skill，并把每条
        // 贡献折成 trace 进 ctx（build_turn_trace 拷进 TurnTrace.plugin_contributions）。
        // 全程 fail-soft：host/追加任何环节出错只 warn，绝不中断回合。ContextFilter 删块源
        // 已接（模组图谱 SpoilerMeta 派生私有 block 视图）；AfterLlmStream verifier 见 verify_after_stream。
        self.apply_context_assembly_plugins(ctx, input, &mut gm_skill)
            .await;
        ctx.gm_skill = gm_skill;
        if let Some(block) = self.errata.errata_block() {
            ctx.errata_blocks.push(block);
        }
        if let Some(block) = self.errata.standing_reminder_block() {
            ctx.errata_blocks.push(block);
        }
        if ctx.mode_id.is_some() {
            if let Some(block) = crate::tools::frame::novelty_block(&ctx.active_frames) {
                ctx.errata_blocks.push(block);
            }
        }
        // 活动 NPC 行为指引（TC-D3-02）：fail-soft 派生 prompt-safe 块；无块 ⇒ None ⇒
        // dynamic tail 不写块（非 NPC 回合字节不变，缓存稳定）。P4.6 Part B（flag-gated）：
        // 当 World 反应隐含 Attack intent 时，本调用还会把 Rules 结算出的 NPC-action 结果
        // 折进 ctx.resolved_gate_facts（→ dynamic tail / player_perceivable_facts，load-bearing）
        // 并向 Flight Recorder 记一条可观测 trace —— OFF ⇒ 全 no-op ⇒ 字节不变。
        // MAT.M7 (D2): the player deliberately engaging a present NPC IS the "met" act.
        // The opposed pre-pass has DETERMINISTICALLY resolved (fail-closed, by id) the
        // contact/opposed target the player initiated this turn. If that target is an active
        // NPC, write a PlayerExposed for it BEFORE build_npc_behavior_guidance runs — so the
        // same-turn met/engaged gate (which reads list_surfaced_entities) now treats it as
        // met and stops constraining it (breaks the M6 self-identification deadlock where an
        // un-met NPC can never be named, so can never become met). Enforce-only; Off/Shadow
        // ⇒ record_player_engaged_npc is a pure no-op ⇒ byte-identical baseline.
        let mat_mode = trpg_model::MaterializationAffordanceMode::from_env();
        if mat_mode.is_enforce() {
            if let Some(binding) = ctx.opposed_binding.as_ref() {
                let engaged_id = binding.persona.actor_id.clone();
                let active = ctx.effective_active_npc_ids(input.state).to_vec();
                self.engine
                    .record_player_engaged_npc(input.request, &active, &engaged_id, mat_mode)
                    .await;
            }
        }
        let npc_guidance = self.build_npc_behavior_guidance(ctx, input).await;
        // P5.6: the Director packet was stashed on ctx by build_npc_behavior_guidance (flag OFF
        // ⇒ None ⇒ no block ⇒ byte-identical). Mirror npc_guidance: pass as Option<&str>.
        let director_packet = ctx.director_packet_block.clone();
        let tail = DynamicTailInput {
            user_input: input.user_input,
            resolved_gate_facts: &ctx.resolved_gate_facts,
            errata_blocks: &ctx.errata_blocks,
            obligations_block: ctx.obligations_block.as_deref(),
            npc_guidance_block: npc_guidance.as_deref(),
            director_packet_block: director_packet.as_deref(),
        };
        ctx.messages = Some(TurnMessages::assemble(
            &ctx.compiled,
            &ctx.gm_skill,
            input.history,
            &tail,
        ));
        ctx.mode_tools = match ctx.mode_id.as_deref() {
            Some(mode) => Some(ToolRegistry::for_mode(&self.data_dir, Some(mode))?),
            None => None,
        };
        ctx.max_tool_rounds = ctx
            .mode_manifest
            .as_ref()
            .and_then(|m| m.tempo.max_tool_rounds)
            .unwrap_or(self.cfg.max_tool_rounds);
        ctx.effect_closure_per_cluster = ctx
            .mode_manifest
            .as_ref()
            .and_then(|m| m.tempo.effect_closure_per_cluster)
            .unwrap_or(false);
        Ok(())
    }

    /// ContextAssembly hook：跑内置 policy 插件 host，把 PromptBlock 贡献的渲染文本以
    /// `\n\n---\n\n`（同 load_gm_skill_with_plugins 风格）追加进 `gm_skill`，并把**每条**
    /// 贡献（含未来 ContextFilter/finding 类）折成 trace 进 `ctx.plugin_contributions`。
    ///
    /// 全程 fail-soft（D2）：插件路径任何环节失败只 warn，绝不中断回合——叙事/缓存优先。
    /// PromptBlock 追加文本 + ContextFilter 删块（私有 block 视图源自模组图谱 SpoilerMeta，
    /// TC-D3-00）。无贡献时 gm_skill 逐字不变（非模组 session = NoSpoilerGuard 空贡献 → 缓存稳定）。
    pub(crate) async fn apply_context_assembly_plugins(
        &self,
        ctx: &mut TurnContext,
        input: &GmTurnInput<'_>,
        gm_skill: &mut String,
    ) {
        let surfaced_entities = self
            .engine
            .db
            .list_surfaced_entities(&input.request.session_id)
            .await
            .unwrap_or_default();
        let compiled_block_ids: Vec<String> = ctx
            .compiled
            .prefix_blocks
            .iter()
            .chain(ctx.compiled.pinned_blocks.iter())
            .chain(ctx.compiled.dynamic_blocks.iter())
            .map(|b| b.block_id.clone())
            .collect();
        // 私有 block 元数据快照（fail-closed 过滤用，不进 prompt）。生产源（TC-D3-00）：
        // 优先从本会话模组图谱的权威 SpoilerMeta 派生场景块视图（fact_id=node_id，secret=
        // 非活动 spoiler 场景）；图谱缺失/非场景块 → 回退显式 tag 分类（向后兼容旧路径）。
        // 活动场景块由 runtime spoiler_guard 逐词裁剪，不整块删（见 derive_scene_block_view）。
        let scene_graph = match input
            .request
            .module_id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
        {
            Some(module_id) => match self.engine.db.load_module_graph(module_id).await {
                Ok(g) => g,
                Err(err) => {
                    tracing::warn!(error = %err, module_id, "private_blocks: module graph 载入失败，回退 tag 分类");
                    None
                }
            },
            None => None,
        };
        let active_scene_id = ctx.state_agent.scene_id.as_deref();
        let private_blocks: Vec<crate::plugin::PrivateBlockView> = ctx
            .compiled
            .prefix_blocks
            .iter()
            .chain(ctx.compiled.pinned_blocks.iter())
            .chain(ctx.compiled.dynamic_blocks.iter())
            .map(|b| {
                scene_graph
                    .as_ref()
                    .and_then(|g| crate::plugin::derive_scene_block_view(b, g, active_scene_id))
                    .unwrap_or_else(|| crate::plugin::builtin_no_spoiler::private_block_view(b))
            })
            .collect();
        // 玩家方已知/已揭示 fact 集（KnowledgeEdge player_party knows_true 投影）。DB 抖动
        // → 空集 = 全按未知（fail-closed），不赌 DB。
        let player_known_fact_ids = self
            .engine
            .db
            .list_player_known_fact_ids(&input.request.session_id)
            .await
            .unwrap_or_default();
        // DA-PIPE-01 provenance: record that the GM context, active NPC mind views,
        // and the player-knowledge projection were all loaded BEFORE the context-
        // policy plugins run. These entries are pushed first, so in the persisted
        // TurnTrace.plugin_contributions they precede every plugin contribution —
        // the ordering (views → plugins) is observable in the artifact, not just a
        // comment. The player_knowledge_view count is the committed player_party
        // knows_true projection, linking the trace to committed knowledge state.
        ctx.plugin_contributions.push(view_load_trace(
            "gm_context",
            &format!("{} compiled block(s)", compiled_block_ids.len()),
        ));
        ctx.plugin_contributions.push(view_load_trace(
            "npc_mind_views",
            // MAT.M7 (D1): 读派生集（prepare_turn_context 填 ctx.compiled.active_npc_ids），
            // 而非陈旧的 input.state.active_npc_ids（Enforce 下后者恒空 → "0 active npc(s)"）。
            &format!(
                "{} active npc(s)",
                ctx.effective_active_npc_ids(input.state).len()
            ),
        ));
        ctx.plugin_contributions.push(view_load_trace(
            "player_knowledge_view",
            &format!("{} player-known fact(s)", player_known_fact_ids.len()),
        ));
        let plugin_ctx = crate::plugin::PluginContext {
            session_id: input.request.session_id.clone(),
            turn_id: input.request.turn_id.clone(),
            ruleset_id: input.request.ruleset_id.clone(),
            module_id: input.request.module_id.clone(),
            hook: crate::plugin::PluginHook::ContextAssembly,
            surfaced_entities,
            narration: None,
            compiled_block_ids,
            player_known_fact_ids,
            private_blocks,
            secret_terms: vec![],
            config: json!({}),
        };
        let contributions = crate::plugin::builtin_plugin_host()
            .run_hook(&plugin_ctx)
            .await;
        // ContextFilter 跨贡献汇总的 drop_block_ids（保守删：T3 才接，今天无内置 emitter →
        // 恒空 → apply_context_filter no-op → ctx.compiled byte-stable）。
        let mut drop_block_ids: Vec<String> = Vec::new();
        for c in &contributions {
            // 每条贡献都记 trace（含 ContextFilter/finding 类）。
            ctx.plugin_contributions.push(c.to_trace());
            match &c.kind {
                // PromptBlock：把渲染文本以同 join 风格追加进 gm_skill。
                crate::plugin::PluginContributionKind::PromptBlock(block) => {
                    gm_skill.push_str("\n\n---\n\n");
                    gm_skill.push_str(&block.content.render_text());
                }
                // ContextFilter：汇总待删 block_id（应用挪到循环后，避免循环内借用 ctx.compiled）。
                crate::plugin::PluginContributionKind::ContextFilter(spec) => {
                    drop_block_ids.extend(spec.drop_block_ids.iter().cloned());
                }
                // VerifierFinding 不在 ContextAssembly hook 产出（见 AfterLlmStream）。
                crate::plugin::PluginContributionKind::VerifierFinding(_) => {}
                // Proposal 仅在 HeavyPostprocess hook 产出；即便误挂到此 hook 也只记 trace
                // （上方已 push）、不应用、绝不落库（propose-not-commit），fail-safe no-op。
                crate::plugin::PluginContributionKind::Proposal(_) => {}
            }
        }
        // ContextFilter 应用（fail-soft）：删命中块 + 经 runtime 同渲染路径重算受影响 band
        // 的 text/hash（drop 集为空 → 完全 no-op，零行为变更）。
        if !drop_block_ids.is_empty() {
            trpg_runtime::apply_context_filter(&mut ctx.compiled, &drop_block_ids);
        }
        // DA-PIPE-01 invariant, enforced at the source: the view-load provenance
        // entries recorded above must precede every plugin contribution in the
        // flight recorder. Zero-cost in release; trips in dev/test if the ordering
        // ever regresses.
        debug_assert!(
            flight_recorder_view_order_ok(&ctx.plugin_contributions),
            "view-load provenance must precede plugin contributions in the flight recorder"
        );
    }

    /// 活动 NPC 行为指引（TC-D3-02）：为每个活动 NPC 载 durable profile → 投该 NPC
    /// 自身 mind view → 派生 secret-gated `NpcBehaviorPlan` → 渲染 prompt-safe 块，
    /// 全部以 `\n\n` 拼接返回。无活动 NPC / 全部跳过 ⇒ None（dynamic tail 不写块，
    /// 非 NPC 回合字节不变 → 缓存稳定）。
    ///
    /// 活动 NPC 源：`RuntimeState.active_npc_ids`（source-backed）。全程 fail-soft/closed：
    /// profile 缺失、NPC id 不稳定、行损坏 / DB 读失败 ⇒ 跳过该 NPC 并 trace why，绝不
    /// 臆造 persona。secret 门（v1 保守）：玩家方未知的「NPC 已知为真」fact id 视作
    /// withheld secret id（仅 id，绝无 GM-only secret 文本或玩家未知 fact 散文）。
    pub(crate) async fn build_npc_behavior_guidance(
        &self,
        ctx: &mut TurnContext,
        input: &GmTurnInput<'_>,
    ) -> Option<String> {
        // MAT.M7 (D1): consume the per-turn DERIVED active set (prepare_turn_context filled
        // ctx.compiled.active_npc_ids), not the stale caller-supplied input.state set (empty
        // under Enforce → no NPC guidance was ever produced). Snapshot to an owned Vec so the
        // later `&mut ctx` borrows (director packet stash) don't conflict. Off/Shadow: derived
        // == caller-supplied ⇒ byte-equal baseline; ctx_provider seam: derived empty ⇒ falls
        // back to state ⇒ existing gm unit tests unchanged.
        let active: Vec<String> = ctx.effective_active_npc_ids(input.state).to_vec();
        let active = &active;
        if active.is_empty() {
            return None;
        }
        let session_id = &input.request.session_id;
        // 玩家方已知 fact 集（fail-closed：DB 抖动 → 空集 = 全按未知 = 全 withheld）。
        let player_known = self
            .engine
            .db
            .list_player_known_fact_ids(session_id)
            .await
            .unwrap_or_default();
        let targets = [trpg_model::NpcRelationshipTarget::PlayerParty];
        // P4.6 Part A: route NPC guidance THROUGH the typed World layer, byte-identically.
        // Same DB loads as the legacy per-NPC path (one `load_npc_profile` + one
        // `load_active_npc_guidance` per active NPC, same skip semantics) — FRAMEWORK §2
        // reflection ①: DB-load count == baseline. We pre-load profiles here (identical to
        // before), keep only the active ids that have a durable profile, then hand the
        // matched (ids, profiles) to `load_world_reaction_plans`, which calls the SAME
        // `load_active_npc_guidance` per NPC in the SAME order.
        let mut npc_ids: Vec<String> = Vec::new();
        let mut profiles: Vec<trpg_model::NpcProfile> = Vec::new();
        for npc_id in active {
            match self.engine.db.load_npc_profile(session_id, npc_id).await {
                Ok(Some(p)) => {
                    npc_ids.push(npc_id.clone());
                    profiles.push(p);
                }
                Ok(None) => {
                    tracing::debug!(npc_id = %npc_id, "npc_behavior_guidance: no durable profile, skipping NPC guidance");
                }
                Err(err) => {
                    tracing::warn!(error = %err, npc_id = %npc_id, "npc_behavior_guidance: profile load failed, skipping NPC guidance");
                }
            }
        }
        // P7.3：World 反应计划装载经 WorldPort 适配器派发（EngineWorldAdapter 仅委托
        // `load_world_reaction_plans`，set 为 lossy/plans 为 lossless render 源不变）——
        // dispatch 间接，byte-identical（OFF 默认无行为变更）。
        let (mut reaction_set, plans) = crate::ports::EngineWorldAdapter(&self.engine)
            .load_reaction_plans(session_id, &npc_ids, &profiles, &targets, &player_known)
            .await;
        // P4.6 Part B (flag-gated, default OFF): when an active NPC's reaction implies an
        // Attack intent, the World layer EMITS the typed intent (it resolves nothing). The
        // Rules/Kernel path settles it downstream via `execute_system_roll_bundle` (the SAME
        // entry the GM `roll_check` tool uses via `tools::settle`, with kernel-default
        // hydration ⇒ a real hit/miss band). OFF ⇒ no-op ⇒ byte-identical turn.
        //
        // The resolved outcome is OBSERVABLY CONSUMED (FRAMEWORK §2, no silent `let _`
        // drop): (a) folded as a player-perceivable `[roll]` gate fact into
        // `ctx.resolved_gate_facts` (→ dynamic tail / `player_perceivable_facts`), making
        // the slice load-bearing; and (b) emitted as a `npc_action_resolved` Flight Recorder
        // trace so the resolution is asserted in the persisted `TurnTrace`.
        //
        // P6.3: the resolved attack is now folded THREE ways — (a) advisory gate-fact,
        // (b) Flight Recorder trace, and (c) a typed NpcActionResolved domain_events row
        // (the structured event log, written from the typed WorldAttackOutcome struct
        // fields — never from the to_gate_fact() display string). The check_results row
        // already lands in Rules; this adds the durable domain-event account.
        if crate::npc_action::world_npc_action_enabled() {
            trpg_runtime::world::derive_attack_intents(&plans, &mut reaction_set);
            let outcomes = crate::npc_action::resolve_world_attack_intents(
                &self.engine,
                session_id,
                &input.request.turn_id,
                &input.request.ruleset_id,
                &reaction_set,
            )
            .await;
            for o in &outcomes {
                // (a) load-bearing: the resolved attack reaches the GM context as a
                //     player-perceivable fact (NOT discarded).
                ctx.resolved_gate_facts.push(o.to_gate_fact());
                // (b) observable: the resolution is recorded in the flight recorder. Summary
                //     carries only npc id + verdict band (no secret prose).
                ctx.plugin_contributions
                    .push(npc_action_resolved_trace(&format!(
                        "{}: blocked={} success={:?} tier={:?} check_id={}",
                        o.npc_id, o.blocked, o.success, o.success_tier, o.check_id
                    )));
            }
            // (c) durable: append one NpcActionResolved domain event per resolved attack,
            //     fail-soft (append failure only warns; never reaches control flow).
            crate::npc_action::emit_npc_action_resolved(
                &self.engine.db,
                &outcomes,
                session_id,
                &input.request.turn_id,
            )
            .await;
        }
        // P5.6: build the GM-only Director brief packet from the ALREADY-LOADED World
        // candidate pool (`reaction_set.reactions`) — no NPC re-load. Flag `TRPG_DIRECTOR_PACKET`
        // default OFF ⇒ `prepare_director_brief` short-circuits to None ⇒ no block ⇒
        // byte-identical baseline. ON ⇒ the rendered `[director_packet]` string is stashed on
        // ctx and folded into DynamicTailInput.director_packet_block by phase_context_assembly
        // (GM-only BP3 tail — NEVER the player-visible narration). gm_truth / player_known are
        // loaded inside `prepare_director_brief` as SEPARATE Option reads (codex fold #4).
        let acting_actor_id = input
            .request
            .viewer
            .actor_id
            .clone()
            .unwrap_or_else(|| "pc.current".to_string());
        // P7.3：Director 简报渲染（thin-async 运行时缝）经 DirectorPort 适配器派发
        // （DirectorAdapter::prepare_brief 仅委托 `prepare_director_brief`，flag OFF ⇒ None
        // byte-identical 基线）——dispatch 间接，无行为变更。
        ctx.director_packet_block = crate::ports::DirectorAdapter
            .prepare_brief(
                &self.engine,
                session_id,
                &reaction_set.reactions,
                &acting_actor_id,
            )
            .await;
        // The GM-context guidance bytes render from the retained (lossless) plans via the
        // SAME `to_guidance_block` method as before — provably byte-identical (locked by
        // `render_is_byte_identical_to_legacy_join`).
        let base_block = trpg_runtime::world::render_world_reaction_block(&plans);
        // MAT.M4 (§7-#6): present vs met/engaged gate. Under Enforce, an active NPC the
        // player has NOT yet met/engaged (its id not in the player-exposed set) may react
        // but must NOT volunteer un-met content / initiate un-provoked reveals (rule 6).
        // We append a prompt-safe reactive-only constraint naming the un-met ids. The
        // secret gate is untouched: knowledge_basis already comes only from
        // facts_can_reveal. Off/Shadow ⇒ inert gate ⇒ byte-identical baseline (no append).
        let mode = trpg_model::MaterializationAffordanceMode::from_env();
        if !mode.is_enforce() {
            return base_block; // 字节级基线：Off/Shadow 不收紧主动开口。
        }
        let exposed = self
            .engine
            .db
            .list_surfaced_entities(session_id)
            .await
            .unwrap_or_default();
        let gate = trpg_runtime::derive_met_engaged_gate(active, &exposed, mode);
        trpg_runtime::restrict_unmet_npc_guidance(base_block, &gate)
    }

    /// 流后校验（spec §4 第 5 步）：NarrationVerifier 对账已流出全文 → 勘误记忆
    /// （注入下一轮 dynamic tail）→ 新增勘误折 MemoryEvent 持久化（tags 含
    /// "gm_errata"；落库失败 `let _ =` 吞错——叙事已交付，校验绝不反向中断回合）。
    /// 流后校验。返回**本回合应入 plugin trace 的贡献记录**（advisory，零行为变更）：
    /// (a) 既算出的 NarrationVerifier findings 折成 trace（让 de-facto
    /// `core.no_mechanical_invention` verifier 在 `trpg explain --plugins` 可见，不重跑检查）；
    /// (b) AfterLlmStream hook 返回的 VerifierFinding 贡献（NoSpoilerGuard v2 是内置
    ///     SecretLeak emitter）。secret_terms 生产源（TC-D3-00）已接：从本会话模组图谱
    ///     SpoilerMeta 采集，模组会话含声明 secret 时非空 → 真发 SecretLeak；无模组/无声明
    ///     secret → 空 = 不检测。调用方（phase_verify_after_stream）把它们 push 进 ctx.plugin_contributions。
    pub(crate) async fn verify_after_stream(
        &mut self,
        request: &ContextRequest,
        ledger: &TurnLedger,
        visible_text: &str,
        active_npc_ids: &[String],
    ) -> VerifyAfterStreamOutcome {
        let verifier = trpg_agent::NarrationVerifier;
        // B7 语义决策：agent 不显式声明引用，引擎代填"已落账事实全集"
        // （ledger_id_set 物化，排序保证确定性）——结构化核对退化为"声称的
        // id 必在账本"恒真 + 子串回退被关闭；`fallback:substring_scan: `
        // 标注路径只在账本为空（id 全集为空 → refs 为空）时出现。
        let referenced_ledger_ids = {
            let mut ids: Vec<String> = trpg_agent::ledger_id_set(ledger.snapshot())
                .into_iter()
                .collect();
            ids.sort();
            ids
        };
        let submission = trpg_agent::FinalNarrationSubmission {
            player_visible_text: visible_text.to_string(),
            mechanical_claims: vec![],
            referenced_ledger_ids,
        };
        let result = verifier.verify(ledger.snapshot(), &submission);
        let entries = self.errata.record(&request.turn_id, &result.findings);
        if !entries.is_empty() {
            let event = self.errata.to_memory_event(request, &entries);
            let _ = self.engine.db.save_memory_event(&event).await;
        }
        // B6：InventedEffect → Effect 追溯债务（补 apply_effect 落账）；J2 修复：
        // MissingCheck → Check 追溯债务（补 roll_check）。叙事已流出不可回收，
        // 债务进下回合清单——处理或 waive_obligation 带理由，债务必清。
        let debts = result
            .findings
            .iter()
            .filter_map(|f| {
                let kind = match f.kind {
                    trpg_agent::VerifierFindingKind::InventedEffect => Some(RetroDebtKind::Effect),
                    trpg_agent::VerifierFindingKind::MissingCheck => Some(RetroDebtKind::Check),
                    _ => None,
                };
                kind.map(|kind| RetroactiveEffectDebt {
                    debt_id: format!("debt_{}", Uuid::new_v4().simple()),
                    turn_id: request.turn_id.clone(),
                    finding_detail: f.detail.clone(),
                    kind,
                    created_at: Utc::now(),
                })
            })
            .collect::<Vec<_>>();
        self.obligations.absorb_retro_debts(debts);

        // —— Plugin trace（advisory，零行为变更）——
        // (a) 把既算出的 NarrationVerifier findings 折成 trace（不重跑检查，复用 result.findings）：
        //     de-facto 的 no_mechanical_invention verifier 自此在 `trpg explain --plugins` 可见。
        let mut traces: Vec<trpg_model::PluginContributionTrace> = result
            .findings
            .iter()
            .map(|f| surface_verifier_finding_trace(f))
            .collect();
        // (b) AfterLlmStream hook 接线（fail-soft）：跑内置 policy host；返回的 VerifierFinding
        //     贡献既 record 进 trace、又喂进既有 ErrataMemory 路径（NoSpoilerGuard v2 即此路径）。
        //     secret_terms 源已接入生产（见 run_after_llm_stream_hook 从模组图谱采集）：有声明
        //     secret 即参与泄漏校验；无声明 / 图谱缺失 → 空 = 不检测（fail-soft）。
        let (hook_traces, hook_findings) = self
            .run_after_llm_stream_hook(request, visible_text, active_npc_ids)
            .await;
        traces.extend(hook_traces);

        // —— P2 步骤5/6：PresentationGate 判定（纯函数，advisory）——
        // gate 输入 = NarrationVerifier 结构核对 findings（InventedEffect/ManualRollRequest 等）
        // ∪ AfterLlmStream hook findings（SecretLeak 来源：NoSpoiler/projection/NPC 一致性）。
        // 仅 Blocker × {SecretLeak,InventedEffect,ManualRollRequest} → Block。本方法只**计算**
        // gate，repair ladder 的真正执行在 phase_verify_after_stream（需 tx/cancel/buffer 就位）。
        let mut gate_findings = result.findings.clone();
        gate_findings.extend(hook_findings);
        let gate_input = trpg_agent::NarrationVerifierResult {
            accepted: gate_findings
                .iter()
                .all(|f| f.severity != trpg_agent::VerifierSeverity::Blocker),
            findings: gate_findings,
            next_required_action: None,
        };
        // P7.3：PresentationGate 判定经 PolicyPort 适配器派发（PresentationPolicyAdapter 仅
        // 委托 `presentation_gate_decision` 纯函数）——dispatch 间接，byte-identical。
        let gate = crate::ports::PresentationPolicyAdapter.presentation_gate(&gate_input);
        // gate 决策折一条 advisory trace（OFF/ON 都记，零行为变更；§20 explain --plugins 可见）。
        traces.push(presentation_gate_trace(&gate));
        // A3(§6 大考)：白名单**之外**单独检 OmittedVisibleResult(空心念白)——旁路信号，
        // 不改 gate 判定本身(仍 Allow)。
        let omitted_visible_repair = omitted_visible_result_needs_repair(&gate_input);

        VerifyAfterStreamOutcome {
            traces,
            gate,
            omitted_visible_repair,
        }
    }

    /// AfterLlmStream hook 接线（fail-soft）。构造只读 PluginContext（narration=visible_text），
    /// 跑内置 policy host；对返回的 VerifierFinding 贡献：折 trace + 喂 ErrataMemory（复用既有
    /// record/to_memory_event 持久化路径）。其余 kind 仅记 trace（AfterLlmStream 不应产 PromptBlock/
    /// ContextFilter，但记录以便观测）。返回供调用方入 plugin trace 的记录。
    async fn run_after_llm_stream_hook(
        &mut self,
        request: &ContextRequest,
        visible_text: &str,
        active_npc_ids: &[String],
    ) -> (
        Vec<trpg_model::PluginContributionTrace>,
        Vec<trpg_agent::VerifierFinding>,
    ) {
        let surfaced_entities = self
            .engine
            .db
            .list_surfaced_entities(&request.session_id)
            .await
            .unwrap_or_default();
        // P3.6 单一来源（behavior-preserving）：流后玩家已知 fact 集经
        // [`trpg_runtime::build_verifier_private_view`] 的 player_known 投影**单点**取数
        // （= `project_for_player_narration` = `list_player_known_fact_ids`，字段独立 fail-closed
        // → DB 抖动空集）。**保留**本回合流后新鲜读取的时机（绝不复用 ContextAssembly ~1423
        // 的快照）。等价性：投影内部用 HashSet，`known_fact_ids_sorted()` 与旧的未排序
        // `list_player_known_fact_ids` 喂下游（projection_after_stream_findings 走 HashSet
        // from_player_known；project_for_npc_speech → viewer_behavior_context 也走 HashSet）
        // 均 order-independent，故 findings/trace/gate 逐字节等价。
        // NPC 言谈投影也由本 view 单源装配（npc_speech_views），下游 npc_consistency_after_stream
        // 直接消费——不再二次载入 NPC profile/投影。展示名取自投影自身的 persona safe-view
        // （`mind.persona.name` = 旧 `profile.name`），可见性门匹配逐字节等价；且 build 的
        // NPC 装载与旧 npc_consistency_after_stream 循环同序同跳过语义（同迭代 active_npc_ids、
        // Ok(None)→skip / Err→skip-with-warn、targets=[PlayerParty]、同 player_known_fact_ids）。
        // 净效果：每回合 NPC load 循环数从 2 回到 baseline 的 1，npc_speech_views 不再是死字段。
        //
        // P3.6 DB-load 等价性（load-equivalence）：旧 `npc_consistency_after_stream` 在
        // `active_npc_ids.is_empty() || visible_text.is_empty() || secret_terms.is_empty()` 时
        // **提前返回、绝不载入任何 NPC profile/mind**。故必须先采集 secret_terms，再据**同一条件**
        // 门控 NPC 装载：满足早返条件 → 传空 npc_ids（零 NPC DB 读，与 baseline 逐次相等），否则
        // 传完整 active_npc_ids（与旧路径同集合同序）。player_known **每回合照读**（旧 hook 恒读
        // `list_player_known_fact_ids`），只有 NPC 载入是条件性的。如此 findings/trace **与** DB-load
        // 计数同时与 baseline 逐字节等价。
        let secret_terms = self
            .harvest_session_secret_terms(request.module_id.as_deref())
            .await;
        // 私有泄漏术语表（生产源，TC-D3-00）：从本会话模组图谱的权威 SpoilerMeta（场景级 +
        // 实体级 secret_terms）采集，每条绑 fact_id（node_id / entity id）。只取显式声明，不做
        // 正文模糊扫描。图谱缺失/DB 抖动 → 空 = 不检测（fail-soft，绝不 panic/拦流）。已揭示的
        // 由 verifier 侧按 fact_id 过滤放行。**绝不**渲染进 prompt（只活在本私有 verifier 输入）。
        let load_npcs = should_load_npc_views(active_npc_ids, visible_text, &secret_terms);
        // P7.3：verifier 私有视图装配（只读 fold）经 KernelPort 适配器派发
        // （EngineKernelAdapter 仅委托 `build_verifier_private_view`）——dispatch 间接，
        // byte-identical（read-only，绝不获取 PlayerPartyEdge side-write，见 ports.rs 不变量①）。
        let private_view = crate::ports::EngineKernelAdapter(&self.engine)
            .verifier_private_view(
                &request.session_id,
                if load_npcs { active_npc_ids } else { &[] },
            )
            .await;
        let player_known_fact_ids = private_view.player_known.known_fact_ids_sorted();
        let plugin_ctx = crate::plugin::PluginContext {
            session_id: request.session_id.clone(),
            turn_id: request.turn_id.clone(),
            ruleset_id: request.ruleset_id.clone(),
            module_id: request.module_id.clone(),
            hook: crate::plugin::PluginHook::AfterLlmStream,
            surfaced_entities,
            narration: Some(visible_text.to_string()),
            compiled_block_ids: vec![],
            player_known_fact_ids,
            private_blocks: vec![],
            secret_terms,
            config: json!({}),
        };
        let contributions = crate::plugin::builtin_plugin_host()
            .run_hook(&plugin_ctx)
            .await;
        let mut traces = Vec::new();
        // VerifierFinding 贡献喂进 ErrataMemory（与既有 NarrationVerifier 路径同口径持久化）。
        let plugin_findings: Vec<trpg_agent::VerifierFinding> = contributions
            .iter()
            .filter_map(|c| match &c.kind {
                crate::plugin::PluginContributionKind::VerifierFinding(vf) => Some(vf.clone()),
                _ => None,
            })
            .collect();
        // TC-D3-05：玩家叙事 projection verifier（确定性、fail-soft）。复用本回合已采集的
        // 同一私有 secret_terms + 玩家已知集（与 NoSpoiler 同源同逻辑），把 secret_terms 折成
        // FactSurfaceMarker，对玩家可见念白跑 player projection 泄漏校验。NoSpoiler 已覆盖的
        // 同一 fact 经 fact-key 去重抹掉，**只保留 NoSpoiler 漏掉的新 fact**——避免重复 errata/
        // trace，生产 trace 计数稳定。绝不回显 secret 正文（finding 只引 fact_id）。
        let projection_new = projection_after_stream_findings(
            visible_text,
            &plugin_ctx.secret_terms,
            &plugin_ctx.player_known_fact_ids,
            &plugin_findings,
        );
        // TC-D3-06：活动 NPC 一致性校验（确定性、fail-soft）。复用本回合已采集的同一私有
        // secret_terms + 玩家已知集——对每个活动 NPC 载其自身 durable profile → NPC 言谈投影
        // （project_for_npc_speech，只读该 NPC 自有 mind/plan，绝不读 GM 世界真相进 NPC 投影），
        // 从同一 secret_terms 标记确定性抽取念白中点名的 candidate fact id，跑 NPC 披露/断言
        // 一致性校验。与既有 SecretLeak（NoSpoiler + projection）按 fact 去重。绝不回显 secret 正文。
        let mut existing_leaks = plugin_findings.clone();
        existing_leaks.extend(projection_new.iter().cloned());
        let npc_new = self.npc_consistency_after_stream(
            visible_text,
            &private_view.npc_speech_views,
            &plugin_ctx.secret_terms,
            &existing_leaks,
        );
        // errata：插件 findings + projection + NPC 一致性去重后的新 findings（同口径持久化）。
        let mut all_findings = plugin_findings;
        all_findings.extend(projection_new.iter().cloned());
        all_findings.extend(npc_new.iter().cloned());
        if !all_findings.is_empty() {
            let entries = self.errata.record(&request.turn_id, &all_findings);
            if !entries.is_empty() {
                let event = self.errata.to_memory_event(request, &entries);
                let _ = self.engine.db.save_memory_event(&event).await;
            }
        }
        for c in &contributions {
            traces.push(c.to_trace());
        }
        // projection 新增 finding 也折 trace（plugin_id 标本投影 verifier，detail 不夹 secret 正文）。
        for f in &projection_new {
            traces.push(projection_verifier_finding_trace(f));
        }
        // NPC 一致性新增 finding 折 trace（plugin_id 标本 NPC 知识一致性 verifier，detail 只引 id）。
        for f in &npc_new {
            traces.push(npc_consistency_finding_trace(f));
        }
        // findings（含 SecretLeak）一并返回，供 PresentationGate 判定（SecretLeak 来源在此 hook）。
        (traces, all_findings)
    }

    /// TC-D3-06：活动 NPC 知识/行为一致性校验的生产半边（纯接线、fail-soft）。**不再**自行载入
    /// NPC profile/投影——调用方已通过 [`trpg_runtime::build_verifier_private_view`] 一次性装配好每个
    /// 活动 NPC 的自有言谈投影（只读该 NPC 自己的边——GM 世界真相绝不进 NPC-owned 投影），此处直接把
    /// 这批投影交两个纯函数 [`npc_consistency_findings`] / [`npc_behavior_consistency_findings`] 出
    /// findings。投影里已携带展示名（`mind.persona.name`），可见性门据此匹配，与旧的 `profile.name`
    /// 路径逐字节等价。无 NPC 投影 / 无 secret_terms / 空念白 → 空（fail-soft，旧行为零变更）。
    fn npc_consistency_after_stream(
        &self,
        visible_text: &str,
        npc_speech_views: &[trpg_runtime::NpcSpeechProjection],
        secret_terms: &[crate::plugin::SecretTerm],
        existing: &[trpg_agent::VerifierFinding],
    ) -> Vec<trpg_agent::VerifierFinding> {
        if npc_speech_views.is_empty() || visible_text.is_empty() || secret_terms.is_empty() {
            return Vec::new();
        }
        // 知识一致性（既有）+ 行为一致性（设计3 §13-P3，新增 advisory）同口径折叠返回。
        // 两检查共用同一批预装载投影（单源 build_verifier_private_view，无额外 DB 读）。
        let mut findings =
            npc_consistency_findings(visible_text, npc_speech_views, secret_terms, existing);
        findings.extend(npc_behavior_consistency_findings(
            visible_text,
            npc_speech_views,
            existing,
        ));
        findings
    }

    /// P3.7：advisory/trace-only 检查点（BeforeCommit / BeforeNarration）。构造最小只读
    /// PluginContext（narration 可选），跑内置 policy host，把**每条**贡献折成 plugin trace
    /// 推进 `ctx.plugin_contributions`。**绝不**把任何 VerifierFinding 折进 blocking gate——
    /// 这两个 hook 是纯 advisory（真正的 mechanics-commit 阻断门控推迟到 P6）。
    /// 内置守卫都不挂这两个 hook，故生产路径恒空贡献 → trace 计数零行为变更。fail-soft。
    async fn run_advisory_trace_hook(
        &self,
        ctx: &mut TurnContext,
        request: &ContextRequest,
        hook: crate::plugin::PluginHook,
        narration: Option<&str>,
    ) {
        let plugin_ctx = crate::plugin::PluginContext {
            session_id: request.session_id.clone(),
            turn_id: request.turn_id.clone(),
            ruleset_id: request.ruleset_id.clone(),
            module_id: request.module_id.clone(),
            hook,
            surfaced_entities: vec![],
            narration: narration.map(str::to_string),
            compiled_block_ids: vec![],
            player_known_fact_ids: vec![],
            private_blocks: vec![],
            secret_terms: vec![],
            config: json!({}),
        };
        let contributions = crate::plugin::builtin_plugin_host()
            .run_hook(&plugin_ctx)
            .await;
        // 只折 trace（含 VerifierFinding 类）——绝不进 gate_findings/errata（advisory-only）。
        for c in &contributions {
            ctx.plugin_contributions.push(c.to_trace());
        }
    }

    /// 采集本会话模组的私有泄漏术语表（生产源，TC-D3-00）。无 module_id（自由场景）或图谱
    /// 缺失/DB 抖动 → 空（fail-soft：泄漏校验退化为不检测，绝不 panic/拦流，旧行为零变更）。
    /// 采全模组声明的 secret_terms（非仅当前场景）：念白提前点名任何未揭示真相都该被抓；
    /// 已揭示的由 verifier 侧按 fact_id 放行。
    async fn harvest_session_secret_terms(
        &self,
        module_id: Option<&str>,
    ) -> Vec<crate::plugin::SecretTerm> {
        let Some(module_id) = module_id.filter(|id| !id.trim().is_empty()) else {
            return vec![];
        };
        match self.engine.db.load_module_graph(module_id).await {
            Ok(Some(graph)) => crate::plugin::harvest_module_secret_terms(&graph),
            Ok(None) => vec![],
            Err(err) => {
                tracing::warn!(error = %err, module_id, "secret_terms 采集失败（module graph 载入），泄漏校验退化不检测");
                vec![]
            }
        }
    }

    /// R5 critical：只持久化回合记录 + status（save_turn）。memory/audit 移到 heavy
    /// （heavy_finalize_memory）。叙事已流出不可回收，save 失败 warn 不 panic（MockLlm
    /// lazy pool 下保持绿）；真实落库由 turns 表 SQL 验证。
    pub(crate) async fn finalize_save_turn(
        &self,
        request: &ContextRequest,
        compiled: &CompiledContext,
        user_input: &str,
        assistant_output: &str,
        status: &str,
    ) {
        let hashes = json!({"prefix": compiled.prefix_hash, "pinned": compiled.pinned_hash, "dynamic": compiled.dynamic_hash});
        if let Err(err) = self
            .engine
            .db
            .save_turn(
                &request.session_id,
                &request.turn_id,
                user_input,
                assistant_output,
                hashes,
                status,
            )
            .await
        {
            tracing::warn!(error = %err, "agent path save_turn failed");
        }
    }

    /// R5 heavy：富版回合记忆 + learning audit（下一回合不强依赖；后台跑）。逐字搬自
    /// 旧 finalize_turn 的 memory/audit 半边。失败只 warn，绝不影响已 save 的 turn（D2）。
    pub(crate) async fn heavy_finalize_memory(
        &self,
        request: &ContextRequest,
        state: &RuntimeState,
        user_input: &str,
        assistant_output: &str,
    ) {
        if assistant_output.trim().is_empty() {
            return;
        }
        let summary: String = assistant_output.chars().take(280).collect();
        // 富版回合记忆（与退役 API turn_postprocess 对齐，spec §4.3）：importance 50、
        // scene/location/actor 取自本回合 RuntimeState、含转录摘录，tags 用
        // ["turn","session_memory"] 并保留 "gm_turn" 便于沿用旧检索。
        let transcript_excerpt = Some(format!(
            "Player: {}\nGM: {}",
            user_input,
            assistant_output.chars().take(2000).collect::<String>()
        ));
        let event = MemoryEvent {
            event_id: format!("mem_turn_{}", Uuid::new_v4().simple()),
            session_id: request.session_id.clone(),
            turn_id: Some(request.turn_id.clone()),
            ruleset_id: request.ruleset_id.clone(),
            module_id: request.module_id.clone(),
            scene_id: state.scene_id.clone(),
            location_id: state.location_id.clone(),
            actor_ids: state.active_npc_ids.clone(),
            visibility: Visibility::GmOnly,
            event_kind: MemoryKind::Event,
            summary,
            transcript_excerpt,
            source: json!({"source":"gm_agent.turn_summary"}),
            tags: vec![
                "turn".to_string(),
                "session_memory".to_string(),
                "gm_turn".to_string(),
            ],
            importance: 50,
            occurred_at: Utc::now(),
        };
        if let Err(err) = self.engine.db.save_memory_event(&event).await {
            tracing::warn!(error = %err, "agent path turn summary memory event failed");
        }
        // spec §4 收尾第三项：learning audit（保留）——沿用旧路径原语
        // RuntimeEngine::audit_learning_for_turn（内部自带 learning_audit_enabled()
        // 门控与自吞错，门关时为 no-op；与旧 run_turn_once L1481 行为对齐）。
        if let Err(err) = self
            .engine
            .audit_learning_for_turn(
                &request.session_id,
                &request.ruleset_id,
                request.module_id.as_deref(),
                &request.turn_id,
                user_input,
                assistant_output,
            )
            .await
        {
            tracing::warn!(error = %err, "agent path learning audit failed");
        }
    }

    /// 确定性收尾（spec §4：save_turn / memory event / learning audit）。R5 拆分后保留
    /// 为薄 wrapper（save→memory 串行），供 run_gm_turn（R1 未退役）复用；新执行器
    /// （execute.rs）走拆分路径（critical save、heavy memory/audit 后台）。
    pub(crate) async fn finalize_turn(
        &self,
        request: &ContextRequest,
        state: &RuntimeState,
        compiled: &CompiledContext,
        user_input: &str,
        assistant_output: &str,
        status: &str,
    ) {
        self.finalize_save_turn(request, compiled, user_input, assistant_output, status)
            .await;
        self.heavy_finalize_memory(request, state, user_input, assistant_output)
            .await;
    }

    // ====================== T4 解释器尾部 phase wrappers ======================
    // execute.rs 解释器经这些 pub(crate) 方法跑后置 phase，全部从 `ctx` 读
    // run_agent_loop 写回的产物（visible_text / ledger / compiled / awaiting_gate）。

    /// PhaseId::VerifyAfterStream — 流后校验（与 run_gm_turn 同语义：visible_text
    /// 非空才跑；空文本无可对账内容）。
    pub(crate) async fn phase_verify_after_stream(
        &mut self,
        ctx: &mut TurnContext,
        input: &GmTurnInput<'_>,
        tx: &tokio::sync::mpsc::Sender<crate::turn_event::TurnEvent>,
        cancel: Option<&CancellationToken>,
        buffered_narration: bool,
    ) {
        if ctx.visible_text.trim().is_empty() && ctx.awaiting_gate.is_some() {
            return;
        }
        let visible = std::mem::take(&mut ctx.visible_text);
        // 流后校验返回 plugin trace 记录（NarrationVerifier findings 折成的 no_mechanical_invention
        // 贡献 + AfterLlmStream hook 贡献）+ PresentationGate 判定。push 进 ctx.plugin_contributions
        // —— 本 phase 在 execute.rs 跑于成功路径 build_turn_trace 之前，故 AfterLlmStream 贡献也进 TurnTrace。
        let outcome = self
            .verify_after_stream(
                input.request,
                &ctx.ledger,
                &visible,
                &input.state.active_npc_ids,
            )
            .await;
        ctx.plugin_contributions.extend(outcome.traces);
        ctx.presentation_gate = outcome.gate;
        ctx.visible_text = visible;

        // —— P2 步骤6：repair ladder（feature-gated + buffered-narration 前置）——
        // 两 env flag 正交：TRPG_PRESENTATION_GATE 默认 OFF ⇒ gate 仅记 trace（上方已记），
        // 零行为变更，**不**进 repair。ON 且 buffered_narration 就位（= TRPG_NARRATOR_SPLIT ON、
        // 文字在校验前经 Narrator 缓冲）时，Block ⇒ 跑修复阶梯；ON 但文字已实时流出（buffer 不在位）
        // ⇒ 退化为仅 trace（回收已流出文字无意义）。绝不回滚已 commit 的 ledger/DB。
        //
        // A3(§6 大考)：白名单之外的 OmittedVisibleResult(空心念白)在 ON+buffered 时**旁路**触发
        // 同一阶梯——split Narrator 无 ReviseText 自修环,靠此补收敛守卫。触发条件加 NARRATOR_SPLIT
        // 显式门(buffered_narration 已蕴含,但显式更清晰),且 OFF(任一 flag 关)恒不进 ⇒ 字节等价。
        let gate_block = ctx.presentation_gate.is_block();
        let omitted_repair = narrator_split_enabled() && outcome.omitted_visible_repair;
        // A3-HARDEN(§6 大考)：机器上下文回显 / 原始 JSON dump 旁路信号。**严格 ON-only**——
        // 谓词仅在 narrator_split_enabled() 时**才计算**(codex 审：避免 OFF 路径"算了但不生效"，
        // 使 OFF 真正不消费该谓词)，且白名单(BLOCKING_KINDS)纹丝不动 ⇒ OFF 字节等价。
        let echo_repair =
            narrator_split_enabled() && narration_is_machine_context_echo(&ctx.visible_text);
        if presentation_gate_enabled()
            && buffered_narration
            && (gate_block || omitted_repair || echo_repair)
        {
            self.run_presentation_repair_ladder(
                ctx,
                input,
                tx,
                cancel,
                omitted_repair,
                echo_repair,
            )
            .await;
        }
    }

    /// P2 Block 修复阶梯（§18，仅 buffered-narration 在位时调用）：
    /// (1) 带收窄 forbidden_reveals 重生 narration 一次（bounded retry=1，re-run Narrator）；
    /// (2) 若仍 Block / LLM 失败 ⇒ 确定性 committed-facts 模板叙事（never blank）。
    /// 全程不回滚已 commit 的机械状态——只重写 ctx.visible_text（assistant_output 单一事实源）。
    async fn run_presentation_repair_ladder(
        &mut self,
        ctx: &mut TurnContext,
        input: &GmTurnInput<'_>,
        tx: &tokio::sync::mpsc::Sender<crate::turn_event::TurnEvent>,
        cancel: Option<&CancellationToken>,
        // A3(§6 大考)：本次阶梯是否为 OmittedVisibleResult 旁路触发。为 true 时,recheck 除
        // gate.is_block() 外还须**已不再漏报可见结果**才采纳重生稿,否则落确定性兜底(收敛保证)。
        include_omitted_repair: bool,
        // A3-HARDEN(§6 大考)：本次阶梯是否为机器上下文回显 / 原始 JSON dump 旁路触发。为 true 时,
        // recheck 还须确认重生稿**不再是机器回显**才采纳,否则落确定性兜底(收敛保证)。
        include_echo_repair: bool,
    ) {
        // 收窄 forbidden_reveals：把触发 Block 的 finding detail 作为禁揭示约束喂回 Narrator。
        let mut forbidden: Vec<String> = match &ctx.presentation_gate {
            crate::presentation_gate::PresentationGate::Block(findings) => {
                findings.iter().map(|f| f.detail.clone()).collect()
            }
            crate::presentation_gate::PresentationGate::Allow => Vec::new(),
        };
        // A3-HARDEN(codex 审折入)：echo 旁路重生时,Block findings 通常为空 ⇒ forbidden 为空 ⇒
        // 模型可能复读同款 JSON。注入一条**通用格式约束**(不含原始 dump 正文,只约束输出形态),
        // 让重生稿写散文而非回吐机器对象。来源为引擎常量 ⇒ 不新增任何泄漏面。
        if include_echo_repair {
            forbidden.push(
                "只用自然语言散文叙述本回合结果，绝不输出工具/机器 JSON、字段名或花括号机器对象，\
                 不得逐字复述「已确认结果」账本块。"
                    .to_string(),
            );
        }
        // (1) 重生一次：从本回合 ledger/ctx 投影 NarrationPacket（带收窄 forbidden_reveals）→ Narrator。
        let adj = crate::packet::AdjudicationPacket::project(
            input.user_input,
            ctx.ledger.snapshot(),
            &ctx.resolved_gate_facts,
            &ctx.visible_text,
            None,
        );
        // A3：重生稿也注入 player-safe scene_context(= 上一回合已交付 narration / 玩家已可见),
        // 让重生有感官 grounding 而非再次空心。来源 player-safe ⇒ 不新增泄漏面。
        let narration = crate::packet::NarrationPacket::project(&adj, "", &forbidden)
            .with_scene_context(&player_safe_scene_context(input));
        let private_tokens = ctx.ledger.private_roll_tokens();
        // P7.3：repair-ladder 的 Narrator 重生同样经 NarratorPort 适配器派发（byte-identical）。
        // `&mut self` 处只读重借 `&*self` 构造适配器，不持有跨 await 的可变借用。
        let regenerated = crate::ports::GmLoopNarratorAdapter(&*self)
            .narrate(&narration, &private_tokens, tx, cancel)
            .await;
        if let Some(text) = regenerated {
            if !text.trim().is_empty() {
                // 重新校验重生文本：若已不再 Block ⇒ 采纳；否则落入确定性模板兜底。
                let recheck = self
                    .verify_after_stream(
                        input.request,
                        &ctx.ledger,
                        &text,
                        &input.state.active_npc_ids,
                    )
                    .await;
                ctx.plugin_contributions.extend(recheck.traces);
                // A3：旁路触发时,recheck 还须确认重生稿不再漏报可见结果(白名单不管 OmittedVisible,
                // 故必须显式查 recheck.omitted_visible_repair),否则不采纳 ⇒ 落确定性兜底(收敛)。
                let still_omitted = include_omitted_repair && recheck.omitted_visible_repair;
                // A3-HARDEN：echo 旁路触发时,重生稿若仍是机器回显则不采纳 ⇒ 落确定性兜底(收敛保证)。
                let still_echo = include_echo_repair && narration_is_machine_context_echo(&text);
                if !recheck.gate.is_block() && !still_omitted && !still_echo {
                    ctx.presentation_gate = recheck.gate;
                    ctx.visible_text = text;
                    return;
                }
                ctx.presentation_gate = recheck.gate;
            }
        }
        // (2) 终端兜底。
        // Q-3-REINFORCE(§6 大考 v3): under gm_craft the hollow "（机械结果）" ledger stub is a
        // BLOCKING defect — a check fired but the turn had ZERO fiction. So instead of the
        // deterministic machine-fact template, do ONE forced re-narration pass that MUST render the
        // confirmed mechanical outcome as real second-person fiction, and ACCEPT its non-empty
        // non-echo output (bounded → still converges: this is the single terminal LLM call, no
        // re-gate loop). Only a total LLM failure falls to a neutral in-fiction continuation
        // sentence — NEVER the machine-fact ledger / "（机械结果）" placeholder.
        // OFF (baseline) keeps the deterministic template byte-for-byte.
        if crate::gm_craft::enabled() {
            let mut forced_forbidden = forbidden.clone();
            forced_forbidden.push(
                "【强制叙事】本回合的机械结果已经确定；你现在必须用第二人称中文散文，把这个结果\
                 作为故事情节呈现出来——写明角色此刻看到/听到/感受到什么、世界如何回应、因果如何\
                 推进。绝不只罗列结果，绝不输出账本块、字段名或花括号机器对象，绝不写「（机械结果）」\
                 之类占位，绝不留空。"
                    .to_string(),
            );
            let forced_packet =
                crate::packet::NarrationPacket::project(&adj, "", &forced_forbidden)
                    .with_scene_context(&player_safe_scene_context(input));
            let forced = crate::ports::GmLoopNarratorAdapter(&*self)
                .narrate(&forced_packet, &private_tokens, tx, cancel)
                .await;
            ctx.visible_text = match forced {
                Some(t) if !t.trim().is_empty() && !narration_is_machine_context_echo(&t) => t,
                // 极端：连强制重述都失败/仍是机器回显 ⇒ 一句中性的「故事继续」散文，绝不落
                // 机械账本模板,绝不出现「（机械结果）」。这是 LLM 彻底失败时的 infra fail-soft。
                _ => "你定了定神，眼前的局势仍在推进，你需要决定下一步怎么做。".to_string(),
            };
            return;
        }
        // (2-baseline) 确定性 committed-facts 模板叙事（§18，never blank）。逐条复述可见证据 token ⇒
        // 必过 OmittedVisibleResult 检查 ⇒ A3 旁路在 bounded retry=1 后**确定性收敛**,不死循环。
        ctx.visible_text = deterministic_committed_facts_narration(&narration);
    }

    /// P6.7 ResolutionCommit（**逻辑/审计边界**，非新 phase、非落库前门）：AgentLoop 结束点。
    /// 机械状态已由循环内工具落库（codex#7）——本边界只是 post-mechanical-resolution 的
    /// 命名审计标记，在此触发**重定位**后的 BeforeCommit advisory hook（trace-only，绝不阻断）。
    /// execute.rs 在 run_agent_loop 返回后调（非取消路径）；run_gm_turn 内联调。
    pub(crate) async fn resolution_commit_boundary(
        &mut self,
        ctx: &mut TurnContext,
        request: &ContextRequest,
    ) {
        // P3.7 BeforeCommit 重定位：从 save_turn 前迁到 AgentLoop 结束这一真正的
        // 机械结算后检查点（advisory/trace-only，本期不阻断）。
        self.run_advisory_trace_hook(ctx, request, crate::plugin::PluginHook::BeforeCommit, None)
            .await;
    }

    /// P6.7 PresentationCommit（**逻辑边界**，非新 phase）：VerifyAfterStream 产出终审结果
    /// （repair ladder 已沉降）后、Finalize/save_turn 前。两件事：
    /// (1) 触发**重定位**后的 BeforeNarration advisory hook —— 现对 split 与非 split 路径都触发
    ///     （真正的「玩家可见前」切点；trace-only，不阻断）。
    /// (2) 玩家认知事实提交：终审 gate 为 Allow ⇒ 按 fact_id 排序逐条 commit 提名；Block ⇒ 全丢弃。
    ///     commit 发生在 repair ladder 之后 ⇒ 先 Block 后修复成 Allow 的回合会提交（codex#6）。
    pub(crate) async fn presentation_commit_boundary(
        &mut self,
        ctx: &mut TurnContext,
        request: &ContextRequest,
    ) {
        // (1) BeforeNarration 重定位：玩家可见前的干净切点，split / 非 split 一致触发。
        let narration = ctx.visible_text.clone();
        self.run_advisory_trace_hook(
            ctx,
            request,
            crate::plugin::PluginHook::BeforeNarration,
            Some(&narration),
        )
        .await;
        // (2) reveal 提名提交（终审 Allow only）。
        let allow = !ctx.presentation_gate.is_block();
        let nominations = std::mem::take(&mut ctx.nominated_reveals);
        self.commit_nominated_reveals(request, nominations, allow)
            .await;
        // (3) P6 revision (§二十四-#13)：drain 本回合玩家拒绝提名 → commit_story_writes 持久化。
        // 这是 commit_story_writes 的真正 per-turn 生产调用方（不再 dead-by-tests）。终审 Allow 时
        // 才提交（Block 回合的整段叙事被拦，拒绝信号一并丢弃，与 reveal 同口径）。
        let rejections = std::mem::take(&mut ctx.rejected_nominations);
        self.commit_story_rejections(request, rejections, allow)
            .await;
    }

    /// P6 revision：把本回合 note_player_rejection 提名 drain 进 `commit_story_writes`
    /// （`TRPG_STORY_WRITE_LOOP` 内部再 gate：OFF ⇒ 该函数即时 Ok 空操作 ⇒ 字节级基线）。
    /// 这是给 `commit_story_writes` 的真实 per-turn 调用方：LLM 经 note_player_rejection PROPOSES，
    /// Kernel 在此 COMMITS，下一回合 P5.3 selector 真把该线索 drop。失败只 warn——剧情持久化
    /// 绝不反向中断已交付的叙事（与 reveal 提交同款 fail-soft）。
    async fn commit_story_rejections(
        &self,
        request: &ContextRequest,
        rejections: Vec<crate::tools::RejectionNomination>,
        allow: bool,
    ) {
        if !allow || rejections.is_empty() {
            return;
        }
        let proposals: Vec<trpg_model::PlayerInterestSignal> = rejections
            .iter()
            .map(|r| trpg_runtime::rejection_proposal(&r.thread_id))
            .collect();
        if let Err(err) = trpg_runtime::commit_story_writes(
            &self.engine.db,
            &request.session_id,
            &proposals,
            &[],
            &request.turn_id,
        )
        .await
        {
            tracing::warn!(
                error = %err,
                "PresentationCommit story rejection persist failed (non-fatal)"
            );
        }
    }

    /// MAT.M3 axis-2 线索发现能供（DP-3）：本回合每条**成功的侦查类检定**若目标解析到
    /// **当前场景引用**的某条线索（`graph.clues`），就提名揭示该线索的**单条 source-backed
    /// FACT**（fact_id = 线索 id，**非**正文逐字），推进既有 `RevealNomination` 通道，由
    /// `presentation_commit_boundary` 终审 Allow 后 commit。
    ///
    /// proposal-only / 跨 crate 干净：检定→线索的纯映射在 `trpg_runtime::clue_reveal_candidates`
    /// （DB-free），本方法只读 ledger + 加载 module graph（只读），把候选映射成 `RevealNomination`
    /// 推进 `ctx.nominated_reveals`——**不**调 `reveal_fact` / 不写 DB（commit 归边界）。
    ///
    /// 双闸 fail-closed：`gating_on` 为前置形参（reveal-gating OFF ⇒ 直接早返，
    /// ctx.nominated_reveals 恒空 = F13 基线）；模式闸在纯函数内（Off/Shadow ⇒ 空候选）。
    /// 幂等：同一线索 fact 跨多条命中检定 / 与既有提名去重（commit primitive 本身幂等，
    /// 提名去重让排序/计数确定；ContextSurfaced 由 runtime 幂等记，本路径不重复记）。
    async fn apply_clue_affordance(
        &self,
        state_agent: &trpg_model::RuntimeState,
        input: &GmTurnInput<'_>,
        snap: &trpg_agent::TurnLedgerSnapshot,
        nominated_reveals: &mut Vec<crate::tools::RevealNomination>,
        gating_on: bool,
    ) {
        let mode = trpg_model::MaterializationAffordanceMode::from_env();
        // 模式闸 + gating 闸：任一关 ⇒ 严格无操作（与纯函数双闸一致，省去无谓 graph 加载）。
        if !mode.is_enforce() || !gating_on {
            return;
        }
        let Some(scene_id) = state_agent.scene_id.clone() else {
            return;
        };
        let Some(module_id) = input
            .request
            .module_id
            .clone()
            .or_else(|| state_agent.module_id.clone())
        else {
            return;
        };
        let graph = match self.engine.db.load_module_graph(&module_id).await {
            Ok(Some(g)) => g,
            _ => return, // 无图谱 → fail-closed（线索能供不揭）。
        };
        let Some(scene) = graph.scenes.iter().find(|s| s.node_id == scene_id) else {
            return;
        };
        // 结果 join 契约（by check_id）→ 逐条跑纯映射，收集候选（DP-3 单 fact / 检定）。
        // 先收集再推送：snapshot() 是 &ctx 不可变借用，与后续 ctx.nominated_reveals 可变推送
        // 不能交叠，故分两段（借用规则）。
        let mut candidates: Vec<crate::tools::RevealNomination> = Vec::new();
        for result in &snap.check_results {
            let success = result
                .outcome
                .get("success")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            if !success {
                continue;
            }
            let Some(contract) = snap
                .check_contracts
                .iter()
                .find(|c| c.check_id == result.check_id)
            else {
                continue;
            };
            let resolved = trpg_runtime::ResolvedCheck {
                success,
                tested_parameter: contract.tested_parameter.as_ref().map(|p| p.label.as_str()),
                check_label: contract.check_label.as_str(),
                action_summary: contract.action_summary.as_str(),
            };
            for cand in
                trpg_runtime::clue_reveal_candidates(&resolved, scene, &graph, mode, gating_on)
            {
                candidates.push(crate::tools::RevealNomination {
                    fact_id: cand.fact_id,
                    reason: Some(cand.reason),
                });
            }
        }
        // 与既有提名 + 候选间去重（同 fact_id 只提名一次；保留首次 reason）。
        for cand in candidates {
            if !nominated_reveals.iter().any(|n| n.fact_id == cand.fact_id) {
                nominated_reveals.push(cand);
            }
        }
    }

    /// P6.7 commit primitive 包装：终审 Allow 时按 fact_id 排序逐条 engine.reveal_fact
    /// （= 既有即时写，现成提交步骤；replay parity 靠排序）；Block ⇒ 全部丢弃、零落库。
    /// engine.reveal_fact / db.record_revealed_fact 内部不变（幂等）。失败只 warn。
    async fn commit_nominated_reveals(
        &self,
        request: &ContextRequest,
        mut nominations: Vec<crate::tools::RevealNomination>,
        allow: bool,
    ) {
        if !allow || nominations.is_empty() {
            return;
        }
        nominations.sort_by(|a, b| a.fact_id.cmp(&b.fact_id));
        // P7.3：PresentationCommit reveal 提交（KernelPort 的唯一真实 commit primitive）经
        // KernelPort 适配器派发（EngineKernelAdapter::commit_reveal_fact 仅委托 `engine.reveal_fact`，
        // 幂等不变）——dispatch 间接，byte-identical（Block ⇒ 上方早返，零落库不变）。
        let kernel = crate::ports::EngineKernelAdapter(&self.engine);
        for n in &nominations {
            if let Err(err) = kernel
                .commit_reveal_fact(
                    &request.session_id,
                    &request.turn_id,
                    &n.fact_id,
                    n.reason.as_deref(),
                )
                .await
            {
                tracing::warn!(
                    error = %err,
                    fact_id = %n.fact_id,
                    "PresentationCommit reveal commit failed"
                );
            }
        }
    }

    /// PhaseId::Finalize（R5 critical）— 只持久化回合记录 + status（save_turn）。
    /// memory/audit 由 phase_finalize_heavy_memory 在 heavy 段后台跑。awaiting 终态用
    /// gate prompt_public 兜底空 visible_text（与 run_gm_turn assistant_output 选择一致）。
    pub(crate) async fn phase_finalize(
        &mut self,
        ctx: &mut TurnContext,
        input: &GmTurnInput<'_>,
        status: &str,
    ) {
        let assistant_output = ctx.heavy_assistant_output();
        // P3.7 BeforeCommit 已重定位到 resolution_commit_boundary（AgentLoop 结束点）——
        // 此处不再触发，save_turn 不再附挂 advisory hook。
        self.finalize_save_turn(
            input.request,
            &ctx.compiled,
            input.user_input,
            &assistant_output,
            status,
        )
        .await;
    }

    /// R5 heavy：phase_finalize 的 memory/audit 半边（execute.rs heavy 段调）。
    /// `assistant_output` 必须由调用方在 `take_outcome` 清空 ctx **之前**按 critical 同口径
    /// 快照传入（awaiting+空 visible_text→gate.prompt_public，否则 visible_text）——heavy
    /// 段先于自身被 spawn 时 ctx 已被 take_outcome 清空，绝不能再从 ctx 现读（否则记忆/审计
    /// 拿到空串、heavy_finalize_memory 早返、富版回合记忆 + learning audit 每回合静默丢失）。
    /// 从 input 读 state 投 RuntimeState 给富版回合记忆。失败只 warn，绝不影响已 save 的 turn。
    pub(crate) async fn phase_finalize_heavy_memory(
        &self,
        assistant_output: &str,
        input: &GmTurnInput<'_>,
    ) {
        self.heavy_finalize_memory(
            input.request,
            input.state,
            input.user_input,
            assistant_output,
        )
        .await;
    }

    /// PhaseId::SceneNavigate（R5 critical）— 切场景决策 + set_session_scene +
    /// SceneChanged world event。返回切换详情供 execute.rs 发 SceneTransition 事件
    /// （R1 旧实现恒 None，本期修复）。**不**深抽/前探（那是 heavy）。
    /// fail-closed：无 module / scene_navigate_critical Err / 未切换 → None（仅 warn）。
    pub(crate) async fn phase_scene_navigate_critical(
        &mut self,
        ctx: &TurnContext,
        input: &GmTurnInput<'_>,
    ) -> Option<SceneTransitionInfo> {
        let module_id = input.request.module_id.as_deref()?;
        match trpg_runtime::scene_navigation::scene_navigate_critical(
            &self.engine.db,
            self.llm.as_ref(),
            &input.request.session_id,
            module_id,
            self.data_dir.as_path(),
            input.user_input,
            &ctx.visible_text,
        )
        .await
        {
            Ok(Some(c)) => Some(SceneTransitionInfo {
                from: c.from,
                to: c.to,
                reason: c.reason,
            }),
            Ok(None) => None,
            Err(err) => {
                tracing::warn!(error = %err, "agent path scene_navigate_critical failed");
                None
            }
        }
    }

    /// PhaseId::SceneNavigate（R5 heavy）— 到场深抽 + frontier 前探（后台，best-effort）。
    /// target 来自 critical 的 SceneTransitionInfo.to；module_id 由调用方校验非空后传入。
    pub(crate) async fn phase_scene_navigate_heavy(&self, target: &str, module_id: &str) {
        trpg_runtime::scene_navigation::scene_navigate_heavy(
            &self.engine.db,
            self.llm.as_ref(),
            module_id,
            target,
            self.data_dir.as_path(),
        )
        .await;
    }

    /// PhaseId::CarryoverDebt — 轮耗尽带债的债务跨回合落账（吸收原 run_gm_turn
    /// `!narrated` 分支的 carryover_block → carryover_memory_event → save_memory_event；
    /// fail-closed：落库失败仅 warn，BP3 注入由下回合 carryover_block 完成）。
    pub(crate) async fn phase_carryover_debt(
        &mut self,
        _ctx: &mut TurnContext,
        input: &GmTurnInput<'_>,
    ) {
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
            Some(gate) => TurnOutcome::AwaitingPlayerRoll {
                check_id: gate.check_id,
                prompt_public: gate.prompt_public,
            },
            None => TurnOutcome::Narration(std::mem::take(&mut ctx.visible_text)),
        }
    }
}

/// P1-3 follow-up：取消等待 future。有令牌 → 等其 `cancelled()`；无令牌 → 永久 pending
/// （让 `select!` 永远选中流事件分支）⇒ 非取消路径与裸 `stream.next().await` 逐字节等价。
async fn wait_for_cancel(cancel: Option<&CancellationToken>) {
    match cancel {
        Some(tok) => tok.cancelled().await,
        None => std::future::pending::<()>().await,
    }
}

/// P1-3 follow-up：令牌是否已 fire（top-of-round / 逼散文前的同步检查）。无令牌恒 false。
fn is_cancelled(cancel: Option<&CancellationToken>) -> bool {
    cancel.is_some_and(|c| c.is_cancelled())
}

/// P1-3 follow-up：取下一个 LLM 流事件，但取消令牌 fire 时**放弃在途生成**。返回 `None`
/// 既表示流自然结束、也表示被取消——用 `*cancelled` 区分（被取消 ⇒ caller `break 'rounds`
/// 不再续轮/逼散文）。`biased` 让每次 poll 先查取消（及时止血、测试确定）；`cancel=None` ⇒
/// 取消分支永久 pending ⇒ 退化为裸 `stream.next().await`，非取消路径逐字节零行为变更。
async fn next_stream_event<S>(
    stream: &mut S,
    cancel: Option<&CancellationToken>,
    cancelled: &mut bool,
) -> Option<anyhow::Result<StreamEvent>>
where
    S: futures_core::Stream<Item = anyhow::Result<StreamEvent>> + Unpin,
{
    tokio::select! {
        biased;
        _ = wait_for_cancel(cancel) => { *cancelled = true; None }
        ev = stream.next() => ev,
    }
}

/// run_agent_loop 私骰尾窗排空：经 mpsc 发 Delta（与 drain_redactor 同语义，
/// 但走 tx.send 而非 on_delta；被门轮 blocked 丢弃）。
/// 构造无工具 Narrator 的最小请求消息（system 风格契约 + user 机械事实）。
/// 只含玩家可感知机械事实摘要 + player_input，无规则原文 / 无 schema / 无 GM 内部推理。
/// A2(§6 大考)：从 GmTurnInput 抽 **player-safe** 场景上下文给 split Narrator 补 grounding。
/// 唯一来源 = 上一回合**已对玩家交付**的 narration（recent_transcript 优先，否则 history 里
/// 最后一条 assistant 消息）——按定义已脱敏、已对玩家可见,故 fail-closed 零新增泄漏面。
/// 截断到稳定上界(避免把整段历史灌爆 prompt);为空 ⇒ 返回空 vec(graceful)。
fn player_safe_scene_context(input: &GmTurnInput<'_>) -> Vec<String> {
    let prior = input.recent_transcript.or_else(|| {
        input
            .history
            .iter()
            .rev()
            .find(|m| m.role == "assistant")
            .map(|m| m.content.as_str())
    });
    match prior {
        Some(text) if !text.trim().is_empty() => {
            // 取尾部 600 字(最近场景),按字符边界安全截断。
            let trimmed = text.trim();
            let snippet: String = if trimmed.chars().count() > 600 {
                trimmed
                    .chars()
                    .rev()
                    .take(600)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect()
            } else {
                trimmed.to_string()
            };
            vec![snippet]
        }
        _ => Vec::new(),
    }
}

/// G1（§6 大考）：env-gated「场景感兜底」。`sensory_floor==true` 且本回合真饿
/// （facts 与 player_perceivable_facts 皆空、scene 非空）时，向 user 消息**追加**一段
/// 兜底子句，许可 Narrator 仅把**已知当前场景文本**重新组织成近景描写（绝不新增事实）。
/// `false` 时与基线装配**字节等价**。唯一生产读 env 处 = run_narrator（不在此处读 env）。
fn build_narrator_messages(
    packet: &crate::packet::NarrationPacket,
    sensory_floor: bool,
    gm_craft: bool,
) -> Vec<serde_json::Value> {
    let mut facts = String::new();
    for f in &packet.what_happened {
        facts.push_str("- ");
        facts.push_str(f);
        facts.push('\n');
    }
    for c in &packet.what_changed {
        facts.push_str("- ");
        facts.push_str(c);
        facts.push('\n');
    }
    let perceivable = packet.player_perceivable_facts.join("；");
    let forbidden = packet.forbidden_reveals.join("；");
    let scene = packet.scene_context.join("\n");
    // G1：在 `scene` 被 user format 移动前先算兜底触发条件（三者皆满足才算真饿）。
    let floor_triggered = sensory_floor
        && facts.is_empty()
        && packet.player_perceivable_facts.is_empty()
        && !scene.trim().is_empty();
    let system = format!(
        "你是 TRPG 叙事者(Narrator)。把已发生的机械事实写成玩家可见的连贯散文。\n\
         风格：{}\n\
         人称：始终以**第二人称「你」**称呼玩家角色(绝不用第三人称「他/她/角色」叙述玩家)。\n\
         感官场景：落地到具体可感知的场景(光线/声音/气味/触感/空间)，**不要**只机械朗读；\n\
         绝不逐字复述或照搬玩家输入原句——把它转写成发生在场景里的画面。\n\
         机械忠实：**逐条**复述下方“本回合机械事实”里的每一条结果(检定成败/伤害/资源/状态)，\n\
         一条都不许略过(玩家必须感知到每个已落账的可见结果)。\n\
         铁律：只叙述下方机械事实与玩家可感知信息；绝不发明未列出的检定/伤害/资源/状态；\n\
         不输出规则原文、工具 JSON、GM 内部推理；不揭示下方“禁止揭示”项。\n\
         禁止揭示：{}",
        packet.style_profile,
        if forbidden.is_empty() {
            "（无）"
        } else {
            forbidden.as_str()
        }
    );
    let user = format!(
        "玩家输入：{}\n\n当前场景(玩家已感知，供延续与感官 grounding，勿复述其原句)：\n{}\n\n\
         本回合机械事实(逐条复述，勿遗漏)：\n{}玩家可感知：{}\n\n\
         请据此用第二人称写一段有场景感、逐条覆盖上述机械结果的连贯散文。",
        packet.player_input,
        if scene.trim().is_empty() {
            "（无前序场景，自行据机械事实落地一个可感知场景）".to_string()
        } else {
            scene
        },
        if facts.is_empty() {
            "（无机械变化）\n".to_string()
        } else {
            facts
        },
        if perceivable.is_empty() {
            "（无）"
        } else {
            perceivable.as_str()
        },
    );
    // G1：仅在本回合真饿（无新增机械事实且无玩家可感知信息）且当前场景非空、
    // 且 sensory_floor 开启时，追加「场景感兜底」尾段。OFF 时 user 字节等价基线。
    let user = if floor_triggered {
        format!(
            "{user}\n\n\
             场景感兜底（仅在本回合没有新增可公开结果时适用）：\n\
             你可以把\"当前场景\"中已经写明、且玩家已经感知过的空间要素重新组织成一两句近景描写。\n\
             硬限制：只可使用\"当前场景\"文本中明示出现的要素；不得补充文本未明示的实体、线索、机关、建筑/房间/道具、NPC意图、规则结论、检定成败、伤害/资源/状态变化；不得把玩家输入里的名词当成已存在场景物；不得断言\"没有线索/没有异常/没有发现\"，除非该否定事实已列在玩家可感知信息或机械事实中。\n\
             若当前场景文本不足以支持重描，只写\"你只能确认眼前这些已见过的环境，暂无新的可公开结果。\"并保持第二人称。"
        )
    } else {
        user
    };
    // Q-1/Q-4 (§2a.1/§2b.1): append GM-craft overlay when TRPG_GM_CRAFT ON; OFF ⇒ byte-equal.
    let system = crate::gm_craft::narrator_system(system, gm_craft);
    vec![
        serde_json::json!({"role": "system", "content": system}),
        serde_json::json!({"role": "user", "content": user}),
    ]
}

async fn drain_redactor_tx(
    redactor: &mut RedactingBuffer,
    blocked: bool,
    tx: &tokio::sync::mpsc::Sender<crate::turn_event::TurnEvent>,
    visible_text: &mut String,
    stream_prose: bool,
) {
    let rest = redactor.finish();
    if blocked || rest.is_empty() {
        return;
    }
    // buffer 模式：只累积进 visible_text,不直发(同 run_agent_loop 三处直发点)。
    if stream_prose {
        let _ = tx
            .send(crate::turn_event::TurnEvent::Delta(rest.clone()))
            .await;
    }
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
fn drain_redactor(
    redactor: &mut RedactingBuffer,
    blocked: bool,
    on_delta: &mut (dyn FnMut(&str) + Send),
    visible_text: &mut String,
) {
    let rest = redactor.finish();
    if blocked || rest.is_empty() {
        return;
    }
    on_delta(&rest);
    visible_text.push_str(&rest);
}

/// 把一条既算出的 NarrationVerifier finding 折成 plugin trace 记录，使 de-facto 的
/// `core.no_mechanical_invention` verifier 在 `trpg explain --plugins` 可见。**纯映射，不重跑
/// 检查、不改 errata 行为**——summary = finding kind（snake_case）+ 截断 detail。
/// P2 步骤6 env flag：`TRPG_PRESENTATION_GATE`（默认 OFF）。OFF ⇒ gate 仅记 trace，零行为
/// 变更（与 TRPG_NARRATOR_SPLIT 正交）。ON ⇒ Block 触发 repair ladder，但仅在 buffered-narration
/// 在位时真正生效（调用方再加 buffer 前置守卫）。
pub(crate) fn presentation_gate_enabled() -> bool {
    std::env::var("TRPG_PRESENTATION_GATE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// A3(§6 大考)：TRPG_NARRATOR_SPLIT 是否 ON(默认 OFF = 字节级基线)。split Narrator 无统一
/// agent loop 的 ReviseText 自修环，故 ON 路径需本修复谓词补一道收敛守卫。execute.rs 已就地
/// 读此 flag 决定 buffer 模式;此 helper 供 phase_verify_after_stream 的 A3 谓词复用同一语义。
pub(crate) fn narrator_split_enabled() -> bool {
    std::env::var("TRPG_NARRATOR_SPLIT")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// A3(§6 大考)：本回合 verify 结果是否含 **Blocker 级 OmittedVisibleResult**(念白漏报了玩家
/// 可见的机械结果 ⇒ 空心念白)。这是 split Narrator 特有的不收敛缺口的触发谓词。
///
/// **绝不触动** charter 锁定白名单(presentation_gate::BLOCKING_KINDS)——OmittedVisibleResult
/// 对 gate 仍判 Allow;本谓词是**旁路**修复信号,只在 ON(PRESENTATION_GATE+NARRATOR_SPLIT)
/// 且 buffered 时把这条 finding 喂回既有 repair ladder(bounded retry + 确定性兜底 ⇒ 收敛)。
pub(crate) fn omitted_visible_result_needs_repair(
    result: &trpg_agent::NarrationVerifierResult,
) -> bool {
    result.findings.iter().any(|f| {
        f.kind == trpg_agent::VerifierFindingKind::OmittedVisibleResult
            && f.severity == trpg_agent::VerifierSeverity::Blocker
    })
}

/// A3-HARDEN(§6 大考)：玩家可见念白是否为**机器上下文回显 / 原始 JSON dump**——即模型把注入的
/// 「本回合已确认结果」机器账本块(含原始 `check_<id>` 记录 JSON)逐字回吐为念白，而非据此生成散文。
/// 这是 split Narrator ON 路径上 ~1/13 turn 的 A3 严重缺口(玩家看到原始机器 JSON 而非念白)。
///
/// **保守谓词，绝不误杀正常念白**(codex 设计审折入)。判据 = **机器对象字段键** 与 **花括号机器对象**
/// 共现且占主导，而**非**单看 header 或单看 `{`：
/// - 确定性兜底模板(`deterministic_committed_facts_narration`)也以「根据本回合已确认的结果」开头，
///   且 `compact_value` 可能把 object outcome 渲染成 `{...}`——故 **header / 裸 `{` 都不能单独触发**，
///   否则会误杀兜底导致不收敛。兜底**绝不含**原始 `"check_id"`/`"roll_id"`/`"effect_id"` 记录字段键。
/// - 正常含 roll 值的散文(`[roll]侦查成功（44/60）[/roll]`)无 `{}`、无机器字段键 ⇒ 永不触发。
///   故意**不**把 `[`/`]` 计入结构密度——`[roll]` 标签会贡献方括号(codex 指出)。
///
/// 触发条件(全为机器 dump 的强信号，任一组合命中即 true)：
/// (A) 含 ≥1 个原始机器记录字段键 (`"check_id"`/`"roll_id"`/`"effect_id"`/`"impact_id"`/`"outcome"`)
///     **且** 花括号对 `{`+`}` ≥ `MIN_BRACES`(=4，≥2 个完整机器对象)；
/// (B) 注入块 header「根据本回合已确认的结果」与 ≥1 个原始机器记录字段键(非裸 `{`)共现——
///     即"回显了带原始字段键的注入账本"。
const MACHINE_ECHO_FIELD_KEYS: [&str; 5] = [
    "\"check_id\"",
    "\"roll_id\"",
    "\"effect_id\"",
    "\"impact_id\"",
    "\"outcome\"",
];
const MACHINE_ECHO_HEADER: &str = "根据本回合已确认的结果";
const MACHINE_ECHO_MIN_BRACES: usize = 4;

pub(crate) fn narration_is_machine_context_echo(visible_text: &str) -> bool {
    let s = visible_text;
    if s.len() < 64 {
        // 太短不可能是注入账本回显；保守不触发(原始 dump ~2057 chars)。
        return false;
    }
    let field_key_hits = MACHINE_ECHO_FIELD_KEYS
        .iter()
        .filter(|k| s.contains(**k))
        .count();
    if field_key_hits == 0 {
        // 无任何原始机器字段键 ⇒ 既非 dump，也不会误杀兜底/散文。
        return false;
    }
    let braces = s.matches('{').count() + s.matches('}').count();
    // (B) header + 原始字段键共现：注入账本被原样回吐。
    if s.contains(MACHINE_ECHO_HEADER) {
        return true;
    }
    // (A) 多个完整机器对象 + 原始字段键：原始 JSON dump 形状(无 header 也算)。
    braces >= MACHINE_ECHO_MIN_BRACES
}

/// P6.7：reveal_fact 提名门控开关（默认 OFF = 字节级基线）。ON ⇒ reveal_fact 不再
/// 即时落库，而是提名到 ctx.nominated_reveals，由 PresentationCommit 边界在终审
/// Allow 后统一提交；OFF ⇒ reveal_fact 即时落库（F13 与今日行为逐字节等价）。
pub(crate) fn reveal_gating_enabled() -> bool {
    std::env::var("TRPG_REVEAL_GATING")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// P2 §18 确定性 committed-facts 模板叙事（Block 修复阶梯末端，never blank）。纯函数：
/// 只用 NarrationPacket 已收窄的玩家可见机械事实摘要 + player_input 拼模板，绝不含 secret /
/// adjudicator_prose / 规则原文。无任何机械事实时回一句中性兜底（绝不空白）。
pub(crate) fn deterministic_committed_facts_narration(
    packet: &crate::packet::NarrationPacket,
) -> String {
    let mut lines: Vec<String> = Vec::new();
    for happened in &packet.what_happened {
        lines.push(format!("· {}", strip_machine_json_objects(happened)));
    }
    for changed in &packet.what_changed {
        lines.push(format!("· {}", strip_machine_json_objects(changed)));
    }
    for fact in &packet.player_perceivable_facts {
        lines.push(format!("· {}", strip_machine_json_objects(fact)));
    }
    if lines.is_empty() {
        "（本回合按已落账的机械结果继续；无新增可公开的机械事实。）".to_string()
    } else {
        format!("根据本回合已确认的结果：\n{}", lines.join("\n"))
    }
}

/// A3-HARDEN(§6 大考, codex ④ 审折入)：把一条机械事实摘要里的**原始 JSON 对象正文**(`{...}`)
/// 抹成一个人类可读的占位标记，**保留**前缀 ledger token(如 `check c_fire:` / `effect e_dmg`)。
///
/// 必要性：`compact_value(&result.outcome)` 会把真实 check outcome(含 `"check_id"` 等原始记录字段键)
/// 原样塞进 `what_happened`(packet.rs)。若兜底直接拼这段,**兜底自身**就满足 machine-echo 谓词的
/// header + 字段键条件 ⇒ 修复阶梯的收敛终点反被守卫判为机器回显(自相矛盾)。抹掉 JSON 正文后,
/// 兜底是纯人话 + ledger token,**确定**不含 `{}`/原始字段键 ⇒ 谓词必放过 ⇒ bounded retry=1 后
/// 真正收敛,且兜底 saved 文本绝不含原始机器 JSON(正是 A3 的本意)。
///
/// **JSON-string-aware 平衡括号扫描**(codex ④ P1 折入)：括号深度计数**跳过**字符串字面量内部的
/// `{`/`}`(并处理 `\"` 转义),否则 `{"note":"}"}` 里字符串中的 `}` 会提前把 depth 降到 0、泄漏后续
/// object 正文。逐字节确定性、不依赖完整 JSON parser(容忍非 JSON 残文,只做对象正文剔除)。
fn strip_machine_json_objects(s: &str) -> String {
    if !s.contains('{') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut depth: usize = 0;
    let mut replaced_here = false;
    let mut in_string = false; // 是否处于 JSON 字符串字面量内部(仅 depth>0 时有意义)
    let mut escaped = false; // 上一字符是否为字符串内的 `\`
    for ch in s.chars() {
        if depth > 0 && in_string {
            // 字符串内部：吞掉所有字符(包含其中的 `{`/`}`)，只跟踪转义与闭引号。
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '{' => {
                if depth == 0 && !replaced_here {
                    out.push_str("（机械结果）");
                    replaced_here = true;
                }
                depth += 1;
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    replaced_here = false;
                }
            }
            '"' if depth > 0 => {
                in_string = true;
            }
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out
}

/// P2：verify_after_stream 的产物——既有 plugin trace 记录 + 本回合 PresentationGate 判定。
pub(crate) struct VerifyAfterStreamOutcome {
    pub traces: Vec<trpg_model::PluginContributionTrace>,
    pub gate: crate::presentation_gate::PresentationGate,
    /// A3(§6 大考)：本回合是否检出 Blocker 级 OmittedVisibleResult(空心念白)。advisory，
    /// **不**经 gate 白名单;调用方在 ON+buffered 时据此旁路触发 repair ladder(收敛守卫)。
    pub omitted_visible_repair: bool,
}

/// P2 步骤8：把 PresentationGate 决策折成 advisory plugin trace（`trpg explain --plugins` 可见
/// "Policy 本回合是否阻断、阻断了哪些 finding"）。零行为变更，仅多记一条 trace。
pub(crate) fn presentation_gate_trace(
    gate: &crate::presentation_gate::PresentationGate,
) -> trpg_model::PluginContributionTrace {
    let summary = match gate {
        crate::presentation_gate::PresentationGate::Allow => "gate=Allow".to_string(),
        crate::presentation_gate::PresentationGate::Block(findings) => {
            let mut kinds: Vec<String> = findings
                .iter()
                .map(|f| {
                    serde_json::to_value(f.kind)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string))
                        .unwrap_or_else(|| format!("{:?}", f.kind))
                })
                .collect();
            kinds.sort();
            kinds.dedup();
            format!("gate=Block kinds=[{}]", kinds.join(","))
        }
    };
    trpg_model::PluginContributionTrace {
        plugin_id: "core.presentation_gate".to_string(),
        hook: crate::plugin::PluginHook::AfterLlmStream
            .as_str()
            .to_string(),
        kind: "presentation_gate".to_string(),
        summary,
    }
}

pub(crate) fn surface_verifier_finding_trace(
    f: &trpg_agent::VerifierFinding,
) -> trpg_model::PluginContributionTrace {
    let kind_str = serde_json::to_value(f.kind)
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| format!("{:?}", f.kind));
    let detail: String = f.detail.chars().take(120).collect();
    let summary = if detail.is_empty() {
        kind_str.clone()
    } else {
        format!("{kind_str}: {detail}")
    };
    trpg_model::PluginContributionTrace {
        plugin_id: "core.no_mechanical_invention".to_string(),
        hook: crate::plugin::PluginHook::AfterLlmStream
            .as_str()
            .to_string(),
        kind: "verifier_finding".to_string(),
        summary,
    }
}

/// MAT.M7 (D1) PURE：本回合消费者可见的 active NPC 集——优先派生集（非空时），否则退回
/// 调用方传入集。确定性、无 IO，便于单测 byte-equal-baseline 不变量。
///
/// - `derived` 非空 ⇒ 取派生集（Enforce 下 = scene.referenced_npc_ids）。
/// - `derived` 空 ⇒ 退回 `caller_supplied`：覆盖 ① ctx_provider 测试 seam（不填派生集）；
///   ② Off/Shadow（apply_npc_activation 无操作 ⇒ 派生集恒等于 caller_supplied，两路同值）。
/// 空-退-空保 s17 不变量（无供给即空）。
fn effective_active_npc_ids<'a>(
    derived: &'a [String],
    caller_supplied: &'a [String],
) -> &'a [String] {
    if derived.is_empty() {
        caller_supplied
    } else {
        derived
    }
}

/// P3.6 load-equivalence 门控：流后是否需要载入活动 NPC 言谈投影。
///
/// **与旧 [`TurnLoop::npc_consistency_after_stream`] 的提前返回条件互为否定**：旧路径在
/// `npc_speech_views.is_empty() || visible_text.is_empty() || secret_terms.is_empty()` 时直接
/// 返回空、绝不载入任何 NPC profile/mind（`npc_speech_views.is_empty()` 当且仅当 baseline 的
/// `active_npc_ids.is_empty()`——空 npc_ids → 空投影）。故唯有三者**全非空**才值得载入；任何一项
/// 为空 → 零 NPC DB 读，与 baseline 逐次相等。这一门只省「确定产不出 finding」的 NPC 载入，
/// findings/trace 不受影响（被门掉的场景旧路径本就返回空）。
fn should_load_npc_views(
    active_npc_ids: &[String],
    visible_text: &str,
    secret_terms: &[crate::plugin::SecretTerm],
) -> bool {
    !active_npc_ids.is_empty() && !visible_text.is_empty() && !secret_terms.is_empty()
}

/// TC-D3-05：AfterLlmStream 玩家叙事 projection verifier（确定性、纯、fail-soft）。
///
/// 把本会话私有 `secret_terms` 折成 [`FactSurfaceMarker`]（每词一 marker，绑其 fact_id；
/// 无 fact_id → 空 = unbound，模型侧 fail-closed），用玩家已知集建 [`PlayerNarrationProjection`]，
/// 对玩家可见念白跑 runtime projection 泄漏校验。**去重**：把已由 `existing`（NoSpoiler 插件
/// findings）覆盖的同一 fact-key 抹掉，只返回 NoSpoiler **漏掉**的新 SecretLeak findings——
/// 二者同源同逻辑时返回空，生产 trace/errata 计数不变。绝不回显 secret 正文。
pub(crate) fn projection_after_stream_findings(
    narration: &str,
    secret_terms: &[crate::plugin::SecretTerm],
    player_known_fact_ids: &[String],
    existing: &[trpg_agent::VerifierFinding],
) -> Vec<trpg_agent::VerifierFinding> {
    use trpg_model::knowledge_leak_verifier::FactSurfaceMarker;
    if narration.is_empty() || secret_terms.is_empty() {
        return Vec::new();
    }
    let markers: Vec<FactSurfaceMarker> = secret_terms
        .iter()
        .map(|t| {
            FactSurfaceMarker::new(t.fact_id.clone().unwrap_or_default(), vec![t.term.clone()])
        })
        .collect();
    let projection =
        trpg_runtime::PlayerNarrationProjection::from_player_known(player_known_fact_ids.to_vec());
    let findings = trpg_runtime::verify_player_narration_leak(narration, &markers, &projection);
    // 去重：跳过 existing（NoSpoiler）已覆盖的 fact-key，并在 projection 自身内去重同 fact。
    let mut seen: std::collections::HashSet<String> = existing
        .iter()
        .filter_map(|f| secret_leak_fact_key(&f.detail).map(str::to_string))
        .collect();
    findings
        .into_iter()
        .filter(|f| match secret_leak_fact_key(&f.detail) {
            Some(k) => seen.insert(k.to_string()),
            None => true,
        })
        .collect()
}

/// 从 SecretLeak finding 的 detail 抽稳定 fact-key（首对单引号内 token）。NoSpoiler 插件与
/// projection verifier 的 detail 都把 fact_id 包在单引号里，故可据此跨来源去重同一 fact 的泄漏。
fn secret_leak_fact_key(detail: &str) -> Option<&str> {
    let start = detail.find('\'')?;
    let rest = &detail[start + 1..];
    let end = rest.find('\'')?;
    Some(&rest[..end])
}

/// 把一条 projection verifier finding 折成 plugin trace（plugin_id 标本投影 verifier）。
/// summary = kind + 截断 detail（detail 只含 fact_id，绝不夹 secret 正文）。
fn projection_verifier_finding_trace(
    f: &trpg_agent::VerifierFinding,
) -> trpg_model::PluginContributionTrace {
    let kind_str = serde_json::to_value(f.kind)
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| format!("{:?}", f.kind));
    let detail: String = f.detail.chars().take(120).collect();
    let summary = if detail.is_empty() {
        kind_str.clone()
    } else {
        format!("{kind_str}: {detail}")
    };
    trpg_model::PluginContributionTrace {
        plugin_id: "core.knowledge_projection_verifier".to_string(),
        hook: crate::plugin::PluginHook::AfterLlmStream
            .as_str()
            .to_string(),
        kind: "verifier_finding".to_string(),
        summary,
    }
}

/// TC-D3-06：活动 NPC 知识一致性校验的**纯函数**内核（确定性、无 IO、fail-soft）。
///
/// 对每个活动 NPC（`(display_name, NpcSpeechProjection)`，投影只含该 NPC 自有 mind/plan）：
/// 1. **可见性门（v1 保守、低误报）**：仅当念白 plausibly 点到该 NPC（其稳定 `npc_id` 或
///    非空展示名作为子串出现在念白）才对其跑检查。**v1 限制**：这是粗粒度子串启发——只用
///    代词/别称指代的 NPC 不会被匹配（漏检优于误报），而展示名恰为他词子串时可能过匹配；
///    后续可换 surfaced-entity 绑定收紧。secret 门控由调用方 player-known 集驱动，与可见性门正交。
/// 2. **确定性 candidate 抽取**：从**同一私有 `secret_terms`**（已是私有 verifier 输入）取
///    term 出现在念白且绑 `fact_id` 的项的 fact_id，去重。与玩家泄漏扫描同 substring 口径。
///    **绝不**把匹配到的 term 文本写进任何 finding/trace——只取其 fact_id。
/// 3. 用该 NPC 言谈投影跑 [`trpg_runtime::verify_npc_disclosure`]（披露越出 facts_can_reveal）
///    与 [`trpg_runtime::verify_npc_asserted_facts`]（把不知道的 fact 当真相断言）。
/// 4. **去重**：与 `existing` SecretLeak（NoSpoiler + 玩家 projection）已覆盖的同一 fact 去重，
///    且本 pass 内每个 fact 至多产一条（按 fact_id）——避免重复 errata/trace。
///
/// 设计意图：玩家未知的 withheld secret 几乎总已被玩家 projection leak 覆盖（同为"玩家未知
/// 术语现身念白"），会被去重抹掉；本 pass 真正新增价值在 projection 漏掉的情形——某 fact 玩家
/// **已知**（projection 放行）但该 NPC 自身**不知道/不该说**（NPC 是说话者时仍属人设穿帮）。
pub fn npc_consistency_findings(
    narration: &str,
    npcs: &[trpg_runtime::NpcSpeechProjection],
    secret_terms: &[crate::plugin::SecretTerm],
    existing: &[trpg_agent::VerifierFinding],
) -> Vec<trpg_agent::VerifierFinding> {
    use trpg_agent::VerifierFindingKind;
    if narration.is_empty() || secret_terms.is_empty() || npcs.is_empty() {
        return Vec::new();
    }
    // 已被 existing SecretLeak 覆盖的 fact-key（跨来源去重：同 fact 不重复报）。NPC finding 的
    // detail 把 npc_id 与 fact_id 各包一对单引号，故用**末**对单引号取 fact_id（NoSpoiler/projection
    // 的 detail 仅一对单引号、首末同位，统一用 npc_finding_fact_key 取到 fact_id）。
    let mut seen: std::collections::HashSet<String> = existing
        .iter()
        .filter(|f| f.kind == VerifierFindingKind::SecretLeak)
        .filter_map(|f| npc_finding_fact_key(&f.detail).map(str::to_string))
        .collect();
    let mut out = Vec::new();
    for proj in npcs {
        // 展示名取自该 NPC 自有投影的 persona safe-view（= 旧 `profile.name`，见 safe_view），
        // crate-clean 投影已携带展示名，无需另载 profile。
        let name = proj.0.mind.persona.name.as_str();
        // 可见性门（v1）：念白未 plausibly 点到该 NPC ⇒ 跳过（降低误报）。
        let involves = narration.contains(proj.npc_id.as_str())
            || (!name.is_empty() && narration.contains(name));
        if !involves {
            continue;
        }
        // 确定性 candidate 抽取：term 现身念白且绑 fact_id 的私有 secret_terms 的 fact_id，去重。
        let mut candidates: Vec<String> = Vec::new();
        for t in secret_terms {
            if t.term.is_empty() || !narration.contains(t.term.as_str()) {
                continue;
            }
            if let Some(fid) = &t.fact_id {
                if !candidates.iter().any(|c| c == fid) {
                    candidates.push(fid.clone());
                }
            }
        }
        if candidates.is_empty() {
            continue;
        }
        let mut findings = trpg_runtime::verify_npc_disclosure(&proj.0, &candidates);
        findings.extend(trpg_runtime::verify_npc_asserted_facts(
            &proj.0,
            &candidates,
        ));
        for f in findings {
            match npc_finding_fact_key(&f.detail) {
                Some(k) => {
                    if seen.insert(k.to_string()) {
                        out.push(f);
                    }
                }
                None => out.push(f),
            }
        }
    }
    out
}

/// 设计3 §13-P3：活动 NPC **行为一致性**校验的纯函数内核（确定性、无 IO、advisory、fail-soft）。
///
/// 与 [`npc_consistency_findings`]（守"NPC 知道什么/能说什么"）正交：本检查守"NPC 被允许怎么行动"
/// ——念白是否呈现了该 NPC 自身行为计划 `forbidden_actions` 里禁止的行为。对每个活动 NPC：
/// 1. **可见性门**（同知识检查的保守子串启发）：念白未 plausibly 点到该 NPC ⇒ 跳过（降误报）。
/// 2. **确定性 marker 派生**：从该 NPC 计划的 `forbidden_actions` 派生 [`BehaviorSurfaceMarker`]——
///    `raise taboo topic: X` 形态绑到具体禁忌词 `X`，其余禁止项绑到其自身文本（保守、低误报）。
/// 3. 交纯函数 [`trpg_runtime::verify_npc_behavior_consistency`] 出 advisory（Warning）findings，
///    detail 仅引 npc_id + 行为标签，绝不回显念白匹配子串。
/// 4. 跨 NPC 按 (npc_id, action_label) 去重；与 `existing` 不混（行为标签 ≠ fact_id 键空间）。
///
/// 无活动 NPC / 空念白 → 空（fail-soft，旧行为零变更）。
pub fn npc_behavior_consistency_findings(
    narration: &str,
    npcs: &[trpg_runtime::NpcSpeechProjection],
    _existing: &[trpg_agent::VerifierFinding],
) -> Vec<trpg_agent::VerifierFinding> {
    if narration.is_empty() || npcs.is_empty() {
        return Vec::new();
    }
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for proj in npcs {
        // 展示名取自该 NPC 自有投影的 persona safe-view（= 旧 `profile.name`，见知识检查）。
        let name = proj.0.mind.persona.name.as_str();
        // 可见性门（v1）：念白未 plausibly 点到该 NPC ⇒ 跳过（降低误报）。
        let involves = narration.contains(proj.npc_id.as_str())
            || (!name.is_empty() && narration.contains(name));
        if !involves {
            continue;
        }
        let markers = behavior_markers_from_plan(&proj.0.plan);
        if markers.is_empty() {
            continue;
        }
        for f in trpg_runtime::verify_npc_behavior_consistency(&proj.0, narration, &markers) {
            // 末对单引号内的 token = action_label（finding detail 末对引号即行为标签）。
            let key = match npc_finding_fact_key(&f.detail) {
                Some(k) => format!("{}::{}", proj.npc_id, k),
                None => format!("{}::{}", proj.npc_id, out.len()),
            };
            if seen.insert(key) {
                out.push(f);
            }
        }
    }
    out
}

/// 从一个 NPC 行为计划的 `forbidden_actions` 确定性派生行为面 marker（无 IO、无 NLP）。
/// `raise taboo topic: X` 形态绑到具体禁忌词 `X`（念白真提该禁忌话题即穿帮）；其余禁止项绑到
/// 其自身文本（保守，几乎只在念白逐字复述该指令时命中，低误报）。
fn behavior_markers_from_plan(
    plan: &trpg_model::NpcBehaviorPlan,
) -> Vec<trpg_model::BehaviorSurfaceMarker> {
    const TABOO_PREFIX: &str = "raise taboo topic: ";
    plan.forbidden_actions
        .iter()
        .map(|fa| {
            let term = fa
                .strip_prefix(TABOO_PREFIX)
                .map(str::to_string)
                .unwrap_or_else(|| fa.clone());
            trpg_model::BehaviorSurfaceMarker::new(fa.clone(), vec![term])
        })
        .collect()
}

/// 从一条 verifier finding 的 detail 抽稳定 fact-key：取**末**对单引号内的 token。NPC finding 的
/// detail 形如 `NPC 'npc_x' ... fact 'fact_y' ...`（npc_id 在前、fact_id 在末对引号），NoSpoiler/
/// projection finding 仅含一对引号（fact_id 即首末），故末对引号统一取到 fact_id，可跨来源去重同一 fact。
fn npc_finding_fact_key(detail: &str) -> Option<&str> {
    let close = detail.rfind('\'')?;
    let open = detail[..close].rfind('\'')?;
    Some(&detail[open + 1..close])
}

/// 把一条 NPC 一致性 finding 折成 plugin trace（plugin_id 标本 NPC 知识一致性 verifier）。
/// summary = kind + 截断 detail（detail 只含 npc_id / fact_id，绝不夹 secret 正文）。
fn npc_consistency_finding_trace(
    f: &trpg_agent::VerifierFinding,
) -> trpg_model::PluginContributionTrace {
    let kind_str = serde_json::to_value(f.kind)
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| format!("{:?}", f.kind));
    let detail: String = f.detail.chars().take(120).collect();
    let summary = if detail.is_empty() {
        kind_str.clone()
    } else {
        format!("{kind_str}: {detail}")
    };
    trpg_model::PluginContributionTrace {
        plugin_id: "core.npc_knowledge_consistency".to_string(),
        hook: crate::plugin::PluginHook::AfterLlmStream
            .as_str()
            .to_string(),
        kind: "verifier_finding".to_string(),
        summary,
    }
}

#[cfg(test)]
mod projection_verifier_tests {
    use super::*;
    use crate::plugin::SecretTerm;
    use trpg_agent::{VerifierFinding, VerifierFindingKind, VerifierSeverity};

    fn secret_term(term: &str, fact_id: &str) -> SecretTerm {
        SecretTerm {
            term: term.into(),
            fact_id: Some(fact_id.into()),
        }
    }

    /// AfterLlmStream projection verifier：玩家未知 fact 的术语出现在念白 → 产 projection
    /// SecretLeak finding（引 fact_id、不回显 secret 正文）；玩家已知 → 放行；
    /// NoSpoiler 已覆盖同 fact → 去重为空。
    #[test]
    fn after_stream_verifier_uses_projection_findings() {
        const SECRET: &str = "管家其实是幕后真凶";
        let terms = vec![secret_term(SECRET, "fact_villain")];
        let narration = format!("旁白：{SECRET}。");

        // 玩家未知 + 无 existing 覆盖 → projection 产 1 条 SecretLeak。
        let out = projection_after_stream_findings(&narration, &terms, &[], &[]);
        assert_eq!(out.len(), 1, "玩家未知 fact 泄漏 → projection finding");
        assert_eq!(out[0].kind, VerifierFindingKind::SecretLeak);
        assert_eq!(out[0].severity, VerifierSeverity::Blocker);
        assert!(
            !out[0].detail.contains(SECRET),
            "finding 绝不回显 secret 正文"
        );
        assert!(out[0].detail.contains("fact_villain"), "应引 fact_id");

        // 玩家已知该 fact → 放行（揭示后可自由复述）。
        let known = vec!["fact_villain".to_string()];
        assert!(projection_after_stream_findings(&narration, &terms, &known, &[]).is_empty());

        // NoSpoiler 已对同 fact 报过 → 去重抹掉，避免重复 errata/trace。
        let existing = vec![VerifierFinding {
            kind: VerifierFindingKind::SecretLeak,
            severity: VerifierSeverity::Blocker,
            detail: "player-visible narration exposes a private secret for unrevealed fact 'fact_villain'".into(),
        }];
        assert!(
            projection_after_stream_findings(&narration, &terms, &[], &existing).is_empty(),
            "NoSpoiler 已覆盖同 fact → projection 去重为空"
        );

        // 无 secret_terms / 空念白 → fail-soft 空。
        assert!(projection_after_stream_findings(&narration, &[], &[], &[]).is_empty());
        assert!(projection_after_stream_findings("", &terms, &[], &[]).is_empty());
    }

    /// fact-key 抽取：从单引号包裹的 detail 取 fact_id（跨来源去重键）。
    #[test]
    fn secret_leak_fact_key_extracts_quoted_id() {
        assert_eq!(
            secret_leak_fact_key("exposes player-unknown fact 'fact_x'"),
            Some("fact_x")
        );
        assert_eq!(secret_leak_fact_key("no quotes here"), None);
    }

    /// P3.6 等价性守卫：流后 verify 路径单一来源（build_verifier_private_view 的 player_known
    /// 投影，已 sorted）喂下游，与旧的散读 `list_player_known_fact_ids`（未排序）逐字节等价。
    /// 核心论证：下游对 player_known 全部经 HashSet 消费（order-independent）。这里证明
    /// projection_after_stream_findings 对 player_known 的**顺序无关**：同一集合的任意排列
    /// 产出完全相同的 findings（kind/severity/detail/顺序）。
    #[test]
    fn p3_6_player_known_findings_are_order_independent() {
        const S_A: &str = "线索甲秘密";
        const S_B: &str = "线索乙秘密";
        let terms = vec![secret_term(S_A, "fact_a"), secret_term(S_B, "fact_b")];
        let narration = format!("旁白：{S_A}与{S_B}同时现身。");

        // 玩家已知集的两种排列（旧散读未排序 vs 新投影 sorted）——视作同一逻辑集合。
        let unsorted = vec!["fact_b".to_string(), "fact_a".to_string()];
        let sorted = {
            let proj = trpg_runtime::PlayerNarrationProjection::from_player_known(unsorted.clone());
            proj.known_fact_ids_sorted()
        };
        assert_eq!(sorted, vec!["fact_a".to_string(), "fact_b".to_string()]);

        let from_unsorted = projection_after_stream_findings(&narration, &terms, &unsorted, &[]);
        let from_sorted = projection_after_stream_findings(&narration, &terms, &sorted, &[]);
        // 两者均放行（fact_a/fact_b 都已知）→ 空，且严格相等（逐字节等价的最强形态）。
        assert_eq!(
            from_unsorted, from_sorted,
            "player_known 排列不应改变 findings（单一来源 sorted 与旧散读等价）"
        );

        // 仅部分已知时同样顺序无关：只知 fact_a，fact_b 仍泄漏。
        let only_a_unsorted = vec!["fact_a".to_string()];
        let only_a_sorted =
            trpg_runtime::PlayerNarrationProjection::from_player_known(only_a_unsorted.clone())
                .known_fact_ids_sorted();
        let f1 = projection_after_stream_findings(&narration, &terms, &only_a_unsorted, &[]);
        let f2 = projection_after_stream_findings(&narration, &terms, &only_a_sorted, &[]);
        assert_eq!(f1, f2, "部分已知时 findings 仍顺序无关");
        assert_eq!(f1.len(), 1, "未知的 fact_b 应泄漏一条");
        assert!(f1[0].detail.contains("fact_b"));
    }

    /// P3.6 load-equivalence：NPC 载入门 `should_load_npc_views` 必须与旧
    /// `npc_consistency_after_stream` 的提前返回条件**逐组合互为否定**。空 secret_terms / 空念白 /
    /// 空 active_npc 任一成立 → 不载入（baseline 零 NPC DB 读）；三者全非空才载入。
    /// 这同时锁定 `npc_behavior_consistency` 在 secret_terms 空时仍被门掉（旧路径同样早返跳过）。
    #[test]
    fn p3_6_npc_load_gate_matches_old_early_return() {
        let npc_ids = vec!["npc_a".to_string()];
        let terms = vec![secret_term("线索甲秘密", "fact_a")];
        let text = "旁白：线索甲秘密现身。";

        // 旧早返条件：active_npc 空 || 念白空 || secret_terms 空。门 = 该条件的否定。
        let cases: &[(&[String], &str, &[SecretTerm], bool)] = &[
            (&npc_ids, text, &terms, true), // 三者全非空 → 载入
            (&[], text, &terms, false),     // 无活动 NPC → 跳过
            (&npc_ids, "", &terms, false),  // 空念白 → 跳过
            (&npc_ids, text, &[], false), // 无 secret_terms → 跳过（旧路径早返，故行为一致性也被门掉）
            (&[], "", &[], false),        // 全空 → 跳过
        ];
        for (ids, vis, st, expected) in cases {
            let load = should_load_npc_views(ids, vis, st);
            assert_eq!(load, *expected, "门 ({ids:?}, {vis:?}, {} terms)", st.len());
            // 与旧早返条件逐字节互为否定（即旧路径会提前返回 ⇔ 不该载入）。
            let old_would_early_return = ids.is_empty() || vis.is_empty() || st.is_empty();
            assert_eq!(
                load, !old_would_early_return,
                "门必须是旧 npc_consistency_after_stream 早返条件的精确否定"
            );
        }
    }
}

#[cfg(test)]
#[path = "turn_loop_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "turn_loop_mode_tests.rs"]
mod mode_tempo_tests;

#[cfg(test)]
#[path = "narrator_fix_tests.rs"]
mod narrator_fix_tests;

#[cfg(test)]
#[path = "scene_sensory_floor_tests.rs"]
mod scene_sensory_floor_tests;
