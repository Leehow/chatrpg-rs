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
use regex::Regex;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tokio_util::sync::CancellationToken;
use trpg_interaction::InteractionLifecycleKernel;
use trpg_llm::{LlmClient, StreamEvent, ToolChoice};
use trpg_model::{
    ChatMessage, CompiledContext, ContextRequest, DirectorPlan, MechanicDue, MechanicalResultView,
    MemoryEvent, MemoryKind, RollVisibility, RuntimeState, StateFrame, Visibility, WorldEventKind,
    WorldReactionCandidate,
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
    // EV-4R (`progress_claims_on_gm_v1`, default OFF): the rendered EvidenceOffer capability
    // list + claim instruction, derived at `phase_context_assembly` from THIS turn's surfaced
    // clue atoms, folded into DynamicTailInput.evidence_offer_block. `evidence_admission` holds
    // the machine OfferSet + catalog + turn_id (= request.turn_id, the alignment fix) across the
    // LLM call, so `run_agent_loop` can admit the GM's `[progress_claims]` against this turn's
    // committed events. flag OFF ⇒ both stay None ⇒ no block, no admission ⇒ byte-identical.
    evidence_offer_block: Option<String>,
    evidence_admission: Option<(
        trpg_model::adventure_ir::EvidenceOfferSet,
        trpg_model::adventure_ir::EvidenceAtomCatalog,
        String,
    )>,
    // P6.7 reveal-gating 提名（agent_loop 内 reveal_fact 在 TRPG_REVEAL_GATING ON 时填，
    // PresentationCommit 边界在终审 Allow 后排序提交）。OFF ⇒ 恒空（reveal_fact 即时落库，基线）。
    nominated_reveals: Vec<crate::tools::RevealNomination>,
    // P6 revision (§二十四-#13 producer→commit)：note_player_rejection 在 TRPG_STORY_WRITE_LOOP ON
    // 时填本回合玩家拒绝的线索提名；PresentationCommit 边界 drain 后调 commit_story_writes 持久化。
    // OFF ⇒ 恒空（工具不注册、通道不挂 ⇒ 字节级基线）。
    rejected_nominations: Vec<crate::tools::RejectionNomination>,
    // L1.1 SPINE (flag `TRPG_DIRECTOR_POST_ADJUDICATION`, default OFF)：post-adjudication
    // committed-result projection. `resolution_commit_boundary` projects the turn ledger's
    // committed checks into read-only `MechanicalResultView`s here so the (L1.2) Beat Director
    // can plan a beat reflecting the REAL outcome. OFF ⇒ 恒空 ⇒ 无消费者 ⇒ 字节等价基线。
    post_adjudication_results: Vec<MechanicalResultView>,
    // L1.1 SPINE：the World reaction candidate pool captured pre-adjudication (in
    // `build_npc_behavior_guidance`) and retained for the post-adjudication Director seam.
    // OFF ⇒ 恒空 ⇒ 字节等价基线。
    world_candidates: Vec<WorldReactionCandidate>,
    // L1.2 SPINE：the post-adjudication Beat DirectorPlan, built at `resolution_commit_boundary`
    // from the committed result (`post_adjudication_results`) + the retained World pool. Per the
    // §2 composition rule it is delivered to narration via a new NarrationPacket carrier (L6.1),
    // NOT the pre-adjudication BP3 tail. OFF ⇒ 恒 None ⇒ 字节等价基线。
    post_adjudication_plan: Option<DirectorPlan>,
    // L4.3 — the PROACTIVE forbidden-reveal set for the current scene (the still-building threads'
    // facts), derived from the scene's ScenePlan at `resolution_commit_boundary` and composed into
    // the narrator's `NarrationPacket.forbidden_reveals`. Gated by `TRPG_DIRECTOR_SCENE_PLAN` ⇒ OFF
    // 恒空 ⇒ 字节等价基线 (run_narrator_phase passes an empty set, exactly as before).
    scene_forbidden_reveals: Vec<String>,
    // Pre-context scene override set only when a deterministic authored-neighbor
    // move is committed before ContextAssembly. The incoming RuntimeState may
    // still carry the old scene from turn entry; this override keeps the current
    // turn's context projection aligned with the just-committed scene.
    pre_context_scene_override: Option<String>,
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
            evidence_offer_block: None,
            evidence_admission: None,
            nominated_reveals: Vec::new(),
            rejected_nominations: Vec::new(),
            post_adjudication_results: Vec::new(),
            world_candidates: Vec::new(),
            post_adjudication_plan: None,
            scene_forbidden_reveals: Vec::new(),
            pre_context_scene_override: None,
        }
    }

    /// 回合 assistant_output 的**单一事实源**派生（与 R1 旧 finalize_turn 逐字一致）：
    /// awaiting 终态且 visible_text 为空 → gate.prompt_public 兜底；否则 → visible_text。
    /// critical phase_finalize（save_turn）与 heavy phase_finalize_heavy_memory（记忆/审计）
    /// 必须用同一值；后者在 `take_outcome` 清空 ctx 后才跑，故调用方须在清空前调此快照。
    /// L1.1 SPINE: project the turn ledger's committed checks onto `ctx` for the (L1.2) Beat
    /// Director, ONLY when the post-adjudication flag is ON. OFF ⇒ leaves the field empty
    /// (no-op) ⇒ byte-identical baseline. Pure read of `self.ledger`; fail-closed (a check with
    /// no `success` bool projects to `Unresolved`, never silently passed).
    pub(crate) fn capture_post_adjudication(&mut self, enabled: bool) {
        self.post_adjudication_results =
            project_post_adjudication_results(self.ledger.snapshot(), enabled);
    }

    /// L1.1 SPINE read-only accessor: the post-adjudication committed-result projection (the
    /// L1.2 Director's input). Empty when the flag is OFF.
    pub(crate) fn post_adjudication_results(&self) -> &[MechanicalResultView] {
        &self.post_adjudication_results
    }

    /// Test-only: seed the turn ledger with a committed check result so the L1.1 ctx-capture
    /// seam (`capture_post_adjudication`) can be exercised without a full GM turn.
    #[cfg(test)]
    pub(crate) fn test_record_check_result(&mut self, result: &trpg_model::CheckResultRecord) {
        self.ledger.record_result(result);
    }

    /// L1.1 SPINE read-only accessor: the captured pre-adjudication World candidate pool.
    /// Empty when the flag is OFF.
    pub(crate) fn world_candidates(&self) -> &[WorldReactionCandidate] {
        &self.world_candidates
    }

    /// L1.2 SPINE read-only accessor: the post-adjudication Beat DirectorPlan (the L6.1 carrier's
    /// input). `None` when the flag is OFF or no plan was built. Consumed by the L6.1 carrier.
    #[allow(dead_code)]
    pub(crate) fn post_adjudication_plan(&self) -> Option<&DirectorPlan> {
        self.post_adjudication_plan.as_ref()
    }

    /// L4.3 read-only accessor: the proactive forbidden-reveal set for the current scene (the
    /// still-building threads' facts). Empty when `TRPG_DIRECTOR_SCENE_PLAN` is OFF ⇒ the narrator
    /// composes an empty forbidden set, byte-identical to baseline.
    pub(crate) fn scene_forbidden_reveals(&self) -> &[String] {
        &self.scene_forbidden_reveals
    }

    /// Test-only: seed the L4.3 proactive forbidden-reveal set so the narrator-phase plumb can be
    /// exercised without a live scene-plan derivation.
    #[cfg(test)]
    pub(crate) fn test_set_scene_forbidden_reveals(&mut self, v: Vec<String>) {
        self.scene_forbidden_reveals = v;
    }

    pub(crate) fn heavy_assistant_output(&self) -> String {
        match &self.awaiting_gate {
            Some(gate) if self.visible_text.trim().is_empty() => gate.prompt_public.clone(),
            _ => self.visible_text.clone(),
        }
    }

    /// When a post-stream SceneNavigate commit proves that the player's movement
    /// reached a real module scene, ensure the buffered player-visible prose also
    /// acknowledges that destination before save/delivery.
    pub(crate) fn repair_visible_text_for_scene_transition(&mut self, info: &SceneTransitionInfo) {
        self.visible_text = scene_transition_visible_repair(&self.visible_text, info);
    }

    pub(crate) fn set_pre_context_scene_override(&mut self, scene_id: String) {
        if !scene_id.trim().is_empty() {
            self.pre_context_scene_override = Some(scene_id);
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
        self.sync_committed_turn_checks_from_db(input, ledger).await;
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
            &mut ctx.resolved_gate_facts,
            reveal_gating,
        )
        .await;
        self.fold_source_backed_nominations_for_module(
            input
                .request
                .module_id
                .as_deref()
                .or(ctx.state_agent.module_id.as_deref()),
            &ctx.nominated_reveals,
            &mut ctx.resolved_gate_facts,
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
        // EV-4R: admit the MAIN GM's `[progress_claims]` sidecar against THIS turn's committed
        // events (shadow). Parses `visible_text` BEFORE verify_after_stream may rewrite it, using
        // the OfferSet/catalog/turn_id held from turn-start. flag OFF ⇒ ctx.evidence_admission is
        // None ⇒ no-op ⇒ byte-identical baseline.
        self.admit_main_gm_progress_claims(ctx, input, &visible_text)
            .await;
        ctx.visible_text = visible_text;
        crate::execute::AgentSignal::Narration
    }

    async fn sync_committed_turn_checks_from_db(
        &self,
        input: &GmTurnInput<'_>,
        ledger: &mut TurnLedger,
    ) {
        match self
            .engine
            .db
            .list_check_contracts_for_turn(&input.request.session_id, &input.request.turn_id)
            .await
        {
            Ok(contracts) => {
                for contract in contracts {
                    ledger.record_contract_if_absent(&contract);
                }
            }
            Err(err) => tracing::warn!(
                error = %err,
                session_id = %input.request.session_id,
                turn_id = %input.request.turn_id,
                "turn ledger sync: list check contracts failed"
            ),
        }
        match self
            .engine
            .db
            .list_check_results_for_turn(&input.request.session_id, &input.request.turn_id)
            .await
        {
            Ok(results) => {
                for result in results {
                    ledger.record_result_if_absent(&result);
                }
            }
            Err(err) => tracing::warn!(
                error = %err,
                session_id = %input.request.session_id,
                turn_id = %input.request.turn_id,
                "turn ledger sync: list check results failed"
            ),
        }
    }

    /// EV-4R shadow admission of the main GM's `[progress_claims]` sidecar. Parses the GM prose
    /// for the closed-schema claims, then reuses `EvidenceGateway::admit` over THIS turn's
    /// committed DomainEvents (turn-local, so each `commit:<i>` basis resolves within the turn)
    /// with `turn_id = request.turn_id` (alignment). SHADOW: every decision is only logged — the
    /// engine is NOT called and no objective is completed (J3 unchanged). flag OFF ⇒
    /// `ctx.evidence_admission` is None (derivation never ran) ⇒ immediate no-op.
    async fn admit_main_gm_progress_claims(
        &self,
        ctx: &mut TurnContext,
        input: &GmTurnInput<'_>,
        visible_text: &str,
    ) {
        // EV-P2: the exact producers (LocationEntered/StateEstablished/ContentDelivery) run
        // post-commit BEFORE the GM audit (GPT Pro run order "exact projectors first"), so
        // most evidence does not depend on the GM's audit compliance. They may be flagged ON
        // independently of the offer/audit path, so handle the case where no offers were
        // derived this turn (admission None) but exact projectors are on.
        let exact_on = trpg_runtime::exact_projectors::progress_exact_projectors_enabled();
        let admission = ctx.evidence_admission.take();
        if admission.is_none() && !exact_on {
            return; // OFF == byte-identical baseline (neither path active)
        }
        // EV-APPLY-WIRE: the engine reads (never writes) the current scene to surface
        // scene-scoped objectives in the frontier — nav-split (no teleport/railroad).
        let current_scene = ctx.state_agent.scene_id.clone().unwrap_or_default();
        let doc = crate::turn_markup_parser::parse_turn_document(visible_text);
        // THIS turn's committed events only (turn-local ⇒ commit:<i> resolves within the turn;
        // also reinforces the turn_id alignment the gateway's CausationMismatch enforces). The
        // turn_id is the turn-start aligned id when offers were derived, else request.turn_id.
        let turn_id = admission
            .as_ref()
            .map(|(_, _, t)| t.clone())
            .unwrap_or_else(|| input.request.turn_id.clone());
        let events: Vec<trpg_model::DomainEvent> = self
            .engine
            .db
            .list_domain_events(&input.request.session_id, 5000)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e.turn_id == turn_id)
            .collect();

        // EV-P2: exact producers FIRST → ONE shared turn-local ledger (ExactDomain). The GM
        // audit/claims below are seeded with it so a GM-witnessed claim duplicating an exact
        // observation is rejected (no double-emit; ExactDomain wins). Still SHADOW.
        let seed = if exact_on {
            self.project_exact_evidence_shadow(input, &turn_id, admission.as_ref(), &doc, &events)
                .await
        } else {
            trpg_model::adventure_ir::EvidenceLedger::new()
        };

        let Some((offer_set, catalog, turn_id)) = admission else {
            return; // exact-only run (no offers this turn): the seed was already logged
        };

        // EV-P4: capability binding producer (shadow), run in BOTH audit + claim modes.
        // The GM's `[evidence_attempts]` tag + a committed successful check ⇒
        // AcceptedEvidence(ActionResolved, ExactDomain). Seeded with the exact-producer
        // ledger so a binding duplicating an exact observation is rejected. flag OFF or
        // no attempts ⇒ no-op. Still SHADOW: engine not consuming, J3 unchanged.
        if trpg_runtime::evidence_binding::progress_capability_binding_enabled()
            && !doc.evidence_attempts.is_empty()
        {
            let (bound, decisions) = trpg_runtime::evidence_binding::bind_capability_evidence(
                &input.request.session_id,
                &turn_id,
                &offer_set,
                &catalog,
                &events,
                &doc.evidence_attempts,
                &seed,
            );
            for d in &decisions {
                match &d.result {
                    Ok(atom) => tracing::info!(
                        session_id = %input.request.session_id,
                        turn_id = %turn_id,
                        cap = %d.cap_id.as_str(),
                        atom = %atom,
                        authority = "ExactDomain",
                        "EV-P4 capability binding ADMITTED action evidence on MAIN GM (shadow; engine not consuming, J3 unchanged)"
                    ),
                    Err(reason) => tracing::info!(
                        session_id = %input.request.session_id,
                        turn_id = %turn_id,
                        cap = %d.cap_id.as_str(),
                        reason = reason.as_str(),
                        "EV-P4 capability binding REJECTED on MAIN GM (shadow)"
                    ),
                }
            }
            tracing::info!(
                session_id = %input.request.session_id,
                turn_id = %turn_id,
                attempts = doc.evidence_attempts.len(),
                bound_admitted = bound.len().saturating_sub(seed.len()),
                "EV-P4 capability binding complete on MAIN GM (shadow; engine not consuming; J3 unchanged)"
            );
        }

        // EV-P1: in audit-mode evaluate the MANDATORY per-offer EvidenceAudit. A missing/
        // incomplete/extra/mismatched audit is a logged ProducerProtocolFailure — NEVER a
        // silent "none" (the EV-4R producer-recall bug). Still SHADOW: no engine, no objective.
        if trpg_runtime::evidence_gateway::progress_evidence_audit_required_enabled() {
            let outcome = self
                .evaluate_main_gm_audit(input, &turn_id, &offer_set, &catalog, &events, &doc, &seed)
                .await;
            // EV-P5: the post-turn witness recall backstop (separate focused LLM call), run
            // when the GM audit was missing/incomplete or complete-but-all-not_observed while
            // structural candidates exist. flag OFF ⇒ no-op. Returns the turn's accumulated
            // ledger (exact ∪ audit ∪ witness).
            let ledger = self
                .run_post_turn_witness(
                    input, &turn_id, &offer_set, &catalog, &events, &doc, &outcome,
                )
                .await;
            // EV-APPLY-WIRE: engine CONSUMES the accumulated ledger → ObjectiveResolved
            // (flag-gated; OFF == byte-identical shadow). This is the slice that ends shadow.
            self.apply_witnessed_progression(input, &turn_id, &current_scene, &ledger)
                .await;
            return;
        }

        // EV-4R path (audit OFF): the optional `[progress_claims]` sidecar.
        if doc.progress_claims.is_empty() {
            return;
        }
        let (ledger, decisions) = crate::evidence_claims::admit_gm_claims(
            &input.request.session_id,
            &turn_id,
            &offer_set,
            &catalog,
            &events,
            &doc.progress_claims,
            &seed,
        );
        for d in &decisions {
            match &d.result {
                Ok(atom) => tracing::info!(
                    session_id = %input.request.session_id,
                    turn_id = %turn_id,
                    cap = %d.cap_id,
                    atom = %atom,
                    authority = "GmWitnessed",
                    "EV-4R claim ADMITTED on MAIN GM (shadow; engine not consuming, J3 unchanged)"
                ),
                Err(reason) => tracing::info!(
                    session_id = %input.request.session_id,
                    turn_id = %turn_id,
                    cap = %d.cap_id,
                    reason = reason.as_str(),
                    "EV-4R claim REJECTED on MAIN GM (shadow)"
                ),
            }
        }
        tracing::info!(
            session_id = %input.request.session_id,
            turn_id = %turn_id,
            claims = doc.progress_claims.len(),
            admitted = ledger.len(),
            "EV-4R admission complete on MAIN GM"
        );
        // EV-APPLY-WIRE: engine CONSUMES this turn's admitted ledger on the EV-4R path too
        // (flag-gated; OFF == byte-identical shadow).
        self.apply_witnessed_progression(input, &turn_id, &current_scene, &ledger)
            .await;
    }

    /// EV-P2 (`progress_exact_projectors_v1`) shadow exact producers, run post-commit BEFORE the
    /// GM audit. Pure Rust LocationEntered (committed `SceneTransitioned` → authored location
    /// atom) + StateEstablished (committed `WorldFactChanged` → authored-state atom) from the
    /// module graph, plus the GM-ref ContentDelivery (verified via the reused gateway → FactLearned
    /// ExactDomain). All append to ONE turn-local ledger (returned as the seed for the GM audit, so
    /// the same observation never double-records). fail-closed: no graph / no authored atom /
    /// unresolved ref / non-player-driven nav ⇒ no evidence. SHADOW: engine not consuming; only logs.
    async fn project_exact_evidence_shadow(
        &self,
        input: &GmTurnInput<'_>,
        turn_id: &str,
        admission: Option<&(
            trpg_model::adventure_ir::EvidenceOfferSet,
            trpg_model::adventure_ir::EvidenceAtomCatalog,
            String,
        )>,
        doc: &crate::turn_document::TurnDocument,
        events: &[trpg_model::DomainEvent],
    ) -> trpg_model::adventure_ir::EvidenceLedger {
        let mut ledger = trpg_model::adventure_ir::EvidenceLedger::new();
        let mut loc = 0usize;
        let mut state = 0usize;

        // Location + State: pure Rust, need the module graph (topology + authored-state leaves).
        if let Some(module_id) = input
            .request
            .module_id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
        {
            if let Ok(Some(graph)) = self.engine.db.load_module_graph(module_id).await {
                let loc_cat = trpg_runtime::exact_projectors::build_location_atom_catalog(&graph);
                for ev in trpg_runtime::exact_projectors::project_location_entered(events, &loc_cat)
                    .entries()
                {
                    if ledger.append(ev.clone()) {
                        loc += 1;
                    }
                }
                let st_cat = trpg_runtime::exact_projectors::build_state_atom_catalog(&graph);
                for ev in trpg_runtime::exact_projectors::project_state_established(events, &st_cat)
                    .entries()
                {
                    if ledger.append(ev.clone()) {
                        state += 1;
                    }
                }
            }
        }

        // ContentDelivery: the GM's verified `[materialized_content]` refs → FactLearned
        // ExactDomain. Needs the turn's OfferSet + clue catalog (only present when offers were
        // derived this turn). Reuses the gateway; appends into the shared ledger.
        let mut content = 0usize;
        if let Some((offer_set, catalog, _)) = admission {
            let before = ledger.len();
            ledger = trpg_runtime::exact_projectors::project_content_delivery(
                &doc.materialized_content,
                &input.request.session_id,
                turn_id,
                offer_set,
                catalog,
                events,
                &ledger,
            );
            content = ledger.len().saturating_sub(before);
        }

        tracing::info!(
            session_id = %input.request.session_id,
            turn_id = %turn_id,
            location_entered = loc,
            state_established = state,
            content_delivered = content,
            exact_evidence = ledger.len(),
            "EV-P2 exact producers ran on MAIN GM (ExactDomain; shadow — engine not consuming, J3 unchanged)"
        );
        ledger
    }

    /// EV-P1 shadow evaluation of the MAIN GM's mandatory `[evidence_audit]` sidecar. Enforces
    /// completeness (every offered cap exactly one decision; matching offer_set_id; no extras)
    /// and admits each Observed decision via the reused `EvidenceGateway`. A missing/incomplete/
    /// extra/mismatched audit ⇒ a logged `ProducerProtocolFailure` (telemetry), NEVER a silent
    /// "none". Telemetry (audit_completeness / observed / not_observed / failure) is logged. Still
    /// SHADOW: the engine is NOT called and no objective completes (J3 unchanged).
    async fn evaluate_main_gm_audit(
        &self,
        input: &GmTurnInput<'_>,
        turn_id: &str,
        offer_set: &trpg_model::adventure_ir::EvidenceOfferSet,
        catalog: &trpg_model::adventure_ir::EvidenceAtomCatalog,
        events: &[trpg_model::DomainEvent],
        doc: &crate::turn_document::TurnDocument,
        seed: &trpg_model::adventure_ir::EvidenceLedger,
    ) -> crate::evidence_audit::AuditOutcome {
        let outcome = crate::evidence_audit::evaluate_gm_audit(
            &input.request.session_id,
            turn_id,
            offer_set,
            catalog,
            events,
            doc.evidence_audit.as_ref(),
            seed,
        );
        for d in &outcome.decisions {
            match &d.result {
                Ok(atom) => tracing::info!(
                    session_id = %input.request.session_id,
                    turn_id = %turn_id,
                    cap = %d.cap_id,
                    atom = %atom,
                    authority = "GmWitnessed",
                    "EV-P1 observed decision ADMITTED on MAIN GM (shadow; engine not consuming, J3 unchanged)"
                ),
                Err(reason) => tracing::info!(
                    session_id = %input.request.session_id,
                    turn_id = %turn_id,
                    cap = %d.cap_id,
                    reason = reason.as_str(),
                    "EV-P1 observed decision REJECTED by gateway on MAIN GM (shadow)"
                ),
            }
        }
        let t = &outcome.telemetry;
        match t.protocol_failure {
            Some(kind) => tracing::warn!(
                session_id = %input.request.session_id,
                turn_id = %turn_id,
                offers = t.offers,
                audit_completeness = t.completeness,
                protocol_failure = kind.as_str(),
                "EV-P1 ProducerProtocolFailure on MAIN GM (audit missing/incomplete — NOT a silent none; shadow)"
            ),
            None => tracing::info!(
                session_id = %input.request.session_id,
                turn_id = %turn_id,
                offers = t.offers,
                audit_completeness = t.completeness,
                observed = t.observed,
                not_observed = t.not_observed,
                admitted = outcome.ledger.len(),
                "EV-P1 EvidenceAudit complete on MAIN GM (shadow; engine not consuming; J3 unchanged)"
            ),
        }
        outcome
    }

    /// EV-P5 (`progress_post_turn_witness_v1`) — the recall backstop. After the GM audit
    /// (which the GM is unreliable at filling), run a SEPARATE focused single-task LLM
    /// witness when the audit was missing/incomplete OR complete-but-all-not_observed
    /// WHILE deterministic structural candidates exist (a committed success on a
    /// check-bindable offer this turn). The extractor sees ONLY the
    /// [`trpg_runtime::WitnessExtractorView`] (no objective/guard/reward), proposes
    /// cap+basis, and Rust admits via the reused EV-P4 binding / EV-4 gateway UNCHANGED.
    /// flag OFF ⇒ no second call (byte-identical baseline). Still SHADOW: the engine is
    /// not consuming the ledger (J3 unchanged).
    async fn run_post_turn_witness(
        &self,
        input: &GmTurnInput<'_>,
        turn_id: &str,
        offer_set: &trpg_model::adventure_ir::EvidenceOfferSet,
        catalog: &trpg_model::adventure_ir::EvidenceAtomCatalog,
        events: &[trpg_model::DomainEvent],
        doc: &crate::turn_document::TurnDocument,
        audit: &crate::evidence_audit::AuditOutcome,
    ) -> trpg_model::adventure_ir::EvidenceLedger {
        if !trpg_runtime::post_turn_witness::progress_post_turn_witness_enabled() {
            return audit.ledger.clone(); // OFF == byte-identical baseline (no second LLM call)
        }
        let candidates = trpg_runtime::post_turn_witness::structural_candidates(
            &input.request.session_id,
            turn_id,
            offer_set,
            events,
        );
        let protocol_failed = audit.telemetry.protocol_failure.is_some();
        let trigger = trpg_runtime::post_turn_witness::witness_trigger(
            protocol_failed,
            audit.telemetry.observed,
            candidates.len(),
        );
        if !trigger.fires() {
            tracing::info!(
                session_id = %input.request.session_id,
                turn_id = %turn_id,
                trigger = trigger.as_str(),
                structural_candidates = candidates.len(),
                witness_invoked = 0,
                "EV-P5 post-turn witness NOT triggered (backstop idle; shadow)"
            );
            return audit.ledger.clone();
        }
        let materialized_refs: Vec<String> = doc
            .materialized_content
            .iter()
            .map(|c| c.delivery_cap.as_str().to_string())
            .collect();
        let view = trpg_runtime::post_turn_witness::build_extractor_view(
            input.user_input,
            offer_set,
            events,
            materialized_refs,
            &candidates,
        );
        let producer = trpg_runtime::post_turn_witness::LlmWitnessProducer::new(self.llm.clone());
        let proposals =
            trpg_runtime::post_turn_witness::EvidenceClaimProducer::propose(&producer, &view).await;
        let (ledger, admissions) = trpg_runtime::post_turn_witness::admit_witness_proposals(
            &input.request.session_id,
            turn_id,
            offer_set,
            catalog,
            events,
            &proposals,
            &audit.ledger,
        );
        for a in &admissions {
            match &a.result {
                Ok(atom) => tracing::info!(
                    session_id = %input.request.session_id,
                    turn_id = %turn_id,
                    cap = %a.cap_id,
                    atom = %atom,
                    "EV-P5 post-turn witness ADMITTED evidence (shadow; engine not consuming, J3 unchanged)"
                ),
                Err(reason) => tracing::info!(
                    session_id = %input.request.session_id,
                    turn_id = %turn_id,
                    cap = %a.cap_id,
                    reason = %reason,
                    "EV-P5 post-turn witness proposal REJECTED (shadow)"
                ),
            }
        }
        tracing::info!(
            session_id = %input.request.session_id,
            turn_id = %turn_id,
            trigger = trigger.as_str(),
            structural_candidates = candidates.len(),
            witness_invoked = 1,
            witness_proposed = proposals.len(),
            witness_admitted = ledger.len().saturating_sub(audit.ledger.len()),
            "EV-P5 post-turn witness complete (recall backstop; shadow — engine not consuming, J3 unchanged)"
        );
        ledger
    }

    /// EV-APPLY-WIRE `witnessed_progression_apply_v1` (the last gap to J3): the engine
    /// CONSUMES this turn's accumulated AcceptedEvidence `ledger`. Until this slice every
    /// prior EV step was SHADOW (evidence admitted, engine never read it). Here, after the
    /// turn's exact/binding/witness admission, an objective whose guard is
    /// `EvidencePresent(<GuardLeaf atom>)` fires `ObjectiveCompleted` once that atom's
    /// evidence is in the ledger ⇒ a durable [`DomainEventKind::ObjectiveResolved`] is
    /// appended (the j3v2 SEMANTIC-axis carrier, prev always 0) and the frontier advances.
    ///
    /// Reuses the EV-APPLY pure core [`trpg_runtime::progression::witnessed_objective_resolutions`]
    /// (engine + EV-P4 objective compiler) — no rebuild. The ENGINE produces the signal, never
    /// the LLM (`LLM ∩ ProgressSignal = ∅`). **nav-split**: never mutates `current_scene`.
    ///
    /// D2 `progress_scene_advance_evidence_v1` adds a SECOND consume on the SAME engine/ledger:
    /// the player's CURRENT scene's evidence-backed advance objective
    /// ([`trpg_runtime::progression::witnessed_scene_advance_resolutions`]) — so a published
    /// module with no prep-packet objectives (homecoming) still advances when a real admitted
    /// authored observation of its scene lands. Gated by its OWN standalone flag.
    ///
    /// Both flags OFF ⇒ immediate no-op ⇒ no graph load, no event, byte-identical to the shadow
    /// path. fail-closed: no module / no graph / no prep-packet / no admitted GuardLeaf ⇒ no
    /// ObjectiveResolved.
    async fn apply_witnessed_progression(
        &self,
        input: &GmTurnInput<'_>,
        turn_id: &str,
        current_scene: &str,
        ledger: &trpg_model::adventure_ir::EvidenceLedger,
    ) {
        let apply_on = trpg_runtime::progression::witnessed_progression_apply_enabled();
        let scene_adv_on = trpg_runtime::progression::scene_advance_evidence_enabled();
        if !apply_on && !scene_adv_on {
            return; // OFF (both flags) == byte-identical baseline (engine never consumes the ledger live)
        }
        let Some(module_id) = input
            .request
            .module_id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
        else {
            return; // fail-closed: no module ⇒ no progression
        };
        // Same graph the offer/catalog path used at turn-start (clue-projected) so the
        // GuardLeaf atom_ids match the admitted evidence's atom_ids exactly (codex A).
        let mut graph = match self.engine.db.load_module_graph(module_id).await {
            Ok(Some(g)) => g,
            _ => return, // fail-closed: graph load failed/absent
        };
        let _ = trpg_runtime::clue_projection::project_clues_onto_scenes(&mut graph);

        // D2 `progress_scene_advance_evidence_v1`: the engine consumes the ledger for the
        // player's CURRENT scene's evidence-backed advance objective → a durable
        // `scene_advance` ObjectiveResolved when a real admitted authored observation of
        // that scene lands. Wall B bridge for published modules with no prep-packet
        // objectives (homecoming). Independent of the prep-packet; runs on the SAME
        // clue-projected graph as the offers. OFF ⇒ this block never runs ⇒ byte-identical.
        if scene_adv_on {
            let scene_adv_resolutions =
                trpg_runtime::progression::witnessed_scene_advance_resolutions(
                    &input.request.session_id,
                    turn_id,
                    &graph,
                    ledger,
                    current_scene,
                );
            // Track whether the player's CURRENT scene's advance objective resolved this turn — the
            // earned-completion signal that gates the E1 SceneUnlocked emit (never unlock unearned).
            let mut current_scene_advanced = false;
            for ev in &scene_adv_resolutions {
                if ev
                    .data
                    .get("scene_id")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    == Some(current_scene.trim())
                {
                    current_scene_advanced = true;
                }
                if let Err(e) = self.engine.db.append_domain_event(ev).await {
                    tracing::warn!(error = %e, turn_id = %turn_id, "D2 scene-advance ObjectiveResolved append failed (non-fatal)");
                } else {
                    tracing::info!(
                        session_id = %input.request.session_id,
                        turn_id = %turn_id,
                        objective = %ev.data.get("objective_id").and_then(|v| v.as_str()).unwrap_or(""),
                        atom = %ev.data.get("atom_id").and_then(|v| v.as_str()).unwrap_or(""),
                        scene = %ev.data.get("scene_id").and_then(|v| v.as_str()).unwrap_or(""),
                        "D2 engine consumed admitted scene observation ⇒ scene_advance ObjectiveResolved (frontier advances; current_scene untouched)"
                    );
                }
            }

            // E1 `scene_transition_gated_v1` EMIT: once the CURRENT scene's advance objective has
            // COMPLETED (the player earned it, above), the ProgressionEngine emits a durable
            // `SceneUnlocked` recording the data-driven next-scene target (authored out-edge else
            // continuous-spine page order; fail-closed if no sensible target ⇒ no event, never
            // teleport). **nav-split**: this ONLY records the unlock — it NEVER writes current_scene;
            // the NavigationResolver (`scene_navigate_critical`) consumes it later THIS SAME turn and
            // performs the physical transition (it is the SOLE current_scene writer). Idempotent on
            // event_id ⇒ a re-emit on a later turn is a no-op. OFF (flag default) ⇒ this block is
            // skipped ⇒ zero SceneUnlocked + navigator consume skipped ⇒ byte-identical baseline.
            if current_scene_advanced && trpg_runtime::progression::scene_transition_gated_enabled()
            {
                if let Some(unlock) = trpg_runtime::progression::scene_unlock_event(
                    &input.request.session_id,
                    turn_id,
                    &graph,
                    current_scene,
                ) {
                    let next = unlock
                        .data
                        .get("next_scene")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if let Err(e) = self.engine.db.append_domain_event(&unlock).await {
                        tracing::warn!(error = %e, turn_id = %turn_id, "E1 SceneUnlocked append failed (non-fatal)");
                    } else {
                        tracing::info!(
                            session_id = %input.request.session_id,
                            turn_id = %turn_id,
                            from_scene = %current_scene,
                            next_scene = %next,
                            "E1 engine emitted SceneUnlocked from a COMPLETED obj.scene_advance (data-driven next-scene target; current_scene untouched — NavigationResolver will transition this turn)"
                        );
                    }
                }
            }
        }

        // EV-APPLY-WIRE prep-packet path (the_vault) — unchanged; gated by the master/apply flag.
        if apply_on {
            let Ok(Some(csp)) = self
                .engine
                .db
                .load_module_prep_packet_session(module_id)
                .await
            else {
                return; // fail-closed: no prep-packet ⇒ no authored evidence-objective
            };
            let resolutions = trpg_runtime::progression::witnessed_objective_resolutions(
                &input.request.session_id,
                turn_id,
                &graph,
                &csp,
                ledger,
                current_scene,
            );
            for ev in &resolutions {
                // Durable SEM_KINDS carrier (j3v2 semantic axis). Idempotent on event_id
                // (de_objresolved_{session}_{objective}) ⇒ one row per objective per session.
                if let Err(e) = self.engine.db.append_domain_event(ev).await {
                    tracing::warn!(error = %e, turn_id = %turn_id, "EV-APPLY-WIRE ObjectiveResolved append failed (non-fatal)");
                } else {
                    tracing::info!(
                        session_id = %input.request.session_id,
                        turn_id = %turn_id,
                        objective = %ev.data.get("objective_id").and_then(|v| v.as_str()).unwrap_or(""),
                        atom = %ev.data.get("atom_id").and_then(|v| v.as_str()).unwrap_or(""),
                        "EV-APPLY-WIRE engine consumed witnessed GuardLeaf evidence ⇒ ObjectiveResolved (J3 semantic progression; frontier advances; current_scene untouched)"
                    );
                }
            }
        }
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
        // Q-MODULE DP-A'/DP-B'：注入模组授权的进场 establishing 素材(runtime 仅 Enforce 填充
        // CompiledContext.scene_establishing,= 当前场景 NON-secret read_aloud,剧透裁剪后)。
        // Off/Shadow ⇒ 该字段为空 ⇒ 注入空 ⇒ 字节等价基线。
        let scene_establishing = ctx.compiled().scene_establishing.clone();
        // OA2 (G-3): player-safe PC 能力档案(runtime 仅 Enforce 填 character_context)。
        let character_context = ctx.compiled().character_context.clone();
        // L6.1 SPINE→narration: the post-adjudication Beat DirectorPlan (built at
        // resolution_commit_boundary BEFORE this phase, execute.rs:242→251) flows into the Narrator
        // as player-safe structured steering tokens, ALONGSIDE scene_establishing/character_context
        // (§2 composition rule). Spine flag OFF ⇒ post_adjudication_plan() is None ⇒ empty tokens ⇒
        // carrier empty ⇒ OFF byte-equal. Reveal facts / forbidden_reveals are NOT carried here.
        let director_plan_tokens = ctx
            .post_adjudication_plan()
            .map(player_safe_director_plan_tokens)
            .unwrap_or_default();
        // L4.3: compose the PROACTIVE forbidden-reveal set (still-building threads' facts, derived
        // from the scene's ScenePlan at resolution_commit_boundary) into the packet's
        // forbidden_reveals ALONGSIDE the carriers. Empty when `TRPG_DIRECTOR_SCENE_PLAN` is OFF ⇒
        // byte-identical baseline (project's 3rd arg was always `&[]`). The reactive narrowing/regen
        // ladder downstream stays the backstop.
        let scene_forbidden = ctx.scene_forbidden_reveals().to_vec();
        // M3 决策#3:故事情绪 carrier。默认 OFF ⇒ 空 tokens ⇒ carrier 空 ⇒ 字节等价基线。ON(opt-in)
        // 时取**玩家已感知层**的连续性散文作为氛围定调来源——绝不含 Story/Director 记忆(零 telegraph)。
        let story_mood_tokens = if narrator_story_mood_enabled() {
            player_safe_scene_context(input)
        } else {
            Vec::new()
        };
        let narration = crate::packet::NarrationPacket::project(&adj, "", &scene_forbidden)
            .with_scene_context(&player_safe_scene_context(input))
            .with_scene_establishing(&scene_establishing)
            .with_character_context(&character_context)
            .with_director_plan(&director_plan_tokens)
            .with_story_mood(&story_mood_tokens);
        let private_tokens = ctx.ledger.private_roll_tokens();
        // P7.3：P1 Narrator 派发经 NarratorPort 适配器（GmLoopNarratorAdapter 仅委托
        // `GmLoop::run_narrator`）——dispatch 间接，byte-identical（无行为变更）。
        let narrated = crate::ports::GmLoopNarratorAdapter(self)
            .narrate(&narration, &private_tokens, tx, cancel)
            .await;
        match narrated {
            Some(text) if !text.trim().is_empty() => {
                ctx.visible_text = text; // Narrator 输出 = 玩家可见单一事实源
                                         // OB-hide / OB-meta (Phase B): the split Narrator (player-facing, constitution ⑧)
                                         // does NOT author hidden facts; the adjudicator (台下) emits [hide]/[meta]. Those
                                         // blocks would be LOST when narrator output replaces visible_text — so preserve
                                         // them RAW (Q-7-REVISED: transport emits raw; the parser classifies them hidden;
                                         // the 战报 labels them). GM_CRAFT-gated ⇒ additive; OFF==baseline (OFF skips this
                                         // whole craft path, and split-OFF never calls run_narrator_phase).
                if crate::gm_craft::enabled() {
                    // (a) LLM-authored [hide]/[meta] from the adjudicator prose (richer secrets
                    //     like [hide kind="secret"] the GM judged exist), if it emitted any.
                    let offstage = crate::gm_craft::extract_offstage_blocks(&adjudicator_prose);
                    if !offstage.is_empty() {
                        ctx.visible_text.push('\n');
                        ctx.visible_text.push_str(&offstage);
                    }
                    // (b) Deterministic floor: synthesize [meta]/[hide 暗骰] from the REAL ledger
                    //     snapshot so the off-stage channels emit truthfully every adjudicated turn
                    //     even when the LLM omits the tags (no invention — committed facts only).
                    let synth =
                        crate::gm_craft::synthesize_offstage_from_ledger(ctx.ledger.snapshot());
                    if !synth.is_empty() {
                        ctx.visible_text.push('\n');
                        ctx.visible_text.push_str(&synth);
                    }
                }
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
            if classified.empty_rolls_unwrapped > 0 || classified.malformed_rolls_unwrapped > 0 {
                ctx.visible_text = classified.player_wire_text.clone();
            }
            tracing::debug!(
                turn_id = %input.request.turn_id,
                meta_blocks = classified.meta_blocks.len(),
                hide_blocks = classified.hide_blocks.len(),
                empty_rolls = classified.empty_rolls_unwrapped,
                // R-1: count of unbound/未定 [roll] blocks unwrapped to narration this turn
                // (the v4 sample asserts ZERO undetermined [roll] reach the player text).
                malformed_rolls = classified.malformed_rolls_unwrapped,
                "gm_craft typed turn-document: audience classified; malformed/empty roll wrappers sanitized before delivery"
            );
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
        self.run_narrator_with_delivery(packet, private_tokens, tx, cancel, true)
            .await
    }

    pub(crate) async fn run_narrator_buffered(
        &self,
        packet: &crate::packet::NarrationPacket,
        private_tokens: &[String],
        tx: &tokio::sync::mpsc::Sender<crate::turn_event::TurnEvent>,
        cancel: Option<&CancellationToken>,
    ) -> Option<String> {
        self.run_narrator_with_delivery(packet, private_tokens, tx, cancel, false)
            .await
    }

    async fn run_narrator_with_delivery(
        &self,
        packet: &crate::packet::NarrationPacket,
        private_tokens: &[String],
        tx: &tokio::sync::mpsc::Sender<crate::turn_event::TurnEvent>,
        cancel: Option<&CancellationToken>,
        stream_prose: bool,
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
                    if stream_prose {
                        let _ = tx
                            .send(crate::turn_event::TurnEvent::Delta(safe.clone()))
                            .await;
                    }
                    visible_text.push_str(&safe);
                }
            }
        }
        if !cancelled {
            let rest = redactor.finish();
            if !rest.is_empty() {
                if stream_prose {
                    let _ = tx
                        .send(crate::turn_event::TurnEvent::Delta(rest.clone()))
                        .await;
                }
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

    /// Pre-ContextAssembly deterministic scene navigation.
    ///
    /// When the player explicitly names an authored neighboring scene in the
    /// same action that starts an investigation there, the GM needs that target
    /// scene's player-safe context before it speaks. This path is intentionally
    /// conservative: it only consumes current-scene authored links and fails
    /// closed on missing module/graph/session state.
    pub(crate) async fn phase_pre_context_scene_navigate(
        &self,
        input: &GmTurnInput<'_>,
    ) -> Option<SceneTransitionInfo> {
        let module_id = input.request.module_id.as_deref()?;
        let graph = match self.engine.db.load_module_graph(module_id).await {
            Ok(Some(graph)) => graph,
            Ok(None) => return None,
            Err(err) => {
                tracing::warn!(error = %err, module_id, "pre-context scene navigate: load_module_graph failed");
                return None;
            }
        };
        let current = match self
            .engine
            .db
            .load_session_scene(&input.request.session_id)
            .await
        {
            Ok(Some(scene_id)) => scene_id.trim().to_string(),
            Ok(None) => return None,
            Err(err) => {
                tracing::warn!(error = %err, session_id = %input.request.session_id, "pre-context scene navigate: load_session_scene failed");
                return None;
            }
        };
        if current.is_empty() {
            return None;
        }
        let target = match trpg_runtime::scene_navigation::resolve_explicit_authored_scene(
            &graph,
            &current,
            input.user_input,
        ) {
            Some(target) => target,
            None => {
                trpg_runtime::scene_navigation::resolve_explicit_authored_scene_semantic(
                    self.llm.as_ref(),
                    &graph,
                    &current,
                    input.user_input,
                )
                .await?
            }
        };
        if target == current {
            return None;
        }
        if let Err(err) = self
            .engine
            .db
            .set_session_scene(&input.request.session_id, &target)
            .await
        {
            tracing::warn!(
                error = %err,
                session_id = %input.request.session_id,
                from = %current,
                to = %target,
                "pre-context scene navigate: set_session_scene failed"
            );
            return None;
        }
        let (from_title, to_title, to_read_aloud, to_summary, to_aliases) = self
            .scene_transition_player_context(module_id, &current, &target)
            .await;
        Some(SceneTransitionInfo {
            from: current,
            to: target.clone(),
            reason: format!(
                "pre-context explicit neighbor move: player named authored scene {target}"
            ),
            from_title,
            to_title,
            to_read_aloud,
            to_summary,
            to_aliases,
        })
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
        if let Some(scene_id) = ctx.pre_context_scene_override.clone() {
            ctx.state_agent.scene_id = Some(scene_id);
        }
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
        // EV-4R: derive THIS turn's EvidenceOfferSet (turn-start, with turn_id = request.turn_id)
        // and stash the rendered capability+claim block + the machine OfferSet/catalog/turn_id on
        // ctx. flag OFF ⇒ both stay None ⇒ no block ⇒ byte-identical baseline.
        self.derive_evidence_offers(ctx, input).await;
        // P5.6: the Director packet was stashed on ctx by build_npc_behavior_guidance (flag OFF
        // ⇒ None ⇒ no block ⇒ byte-identical). Mirror npc_guidance: pass as Option<&str>.
        let director_packet = ctx.director_packet_block.clone();
        let evidence_offer = ctx.evidence_offer_block.clone();
        let tail = DynamicTailInput {
            user_input: input.user_input,
            resolved_gate_facts: &ctx.resolved_gate_facts,
            errata_blocks: &ctx.errata_blocks,
            obligations_block: ctx.obligations_block.as_deref(),
            npc_guidance_block: npc_guidance.as_deref(),
            director_packet_block: director_packet.as_deref(),
            evidence_offer_block: evidence_offer.as_deref(),
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

    /// EV-4R (`progress_claims_on_gm_v1`, default OFF): derive THIS turn's EvidenceOfferSet from
    /// the current scene's surfaced clue atoms and stash (a) the rendered GM prompt block and
    /// (b) the machine OfferSet + catalog + turn_id on `ctx` for post-LLM admission. The turn_id
    /// is `request.turn_id` — the SAME value stamped on this turn's committed DomainEvents (the
    /// alignment fix: a derived `turn_{count}` would make the gateway reject every real claim
    /// with CausationMismatch). Reuses EV-1 clue projection + EV-2 catalog + EV-3
    /// `derive_offer_set` unchanged. fail-closed: no module / unknown scene / no surfaced clue ⇒
    /// empty ⇒ no block. flag OFF ⇒ early return ⇒ ctx fields stay None ⇒ byte-identical
    /// baseline (no graph load, no prompt change, no RNG). NO ruleset/module name-branch.
    async fn derive_evidence_offers(&self, ctx: &mut TurnContext, input: &GmTurnInput<'_>) {
        // EV-P1: the mandatory-audit protocol SUPERSEDES the EV-4R optional-claim path. The
        // OfferSet derivation is identical for both; only the prompt block + the post-LLM
        // evaluation differ. Both flags OFF ⇒ early return ⇒ ctx fields stay None ⇒ baseline.
        let audit_mode = trpg_runtime::evidence_gateway::progress_evidence_audit_required_enabled();
        if !(audit_mode || trpg_runtime::evidence_gateway::progress_claims_on_gm_enabled()) {
            return;
        }
        let Some(module_id) = input
            .request
            .module_id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
        else {
            return;
        };
        let mut graph = match self.engine.db.load_module_graph(module_id).await {
            Ok(Some(g)) => g,
            Ok(None) => return,
            Err(err) => {
                tracing::warn!(error = %err, module_id, "EV-4R offer derivation: module graph load failed");
                return;
            }
        };
        // Surfaced clues come from page-containment projection (CL-1); we own this graph copy.
        let _ = trpg_runtime::clue_projection::project_clues_onto_scenes(&mut graph);
        let mut catalog = trpg_runtime::evidence_projection::build_evidence_atom_catalog(&graph);
        // EV-P4 (B): the_vault's authored mission objectives live in the prep-packet, NOT
        // the (flattened) module_graph. Enrich the catalog with GuardLeaf objective atoms
        // from the prep-packet so flowing evidence is progression-relevant. Flag-gated ⇒
        // OFF == byte-identical baseline (no DB read, no atoms). The EV-P3 offer path
        // (active under the master flag) surfaces these scene-tagged observable actions.
        if trpg_runtime::evidence_binding::progress_capability_binding_enabled() {
            if let Ok(Some(csp)) = self
                .engine
                .db
                .load_module_prep_packet_session(module_id)
                .await
            {
                let mut guard_leaves = 0usize;
                for atom in trpg_model::adventure_ir::compile_prep_packet_guard_leaves(&graph, &csp)
                {
                    if catalog.insert(atom) {
                        guard_leaves += 1;
                    }
                }
                if guard_leaves > 0 {
                    tracing::info!(
                        module_id,
                        guard_leaves,
                        "EV-P4 prep-packet GuardLeaf objective atoms compiled into the catalog (shadow)"
                    );
                }
            }
        }
        let current_scene = ctx.state_agent.scene_id.as_deref().unwrap_or("");
        let turn_id = input.request.turn_id.clone();
        let offer_set = trpg_runtime::evidence_offers::derive_offer_set(
            &graph,
            &catalog,
            current_scene,
            &input.request.session_id,
            &turn_id,
        );
        if offer_set.is_empty() {
            return; // fail-closed: nothing surfaced ⇒ no block ⇒ byte-identical
        }
        // EV-P1 audit-mode renders the mandatory per-offer audit block (instruction + 5
        // few-shot + closed `[evidence_audit]` format); EV-4R renders the optional claim block.
        let mut block = if audit_mode {
            crate::evidence_audit::render_gm_audit_block(&offer_set)
        } else {
            crate::evidence_claims::render_gm_offer_and_claim_block(&offer_set)
        };
        if block.trim().is_empty() {
            return;
        }
        // EV-P2: when the exact projectors are ON, additionally ask the GM to emit a
        // `[materialized_content]` ref for any authored clue whose content it actually
        // presented this turn (the ContentDeliveryProducer's input). Appended only inside the
        // flag guard ⇒ OFF prompt bytes unchanged.
        if trpg_runtime::exact_projectors::progress_exact_projectors_enabled() {
            block.push_str("\n\n");
            block.push_str(&crate::evidence_audit::render_content_delivery_instruction());
        }
        // EV-P4: when capability binding is ON, ask the GM to tag a check with the
        // offered action it attempts (`[evidence_attempts]`). Appended only inside the
        // flag guard ⇒ OFF prompt bytes unchanged.
        if trpg_runtime::evidence_binding::progress_capability_binding_enabled() {
            block.push_str("\n\n");
            block.push_str(&trpg_runtime::evidence_binding::render_attempt_instruction());
        }
        tracing::info!(
            session_id = %input.request.session_id,
            turn_id = %turn_id,
            catalog_atoms = catalog.len(),
            offers = offer_set.len(),
            audit_mode,
            "EV-P1/EV-4R progress offers computed for MAIN GM (turn_id aligned to request.turn_id)"
        );
        ctx.evidence_offer_block = Some(block);
        ctx.evidence_admission = Some((offer_set, catalog, turn_id));
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
                // L9.2 BeatWeight：Director MATERIAL（评分时择用），非 prompt/filter；此应用
                // 站点只记 trace（上方已 push）、不在此应用，fail-safe no-op。内置 always-on host
                // 不注册任何 BeatWeight emitter ⇒ 此臂在 OFF 基线永不命中（字节等价）。
                crate::plugin::PluginContributionKind::BeatWeight(_) => {}
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
        // L1.1 SPINE: retain the pre-adjudication World candidate pool for the post-adjudication
        // Director seam (`resolution_commit_boundary`). Flag-gated (default OFF ⇒ no stash ⇒
        // byte-identical baseline). The Director (L1.2) reads it ALONGSIDE the committed result
        // so a relocated beat can still pull toward live world reactions (content-gravity).
        if director_post_adjudication_enabled() {
            ctx.world_candidates = reaction_set.reactions.clone();
        }
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
        //
        // L1.2 SPINE: when `TRPG_DIRECTOR_POST_ADJUDICATION` is ON, the Director runs AFTER
        // adjudication (at `resolution_commit_boundary`, seeing the committed result) and is
        // delivered via the L6.1 carrier. Building the pre-adjudication packet here too would be a
        // double-build with a stale (pre-result) plan, so we SKIP it under the spine flag —
        // `director_packet_block` stays None. OFF ⇒ unchanged ⇒ byte-identical baseline.
        if !director_post_adjudication_enabled() {
            ctx.director_packet_block = crate::ports::DirectorAdapter
                .prepare_brief(
                    &self.engine,
                    session_id,
                    &reaction_set.reactions,
                    &acting_actor_id,
                )
                .await;
        }
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
        let mut verification_snapshot = ledger.snapshot().clone();
        if let Ok(facts) = self
            .engine
            .db
            .list_recent_world_facts(&request.session_id, 16)
            .await
        {
            verification_snapshot.committed_world_facts = facts
                .into_iter()
                .map(|fact| {
                    let summary = [
                        fact.summary.as_str(),
                        fact.subject.as_str(),
                        fact.predicate.as_str(),
                        fact.object.as_str(),
                    ]
                    .into_iter()
                    .filter(|part| !part.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                    trpg_agent::CommittedWorldFactEvidence {
                        fact_id: fact.fact_id,
                        summary,
                        truth_status: fact.truth_status,
                    }
                })
                .collect();
        }
        // B7 语义决策：agent 不显式声明引用，引擎代填"本回合 ledger id 全集"
        // （ledger_id_set 物化，排序保证确定性），但子串回退仍独立运行；
        // 额外接入 committed world facts 只作为既成事实证据，不作为结构化 ledger id。
        let referenced_ledger_ids = {
            let mut ids: Vec<String> = trpg_agent::ledger_id_set(&verification_snapshot)
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
        let result = verifier.verify(&verification_snapshot, &submission);
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
        // Gate 策略：显式 TRPG_PRESENTATION_GATE 开启时全局执行；module play 默认执行，
        // 因为玩家行动菜单/秘密泄漏属于产品契约违例而不是可选诊断。非 module 且 env OFF
        // 仍维持基线：只记 trace、不进 repair。只有 buffered_narration 在位时才修复；
        // 若文字已实时流出，则退化为仅 trace（回收已流出文字无意义）。绝不回滚已 commit
        // 的 ledger/DB。
        //
        // A3(§6 大考)：白名单之外的 OmittedVisibleResult(空心念白)在 ON+buffered 时**旁路**触发
        // 同一阶梯——split Narrator 无 ReviseText 自修环,靠此补收敛守卫。触发条件加 NARRATOR_SPLIT
        // 显式门(buffered_narration 已蕴含,但显式更清晰),且 OFF(任一 flag 关)恒不进 ⇒ 字节等价。
        let gate_block = ctx.presentation_gate.is_block();
        let missing_committed_roll = committed_player_visible_check_missing_roll(
            &ctx.ledger.snapshot().check_results,
            &ctx.visible_text,
        );
        let omitted_repair = omitted_visible_repair_path_enabled(
            input.request.module_id.is_some(),
            narrator_split_enabled(),
        ) && (outcome.omitted_visible_repair || missing_committed_roll);
        // A3-HARDEN(§6 大考)：机器上下文回显 / 原始 JSON dump 旁路信号。**严格 ON-only**——
        // 谓词仅在 narrator_split_enabled() 时**才计算**(codex 审：避免 OFF 路径"算了但不生效"，
        // 使 OFF 真正不消费该谓词)，且白名单(BLOCKING_KINDS)纹丝不动 ⇒ OFF 字节等价。
        let echo_repair =
            narrator_split_enabled() && narration_is_machine_context_echo(&ctx.visible_text);
        if presentation_gate_enforced(input.request.module_id.is_some())
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
        if presentation_gate_enforced(input.request.module_id.is_some()) {
            if let Some(repaired) =
                source_limited_missing_address_repair(input.user_input, &ctx.visible_text)
            {
                ctx.visible_text = repaired;
            }
            if let Some(repaired) = deterministic_manifest_list_prose_repair(&ctx.visible_text) {
                ctx.visible_text = repaired;
            }
            if let Some(repaired) = deterministic_player_agency_menu_tail_repair(&ctx.visible_text)
            {
                ctx.visible_text = repaired;
            }
            if let Some(repaired) = deterministic_dangling_colon_tail_repair(&ctx.visible_text) {
                ctx.visible_text = repaired;
            }
            if let Some(repaired) = unwrap_nonauditable_roll_blocks(&ctx.visible_text) {
                ctx.visible_text = repaired;
            }
            ctx.visible_text = unwrap_player_visible_system_wrappers(&ctx.visible_text);
            self.ensure_committed_visible_projection_after_repairs(ctx, input.user_input)
                .await;
            self.ensure_concrete_information_request_answered_after_repairs(ctx, input.user_input)
                .await;
            if let Some(repaired) = deterministic_dangling_colon_tail_repair(&ctx.visible_text) {
                ctx.visible_text = repaired;
            }
        }
        if presentation_gate_enforced(input.request.module_id.is_some())
            && buffered_narration
            && player_agency_menu_cue_needs_block(&ctx.visible_text)
        {
            ctx.presentation_gate = crate::presentation_gate::PresentationGate::Block(vec![
                trpg_agent::VerifierFinding {
                    kind: trpg_agent::VerifierFindingKind::PlayerAgencyViolation,
                    severity: trpg_agent::VerifierSeverity::Blocker,
                    detail: "final visible text still contains an explicit action menu cue"
                        .to_string(),
                },
            ]);
            ctx.plugin_contributions
                .push(presentation_gate_trace(&ctx.presentation_gate));
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
        if let crate::presentation_gate::PresentationGate::Block(findings) = &ctx.presentation_gate
        {
            if findings.iter().any(|f| f.detail.contains("manifest list")) {
                if let Some(text) = deterministic_manifest_list_prose_repair(&ctx.visible_text) {
                    let recheck = self
                        .verify_after_stream(
                            input.request,
                            &ctx.ledger,
                            &text,
                            &input.state.active_npc_ids,
                        )
                        .await;
                    ctx.plugin_contributions.extend(recheck.traces);
                    let still_omitted = include_omitted_repair
                        && (recheck.omitted_visible_repair
                            || committed_player_visible_check_missing_roll(
                                &ctx.ledger.snapshot().check_results,
                                &text,
                            ));
                    let still_echo =
                        include_echo_repair && narration_is_machine_context_echo(&text);
                    if !recheck.gate.is_block() && !still_omitted && !still_echo {
                        ctx.presentation_gate = recheck.gate;
                        ctx.visible_text = text;
                        return;
                    }
                    ctx.presentation_gate = recheck.gate;
                }
            }
        }
        let regenerated = self
            .run_narrator_buffered(&narration, &private_tokens, tx, cancel)
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
                let still_omitted = include_omitted_repair
                    && (recheck.omitted_visible_repair
                        || committed_player_visible_check_missing_roll(
                            &ctx.ledger.snapshot().check_results,
                            &text,
                        ));
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
            let forced = self
                .run_narrator_buffered(&forced_packet, &private_tokens, tx, cancel)
                .await;
            ctx.visible_text = match forced {
                Some(t)
                    if !t.trim().is_empty()
                        && !narration_is_machine_context_echo(&t)
                        && !committed_player_visible_check_missing_roll(
                            &ctx.ledger.snapshot().check_results,
                            &t,
                        ) =>
                {
                    t
                }
                // 极端：连强制重述都失败/仍是机器回显 ⇒ 一句中性的「故事继续」散文，绝不落
                // 机械账本模板,绝不出现「（机械结果）」。这是 LLM 彻底失败时的 infra fail-soft。
                _ if ctx.ledger.snapshot().check_results.is_empty() => {
                    "你定了定神，眼前的局势仍在推进，你需要决定下一步怎么做。".to_string()
                }
                _ => deterministic_committed_facts_narration(&narration),
            };
            let terminal_recheck = self
                .verify_after_stream(
                    input.request,
                    &ctx.ledger,
                    &ctx.visible_text,
                    &input.state.active_npc_ids,
                )
                .await;
            ctx.plugin_contributions.extend(terminal_recheck.traces);
            ctx.presentation_gate = terminal_recheck.gate;
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
        // L1.1 SPINE: at this post-adjudication seam the committed check_result/effects are
        // finally available (the inversion fix — the pre-adjudication Director at the
        // ContextAssembly phase could NOT see them). Flag-gated capture (default OFF ⇒ no-op ⇒
        // byte-identical baseline): project the committed results onto ctx for the Beat Director.
        // The World candidate pool was already retained pre-adjudication in
        // `build_npc_behavior_guidance` (also flag-gated).
        let post_adjudication = director_post_adjudication_enabled();
        ctx.capture_post_adjudication(post_adjudication);
        // L1.2 SPINE: invoke the Beat Director POST-adjudication so its plan reflects the REAL
        // committed result (fail-forward on failure, escalate on success). The typed plan is
        // stashed on ctx and delivered to narration via the L6.1 carrier — NOT the pre-adjudication
        // BP3 tail (skipped under this flag, no double-build). OFF ⇒ this block never runs ⇒
        // byte-identical baseline. Fail-soft: a None plan leaves ctx unchanged.
        if post_adjudication {
            let candidates = ctx.world_candidates().to_vec();
            let results = ctx.post_adjudication_results().to_vec();
            let acting_actor_id = request
                .viewer
                .actor_id
                .clone()
                .unwrap_or_else(|| "pc.current".to_string());
            let plan = crate::ports::DirectorAdapter
                .prepare_plan_post_adjudication(
                    &self.engine,
                    &request.session_id,
                    &candidates,
                    &acting_actor_id,
                    &results,
                )
                .await;
            // L1.3 SPINE observability: emit the post-adjudication plan summary so a live turn
            // can PROVE the beat reflects the committed result (target `director_spine`, only
            // under the flag ⇒ never fires OFF ⇒ baseline unchanged). The committed disposition is
            // summarized from the projected results (the very signal the overlay keyed on).
            if let Some(p) = plan.as_ref() {
                let committed: Vec<String> = results
                    .iter()
                    .map(|r| format!("{}:{:?}", r.check_id, r.outcome))
                    .collect();
                tracing::info!(
                    target: "director_spine",
                    turn_id = %request.turn_id,
                    beat_kind = ?p.beat_kind,
                    desired_change = %p.desired_change,
                    committed_results = ?committed,
                    "post-adjudication DirectorPlan built (spine: beat reflects committed result)"
                );
            }
            ctx.post_adjudication_plan = plan;
        }
        // L4.3: derive the current scene's PROACTIVE forbidden-reveal set (still-building threads'
        // facts) and stash it for the narrator phase to compose into NarrationPacket.forbidden_reveals
        // ALONGSIDE the carriers. Flag-gated by `TRPG_DIRECTOR_SCENE_PLAN` (the helper returns empty
        // when OFF ⇒ ctx field stays empty ⇒ the narrator composes an empty set ⇒ byte-identical
        // baseline). Runs here (before the Narrator phase, execute.rs:242→251) so the set is ready;
        // fail-soft (never errors) — the reactive gate stays the backstop.
        ctx.scene_forbidden_reveals = trpg_runtime::scene_forbidden_reveals(
            &self.engine.db,
            &request.session_id,
            request.module_id.as_deref(),
        )
        .await;
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
        let mut projected_reveals = nominations.clone();
        if allow {
            for fact_id in self
                .engine
                .db
                .list_player_learned_fact_ids_for_turn(&request.session_id, &request.turn_id)
                .await
                .unwrap_or_default()
            {
                if !projected_reveals.iter().any(|n| n.fact_id == fact_id) {
                    projected_reveals.push(crate::tools::RevealNomination {
                        fact_id,
                        reason: None,
                    });
                }
            }
        }
        if allow {
            self.fold_source_backed_nominations_for_module(
                request
                    .module_id
                    .as_deref()
                    .or(ctx.state_agent.module_id.as_deref()),
                &projected_reveals,
                &mut ctx.resolved_gate_facts,
            )
            .await;
            let player_visible_facts =
                player_visible_fact_projection(ctx.ledger.snapshot(), &ctx.resolved_gate_facts);
            if let Some(repaired) = self
                .rewrite_missing_facts_for_output_language(&ctx.visible_text, &player_visible_facts)
                .await
            {
                ctx.visible_text = repaired;
                ctx.presentation_gate = crate::presentation_gate::PresentationGate::Allow;
                ctx.plugin_contributions
                    .push(presentation_gate_trace(&ctx.presentation_gate));
            }
            if let Some(repaired) = append_missing_resolved_gate_facts_to_visible(
                &ctx.visible_text,
                &player_visible_facts,
            ) {
                ctx.visible_text = repaired;
                ctx.presentation_gate = crate::presentation_gate::PresentationGate::Allow;
                ctx.plugin_contributions
                    .push(presentation_gate_trace(&ctx.presentation_gate));
            }
        }
        // M1: the fact_ids revealed (made player-known) THIS turn are exactly the newly-known facts
        // that floor a matching Dormant story thread `Dormant → Introduced` (a reveal IS the
        // PlayerLearnedFact edge `apply_thread_opened` keys on). Snapshot the ids BEFORE the move,
        // and only on a committed (Allow) turn — a Blocked reveal never becomes player-known, so it
        // floors nothing (same gate discipline as the reveal commit itself).
        let newly_known_fact_ids: Vec<String> = if allow {
            projected_reveals
                .iter()
                .map(|n| n.fact_id.clone())
                .collect()
        } else {
            Vec::new()
        };
        self.commit_nominated_reveals(request, nominations, allow)
            .await;
        // (3) P6 revision (§二十四-#13)：drain 本回合玩家拒绝提名 → commit_story_writes 持久化。
        // 这是 commit_story_writes 的真正 per-turn 生产调用方（不再 dead-by-tests）。终审 Allow 时
        // 才提交（Block 回合的整段叙事被拦，拒绝信号一并丢弃，与 reveal 同口径）。M1 起同时把本回合
        // 真实揭示的 fact ids 接进去，关闭 `&[]` 占位（StoryThreadOpened floor 终于由真实揭示触发）。
        let rejections = std::mem::take(&mut ctx.rejected_nominations);
        self.commit_story_rejections(request, rejections, &newly_known_fact_ids, allow)
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
        newly_known_fact_ids: &[String],
        allow: bool,
    ) {
        // Nothing to commit when blocked, or when neither a rejection nor a newly-known fact landed
        // (avoids a needless story_state load on a quiet turn). The flag gate still lives inside
        // `commit_story_writes` (OFF ⇒ Ok no-op), so OFF stays byte-identical baseline regardless.
        if !allow || (rejections.is_empty() && newly_known_fact_ids.is_empty()) {
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
            newly_known_fact_ids,
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
        resolved_gate_facts: &mut Vec<String>,
        gating_on: bool,
    ) {
        // CL-2(b) DP-C: this is the clue-affordance path — reached only with a module
        // loaded below, i.e. a **module-bound** session. Use the module-bound-aware mode
        // so an unset env defaults to Enforce for module play (Q4 DP-C), while an explicit
        // env value still wins in both directions (non-module/unset stays byte-baseline:
        // the function early-returns when no module is bound). gating闸 (reveal-gating)
        // remains an independent precondition — both闸 must be ON for a player-visible reveal.
        let mode = trpg_model::MaterializationAffordanceMode::from_env_for_session(true);
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
        let mut graph = match self.engine.db.load_module_graph(&module_id).await {
            Ok(Some(g)) => g,
            _ => return, // 无图谱 → fail-closed（线索能供不揭）。
        };
        // CL-1: clue→scene projection (flag-gated, default OFF == byte-baseline). When ON,
        // project each module clue onto the scene whose [page_start, page_end] contains its
        // page, populating `referenced_clue_ids` so `clue_reveal_candidates` has clues to
        // resolve. Fail-closed (no page / out-of-range / ambiguous → skipped). The persisted
        // graph is never mutated — this only augments the in-memory copy for this turn.
        if trpg_runtime::clue_projection_enabled() {
            let _ = trpg_runtime::project_clues_onto_scenes(&mut graph);
        }
        let Some(scene) = graph.scenes.iter().find(|s| s.node_id == scene_id) else {
            return;
        };
        let known_fact_ids = self
            .engine
            .db
            .list_revealed_facts(&input.request.session_id)
            .await
            .unwrap_or_default();
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
            for cand in trpg_runtime::clue_reveal_candidates_with_known(
                &resolved,
                scene,
                &graph,
                mode,
                gating_on,
                &known_fact_ids,
            ) {
                candidates.push(crate::tools::RevealNomination {
                    fact_id: cand.fact_id,
                    reason: Some(cand.reason),
                });
            }
        }
        // 与既有提名 + 候选间去重（同 fact_id 只提名一次；保留首次 reason）。
        for cand in candidates {
            if !nominated_reveals.iter().any(|n| n.fact_id == cand.fact_id) {
                if let Some(fact) = Self::source_backed_clue_projection_fact(&graph, &cand.fact_id)
                {
                    if !resolved_gate_facts.iter().any(|existing| existing == &fact) {
                        resolved_gate_facts.push(fact);
                    }
                }
                nominated_reveals.push(cand);
            }
        }
    }

    async fn rewrite_missing_facts_for_output_language(
        &self,
        visible_text: &str,
        facts: &[String],
    ) -> Option<String> {
        let output_language = crate::gm_craft::output_language_from_env()?;
        let missing =
            missing_rewrite_facts_for_output_language(visible_text, facts, &output_language);
        if missing.is_empty() {
            return None;
        }

        let system = format!(
            "You rewrite TRPG player-visible narration without changing adjudication.\n\
             {}\n\
             Hard constraints: preserve every [roll]...[/roll] block verbatim; do not add checks, \
             damage, resources, NPC actions, hidden information, or action menus; weave the \
             confirmed facts into natural second-person narration instead of a numbered or bullet \
             list.",
            crate::gm_craft::output_language_contract(Some(&output_language))
        );
        let user = format!(
            "Existing player-visible text:\n{visible_text}\n\n\
             Confirmed player-known facts that must be visible in the same turn:\n{}\n\n\
             Rewrite the text so those confirmed facts are present in the configured output \
             language. Return only the final player-visible text.",
            missing
                .iter()
                .map(|fact| format!("- {fact}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let original_rolls = auditable_roll_block_count(visible_text);
        let repaired = self
            .llm
            .complete_text(
                vec![
                    ChatMessage {
                        role: "system".to_string(),
                        content: system,
                    },
                    ChatMessage {
                        role: "user".to_string(),
                        content: user,
                    },
                ],
                0.1,
            )
            .await
            .ok()?;
        let repaired = repaired.trim();
        if repaired.is_empty()
            || auditable_roll_block_count(repaired) < original_rolls
            || player_agency_menu_cue_needs_block(repaired)
            || (crate::gm_craft::output_language_is_chinese(Some(&output_language))
                && !contains_cjk(repaired))
        {
            return None;
        }
        Some(repaired.to_string())
    }

    async fn rewrite_roll_only_success_for_visible_information(
        &self,
        visible_text: &str,
        player_input: &str,
        scene_establishing: &[String],
    ) -> Option<String> {
        if !crate::gm_craft::enabled() && crate::gm_craft::output_language_from_env().is_none() {
            return None;
        }
        let output_language = crate::gm_craft::output_language_from_env();
        let language_contract =
            crate::gm_craft::output_language_contract(output_language.as_deref());
        let scene_context = if scene_establishing.is_empty() {
            "No additional player-safe scene text is available. Do not invent hidden clues; state only what the successful action makes safely visible or what limits remain."
                .to_string()
        } else {
            scene_establishing
                .iter()
                .take(3)
                .map(|line| line.trim())
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        };
        let system = format!(
            "You repair a TRPG turn whose successful check was rendered as a roll-only ledger.\n\
             {}\n\
             Hard constraints: preserve every [roll]...[/roll] block verbatim; do not add checks, \
             damage, resources, hidden Keeper information, secret names, or action menus. Use only \
             the public roll, the player's declared action, and the player-safe scene context. Add \
             concrete player-visible consequence: what the character can now see/hear/confirm, what \
             was ruled out in the visible area, or what their current position/status is. If the \
             context does not support a secret clue, do not invent one.",
            language_contract
        );
        let user = format!(
            "Current roll-only player-visible text:\n{visible_text}\n\n\
             Player action:\n{player_input}\n\n\
             Player-safe scene context:\n{scene_context}\n\n\
             Rewrite into final player-visible narration. Return only the final narration."
        );
        let original_rolls = auditable_roll_block_count(visible_text);
        let repaired = self
            .llm
            .complete_text(
                vec![
                    ChatMessage {
                        role: "system".to_string(),
                        content: system,
                    },
                    ChatMessage {
                        role: "user".to_string(),
                        content: user,
                    },
                ],
                0.1,
            )
            .await
            .ok()?;
        let repaired = repaired.trim();
        if repaired.is_empty()
            || visible_is_roll_only_committed_summary(repaired)
            || auditable_roll_block_count(repaired) < original_rolls
            || player_agency_menu_cue_needs_block(repaired)
            || narration_is_machine_context_echo(repaired)
            || (output_language
                .as_deref()
                .map(|lang| crate::gm_craft::output_language_is_chinese(Some(lang)))
                .unwrap_or(false)
                && !contains_cjk(repaired))
        {
            return None;
        }
        Some(repaired.to_string())
    }

    pub(crate) async fn ensure_scene_transition_visible_information_after_repair(
        &self,
        ctx: &mut TurnContext,
        info: &SceneTransitionInfo,
    ) {
        if !scene_transition_generic_success_needs_rewrite(&ctx.visible_text) {
            return;
        }
        let Some(source_text) = info
            .to_read_aloud
            .as_deref()
            .or(info.to_summary.as_deref())
            .map(str::trim)
            .filter(|text| !text.is_empty())
        else {
            return;
        };
        let Some(output_language) = crate::gm_craft::output_language_from_env() else {
            return;
        };
        let system = format!(
            "You repair a TRPG scene-transition narration that became too generic.\n\
             {}\n\
             Hard constraints: preserve every [roll]...[/roll] block verbatim; do not add checks, \
             damage, resources, hidden Keeper information, secret names, or action menus. Use only \
             the current visible text plus the player-safe destination scene text. Turn the \
             destination text into natural second-person narration in the configured language; do \
             not dump it as a list.",
            crate::gm_craft::output_language_contract(Some(&output_language))
        );
        let destination = info
            .to_title
            .as_deref()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or(info.to.as_str());
        let user = format!(
            "Current player-visible text:\n{}\n\n\
             Destination scene: {destination}\n\
             Player-safe destination scene text:\n{source_text}\n\n\
             Rewrite into final player-visible narration. Return only the final narration.",
            ctx.visible_text
        );
        let original_rolls = auditable_roll_block_count(&ctx.visible_text);
        let repaired = match self
            .llm
            .complete_text(
                vec![
                    ChatMessage {
                        role: "system".to_string(),
                        content: system,
                    },
                    ChatMessage {
                        role: "user".to_string(),
                        content: user,
                    },
                ],
                0.1,
            )
            .await
        {
            Ok(text) => text,
            Err(_) => return,
        };
        let repaired = repaired.trim();
        if repaired.is_empty()
            || auditable_roll_block_count(repaired) < original_rolls
            || player_agency_menu_cue_needs_block(repaired)
            || (crate::gm_craft::output_language_is_chinese(Some(&output_language))
                && !contains_cjk(repaired))
            || scene_transition_generic_success_needs_rewrite(repaired)
        {
            return;
        }
        ctx.visible_text = repaired.to_string();
        ctx.presentation_gate = crate::presentation_gate::PresentationGate::Allow;
        ctx.plugin_contributions
            .push(presentation_gate_trace(&ctx.presentation_gate));
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

    async fn fold_source_backed_nominations_for_module(
        &self,
        module_id: Option<&str>,
        nominations: &[crate::tools::RevealNomination],
        resolved_gate_facts: &mut Vec<String>,
    ) {
        if nominations.is_empty() {
            return;
        }
        let Some(module_id) = module_id else {
            return;
        };
        let graph = match self.engine.db.load_module_graph(module_id).await {
            Ok(Some(graph)) => graph,
            _ => return,
        };
        Self::fold_source_backed_nominated_reveals(&graph, nominations, resolved_gate_facts);
    }

    pub(crate) fn fold_source_backed_nominated_reveals(
        graph: &trpg_model::ModuleGraph,
        nominations: &[crate::tools::RevealNomination],
        resolved_gate_facts: &mut Vec<String>,
    ) {
        for nomination in nominations {
            if let Some(fact) = Self::source_backed_clue_projection_fact(graph, &nomination.fact_id)
            {
                if !resolved_gate_facts.iter().any(|existing| existing == &fact) {
                    resolved_gate_facts.push(fact);
                }
            }
        }
    }

    fn clue_json_id(clue: &Value) -> Option<&str> {
        clue.get("id")
            .or_else(|| clue.get("clue_id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }

    fn clue_json_text(clue: &Value) -> Option<&str> {
        ["text", "clue", "discovery", "meaning", "summary"]
            .iter()
            .find_map(|key| {
                clue.get(*key)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
            })
    }

    pub(crate) fn source_backed_clue_gate_fact(
        graph: &trpg_model::ModuleGraph,
        fact_id: &str,
    ) -> Option<String> {
        let fact_id = fact_id.trim();
        if fact_id.is_empty() {
            return None;
        }
        let clue = graph
            .clues
            .iter()
            .find(|clue| Self::clue_json_id(clue) == Some(fact_id))?;
        let text = Self::clue_json_text(clue)?;
        Some(text.to_string())
    }

    fn source_backed_clue_projection_fact(
        graph: &trpg_model::ModuleGraph,
        fact_id: &str,
    ) -> Option<String> {
        let text = Self::source_backed_clue_gate_fact(graph, fact_id)?;
        Some(format!("已揭示的模组线索 {fact_id}: {text}"))
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

    /// PresentationGate hard delivery path: when the final gate is Block and the
    /// transport has held player-visible prose, replace the blocked prose with a
    /// neutral, non-menu fallback before save/TurnComplete. The gate itself stays
    /// Block so traces, warnings, and reveal-commit policy remain auditable.
    pub(crate) fn apply_presentation_gate_safe_fallback(
        &self,
        ctx: &mut TurnContext,
        player_input: &str,
    ) {
        if !ctx.presentation_gate.is_block() {
            return;
        }
        let snapshot = ctx.ledger.snapshot();
        let adj = crate::packet::AdjudicationPacket::project(
            player_input,
            &snapshot,
            &ctx.resolved_gate_facts,
            &ctx.visible_text,
            None,
        );
        let narration = crate::packet::NarrationPacket::project(&adj, "", &[]);
        let committed = deterministic_committed_facts_narration(&narration);
        ctx.visible_text = if !narration.what_happened.is_empty()
            || !narration.what_changed.is_empty()
            || !narration.player_perceivable_facts.is_empty()
        {
            committed
        } else {
            presentation_gate_safe_fallback_text(player_input)
        };
        // The blocked draft must not commit any player-knowledge/story side effects,
        // but the final delivered fallback is safe, deterministic, and auditable.
        ctx.nominated_reveals.clear();
        ctx.rejected_nominations.clear();
        ctx.presentation_gate = crate::presentation_gate::PresentationGate::Allow;
        ctx.plugin_contributions
            .push(presentation_gate_trace(&ctx.presentation_gate));
    }

    /// Final post-repair guard: deterministic cleanup steps may run after the
    /// PresentationGate repair ladder and accidentally drop a committed,
    /// player-visible check from the final text. Re-project committed facts at
    /// the end of the cleanup chain so the saved/streamed narration remains
    /// auditable.
    pub(crate) async fn ensure_committed_visible_projection_after_repairs(
        &self,
        ctx: &mut TurnContext,
        player_input: &str,
    ) {
        let snapshot = ctx.ledger.snapshot();
        let player_visible_facts =
            player_visible_fact_projection(snapshot, &ctx.resolved_gate_facts);
        let missing_roll =
            committed_player_visible_check_missing_roll(&snapshot.check_results, &ctx.visible_text);
        let missing_resolved_facts =
            resolved_gate_facts_missing_from_visible(&player_visible_facts, &ctx.visible_text);
        let roll_only_success_needs_rewrite =
            roll_only_success_summary_needs_visible_information(snapshot, &ctx.visible_text);
        if !missing_roll && !missing_resolved_facts && !roll_only_success_needs_rewrite {
            return;
        }
        if roll_only_success_needs_rewrite && !missing_resolved_facts {
            if let Some(repaired) = self
                .rewrite_roll_only_success_for_visible_information(
                    &ctx.visible_text,
                    player_input,
                    &ctx.compiled.scene_establishing,
                )
                .await
            {
                ctx.visible_text = repaired;
                ctx.presentation_gate = crate::presentation_gate::PresentationGate::Allow;
                ctx.plugin_contributions
                    .push(presentation_gate_trace(&ctx.presentation_gate));
            }
            if visible_is_roll_only_committed_summary(&ctx.visible_text) {
                if let Some(repaired) =
                    deterministic_roll_only_success_visible_fallback(&ctx.visible_text)
                {
                    ctx.visible_text = repaired;
                    ctx.presentation_gate = crate::presentation_gate::PresentationGate::Allow;
                    ctx.plugin_contributions
                        .push(presentation_gate_trace(&ctx.presentation_gate));
                }
            }
            if !visible_is_roll_only_committed_summary(&ctx.visible_text) {
                return;
            }
        }
        if !missing_roll && missing_resolved_facts {
            if let Some(repaired) = self
                .rewrite_missing_facts_for_output_language(&ctx.visible_text, &player_visible_facts)
                .await
            {
                ctx.visible_text = repaired;
                ctx.presentation_gate = crate::presentation_gate::PresentationGate::Allow;
                ctx.plugin_contributions
                    .push(presentation_gate_trace(&ctx.presentation_gate));
                return;
            }
            if let Some(repaired) = append_missing_resolved_gate_facts_to_visible(
                &ctx.visible_text,
                &player_visible_facts,
            ) {
                ctx.visible_text = repaired;
                ctx.presentation_gate = crate::presentation_gate::PresentationGate::Allow;
                ctx.plugin_contributions
                    .push(presentation_gate_trace(&ctx.presentation_gate));
                return;
            }
            if output_language_rewrite_required_for_missing_facts(
                &ctx.visible_text,
                &player_visible_facts,
            ) {
                return;
            }
        }
        let adj = crate::packet::AdjudicationPacket::project(
            player_input,
            snapshot,
            &player_visible_facts,
            &ctx.visible_text,
            None,
        );
        let narration = crate::packet::NarrationPacket::project(&adj, "", &[]);
        if narration.what_happened.is_empty()
            && narration.what_changed.is_empty()
            && narration.player_perceivable_facts.is_empty()
        {
            return;
        }
        ctx.visible_text = deterministic_committed_facts_narration(&narration);
        if visible_is_roll_only_committed_summary(&ctx.visible_text) {
            if let Some(repaired) =
                deterministic_roll_only_success_visible_fallback(&ctx.visible_text)
            {
                ctx.visible_text = repaired;
            }
        }
        ctx.presentation_gate = crate::presentation_gate::PresentationGate::Allow;
        ctx.plugin_contributions
            .push(presentation_gate_trace(&ctx.presentation_gate));
    }

    pub(crate) async fn ensure_concrete_information_request_answered_after_repairs(
        &self,
        ctx: &mut TurnContext,
        player_input: &str,
    ) {
        if !player_input_requests_concrete_visible_information(player_input)
            || ctx.visible_text.trim().is_empty()
        {
            return;
        }
        let scene_context = if ctx.compiled.scene_establishing.is_empty() {
            "No additional player-safe scene context is available. Do not invent hidden Keeper information; if a requested item is not visible, say that it is not visible or cannot be confirmed from the current position."
                .to_string()
        } else {
            ctx.compiled
                .scene_establishing
                .iter()
                .take(3)
                .map(|line| line.trim())
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        };
        let output_language = crate::gm_craft::output_language_from_env();
        let language_contract =
            crate::gm_craft::output_language_contract(output_language.as_deref());
        let audit_system = format!(
            "You are an evidence-based TRPG response-contract auditor.\n\
             {}\n\
             Decide whether the GM's player-visible response satisfies the player's concrete request for visible information. \
             A response is sufficient only if it gives concrete visible facts, explicitly says a requested item is not visible/unknown from the current position, asks for clarification, requests a check, or creates a tracked pending item. \
             Continuity contract: the response must not contradict player-stated facts in the current request or already player-visible facts, including notes, photos, measurements, positions, tools used, and facts the player explicitly says they are carrying forward. \
             Measurement contract: if the player used an appropriate measuring tool or the GM says a measurement was completed, the response must provide actual or approximate readings, or explain the concrete physical obstruction/check failure that prevents those readings. It is insufficient to acknowledge the measurement and then say the readings are not present in the record. \
             It is insufficient if it merely says there are several things/positions/clues without naming them. \
             Return JSON only with keys: needs_repair:boolean, reason:string, missing_items:string[].",
            language_contract
        );
        let audit_user = format!(
            "Player request:\n{player_input}\n\n\
             GM response:\n{}\n\n\
             Player-safe scene context:\n{scene_context}",
            ctx.visible_text
        );
        let Ok(audit) = self
            .llm
            .complete_json(
                vec![
                    ChatMessage {
                        role: "system".to_string(),
                        content: audit_system,
                    },
                    ChatMessage {
                        role: "user".to_string(),
                        content: audit_user,
                    },
                ],
                0.0,
            )
            .await
        else {
            return;
        };
        let (needs_repair, missing_items) = concrete_information_audit_needs_repair(&audit);
        if !needs_repair {
            return;
        }
        let original_rolls = auditable_roll_block_count(&ctx.visible_text);
        let missing = if missing_items.is_empty() {
            "The response did not concretely answer the player's requested visible information."
                .to_string()
        } else {
            missing_items.join("; ")
        };
        let rewrite_system = format!(
            "You repair a TRPG GM response that failed a response-contract audit.\n\
             {}\n\
             Hard constraints: preserve every existing [roll]...[/roll] block verbatim; do not add checks, damage, resources, hidden Keeper information, secret names, or action menus. \
             Use only the player's request, the current response, and the player-safe scene context. \
             Keep player-stated and already player-visible facts continuous; do not rewrite them into uncertainty or deny that the player has notes, photos, measurements, positions, or tools they explicitly carried forward. \
             For each missing requested item, either provide a concrete player-visible fact supported by the current scene context or explicitly say that it is not visible/cannot be confirmed from the current position. \
             For measurements, give actual or approximate readings if the current response says the measuring action happened; otherwise explain the concrete obstruction/check failure that made the readings unavailable. \
             Return only the final player-visible narration.",
            language_contract
        );
        let rewrite_user = format!(
            "Player request:\n{player_input}\n\n\
             Current GM response:\n{}\n\n\
             Missing requested items:\n{missing}\n\n\
             Player-safe scene context:\n{scene_context}",
            ctx.visible_text
        );
        let Ok(repaired) = self
            .llm
            .complete_text(
                vec![
                    ChatMessage {
                        role: "system".to_string(),
                        content: rewrite_system,
                    },
                    ChatMessage {
                        role: "user".to_string(),
                        content: rewrite_user,
                    },
                ],
                0.1,
            )
            .await
        else {
            return;
        };
        let repaired = repaired.trim();
        if repaired.is_empty()
            || player_agency_menu_cue_needs_block(repaired)
            || narration_is_machine_context_echo(repaired)
            || auditable_roll_block_count(repaired) < original_rolls
            || (output_language
                .as_deref()
                .map(|lang| crate::gm_craft::output_language_is_chinese(Some(lang)))
                .unwrap_or(false)
                && !contains_cjk(repaired))
        {
            return;
        }
        ctx.visible_text = repaired.to_string();
        ctx.presentation_gate = crate::presentation_gate::PresentationGate::Allow;
        ctx.plugin_contributions
            .push(presentation_gate_trace(&ctx.presentation_gate));
    }

    /// Emit prose that was buffered for terminal presentation-gate verification.
    /// This is intentionally a late delivery point: callers invoke it only after
    /// VerifyAfterStream and PresentationCommit have settled the final text.
    pub(crate) async fn emit_buffered_visible_text(
        &self,
        ctx: &TurnContext,
        tx: &tokio::sync::mpsc::Sender<crate::turn_event::TurnEvent>,
    ) {
        if ctx.visible_text.trim().is_empty() || ctx.awaiting_gate.is_some() {
            return;
        }
        let _ = tx
            .send(crate::turn_event::TurnEvent::Delta(
                ctx.visible_text.clone(),
            ))
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
            Ok(Some(c)) => {
                let (from_title, to_title, to_read_aloud, to_summary, to_aliases) = self
                    .scene_transition_player_context(module_id, &c.from, &c.to)
                    .await;
                // L4.2 — on a COMMITTED scene change, derive the scene's ScenePlan from the live
                // story threads (+ module director config) and emit exactly one ScenePlanCreated
                // ledger event. Flag-gated (`TRPG_DIRECTOR_SCENE_PLAN`, default OFF ⇒ immediate
                // no-op, byte-identical baseline); additive + fail-soft (never reverses the switch).
                trpg_runtime::scene_plan_emit::emit_scene_plan_on_change(
                    &self.engine.db,
                    &input.request.session_id,
                    module_id,
                    &input.request.turn_id,
                    &c.to,
                )
                .await;
                Some(SceneTransitionInfo {
                    from: c.from,
                    to: c.to,
                    reason: c.reason,
                    from_title,
                    to_title,
                    to_read_aloud,
                    to_summary,
                    to_aliases,
                })
            }
            Ok(None) => None,
            Err(err) => {
                tracing::warn!(error = %err, "agent path scene_navigate_critical failed");
                None
            }
        }
    }

    async fn scene_transition_player_context(
        &self,
        module_id: &str,
        from: &str,
        to: &str,
    ) -> (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Vec<String>,
    ) {
        let Ok(Some(graph)) = self.engine.db.load_module_graph(module_id).await else {
            return (None, None, None, None, Vec::new());
        };
        let from_title = graph
            .scenes
            .iter()
            .find(|s| s.node_id == from)
            .map(|s| s.title.clone())
            .filter(|s| !s.trim().is_empty());
        let Some(to_scene) = graph.scenes.iter().find(|s| s.node_id == to) else {
            return (from_title, None, None, None, Vec::new());
        };
        let mut aliases = Vec::new();
        push_alias(&mut aliases, &to_scene.node_id);
        push_alias(&mut aliases, &to_scene.title);
        for loc in &to_scene.referenced_location_ids {
            push_alias(&mut aliases, loc);
        }
        (
            from_title,
            Some(to_scene.title.clone()).filter(|s| !s.trim().is_empty()),
            to_scene.read_aloud.clone().filter(|s| !s.trim().is_empty()),
            Some(to_scene.summary.clone()).filter(|s| !s.trim().is_empty()),
            aliases,
        )
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

/// M3 决策#3:Narrator 故事情绪 carrier 的 opt-in 开关。**默认 OFF**(零 telegraph 风险;carrier
/// 留着但空)——仅显式 `1`/`true`/`on`/`yes` 开。OFF ⇒ `with_story_mood` 收空 ⇒ 字节等价基线。
pub(crate) fn narrator_story_mood_enabled() -> bool {
    matches!(
        std::env::var("TRPG_NARRATOR_STORY_MOOD")
            .ok()
            .map(|v| v.to_ascii_lowercase())
            .as_deref(),
        Some("1") | Some("true") | Some("on") | Some("yes")
    )
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
    let establishing = packet.scene_establishing.join("\n");
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
         连续性忠实：从“当前场景”和本回合机械事实确立的**当前所在位置**继续；不要重述场景的到达/入口定场，玩家已深入内部或离开入口时绝不把镜头拉回入口、外景或旧开场图景。\n\
         既成状态忠实：已由机械事实、玩家可感知信息或当前场景确立的状态必须保持，不复活、不反转；例如供电已切断、灯已熄、设备已停，就不要再写同一设备仍通电、闪烁、嗡鸣或正常握手，除非机械事实明确给出新的独立来源。\n\
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
    // Q-MODULE DP-A'/DP-B': append the authored scene-establishing material when present
    // (runtime fills it only under materialization Enforce). Empty ⇒ zero extra bytes ⇒ OFF /
    // non-Enforce byte-equal. The Narrator must WEAVE it (named location / who-is-present /
    // atmosphere) into the second-person fiction — NEVER a verbatim box-text dump, NEVER a list
    // (①②③), NEVER a menu (respects Q-4 no-dump). Source-anchored: invent nothing beyond it.
    let user = if establishing.trim().is_empty() {
        user
    } else {
        format!(
            "{user}\n\n\
             本场景的模组进场素材(作者授权、玩家可知的场景定调；仅供你改写,绝不逐字照搬):\n{establishing}\n\
             指令:把上述素材**编织**进你这一段第二人称散文——给出具名的地点、在场的人/物、\
             此刻的氛围,让开场有真实的模组质感;**绝不**逐字倾倒原文、**绝不**列成清单(①②③)、\
             **绝不**写成选项菜单;只呈现此刻自然可感知的部分,若该场景在前文已建立过,只带入\
             尚未呈现的新要素、不要重复已叙述过的定调;不得据此发明素材之外的实体/线索/机关/结论。"
        )
    };
    // OA2 (G-3): append the player-safe PC competency profile when present (runtime fills it only
    // under materialization Enforce). Empty ⇒ zero extra bytes ⇒ OFF / non-Enforce byte-equal.
    // The Narrator may REFERENCE the PC's competencies/traits (e.g. an observant investigator
    // notices, a strong agent forces) but must NEVER recite raw numbers / list the sheet (Q-4).
    let character = packet.character_context.join("\n");
    let user = if character.trim().is_empty() {
        user
    } else {
        format!(
            "{user}\n\n\
             你的角色能力档案(玩家自知;仅供你让念白贴合 PC 的强项/弱项,绝不逐字罗列数值):\n{character}\n\
             指令:当与该回合行动相关时,可让叙述自然体现 PC 的能力倾向(如擅长观察者更易留意细节、\
             体格强者动作更具压迫感);**绝不**报数值、**绝不**罗列技能清单、**绝不**发明档案外的能力。"
        )
    };
    // M3 决策#3:append the player-safe STORY MOOD when present (runtime fills it only when
    // TRPG_NARRATOR_STORY_MOOD is ON; default OFF ⇒ carrier empty ⇒ zero extra bytes ⇒ byte-equal).
    // Player-perceived atmosphere ONLY — never plot/clue/foreshadow (zero telegraph). The Narrator
    // lets the felt tone color the prose; it must NOT state mood as fact or invent beyond it.
    let mood = packet.story_mood.join("；");
    let user = if mood.trim().is_empty() {
        user
    } else {
        format!(
            "{user}\n\n\
             此刻的情绪/氛围(玩家已感知的基调,仅供你为念白定调,绝不当作事实陈述、绝不据此发明线索):\n{mood}\n\
             指令:让上述氛围自然渗入你的第二人称散文(光影/气息/节奏/体感),**绝不**直接断言\"气氛很X\"、\
             **绝不**揭示任何未在机械事实或玩家可感知信息中出现的情节/线索/伏笔。"
        )
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
    pub from_title: Option<String>,
    pub to_title: Option<String>,
    pub to_read_aloud: Option<String>,
    pub to_summary: Option<String>,
    pub to_aliases: Vec<String>,
}

fn push_alias(aliases: &mut Vec<String>, value: &str) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return;
    }
    aliases.push(trimmed.to_string());
    if trimmed.contains('_') {
        aliases.push(trimmed.replace('_', " "));
    }
}

fn normalize_transition_text(value: &str) -> String {
    value
        .chars()
        .flat_map(|c| c.to_lowercase())
        .map(|c| {
            if c.is_alphanumeric() || ('\u{4e00}'..='\u{9fff}').contains(&c) {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
}

fn visible_mentions_transition_destination(visible_text: &str, info: &SceneTransitionInfo) -> bool {
    let hay = normalize_transition_text(visible_text);
    let mut aliases = info.to_aliases.clone();
    push_alias(&mut aliases, &info.to);
    if let Some(title) = info.to_title.as_deref() {
        push_alias(&mut aliases, title);
    }
    aliases.iter().any(|alias| {
        let needle = normalize_transition_text(alias);
        let needle = needle.trim();
        needle.chars().count() >= 4 && hay.contains(needle)
    })
}

fn visible_has_transition_stall_marker(visible_text: &str) -> bool {
    let hay = visible_text.to_lowercase();
    [
        "没有再往前动",
        "没有往前动",
        "没有任何新的记录",
        "没有任何新的线索",
        "没有新的记录",
        "没有新的线索",
        "没有新的消息",
        "仍只是刚才",
        "仍然只是刚才",
        "still where you were",
        "nothing changes",
        "nothing moves forward",
        "no new lead",
        "no new clue",
        "no new information",
    ]
    .iter()
    .any(|marker| hay.contains(marker))
}

fn player_input_requests_concrete_visible_information(player_input: &str) -> bool {
    let lower = player_input.to_ascii_lowercase();
    let asks_for_information = [
        "哪些",
        "哪个",
        "什么",
        "是否",
        "有没有",
        "看见",
        "看到",
        "确认",
        "说清楚",
        "量",
        "测量",
        "读数",
        "what",
        "which",
        "whether",
        "where",
        "visible",
        "confirm",
        "specific",
        "measure",
        "measuring",
        "measured",
        "reading",
    ]
    .iter()
    .any(|cue| lower.contains(cue));
    asks_for_information
}

fn concrete_information_audit_needs_repair(audit: &Value) -> (bool, Vec<String>) {
    let needs_repair = audit
        .get("needs_repair")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            audit
                .get("sufficient")
                .and_then(Value::as_bool)
                .map(|sufficient| !sufficient)
                .unwrap_or(false)
        });
    let missing_items = audit
        .get("missing_items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    (needs_repair, missing_items)
}

fn strip_roll_blocks_for_transition_repair(visible_text: &str) -> String {
    let mut out = String::with_capacity(visible_text.len());
    let mut rest = visible_text;
    loop {
        let lower = rest.to_ascii_lowercase();
        let Some(start) = lower.find("[roll]") else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        let after_start = start + "[roll]".len();
        let after = &rest[after_start..];
        let after_lower = after.to_ascii_lowercase();
        let Some(end) = after_lower.find("[/roll]") else {
            out.push_str(&rest[start..]);
            break;
        };
        rest = &after[end + "[/roll]".len()..];
    }
    out
}

fn visible_is_roll_only_committed_summary(visible_text: &str) -> bool {
    if !visible_text.to_ascii_lowercase().contains("[roll]") {
        return false;
    }
    let stripped = strip_machine_check_confirmation_lines_for_transition_repair(
        &strip_roll_blocks_for_transition_repair(visible_text),
    )
    .replace("根据本回合已确认的结果", "")
    .replace("confirmed results", "")
    .replace("confirmed result", "");
    stripped
        .chars()
        .all(|ch| ch.is_whitespace() || matches!(ch, '·' | ':' | '：' | '-' | '—' | '.' | '。'))
}

fn roll_only_committed_summary_has_non_successful_roll(visible_text: &str) -> bool {
    if !visible_is_roll_only_committed_summary(visible_text) {
        return false;
    }
    let mut rest = visible_text;
    loop {
        let lower = rest.to_ascii_lowercase();
        let Some(start) = lower.find("[roll]") else {
            return false;
        };
        let after_start = start + "[roll]".len();
        let after = &rest[after_start..];
        let after_lower = after.to_ascii_lowercase();
        let Some(end) = after_lower.find("[/roll]") else {
            return false;
        };
        if roll_block_has_non_successful_outcome(&after[..end]) {
            return true;
        }
        rest = &after[end + "[/roll]".len()..];
    }
}

fn roll_only_success_summary_needs_visible_information(
    snapshot: &trpg_agent::TurnLedgerSnapshot,
    visible_text: &str,
) -> bool {
    if !visible_is_roll_only_committed_summary(visible_text)
        || roll_only_committed_summary_has_non_successful_roll(visible_text)
    {
        return false;
    }
    if roll_only_committed_summary_has_successful_roll(visible_text) {
        return true;
    }
    snapshot.check_results.iter().any(|result| {
        check_result_has_player_visible_roll(result)
            && check_result_outcome_indicates_success(result)
    })
}

fn deterministic_roll_only_success_visible_fallback(visible_text: &str) -> Option<String> {
    if !visible_is_roll_only_committed_summary(visible_text)
        || roll_only_committed_summary_has_non_successful_roll(visible_text)
        || !roll_only_committed_summary_has_successful_roll(visible_text)
    {
        return None;
    }
    let labels = roll_block_labels(visible_text);
    let mut out = visible_text.trim().to_string();
    out.push_str("\n\n");
    if labels.is_empty() {
        out.push_str(
            "这次成功检查已经落实为玩家可见状态：你已经完成本回合声明的近处检查；当前位置、\
             退路和被直接检查到的范围仍受控。未被照到、未被触碰或更深处的内容仍保持未知。",
        );
    } else {
        out.push_str("这次成功检查已经落实为玩家可见状态：你已经完成");
        out.push_str(&labels.join("；"));
        out.push_str(
            "。这些结果只确认当前行动直接覆盖到的可见或可听范围；当前位置、退路和近处环境\
             仍受控。未被照到、未被触碰或更深处的内容仍保持未知。",
        );
    }
    if player_agency_menu_cue_needs_block(&out) {
        return None;
    }
    Some(out)
}

fn roll_block_labels(visible_text: &str) -> Vec<String> {
    let mut labels = Vec::new();
    let mut rest = visible_text;
    loop {
        let lower = rest.to_ascii_lowercase();
        let Some(start) = lower.find("[roll]") else {
            return labels;
        };
        let after_start = start + "[roll]".len();
        let after = &rest[after_start..];
        let after_lower = after.to_ascii_lowercase();
        let Some(end) = after_lower.find("[/roll]") else {
            return labels;
        };
        let block = after[..end].trim();
        let label = block
            .split_once(':')
            .map(|(head, _)| head)
            .or_else(|| block.split_once('：').map(|(head, _)| head))
            .unwrap_or(block)
            .trim()
            .trim_start_matches('·')
            .trim();
        if !label.is_empty() && !labels.iter().any(|existing| existing == label) {
            labels.push(label.to_string());
        }
        rest = &after[end + "[/roll]".len()..];
    }
}

fn scene_transition_generic_success_needs_rewrite(visible_text: &str) -> bool {
    let lower = visible_text.to_ascii_lowercase();
    if !lower.contains("[roll]")
        || roll_only_committed_summary_has_non_successful_roll(visible_text)
    {
        return false;
    }
    [
        "推进到新的可见区域",
        "你现在的位置已经改变",
        "新的位置已经成为玩家可见事实",
        "新的位置已经是玩家可见事实",
    ]
    .iter()
    .any(|marker| visible_text.contains(marker))
}

fn check_result_has_player_visible_roll(result: &trpg_model::CheckResultRecord) -> bool {
    !matches!(result.roll.visibility, RollVisibility::PrivateGmRoll)
        && (matches!(
            result.roll.visibility,
            RollVisibility::PublicGmRoll
                | RollVisibility::PlayerRollRequired
                | RollVisibility::PassiveResolution
        ) || !result.roll.result.is_null())
}

fn check_result_outcome_indicates_success(result: &trpg_model::CheckResultRecord) -> bool {
    if let Some(success) = result.outcome.get("success").and_then(|v| v.as_bool()) {
        return success;
    }
    for key in ["degree", "success_tier", "band", "outcome", "result"] {
        if let Some(value) = result.outcome.get(key).and_then(|v| v.as_str()) {
            return outcome_token_is_success(value);
        }
    }
    result
        .outcome
        .as_str()
        .map(outcome_token_is_success)
        .unwrap_or(false)
}

fn outcome_token_is_success(token: &str) -> bool {
    let normalized = token.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return false;
    }
    if outcome_token_is_failure(&normalized) {
        return false;
    }
    normalized.contains("success")
        || matches!(
            normalized.as_str(),
            "regular" | "normal" | "hard" | "extreme" | "critical"
        )
        || token.contains('成') && token.contains('功')
}

fn outcome_token_is_failure(normalized: &str) -> bool {
    normalized.contains("fail")
        || normalized.contains("failure")
        || normalized.contains("fumble")
        || normalized.contains("botch")
        || normalized.contains("blocked")
        || normalized.contains("pending")
        || normalized.contains("unresolved")
        || normalized.contains("失败")
}

fn roll_only_committed_summary_has_successful_roll(visible_text: &str) -> bool {
    let mut rest = visible_text;
    loop {
        let lower = rest.to_ascii_lowercase();
        let Some(start) = lower.find("[roll]") else {
            return false;
        };
        let after_start = start + "[roll]".len();
        let after = &rest[after_start..];
        let after_lower = after.to_ascii_lowercase();
        let Some(end) = after_lower.find("[/roll]") else {
            return false;
        };
        if roll_block_has_successful_outcome(&after[..end]) {
            return true;
        }
        rest = &after[end + "[/roll]".len()..];
    }
}

fn roll_block_has_successful_outcome(block: &str) -> bool {
    let normalized = block
        .to_ascii_lowercase()
        .replace('：', ":")
        .replace('，', ",");
    !roll_block_has_non_successful_outcome(block)
        && (normalized.contains("result:success")
            || normalized.contains("outcome:success")
            || normalized.contains("结果:success")
            || normalized.contains("result:strong_success")
            || normalized.contains("outcome:strong_success")
            || normalized.contains("结果:strong_success")
            || normalized.contains("结果:成功"))
}

fn roll_block_has_non_successful_outcome(block: &str) -> bool {
    let normalized = block
        .to_ascii_lowercase()
        .replace('：', ":")
        .replace('，', ",");
    [
        "result:strong_failure",
        "result:failure",
        "result:fail",
        "result:fumble",
        "outcome:strong_failure",
        "outcome:failure",
        "outcome:fail",
        "outcome:fumble",
        "结果:strong_failure",
        "结果:failure",
        "结果:fail",
        "结果:fumble",
        "结果:失败",
        "结果:强失败",
        "结果:大失败",
        "结果:失手",
        "— failure",
        "- failure",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
}

fn strip_machine_check_confirmation_lines_for_transition_repair(text: &str) -> String {
    text.lines()
        .filter(|line| !line_is_machine_check_confirmation(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn line_is_machine_check_confirmation(line: &str) -> bool {
    let mut trimmed = line.trim();
    while let Some(rest) = trimmed
        .strip_prefix(|ch: char| ch.is_whitespace() || matches!(ch, '·' | '*' | '•' | '-' | '—'))
    {
        trimmed = rest.trim_start();
    }
    let lowered = trimmed
        .trim_end_matches(|ch: char| ch.is_whitespace() || matches!(ch, '.' | '。'))
        .to_ascii_lowercase();
    if lowered.is_empty() {
        return false;
    }
    (lowered.starts_with("the ") || lowered.starts_with("check "))
        && (lowered.ends_with(" check succeeds") || lowered.ends_with(" check succeeded"))
}

pub(crate) fn scene_transition_visible_repair(
    visible_text: &str,
    info: &SceneTransitionInfo,
) -> String {
    let roll_only_committed_summary = visible_is_roll_only_committed_summary(visible_text);
    let empty_visible = visible_text.trim().is_empty();
    let chinese_output = crate::gm_craft::output_language_from_env()
        .as_deref()
        .map(|lang| crate::gm_craft::output_language_is_chinese(Some(lang)))
        .unwrap_or(false);
    if !empty_visible
        && !roll_only_committed_summary
        && (visible_mentions_transition_destination(visible_text, info)
            || !visible_has_transition_stall_marker(visible_text))
    {
        return visible_text.to_string();
    }
    let to_label = info
        .to_title
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(info.to.as_str());
    let from_label = info
        .from_title
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(info.from.as_str());
    let mut lines = Vec::new();
    if roll_only_committed_summary && !visible_text.trim().is_empty() {
        lines.push(sanitize_player_visible_multiline(visible_text.trim()));
    }
    if chinese_output && !contains_cjk(from_label) && !contains_cjk(to_label) {
        lines.push("你按自己的计划离开上一处位置，推进到新的可见区域。".to_string());
    } else {
        lines.push(format!(
            "你离开 {from_label}，按自己的计划来到 {to_label}。"
        ));
    }
    let may_surface_destination_text =
        !roll_only_committed_summary_has_non_successful_roll(visible_text);
    let mut added_chinese_generic_position_fact = false;
    if may_surface_destination_text {
        if let Some(read_aloud) = info.to_read_aloud.as_deref() {
            if !chinese_output || contains_cjk(read_aloud) {
                lines.push(read_aloud.trim().to_string());
            }
        } else if let Some(summary) = info.to_summary.as_deref() {
            if !chinese_output || contains_cjk(summary) {
                lines.push(summary.trim().to_string());
            }
        }
        if chinese_output && lines.len() == 2 {
            lines.push(
                "你现在的位置已经改变；门口、退路和周围明显危险仍在可观察范围内。".to_string(),
            );
            added_chinese_generic_position_fact = true;
        }
    }
    if roll_only_committed_summary {
        if chinese_output {
            if !added_chinese_generic_position_fact {
                lines.push(
                    "新的位置已经成为玩家可见事实；门口、退路和明显危险仍是当前能直接确认的范围。"
                        .to_string(),
                );
            }
        } else {
            lines.push(
                "新的位置已经是玩家可见事实；下一步应从这里的可见范围、退路和明显危险继续结算。"
                    .to_string(),
            );
        }
    } else {
        lines.push(
            "你还没有被迫越过新的门槛；眼前的入口、窗户、周边视线和明显危险都保持在可观察范围内。"
                .to_string(),
        );
    }
    lines.join("\n\n")
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
/// P2 步骤6 env flag：`TRPG_PRESENTATION_GATE`。OFF 时非 module 回合仅记 trace，零行为
/// 变更；ON 时全局执行。module play 默认执行 presentation gate，因为玩家行动菜单/秘密泄漏
/// 是玩家可见产品契约违例。Block 触发 repair ladder，但仅在 buffered-narration 在位时真正
/// 生效（调用方再加 buffer 前置守卫）。
pub(crate) fn presentation_gate_enabled() -> bool {
    std::env::var("TRPG_PRESENTATION_GATE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

pub(crate) fn presentation_gate_enforced(module_present: bool) -> bool {
    module_present || presentation_gate_enabled()
}

/// A3(§6 大考)：TRPG_NARRATOR_SPLIT 是否 ON(默认 OFF = 字节级基线)。split Narrator 无统一
/// agent loop 的 ReviseText 自修环，故 ON 路径需本修复谓词补一道收敛守卫。execute.rs 已就地
/// 读此 flag 决定 buffer 模式;此 helper 供 phase_verify_after_stream 的 A3 谓词复用同一语义。
pub(crate) fn narrator_split_enabled() -> bool {
    std::env::var("TRPG_NARRATOR_SPLIT")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// L1.1 SPINE: is the post-adjudication Director seam ON? Flag `TRPG_DIRECTOR_POST_ADJUDICATION`,
/// default OFF (= byte-identical baseline). Mirrors `narrator_split_enabled`. When ON, the
/// `resolution_commit_boundary` projects committed results + retains the World candidate pool so
/// the (L1.2) Beat Director can plan a beat reflecting the REAL outcome — fixing the turn-order
/// inversion (Director currently built pre-adjudication at the ContextAssembly phase).
pub(crate) fn director_post_adjudication_enabled() -> bool {
    std::env::var("TRPG_DIRECTOR_POST_ADJUDICATION")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// L1.1 SPINE: project the committed check results from the turn ledger snapshot into read-only
/// [`MechanicalResultView`]s, ONLY when the post-adjudication flag is ON. OFF ⇒ empty (no
/// consumer ⇒ byte-identical baseline). Pure; fail-closed (each view's outcome is `Unresolved`
/// unless the committed `outcome.success` bool is present — see `MechanicalResultView::from`).
pub(crate) fn project_post_adjudication_results(
    snapshot: &trpg_agent::TurnLedgerSnapshot,
    enabled: bool,
) -> Vec<MechanicalResultView> {
    if !enabled {
        return Vec::new();
    }
    snapshot
        .check_results
        .iter()
        .map(MechanicalResultView::from)
        .collect()
}

/// L6.1 SPINE→narration: project a post-adjudication [`DirectorPlan`] into **player-safe
/// structured steering tokens** for the Narrator. ONLY the enum/short-string steering fields
/// (`beat_kind` / `desired_change` / `dramatic_function`) — these are narrative-direction
/// directives the Narrator uses to shape prose, NOT secret facts. Deliberately EXCLUDES
/// `reveal_candidate_fact_ids`, `selected_world_candidates`, `must_preserve`/`must_avoid`,
/// `spotlight_target`, and any `world_query` prose (those stay 台下 proposal/audit; reveal is a
/// proposal, `forbidden_reveals` flows to the Narrator separately). A `None`/empty plan ⇒ empty
/// vec ⇒ the `director_plan` carrier stays empty ⇒ OFF byte-equal. Generic (§二-⑪: no ruleset
/// branch).
pub(crate) fn player_safe_director_plan_tokens(plan: &DirectorPlan) -> Vec<String> {
    let mut tokens = vec![format!("beat:{}", plan.beat_kind.as_str())];
    let change = plan.desired_change.trim();
    if !change.is_empty() {
        tokens.push(format!("desired_change:{change}"));
    }
    let func = plan.dramatic_function.trim();
    if !func.is_empty() {
        tokens.push(format!("dramatic_function:{func}"));
    }
    tokens
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

pub(crate) fn omitted_visible_repair_path_enabled(
    module_present: bool,
    narrator_split_on: bool,
) -> bool {
    narrator_split_on || presentation_gate_enforced(module_present)
}

pub(crate) fn committed_player_visible_check_missing_roll(
    check_results: &[trpg_model::CheckResultRecord],
    visible_text: &str,
) -> bool {
    let player_visible_checks = check_results
        .iter()
        .filter(|result| {
            !matches!(result.roll.visibility, RollVisibility::PrivateGmRoll)
                && (matches!(
                    result.roll.visibility,
                    RollVisibility::PublicGmRoll
                        | RollVisibility::PlayerRollRequired
                        | RollVisibility::PassiveResolution
                ) || !result.roll.result.is_null())
        })
        .count();
    if player_visible_checks == 0 {
        return false;
    }
    auditable_roll_block_count(visible_text) < player_visible_checks
}

fn auditable_roll_block_count(visible_text: &str) -> usize {
    let lower = visible_text.to_ascii_lowercase();
    let mut rest = lower.as_str();
    let mut count = 0usize;
    while let Some(start) = rest.find("[roll]") {
        let after = &rest[start + "[roll]".len()..];
        let Some(end) = after.find("[/roll]") else {
            return count;
        };
        if roll_block_is_auditable(&after[..end]) {
            count += 1;
        }
        rest = &after[end + "[/roll]".len()..];
    }
    count
}

pub(crate) fn resolved_gate_facts_missing_from_visible(
    facts: &[String],
    visible_text: &str,
) -> bool {
    facts
        .iter()
        .any(|fact| !resolved_gate_fact_visible(fact, visible_text))
}

fn player_visible_fact_projection(
    snapshot: &trpg_agent::TurnLedgerSnapshot,
    resolved_gate_facts: &[String],
) -> Vec<String> {
    let mut out = Vec::new();
    for fact in resolved_gate_facts {
        push_unique_fact(&mut out, fact);
    }
    for fact in committed_success_public_facts_from_checks(snapshot) {
        push_unique_fact(&mut out, &fact);
    }
    out
}

fn committed_success_public_facts_from_checks(
    snapshot: &trpg_agent::TurnLedgerSnapshot,
) -> Vec<String> {
    let mut out = Vec::new();
    for result in &snapshot.check_results {
        if !MechanicalResultView::from(result).outcome.is_success() {
            continue;
        }
        let Some(contract) = snapshot
            .check_contracts
            .iter()
            .find(|contract| contract.check_id == result.check_id)
        else {
            continue;
        };
        let success_public = contract.stakes.success_public.trim();
        if success_public.is_empty() {
            continue;
        }
        push_unique_fact(&mut out, success_public);
    }
    out
}

fn push_unique_fact(out: &mut Vec<String>, fact: &str) {
    let fact = fact.trim();
    if fact.is_empty() {
        return;
    }
    let normalized = fact.to_ascii_lowercase();
    if !out
        .iter()
        .any(|existing| existing.trim().to_ascii_lowercase() == normalized)
    {
        out.push(fact.to_string());
    }
}

pub(crate) fn append_missing_resolved_gate_facts_to_visible(
    visible_text: &str,
    facts: &[String],
) -> Option<String> {
    let missing: Vec<String> = facts
        .iter()
        .filter_map(|fact| appendable_player_visible_fact(fact, visible_text))
        .collect();
    if missing.is_empty() {
        return None;
    }
    let visible = visible_text.trim();
    if visible.is_empty() {
        return Some(missing.join("\n"));
    }
    Some(format!("{visible}\n\n{}", missing.join("\n")))
}

fn missing_rewrite_facts_for_output_language(
    visible_text: &str,
    facts: &[String],
    output_language: &str,
) -> Vec<String> {
    let candidates: Vec<String> = facts
        .iter()
        .filter(|fact| !resolved_gate_fact_visible(fact, visible_text))
        .filter_map(|fact| {
            let raw = fact.trim();
            if raw.is_empty() || raw.contains("[roll]") {
                return None;
            }
            let clean = sanitize_player_visible_summary_line(raw);
            if clean.is_empty() || bookkeeping_fact_not_player_visible(raw, &clean) {
                return None;
            }
            Some(clean)
        })
        .collect();
    if candidates.is_empty()
        || !output_language_requires_semantic_rewrite(output_language, visible_text, &candidates)
    {
        return Vec::new();
    }
    candidates
}

fn output_language_rewrite_required_for_missing_facts(
    visible_text: &str,
    facts: &[String],
) -> bool {
    crate::gm_craft::output_language_from_env()
        .as_deref()
        .map(|language| {
            !missing_rewrite_facts_for_output_language(visible_text, facts, language).is_empty()
        })
        .unwrap_or(false)
}

fn output_language_requires_semantic_rewrite(
    output_language: &str,
    visible_text: &str,
    facts: &[String],
) -> bool {
    if crate::gm_craft::output_language_is_chinese(Some(output_language)) {
        return !contains_cjk(visible_text) || facts.iter().any(|fact| !contains_cjk(fact));
    }
    let lowered = output_language
        .trim()
        .to_ascii_lowercase()
        .replace('_', "-");
    if matches!(lowered.as_str(), "en" | "en-us" | "english") {
        return contains_cjk(visible_text) || facts.iter().any(|fact| contains_cjk(fact));
    }
    true
}

fn appendable_player_visible_fact(raw_fact: &str, visible_text: &str) -> Option<String> {
    let raw = raw_fact.trim();
    if raw.is_empty() || raw.contains("[roll]") || resolved_gate_fact_visible(raw, visible_text) {
        return None;
    }
    let fact = sanitize_player_visible_summary_line(raw);
    if fact.is_empty() || bookkeeping_fact_not_player_visible(raw, &fact) {
        return None;
    }
    let visible_is_chinese = contains_cjk(visible_text);
    let fact_is_chinese = contains_cjk(&fact);
    let explicit_chinese_output = crate::gm_craft::output_language_from_env()
        .as_deref()
        .map(|lang| crate::gm_craft::output_language_is_chinese(Some(lang)))
        .unwrap_or(false);
    if explicit_chinese_output && !fact_is_chinese {
        return None;
    }
    if visible_is_chinese && !fact_is_chinese && !machine_source_backed_fact(raw) {
        return None;
    }
    Some(format!("你进一步确认：{fact}。"))
}

fn machine_source_backed_fact(raw_fact: &str) -> bool {
    let trimmed = raw_fact.trim();
    let lowered = trimmed.to_ascii_lowercase();
    trimmed.starts_with("已揭示的模组线索 ")
        || lowered.starts_with("module clue ")
        || lowered.starts_with("handout_")
        || lowered.starts_with("handout-")
}

fn bookkeeping_fact_not_player_visible(raw_fact: &str, sanitized_fact: &str) -> bool {
    let raw = raw_fact.to_ascii_lowercase();
    let sanitized = sanitized_fact.to_ascii_lowercase();
    if sanitized.contains(" check succeeds")
        || sanitized.contains(" check fails")
        || sanitized.contains(" check failed")
        || sanitized.contains(" check succeeded")
        || sanitized.starts_with("the ") && sanitized.contains(" check ")
    {
        return true;
    }
    raw.contains("\"check_id\"")
        || raw.contains("\"roll_id\"")
        || raw.contains("\"effect_id\"")
        || raw.contains("turnledger")
}

fn contains_cjk(text: &str) -> bool {
    text.chars()
        .any(|c| matches!(c, '\u{3400}'..='\u{9fff}' | '\u{f900}'..='\u{faff}'))
}

fn resolved_gate_fact_visible(fact: &str, visible_text: &str) -> bool {
    let fact = fact.trim();
    if fact.is_empty() || fact.contains("[roll]") {
        return true;
    }
    let visible = visible_text.to_ascii_lowercase();
    if visible.contains(&fact.to_ascii_lowercase()) {
        return true;
    }
    let content = fact
        .split_once(':')
        .map(|(_, rest)| rest)
        .unwrap_or(fact)
        .to_ascii_lowercase();
    let tokens: Vec<String> = content
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| token.len() >= 4)
        .filter(|token| {
            !matches!(
                *token,
                "that"
                    | "this"
                    | "with"
                    | "from"
                    | "into"
                    | "record"
                    | "records"
                    | "search"
                    | "public"
                    | "clue"
                    | "module"
                    | "revealed"
                    | "confirms"
                    | "summary"
            )
        })
        .map(str::to_string)
        .collect();
    if tokens.is_empty() {
        return false;
    }
    let hits = tokens
        .iter()
        .filter(|token| visible.contains(token.as_str()))
        .count();
    hits >= 2 || (tokens.len() == 1 && hits == 1)
}

fn roll_block_is_auditable(block: &str) -> bool {
    let lower = block.to_ascii_lowercase();
    let has_number = lower.chars().any(|ch| ch.is_ascii_digit());
    let has_result = [
        "success",
        "failure",
        "failed",
        "成功",
        "失败",
        "大成功",
        "大失败",
        "部分成功",
    ]
    .iter()
    .any(|cue| lower.contains(cue));
    let has_target = ["target", "目标", "vs", "dc", "dv", "≤", ">=", "难度"]
        .iter()
        .any(|cue| lower.contains(cue));
    has_number && has_result && has_target
}

pub(crate) fn unwrap_nonauditable_roll_blocks(visible_text: &str) -> Option<String> {
    let mut changed = false;
    let mut out = String::with_capacity(visible_text.len());
    let mut rest = visible_text;
    loop {
        let Some(start) = rest.to_ascii_lowercase().find("[roll]") else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        let after_start = start + "[roll]".len();
        let after = &rest[after_start..];
        let Some(end) = after.to_ascii_lowercase().find("[/roll]") else {
            out.push_str(&rest[start..]);
            break;
        };
        let block = &after[..end];
        if roll_block_is_auditable(block) {
            out.push_str("[roll]");
            out.push_str(block);
            out.push_str("[/roll]");
        } else {
            changed = true;
            out.push_str(block.trim());
        }
        rest = &after[end + "[/roll]".len()..];
    }
    changed.then_some(out)
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
        let happened = sanitize_player_visible_summary_line(happened);
        if uncommitted_pending_check_fact(&happened)
            || bookkeeping_fact_not_player_visible(&happened, &happened)
        {
            continue;
        }
        if happened.contains("[roll]") {
            lines.push(format!("· {happened}"));
        } else {
            lines.push(format!("· [roll]{happened}[/roll]"));
        }
    }
    for changed in &packet.what_changed {
        let changed = sanitize_player_visible_summary_line(changed);
        if bookkeeping_fact_not_player_visible(&changed, &changed) {
            continue;
        }
        lines.push(format!("· {changed}"));
    }
    for fact in &packet.player_perceivable_facts {
        let fact = sanitize_player_visible_summary_line(fact);
        if bookkeeping_fact_not_player_visible(&fact, &fact) {
            continue;
        }
        lines.push(format!("· {fact}"));
    }
    if lines.is_empty() {
        deterministic_player_input_grounding_narration(packet).unwrap_or_else(|| {
            "（本回合按已落账的机械结果继续；无新增可公开的机械事实。）".to_string()
        })
    } else {
        format!("根据本回合已确认的结果：\n{}", lines.join("\n"))
    }
}

fn sanitize_player_visible_summary_line(line: &str) -> String {
    strip_check_id_marker(&strip_module_clue_machine_prefix(
        &strip_machine_json_objects(line),
    ))
}

fn sanitize_player_visible_multiline(text: &str) -> String {
    text.lines()
        .map(sanitize_player_visible_summary_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_check_id_marker(line: &str) -> String {
    let mut out = line.to_string();
    while let Some(start) = out.find("检定[check") {
        let Some(end_rel) = out[start..].find(']') else {
            break;
        };
        let end = start + end_rel + 1;
        let remove_end = if out[end..].starts_with(' ') {
            end + ' '.len_utf8()
        } else {
            end
        };
        out.replace_range(start..remove_end, "");
    }
    out
}

fn strip_module_clue_machine_prefix(line: &str) -> String {
    let trimmed = line.trim();
    let lowered = trimmed.to_ascii_lowercase();
    let has_machine_prefix = trimmed.starts_with("已揭示的模组线索 ")
        || lowered.starts_with("module clue ")
        || lowered.starts_with("handout_")
        || lowered.starts_with("handout-");
    if !has_machine_prefix {
        return trimmed.to_string();
    }
    trimmed
        .split_once(':')
        .map(|(_, rest)| rest.trim().to_string())
        .filter(|rest| !rest.is_empty())
        .unwrap_or_else(|| trimmed.to_string())
}

fn deterministic_player_input_grounding_narration(
    packet: &crate::packet::NarrationPacket,
) -> Option<String> {
    let input = packet.player_input.to_ascii_lowercase();
    let looks_like_bedroom_threshold_probe =
        (input.contains("bedroom") || input.contains("bed") || input.contains("wardrobe"))
            && (input.contains("threshold")
                || input.contains("doorway")
                || input.contains("door frame")
                || input.contains("doorframe"))
            && (input.contains("landing")
                || input.contains("retreat")
                || input.contains("back toward"));
    if looks_like_bedroom_threshold_probe {
        return Some(
            "你把行动停在楼上房间门口这一条已经确认的接点上：门口、床、衣柜、窗框、纸张和身后的退路，是目前你能确定的范围。\
床或家具是否会自行移动、纸张上是否有可读内容、房间深处是否安全，还没有被确认；这些都只能算待查风险。\
此刻清楚的是：你仍在门口和退路之间，接下来必须先处理门口观察、远距离触碰或突发反应，才能安全决定是否进入房间。"
                .to_string(),
        );
    }

    let looks_like_house_exterior_lock_survey = (input.contains("house")
        || input.contains("building"))
        && (input.contains("outside")
            || input.contains("exterior")
            || input.contains("circle")
            || input.contains("entrance"))
        && (input.contains("lock") || input.contains("key") || input.contains("entry"))
        && (input.contains("safe") || input.contains("retreat") || input.contains("exit"));
    if looks_like_house_exterior_lock_survey {
        return Some(
            "你仍在屋外，先把行动停在可公开确认的范围内：外墙、前后方向的入口线索、可测试的外门锁，以及身后的退路。\
手里的钥匙和最少暴露的外锁构成当前最明确的入口接点，但锁是否已经顺利打开、屋内是否安全、窗内/地下室/邻近地面是否有可靠线索，还没有被确认。\
下一步应先把这个入口接点结算清楚，再决定是否越过门槛。"
                .to_string(),
        );
    }

    let looks_like_ground_floor_sweep = (input.contains("ground floor")
        || input.contains("ground-floor"))
        && (input.contains("entry") || input.contains("hall") || input.contains("one room"))
        && (input.contains("search")
            || input.contains("checks")
            || input.contains("photographs")
            || input.contains("marks doors"))
        && (input.contains("back path") || input.contains("retreat") || input.contains("behind"));
    if looks_like_ground_floor_sweep {
        return Some(
            "你把行动停在一楼入口和相邻房间这条已经确认的搜索线上：入口、已标记的房门、近处家具、可拍照的纸张痕迹，以及身后的退路，是目前你能确定的范围。\
上行或下行路线是否可用、地面扰动是否指向具体来源、哪些物件值得带走或触碰，还没有被确认。\
此刻清楚的是：一楼仍有房间、门、气味/气流或可见痕迹可以逐一落实，新的事实要从这些接点里产生。"
                .to_string(),
        );
    }

    let looks_like_basement_descent = (input.contains("basement") || input.contains("cellar"))
        && (input.contains("stair")
            || input.contains("stairs")
            || input.contains("descend")
            || input.contains("descends")
            || input.contains("way down"))
        && (input.contains("handkerchief")
            || input.contains("return marker")
            || input.contains("wedged open")
            || input.contains("retreat")
            || input.contains("door handle"))
        && (input.contains("flashlight")
            || input.contains("walking stick")
            || input.contains("testing boards")
            || input.contains("drafts")
            || input.contains("disturbed earth")
            || input.contains("moving threat"));
    if looks_like_basement_descent {
        return Some(
            "你把行动停在地下室入口和第一段楼梯这条已经确认的接点上：地下室门口、楼梯顶端、手帕标记、楔开的门和身后的退路，是目前你能确定的范围。\
更深处的楼梯是否稳固、是否有气流/松砖/扰动泥土/刮痕/暗板、是否存在尸体、仪式物或会动的威胁，还没有被确认；这些都只能算待查风险。\
此刻清楚的是：你仍在楼梯口或第一段台阶附近，退路仍在身后；继续之前，得先落实这里能听见什么、看见什么，以及这条退路是否仍然可靠。"
                .to_string(),
        );
    }

    let looks_like_upper_floor_search = (input.contains("upstairs")
        || input.contains("upper floor")
        || input.contains("landing"))
        && !input.contains("ground floor")
        && !input.contains("ground-floor")
        && (input.contains("stair") || input.contains("stairs") || input.contains("route holds"))
        && (input.contains("room")
            || input.contains("bed")
            || input.contains("wardrobe")
            || input.contains("window")
            || input.contains("paper")
            || input.contains("wall"))
        && (input.contains("landing") || input.contains("back") || input.contains("retreat"));
    if looks_like_upper_floor_search {
        return Some(
            "你把行动接到楼梯和楼上入口处，但先只确认可公开落地的部分：楼梯可逐级试探，楼梯平台和邻近房门是当前最清楚的上层接点，退路仍沿楼梯保持在身后。\
房间里的床、衣柜、窗、纸张、墙面痕迹以及是否有家具或物体自行移动，还没有被确认。\
下一步需要把某一个门口或房间的观察/检定结算清楚，再把它当作已知事实继续推进。"
                .to_string(),
        );
    }

    let looks_like_explicit_exterior_door_action = input.contains("unlock")
        || input.contains("turns the key")
        || input.contains("turn the key")
        || input.contains("opens it")
        || input.contains("open it")
        || input.contains("eases the door")
        || input.contains("exterior door")
        || input.contains("entry hall");
    let looks_like_threshold_entry = looks_like_explicit_exterior_door_action
        && (input.contains("step just inside")
            || input.contains("threshold")
            || input.contains("entry hall")
            || input.contains("visible rooms")
            || input.contains("stairs"))
        && (input.contains("door") || input.contains("house"));
    if !looks_like_threshold_entry {
        return None;
    }

    Some(
        "你把行动停在外门和门槛交界的可见范围内，身后的门口和退路仍然保留着。\
手电压低扫过近处，眼前只确认门厅边缘、邻近房门轮廓、楼梯轮廓和一股陈旧封闭的空气。\
更深处是否安全、地面是否有足迹或扰动、哪些房间值得先查，还没有被确认；你现在只能按门口可见范围逐项检查。"
            .to_string(),
    )
}

pub(crate) fn presentation_gate_safe_fallback_text(player_input: &str) -> String {
    let packet = crate::packet::NarrationPacket {
        player_input: player_input.to_string(),
        ..Default::default()
    };
    deterministic_player_input_grounding_narration(&packet).unwrap_or_else(|| {
        "你把动作停在当前已经确认的位置。没有新的公开通路、伤害或警报被确认；当前位置、退路和仍未确认的细节需要继续结算。"
            .to_string()
    })
}

fn uncommitted_pending_check_fact(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    text.contains("待结算")
        || text.contains("尚未绑定真实检定")
        || text.contains("勿当作已掷骰")
        || text.contains("未定")
        || text.contains("未知")
        || lower.contains("awaiting_binding")
        || lower.contains("unbound")
        || lower.contains("pending check")
        || lower.contains("pending")
}

/// PresentationGate 对「线索/观察结果以项目符号清单 dump 给玩家」会 fail-closed。这个纯函数只处理
/// 已被判为 manifest-list 的文本：保留可见事实，把列表形态改写成自然散文；它不修复行动菜单，
/// 也不新增任何机械/剧情事实。调用方必须重新跑 verifier，过不了仍落原 fail-closed 兜底。
pub(crate) fn deterministic_manifest_list_prose_repair(visible_text: &str) -> Option<String> {
    let text = visible_text.trim();
    if text.is_empty() || manifest_repair_looks_like_action_menu(text) {
        return None;
    }

    let mut lead: Vec<String> = Vec::new();
    let mut items: Vec<String> = Vec::new();
    let mut tail: Vec<String> = Vec::new();

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(item) = strip_manifest_list_marker(line) {
            let item = clean_manifest_fragment(item);
            if !item.is_empty() {
                items.push(item);
            }
            continue;
        }

        let fragment = clean_manifest_dump_heading(line)
            .map(clean_manifest_fragment)
            .unwrap_or_else(|| clean_manifest_fragment(line));
        if fragment.is_empty() {
            continue;
        }
        if items.is_empty() {
            lead.push(fragment);
        } else if manifest_tail_fragment_looks_like_action_menu(&fragment) {
            continue;
        } else {
            tail.push(fragment);
        }
    }

    if items.len() < 2 {
        if let Some((inline_lead, inline_items, inline_tail)) =
            extract_inline_observation_manifest(text)
        {
            lead = inline_lead;
            items = inline_items;
            tail = inline_tail;
        } else {
            return None;
        }
    }
    if manifest_items_look_like_action_menu(&items) {
        return None;
    }

    let english_style = manifest_repair_prefers_english(text);
    let mut parts: Vec<String> = Vec::new();
    if !lead.is_empty() {
        parts.push(join_manifest_sentences_with_style(&lead, english_style));
    }
    if english_style {
        let prose_items: Vec<String> = items
            .iter()
            .map(|item| naturalize_english_manifest_fragment(item))
            .filter(|item| !item.trim().is_empty())
            .collect();
        if prose_items.len() < 2 {
            return None;
        }
        parts.push(join_manifest_sentences_with_style(&prose_items, true));
    } else {
        parts.push(format!("你把这些观察串在一起：{}。", items.join("；")));
    }
    if !tail.is_empty() {
        parts.push(join_manifest_sentences_with_style(&tail, english_style));
    }

    Some(parts.join("\n"))
}

fn extract_inline_observation_manifest(
    text: &str,
) -> Option<(Vec<String>, Vec<String>, Vec<String>)> {
    let cues = [
        "You connect these observations:",
        "you connect these observations:",
        "Taken together,",
        "taken together,",
        "你把这些观察串在一起：",
    ];
    let (cue, start) = cues
        .iter()
        .find_map(|cue| text.find(cue).map(|idx| (*cue, idx)))?;
    let before = text[..start].trim();
    let after = text[start + cue.len()..].trim();
    if after.is_empty() {
        return None;
    }
    let mut segments: Vec<String> = after
        .split([';', '；'])
        .map(clean_manifest_fragment)
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.len() < 2 {
        return None;
    }
    if manifest_items_look_like_action_menu(&segments)
        || prose_segments_look_like_action_list(&format!("{cue} {}", segments.join("; ")))
    {
        return None;
    }
    let lead = if before.is_empty() {
        Vec::new()
    } else {
        vec![clean_manifest_fragment(before)]
    };
    // Keep the repair conservative: the inline manifest itself is the tail of the sentence,
    // so all semicolon fragments are treated as observations, not as a separate postscript.
    Some((lead, std::mem::take(&mut segments), Vec::new()))
}

fn manifest_repair_prefers_english(text: &str) -> bool {
    let ascii_alpha = text.chars().filter(|ch| ch.is_ascii_alphabetic()).count();
    let cjk = text
        .chars()
        .filter(|ch| ('\u{4e00}'..='\u{9fff}').contains(ch))
        .count();
    ascii_alpha >= 24 && ascii_alpha > cjk.saturating_mul(3)
}

fn manifest_items_look_like_action_menu(items: &[String]) -> bool {
    if items.len() < 3 {
        return false;
    }
    items
        .iter()
        .filter(|item| manifest_item_looks_like_player_action(item))
        .count()
        >= 2
}

fn manifest_item_looks_like_player_action(item: &str) -> bool {
    let trimmed = item.trim_start();
    let chinese_action = [
        "趁",
        "继续",
        "试着",
        "尝试",
        "冒险",
        "顺着",
        "确认",
        "切断",
        "断开",
        "撬",
        "靠近",
        "贴",
        "冲",
        "开火",
        "射击",
        "喊话",
        "撤",
        "进入",
        "压制",
        "破坏",
        "转向",
        "转去",
        "对警员",
        "先观察",
        "先看",
        "控制",
        "拖拽",
        "缴械",
    ]
    .iter()
    .any(|starter| trimmed.starts_with(starter));
    if chinese_action {
        return true;
    }

    let lower = trimmed.to_lowercase();
    let lower = lower
        .strip_prefix("or ")
        .or_else(|| lower.strip_prefix("and "))
        .unwrap_or(&lower)
        .trim_start();
    english_action_cues().iter().any(|cue| {
        lower.starts_with(&format!("{cue} ")) || lower.starts_with(&format!("{cue}ing "))
    })
}

fn manifest_tail_fragment_looks_like_action_menu(fragment: &str) -> bool {
    let text = fragment.trim();
    if text.is_empty() {
        return false;
    }
    let lower = text.to_lowercase();
    if [
        "接下来你可以",
        "下一步你可以",
        "你接下来可以",
        "你现在可以",
        "your next move could be",
    ]
    .iter()
    .any(|cue| lower.contains(cue))
    {
        return true;
    }
    if chinese_ordinal_direction_menu(text) {
        return true;
    }
    if chinese_where_start_or_menu(text) {
        return true;
    }
    if chinese_decide_is_or_action_menu(text) {
        return true;
    }
    if chinese_binary_path_action_menu(text) {
        return true;
    }
    if chinese_you_can_also_action_menu(text) {
        return true;
    }
    if chinese_lead_branch_action_menu(text) {
        return true;
    }
    if chinese_repeated_path_action_menu(text) {
        return true;
    }
    if chinese_either_or_action_menu(text) {
        return true;
    }
    if chinese_soft_next_action_menu(text) {
        return true;
    }
    prose_segments_look_like_action_list(text)
}

fn manifest_repair_looks_like_action_menu(text: &str) -> bool {
    let lower = text.to_lowercase();
    if [
        "选择其一",
        "可以选择",
        "你现在可以立刻",
        "你接下来可以",
        "下一步你可以",
        "下一步可以",
        "可以直接做",
        "可以立刻做",
        "可以立刻做的有",
        "做的有",
        "几条路",
        "选哪一个",
    ]
    .iter()
    .any(|cue| lower.contains(cue))
    {
        return true;
    }

    manifest_repair_has_prepared_action_menu(&lower)
}

pub(crate) fn player_agency_menu_cue_needs_block(visible_text: &str) -> bool {
    let text = visible_text.trim();
    if text.is_empty() {
        return false;
    }
    let lower = text.to_lowercase();
    if unclosed_dialogue_quote(&lower, text) {
        return true;
    }
    if [
        "选择其一",
        "可以选择",
        "你现在可以立刻",
        "你接下来可以",
        "下一步你可以",
        "下一步可以",
        "可以直接做",
        "可以立刻做",
        "可以立刻做的有",
        "选哪一个",
        "your next move could be",
    ]
    .iter()
    .any(|cue| lower.contains(cue))
    {
        return true;
    }
    if chinese_ordinal_direction_menu(text) {
        return true;
    }
    if chinese_where_start_or_menu(text) {
        return true;
    }
    if chinese_decide_is_or_action_menu(text) {
        return true;
    }
    if chinese_binary_path_action_menu(text) {
        return true;
    }
    if chinese_you_can_also_action_menu(text) {
        return true;
    }
    if chinese_lead_branch_action_menu(text) {
        return true;
    }
    if chinese_repeated_path_action_menu(text) {
        return true;
    }
    if chinese_either_or_action_menu(text) {
        return true;
    }
    if chinese_soft_next_action_menu(text) {
        return true;
    }

    if lower.contains("where do you want to start")
        && (lower.contains(" or ") || lower.contains("—") || lower.contains(','))
    {
        return true;
    }
    if lower.contains("you can choose where")
        && (lower.contains("goes first")
            || lower.contains("go first")
            || lower.contains("starts first")
            || lower.contains("start first"))
    {
        return true;
    }
    if lower.contains("if you want")
        && (lower.contains("one of several")
            || lower.contains("natural directions")
            || lower.contains("you can now"))
        && (lower.contains(" or ") || lower.contains(',') || lower.contains(':'))
    {
        return true;
    }
    if lower.contains("do you ")
        && lower.contains(" or ")
        && (lower.contains("press deeper") || lower.contains("leave "))
    {
        return true;
    }
    if lower.contains("do you go first")
        && lower.contains(" or ")
        && (lower.contains("records") || lower.contains("paper") || lower.contains("office"))
    {
        return true;
    }
    if (lower.contains("what do you want to try first")
        || lower.contains("which will you try first")
        || lower.contains("which do you try first")
        || lower.contains("which would you try first"))
        && lower.contains(" or ")
        && (lower.contains("records") || lower.contains("archives") || lower.contains("paper"))
    {
        return true;
    }
    if english_first_choice_alternative_menu(&lower) {
        return true;
    }
    if english_do_you_action_sequence_menu(&lower) {
        return true;
    }
    if english_whether_you_or_action_menu(&lower) {
        return true;
    }
    if english_choice_of_whether_action_menu(&lower) {
        return true;
    }
    if english_natural_directions_action_menu(&lower) {
        return true;
    }
    if english_you_can_action_sequence_menu(&lower) {
        return true;
    }
    if english_you_may_action_sequence_menu(&lower) {
        return true;
    }
    if english_next_move_is_either_menu(&lower) {
        return true;
    }
    if english_next_move_can_follow_or_take_menu(&lower) {
        return true;
    }
    if english_pursue_either_lead_menu(&lower) {
        return true;
    }
    if english_fragmented_paper_trail_direction_menu(&lower) {
        return true;
    }
    if english_do_you_have_character_or_menu(&lower) {
        return true;
    }
    if english_tone_choice_menu(&lower) {
        return true;
    }
    if english_connect_observations_action_menu(&lower) {
        return true;
    }
    if english_character_can_action_sequence_menu(&lower) {
        return true;
    }
    if english_if_you_want_character_can_commit_probe_menu(&lower) {
        return true;
    }
    if english_if_you_choose_you_can_or_menu(&lower) {
        return true;
    }
    if english_if_you_want_target_first_menu(&lower) {
        return true;
    }
    if english_if_you_want_press_or_paper_trail_menu(&lower) {
        return true;
    }
    if english_next_meaningful_or_open_enough_menu(&lower) {
        return true;
    }
    if english_clear_choice_target_menu(&lower) {
        return true;
    }
    if english_begin_or_live_direction_target_menu(&lower) {
        return true;
    }
    if english_branching_target_menu(&lower) {
        return true;
    }
    if english_obvious_lines_pursue_first_menu(&lower) {
        return true;
    }
    if english_will_you_start_or_more_menu(&lower) {
        return true;
    }
    if english_where_next_action_menu(&lower) {
        return true;
    }
    if english_what_now_inline_action_menu(&lower) {
        return true;
    }
    if english_what_do_first_action_menu(&lower) {
        return true;
    }
    if english_what_next_action_menu(&lower) {
        return true;
    }

    if text.contains("你可以直接以你的方式开口")
        || (text.contains("你准备怎么") && text.contains("比如"))
    {
        return true;
    }

    let connector_count = text.matches('；').count()
        + text.matches(';').count()
        + text.matches("还是").count()
        + text.matches("或者").count()
        + text.matches("或是").count();
    if text.contains("比如") {
        let list_separator_count =
            connector_count + text.matches('，').count() + text.matches('、').count();
        let actionish_count = [
            "端出", "强调", "施压", "话术", "说服", "请求", "继续", "转去", "立刻", "顺着", "追",
            "查", "开口", "拿下", "沿", "换", "绕", "找", "搭话", "进去", "进入", "拉近", "准备",
        ]
        .iter()
        .filter(|cue| text.contains(**cue))
        .count();
        return list_separator_count >= 2 && actionish_count >= 2;
    }
    if connector_count < 2 {
        return false;
    }

    (text.contains("你把这些观察串在一起") || lower.contains("you connect these observations"))
        && prose_segments_look_like_action_list(text)
}

pub(crate) fn deterministic_player_agency_menu_tail_repair(visible_text: &str) -> Option<String> {
    let text = visible_text.trim();
    if text.is_empty() || !player_agency_menu_cue_needs_block(text) {
        return None;
    }

    if let Some(repaired) = deterministic_fragmented_paper_trail_menu_repair(text) {
        if !player_agency_menu_cue_needs_block(&repaired) {
            return Some(repaired);
        }
    }
    if let Some(repaired) = deterministic_english_tone_choice_menu_repair(text) {
        if !player_agency_menu_cue_needs_block(&repaired) {
            return Some(repaired);
        }
    }

    let paragraphs: Vec<&str> = text.split("\n\n").collect();
    if paragraphs.len() > 1 {
        for idx in 0..paragraphs.len() {
            let paragraph = paragraphs[idx].trim();
            if paragraph.is_empty() || !player_agency_menu_cue_needs_block(paragraph) {
                continue;
            }
            let candidate = paragraphs
                .iter()
                .enumerate()
                .filter_map(|(candidate_idx, part)| {
                    if candidate_idx == idx {
                        None
                    } else {
                        let part = part.trim();
                        (!part.is_empty()).then_some(part)
                    }
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            if !candidate.trim().is_empty() && !player_agency_menu_cue_needs_block(&candidate) {
                return Some(candidate);
            }
        }
    }

    for (start, end) in sentence_spans_including_trailing_space(text) {
        let sentence = text[start..end].trim();
        if sentence.is_empty() || !player_agency_menu_cue_needs_block(sentence) {
            continue;
        }
        let mut candidate = String::new();
        candidate.push_str(text[..start].trim_end());
        let suffix = text[end..].trim_start();
        if !candidate.is_empty() && !suffix.is_empty() {
            candidate.push(' ');
        }
        candidate.push_str(suffix);
        let candidate = candidate.trim().to_string();
        if !candidate.is_empty() && !player_agency_menu_cue_needs_block(&candidate) {
            return Some(candidate);
        }
    }

    let mut candidates: Vec<(&str, &str)> = Vec::new();
    for delimiter in ["\n\n", "\n"] {
        for (idx, _) in text.match_indices(delimiter) {
            candidates.push((&text[..idx], &text[idx + delimiter.len()..]));
        }
    }
    for delimiter in [". ", "! ", "? "] {
        for (idx, _) in text.match_indices(delimiter) {
            candidates.push((&text[..idx + 1], &text[idx + delimiter.len()..]));
        }
    }

    candidates.sort_by_key(|(head, _)| std::cmp::Reverse(head.len()));
    for (head, tail) in candidates {
        let head = head.trim();
        let tail = tail.trim();
        if head.is_empty() || tail.is_empty() {
            continue;
        }
        if player_agency_menu_cue_needs_block(tail) && !player_agency_menu_cue_needs_block(head) {
            return Some(head.to_string());
        }
    }
    None
}

pub(crate) fn deterministic_dangling_colon_tail_repair(visible_text: &str) -> Option<String> {
    let trimmed = visible_text.trim_end();
    if trimmed.is_empty() || !(trimmed.ends_with('：') || trimmed.ends_with(':')) {
        return None;
    }
    let mut repaired = trimmed
        .trim_end_matches(|ch| ch == '：' || ch == ':')
        .trim_end()
        .to_string();
    if repaired.is_empty() {
        return None;
    }
    if !repaired.ends_with(|ch: char| {
        matches!(
            ch,
            '。' | '！' | '？' | '.' | '!' | '?' | '”' | '"' | '\'' | '’'
        )
    }) {
        repaired.push(if contains_cjk(&repaired) { '。' } else { '.' });
    }
    (repaired != visible_text.trim()).then_some(repaired)
}

fn deterministic_english_tone_choice_menu_repair(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    if !english_tone_choice_menu(&lower) {
        return None;
    }
    let start = [
        "how does evelyn try to get in",
        "how does evelyn approach",
        "how does she approach",
        "how does he approach",
        "how do you approach",
        "how will she approach",
        "how will he approach",
        "how does she try to get past",
        "how does she press",
        "how does he press",
        "how do you press",
        "how does the character press",
        "does she lean on",
        "do you have her lean on",
        "how she does it matters",
        "what tone does",
        "what tone do",
        "if she leans",
        "if she tries",
        "tell me her approach",
    ]
    .iter()
    .filter_map(|marker| lower.find(marker))
    .min()?;
    let repaired = text[..start]
        .trim_end_matches(|ch: char| ch.is_whitespace() || matches!(ch, '-' | '—' | ':' | ';'))
        .trim()
        .to_string();
    (!repaired.is_empty()).then_some(repaired)
}

fn deterministic_fragmented_paper_trail_menu_repair(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    if !english_fragmented_paper_trail_direction_menu(&lower) {
        return None;
    }
    let start = [
        "the most promising next channels appear",
        "most promising next channels appear",
        "the most promising next channels",
        "most promising next channels",
        "the higher courts / serious legal records",
        "higher courts / serious legal records",
        "the higher courts /",
    ]
    .iter()
    .find_map(|marker| lower.find(marker))?;
    let tail = &lower[start..];
    let end = if tail.starts_with("the most promising next channels")
        || tail.starts_with("most promising next channels")
    {
        tail.find('.')
            .map(|idx| start + idx + 1)
            .unwrap_or(text.len())
    } else {
        tail.find("a courteous clerk")
            .or_else(|| tail.find("a clerk"))
            .map(|idx| start + idx)
            .or_else(|| {
                lower[start..]
                    .find("you can follow this by turning next toward")
                    .map(|idx| start + idx)
            })?
    };
    if end <= start {
        return None;
    }
    let mut repaired = String::new();
    repaired.push_str(text[..start].trim_end());
    let suffix = text[end..].trim_start();
    if !repaired.is_empty() && !suffix.is_empty() {
        repaired.push(' ');
    }
    repaired.push_str(suffix);
    let repaired = repaired.trim().to_string();
    (!repaired.is_empty()).then_some(repaired)
}

fn sentence_spans_including_trailing_space(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0usize;
    for (idx, ch) in text.char_indices() {
        if !matches!(ch, '.' | '!' | '?') {
            continue;
        }
        let mut end = idx + ch.len_utf8();
        while end < text.len()
            && text[end..]
                .chars()
                .next()
                .is_some_and(|next| next.is_whitespace())
        {
            end += text[end..].chars().next().map(char::len_utf8).unwrap_or(0);
        }
        if start < end {
            spans.push((start, end));
        }
        start = end;
    }
    if start < text.len() {
        spans.push((start, text.len()));
    }
    spans
}

pub(crate) fn unwrap_player_visible_system_wrappers(visible_text: &str) -> String {
    visible_text
        .replace("[system]", "")
        .replace("[/system]", "")
        .replace("[SYSTEM]", "")
        .replace("[/SYSTEM]", "")
}

fn unclosed_dialogue_quote(lower: &str, text: &str) -> bool {
    let straight_unbalanced = text.matches('"').count() % 2 == 1;
    let curly_unbalanced = text.matches('“').count() > text.matches('”').count();
    if !straight_unbalanced && !curly_unbalanced {
        return false;
    }

    let trimmed = text.trim_start();
    let quote_opens_visible_speech = trimmed.starts_with('"')
        || trimmed.starts_with('“')
        || text.contains("\n\"")
        || text.contains("\n“");
    let dialogue_cue = [
        " says",
        " asks",
        " replies",
        " answers",
        " tells you",
        " murmurs",
        " whispers",
        "leans forward",
        "spreads his hands",
        "spreads her hands",
    ]
    .iter()
    .any(|cue| lower.contains(cue));

    quote_opens_visible_speech || dialogue_cue
}

fn english_action_cues() -> [&'static str; 31] {
    [
        "ask",
        "back",
        "change",
        "check",
        "commit",
        "continue",
        "dig",
        "enter",
        "examine",
        "examining",
        "follow",
        "go",
        "head",
        "inspect",
        "investigate",
        "leave",
        "look",
        "move",
        "open",
        "press",
        "probe",
        "pull",
        "push",
        "question",
        "search",
        "shift",
        "start",
        "turn",
        "try",
        "withdraw",
        "work",
    ]
}

fn english_first_choice_alternative_menu(lower: &str) -> bool {
    lower.split(['.', '!', '?', '\n']).any(|sentence| {
        let sentence = sentence.trim();
        if sentence.is_empty() || !sentence.contains("first") {
            return false;
        }
        if !(sentence.contains(" or ") || sentence.contains("—or ") || sentence.contains("-or "))
        {
            return false;
        }
        if !["which ", "what ", "where "]
            .iter()
            .any(|cue| sentence.contains(*cue))
        {
            return false;
        }
        let has_branch_surface = sentence.contains(':')
            || sentence.contains('—')
            || sentence.contains('-')
            || sentence.contains('/')
            || sentence.matches(',').count() >= 1;
        if !has_branch_surface {
            return false;
        }
        let has_action_or_route_verb = [
            "try",
            "pursue",
            "follow",
            "begin",
            "start",
            "go",
            "check",
            "search",
            "investigate",
            "look into",
            "visit",
            "enter",
            "open",
            "examine",
            "inspect",
            "focus",
            "choose",
            "take",
        ]
        .iter()
        .any(|cue| sentence.contains(*cue));
        has_action_or_route_verb
            || [
                "which lead",
                "which line",
                "what line",
                "where does",
                "where do",
            ]
            .iter()
            .any(|cue| sentence.contains(cue))
    })
}

fn english_whether_you_or_action_menu(lower: &str) -> bool {
    let Some((_, tail)) = lower.split_once("whether you ") else {
        return false;
    };
    let Some((left, right)) = tail.split_once(" or ") else {
        return false;
    };
    let action_cues = english_action_cues();
    let left_action = action_cues.iter().any(|cue| left.contains(cue));
    let right_action = action_cues.iter().any(|cue| right.contains(cue));
    left_action && right_action
}

fn english_do_you_action_sequence_menu(lower: &str) -> bool {
    lower.split(['.', '!', '?']).any(|sentence| {
        let sentence = sentence.trim();
        sentence.starts_with("do you ")
            && sentence.contains(" or ")
            && sentence.matches(',').count() >= 1
            && sentence
                .split_once("do you ")
                .map(|(_, tail)| english_action_sequence_hit_count(tail.trim()) >= 2)
                .unwrap_or(false)
    })
}

fn english_choice_of_whether_action_menu(lower: &str) -> bool {
    let Some((_, tail)) = lower.split_once("choice of whether to ") else {
        return false;
    };
    let sentence = tail.split(['.', '!', '?']).next().unwrap_or(tail).trim();
    sentence.contains(" or ")
        && sentence.matches(',').count() >= 1
        && english_action_sequence_hit_count(sentence) >= 2
}

fn english_natural_directions_action_menu(lower: &str) -> bool {
    if !lower.contains("direction")
        || !(lower.contains("can naturally")
            || lower.contains("naturally press")
            || lower.contains("few directions")
            || lower.contains("natural directions"))
    {
        return false;
    }
    if !(lower.contains(':') && (lower.contains(" or ") || lower.contains(','))) {
        return false;
    }
    let action_hits = english_action_cues()
        .iter()
        .filter(|cue| lower.contains(**cue))
        .count();
    action_hits >= 2
}

fn english_you_can_action_sequence_menu(lower: &str) -> bool {
    let tail = if let Some((_, tail)) = lower.split_once("you can ") {
        tail
    } else if let Some((_, tail)) = lower.split_once("you could ") {
        tail
    } else {
        return false;
    };
    let sentence = tail.split(['.', '!', '?']).next().unwrap_or(tail).trim();
    if !(sentence.contains(" or ") && sentence.matches(',').count() >= 1) {
        return false;
    }
    english_action_sequence_hit_count(sentence) >= 2
}

fn english_you_may_action_sequence_menu(lower: &str) -> bool {
    lower.split(['.', '!', '?']).any(|sentence| {
        let Some((_, tail)) = sentence.split_once("you may ") else {
            return false;
        };
        let tail = tail.trim();
        tail.contains(" or ")
            && tail.matches(',').count() >= 1
            && english_action_sequence_hit_count(tail) >= 2
    })
}

fn english_next_move_is_either_menu(lower: &str) -> bool {
    lower
        .split(['.', '!', '?'])
        .any(|sentence| sentence.contains("next move is either") && sentence.contains(" or "))
}

fn english_next_move_can_follow_or_take_menu(lower: &str) -> bool {
    lower.split(['.', '!', '?']).any(|sentence| {
        sentence.contains("next move can")
            && (sentence.contains("follow") || sentence.contains("pursue"))
            && (sentence.contains("take you") || sentence.contains("take her"))
            && (sentence.contains(" or ") || sentence.contains("—or "))
    })
}

fn english_pursue_either_lead_menu(lower: &str) -> bool {
    lower.split(['.', '!', '?']).any(|sentence| {
        (sentence.contains("pursue either lead")
            || sentence.contains("follow either lead")
            || sentence.contains("chase either lead"))
            && (sentence.contains(':') || sentence.contains(" or ") || sentence.contains("—or "))
    })
}

fn english_fragmented_paper_trail_direction_menu(lower: &str) -> bool {
    let has_paper_frame = [
        "paper trail",
        "public books",
        "public filings",
        "hall of records",
        "legal records",
        "public records",
        "public-index",
    ]
    .iter()
    .any(|cue| lower.contains(cue));
    let has_court_target = [
        "higher courts",
        "serious legal records",
        "the courts",
        "court records",
        "courts",
    ]
    .iter()
    .any(|cue| lower.contains(cue));
    let has_police_target = [
        "central police station",
        "police records",
        "police files",
        "the police",
    ]
    .iter()
    .any(|cue| lower.contains(cue));
    let has_site_target = [
        "corbitt house",
        "house itself",
        "chapel of contemplation",
        "the chapel",
    ]
    .iter()
    .any(|cue| lower.contains(cue));
    let has_direction_targets = has_court_target && has_police_target && has_site_target;
    let has_fragment = lower.contains("or leave the paper trail")
        || lower.contains("you can follow this by turning next toward")
        || lower.contains("turning next toward")
        || lower.contains("most promising next channels")
        || lower.contains("next channels appear")
        || lower.contains("appear to be the courts");
    has_paper_frame && has_direction_targets && has_fragment
}

fn english_do_you_have_character_or_menu(lower: &str) -> bool {
    lower.split(['.', '!', '?']).any(|sentence| {
        let sentence = sentence.trim();
        if !sentence.starts_with("do you have ")
            || !(sentence.contains(" or ") || sentence.contains("—or "))
        {
            return false;
        }
        let action_hits = [
            "pry", "hold", "examine", "inspect", "open", "pull", "probe", "press", "retreat",
            "withdraw",
        ]
        .iter()
        .filter(|cue| sentence.contains(**cue))
        .count();
        action_hits >= 2
    })
}

fn english_tone_choice_menu(lower: &str) -> bool {
    let has_frame = [
        "what tone does",
        "what tone do",
        "how does she press",
        "how does he press",
        "how do you press",
        "how does the character press",
    ]
    .iter()
    .any(|cue| lower.contains(cue));
    let approach_frame =
        (lower.contains("how does ") || lower.contains("how do ") || lower.contains("how will "))
            && lower.contains(" approach");
    let by_choice_frame =
        (lower.contains(" by ") || lower.contains("—by") || lower.contains("-by"))
            && lower.contains(" or ");
    let skill_hits = [
        "charm",
        "persuasion",
        "persuade",
        "intimidation",
        "intimidate",
        "fast talk",
        "quick bluff",
        "appeal",
        "argument",
        "professional",
        "credentials",
        "reasonable request",
        "professional courtesy",
        "polite persuasion",
        "presses politely",
        "flatters",
        "bluffs",
        "bluff",
        "urgency",
        "press credentials",
        "fast-talking",
        "pressure",
        "blunt pressure",
        "bully",
        "courtesy",
    ]
    .iter()
    .filter(|cue| lower.contains(**cue))
    .count();
    if approach_frame && by_choice_frame && skill_hits >= 3 {
        return true;
    }
    if approach_frame
        && ["if she", "if he", "if they", "if you"]
            .iter()
            .any(|cue| lower.contains(cue))
        && (lower.contains(" or tries ") || lower.contains(" or try ") || lower.contains(" or "))
        && skill_hits >= 3
    {
        return true;
    }
    if (lower.contains("do you have her lean on") || lower.contains("how she does it matters"))
        && skill_hits >= 3
    {
        return true;
    }
    if (lower.contains("how does she try to get past") || lower.contains("does she lean on"))
        && lower.contains(" or ")
        && skill_hits >= 2
    {
        return true;
    }
    if (lower.contains("how does evelyn try to get in") || lower.contains("tell me her approach"))
        && (lower.contains("if she leans") || lower.contains("if she tries"))
        && skill_hits >= 2
    {
        return true;
    }
    if !has_frame {
        return false;
    }
    let as_branches = lower.matches(" as a ").count()
        + lower.matches(" as an ").count()
        + lower.matches(" or as ").count()
        + lower.matches("? as ").count()
        + lower.matches(". as ").count()
        + lower.matches("; as ").count()
        + lower.matches(": as ").count();
    as_branches >= 3
        && [" or as ", "tone", "approach", "appeal", "argument"]
            .iter()
            .any(|cue| lower.contains(cue))
}

fn english_connect_observations_action_menu(lower: &str) -> bool {
    let Some((_, tail)) = lower.split_once("you connect these observations") else {
        return false;
    };
    let tail = tail.split_once(':').map(|(_, tail)| tail).unwrap_or(tail);
    let sentence = tail.split(['.', '!', '?']).next().unwrap_or(tail).trim();
    if !sentence.contains(" or ")
        || !(sentence.matches(';').count() >= 1 || sentence.matches(',').count() >= 1)
    {
        return false;
    }
    english_action_sequence_hit_count(sentence) >= 2
}

fn english_character_can_action_sequence_menu(lower: &str) -> bool {
    lower.split(['.', '!', '?']).any(|sentence| {
        sentence.contains(" can ")
            && sentence.contains(" or ")
            && sentence.matches(',').count() >= 1
            && sentence
                .split_once(" can ")
                .map(|(_, tail)| english_action_sequence_hit_count(tail.trim()) >= 2)
                .unwrap_or(false)
    })
}

fn english_if_you_want_character_can_commit_probe_menu(lower: &str) -> bool {
    if !lower.contains("if you want") || !lower.contains(" can ") || !lower.contains("commit") {
        return false;
    }
    if !(lower.contains("specific next probe")
        || lower.contains("next probe")
        || lower.contains("attention to first"))
    {
        return false;
    }
    if !(lower.contains(" or ") || lower.contains("—or ")) {
        return false;
    }
    ["bed", "wardrobe", "paper", "window", "door", "room"]
        .iter()
        .filter(|cue| lower.contains(**cue))
        .count()
        >= 3
}

fn english_if_you_choose_you_can_or_menu(lower: &str) -> bool {
    let has_frame = lower.contains("if you choose")
        || lower.contains("if she chooses")
        || lower.contains("if he chooses")
        || lower.contains("if they choose");
    if !has_frame {
        return false;
    }
    let Some((_, tail)) = lower.split_once(" can ") else {
        return false;
    };
    [
        " or you can ",
        " or she can ",
        " or he can ",
        " or they can ",
        "—or you can ",
        "—or she can ",
        "—or he can ",
        "—or they can ",
    ]
    .iter()
    .any(|connector| {
        tail.split_once(connector)
            .map(|(first, second)| english_actionish_part(first) && english_actionish_part(second))
            .unwrap_or(false)
    })
}

fn english_actionish_part(part: &str) -> bool {
    english_action_sequence_hit_count(part) >= 1
        || ["approach", "hold", "remain", "stay"].iter().any(|cue| {
            part.starts_with(*cue)
                || part.contains(&format!(" {cue} "))
                || part.contains(&format!(" {cue}ing "))
        })
}

fn english_if_you_want_target_first_menu(lower: &str) -> bool {
    if !lower.contains("if you want") {
        return false;
    }
    if !(lower.contains(" or ") || lower.contains("—or ")) {
        return false;
    }
    let frame = lower.contains("target first")
        || lower.contains("one specific target")
        || lower.contains("same doorway method")
        || lower.contains("same threshold method")
        || lower.contains("pressing this same");
    if !frame {
        return false;
    }
    [
        "bed", "wardrobe", "paper", "papers", "window", "doorway", "withdraw",
    ]
    .iter()
    .filter(|cue| lower.contains(**cue))
    .count()
        >= 3
}

fn english_if_you_want_press_or_paper_trail_menu(lower: &str) -> bool {
    lower.contains("if you want")
        && lower.contains("press him further")
        && (lower.contains(" or you can ") || lower.contains("—or you can "))
        && (lower.contains("paper trail") || lower.contains("records"))
}

fn english_next_meaningful_or_open_enough_menu(lower: &str) -> bool {
    let action_cues = [
        "commit", "shift", "study", "canvass", "listen", "widen", "make", "step", "enter", "go",
    ];
    lower.split(['.', '!', '?']).any(|sentence| {
        let has_connector = sentence.contains(" or ") || sentence.contains("—or ");
        if !has_connector {
            return false;
        }
        if sentence.contains("next meaningful move") && sentence.contains(':') {
            let tail = sentence
                .split_once("next meaningful move")
                .map(|(_, tail)| tail)
                .unwrap_or(sentence);
            return action_cues
                .iter()
                .filter(|cue| {
                    tail.split_whitespace()
                        .any(|word| word.trim_matches(|c: char| !c.is_alphanumeric()) == **cue)
                })
                .count()
                >= 2;
        }
        if sentence.contains("open enough now to") {
            let tail = sentence
                .split_once("open enough now to")
                .map(|(_, tail)| tail)
                .unwrap_or(sentence);
            return action_cues
                .iter()
                .filter(|cue| {
                    tail.split_whitespace()
                        .any(|word| word.trim_matches(|c: char| !c.is_alphanumeric()) == **cue)
                })
                .count()
                >= 2;
        }
        false
    })
}

fn english_action_sequence_hit_count(sentence: &str) -> usize {
    english_action_cues()
        .iter()
        .filter(|cue| {
            sentence.starts_with(**cue)
                || sentence.contains(&format!(" {cue} "))
                || sentence.contains(&format!(" {cue}ing "))
        })
        .count()
}

fn english_clear_choice_target_menu(lower: &str) -> bool {
    if !lower.contains("clear choice") {
        return false;
    }
    if !(lower.contains("where to") || lower.contains("what to") || lower.contains("which ")) {
        return false;
    }
    let tail = lower
        .split_once("clear choice")
        .map(|(_, tail)| tail)
        .unwrap_or(lower);
    let sentence = tail.split(['.', '!', '?']).next().unwrap_or(tail).trim();
    sentence.contains(':')
        && sentence.contains(" or ")
        && (sentence.contains(',') || sentence.contains(';'))
}

fn english_begin_or_live_direction_target_menu(lower: &str) -> bool {
    let frame = lower.contains("where do you mean to begin")
        || lower.contains("where do you begin")
        || lower.contains("what do you pursue next")
        || lower.contains("live directions")
        || lower.contains("point in three directions")
        || lower.contains("point in three live directions")
        || lower.contains("point in three promising directions")
        || lower.contains("points in three directions")
        || lower.contains("points in three live directions")
        || lower.contains("points in three clear directions")
        || lower.contains("points in three promising directions")
        || lower.contains("three immediate lines of pursuit")
        || lower.contains("immediate lines of pursuit")
        || lower.contains("lines of pursuit");
    if !frame || !lower.contains(" or ") {
        return false;
    }
    if lower.contains(':') || lower.matches(',').count() >= 2 {
        return true;
    }
    lower.matches(';').count() >= 1
        && (lower.contains("what does ") || lower.contains("what do "))
        && lower.contains(" next")
}

fn english_branching_target_menu(lower: &str) -> bool {
    let frame = lower.contains("obvious next avenues")
        || lower.contains("next avenues are")
        || lower.contains("obvious next lines of inquiry")
        || lower.contains("next solid leads")
        || lower.contains("line of inquiry clearly branches outward")
        || lower.contains("line of inquiry branches outward")
        || lower.contains("branches outward")
        || lower.contains("branches into")
        || lower.contains("paper trail might continue")
        || lower.contains("if you want to press it further")
        || lower.contains("obvious directions suggest themselves")
        || lower.contains("two obvious directions")
        || lower.contains("next useful angle is")
        || lower.contains("next pressure points")
        || lower.contains("for example, focusing on")
        || lower.contains("follow this outward from here")
        || lower.contains("follow this outward")
        || lower.contains("choice of pressure points")
        || lower.contains("clearer choice of pressure points")
        || lower.contains("offers more avenues");
    if !frame {
        return false;
    }
    let has_connector = lower.contains(" or ")
        || lower.contains("though you could")
        || (lower.contains("next pressure points") && lower.contains("follow first"))
        || (lower.contains("obvious next avenues") && lower.contains(" and "))
        || (lower.contains("obvious next lines of inquiry")
            && (lower.contains(" and ") || lower.contains(',')))
        || (lower.contains("two obvious directions") && lower.contains(" and "));
    if !has_connector {
        return false;
    }
    if lower.contains("obvious next avenues") && lower.contains(" and ") {
        return true;
    }
    if lower.contains("two obvious directions") && lower.contains(" and ") {
        return true;
    }
    if lower.contains("next pressure points") && lower.contains("follow first") {
        return true;
    }
    if lower.contains("obvious next lines of inquiry")
        && (lower.contains(" and ") || lower.contains(','))
    {
        return true;
    }
    if lower.contains("next solid leads") && lower.contains(" or ") {
        return true;
    }
    lower.contains(':') || lower.matches(',').count() >= 1 || lower.contains(';')
}

fn english_obvious_lines_pursue_first_menu(lower: &str) -> bool {
    lower.contains("obvious lines of inquiry")
        && lower.contains("pursue first")
        && (lower.contains(':') || lower.matches(',').count() >= 2 || lower.contains(';'))
}

fn english_will_you_start_or_more_menu(lower: &str) -> bool {
    (lower.contains("will you start") || lower.contains("will you begin"))
        && lower.contains(" or ")
        && (lower.contains("something more")
            || lower.contains("before you go")
            || lower.contains("from me here")
            || lower.contains("start with the records")
            || lower.contains("begin with the records")
            || lower.contains("the newspapers")
            || lower.contains("neighborhood"))
}

fn english_where_next_action_menu(lower: &str) -> bool {
    if !(lower.contains("where does ") || lower.contains("where do ")) {
        return false;
    }
    if !lower.contains(" next") {
        return false;
    }
    let tail = if let Some((_, tail)) = lower.split_once(':') {
        tail
    } else {
        lower.split_once('?').map(|(_, tail)| tail).unwrap_or(lower)
    }
    .trim();
    if !tail.contains(" or ") {
        return false;
    }
    tail.matches(',').count() >= 1
        || tail.contains(':')
        || english_action_cues()
            .iter()
            .filter(|cue| tail.contains(**cue))
            .count()
            >= 2
}

fn english_what_next_action_menu(lower: &str) -> bool {
    if !(lower.contains("what does ") || lower.contains("what do ")) {
        return false;
    }
    if lower.split(['.', '!', '?']).any(|sentence| {
        let sentence = sentence.trim();
        let focus_frame = sentence.contains(" focus on first")
            || sentence.contains(" examine first")
            || sentence.contains(" inspect first");
        focus_frame
            && sentence.contains(':')
            && (sentence.contains(" or ") || sentence.matches(',').count() >= 2)
    }) {
        return true;
    }
    if !lower.contains(" next") {
        return false;
    }
    let tail = lower
        .split_once(" next")
        .map(|(_, tail)| tail)
        .unwrap_or(lower)
        .trim();
    tail.contains(" or ")
        && (tail.contains(',') || tail.contains('—') || tail.contains(':') || tail.contains('?'))
}

fn english_what_now_inline_action_menu(lower: &str) -> bool {
    let Some((head, tail)) = lower.split_once("do now?") else {
        return false;
    };
    if !(head.contains("what does ") || head.contains("what do ")) {
        return false;
    }
    let sentence = tail.split(['.', '!', '?']).next().unwrap_or(tail).trim();
    if sentence.is_empty() {
        return false;
    }
    if !(sentence.contains(" or ") || sentence.contains("—or ")) {
        return false;
    }
    sentence.contains(',')
        || sentence.contains(';')
        || sentence.contains(':')
        || sentence.contains('—')
        || english_action_sequence_hit_count(sentence) >= 2
}

fn english_what_do_first_action_menu(lower: &str) -> bool {
    let Some((head, tail)) = lower.split_once("do first") else {
        return false;
    };
    if !(head.contains("what does ") || head.contains("what do ")) {
        return false;
    }
    let sentence = tail.split(['.', '!', '?']).next().unwrap_or(tail).trim();
    if sentence.is_empty() {
        return false;
    }
    if !(sentence.contains(" or ") || sentence.contains("—or ")) {
        return false;
    }
    (sentence.contains(',')
        || sentence.contains(';')
        || sentence.contains(':')
        || sentence.contains('—'))
        && english_action_sequence_hit_count(sentence) >= 2
}

fn chinese_where_start_or_menu(text: &str) -> bool {
    text.contains("还是")
        && (text.contains("你想先") || text.contains("你先") || text.contains("先从"))
        && (text.contains("下手") || text.contains("开始") || text.contains("着手"))
}

fn chinese_decide_is_or_action_menu(text: &str) -> bool {
    if !(text.contains("决定") && text.contains("是") && text.contains("还是")) {
        return false;
    }
    let action_hits = [
        "继续",
        "冒",
        "往下",
        "试",
        "换办法",
        "先换",
        "下探",
        "退",
        "靠近",
        "进入",
        "查",
    ]
    .iter()
    .filter(|cue| text.contains(**cue))
    .count();
    action_hits >= 2
}

fn chinese_you_can_also_action_menu(text: &str) -> bool {
    if !(text.contains("你可以")
        && (text.contains("也可以") || text.contains("或者") || text.contains("或是")))
    {
        return false;
    }
    let has_next_action_frame = text.contains("你接下来")
        || text.contains("接下来要")
        || text.contains("下一步")
        || text.contains("往哪")
        || text.contains("哪一条")
        || text.contains("先压下去");
    let has_inline_branch_tail = text.contains("或者直接")
        || text.contains("或是直接")
        || text.contains("或者接着")
        || text.contains("或是接着")
        || text.contains("或者继续")
        || text.contains("或是继续");
    if !has_next_action_frame && !has_inline_branch_tail {
        return false;
    }
    let action_hits = [
        "继续", "先查", "直奔", "前往", "转去", "深挖", "调 ", "调取", "查", "追", "去", "进",
        "走", "压",
    ]
    .iter()
    .filter(|cue| text.contains(**cue))
    .count();
    action_hits >= 2
}

fn chinese_lead_branch_action_menu(text: &str) -> bool {
    let has_frame = ["线索：", "方向：", "路：", "抓手："]
        .iter()
        .any(|cue| text.contains(cue));
    if !has_frame || !(text.contains("或者") || text.contains("或是") || text.contains("还是"))
    {
        return false;
    }
    let tail = text
        .split_once('：')
        .map(|(_, rest)| rest)
        .or_else(|| text.split_once(':').map(|(_, rest)| rest))
        .unwrap_or(text);
    prose_segments_look_like_action_list(tail)
}

fn chinese_repeated_path_action_menu(text: &str) -> bool {
    let has_next_frame =
        text.contains("下一步") || text.contains("决定下一步") || text.contains("可以从这里决定");
    if !has_next_frame || text.matches("一条").count() < 2 {
        return false;
    }
    let has_path_frame = [
        "主方向",
        "方向",
        "路线",
        "路线先",
        "往哪一条",
        "往哪条",
        "哪一条线",
        "哪条线",
    ]
    .iter()
    .any(|cue| text.contains(cue));
    if !has_path_frame {
        return false;
    }
    let action_hits = [
        "回头",
        "继续",
        "往楼上",
        "往地下",
        "往地下室",
        "推进",
        "深查",
        "细查",
        "啃",
        "查",
        "去",
    ]
    .iter()
    .filter(|cue| text.contains(**cue))
    .count();
    action_hits >= 2
}

fn chinese_either_or_action_menu(text: &str) -> bool {
    let has_next_frame = [
        "你接下来",
        "接下来你",
        "接下来要",
        "下一步",
        "现在你",
        "此刻你",
        "你眼下还能做",
        "眼下还能做",
        "都得换个办法",
        "换个办法：",
    ]
    .iter()
    .any(|cue| text.contains(cue));
    if !has_next_frame || !text.contains("要么") {
        return false;
    }
    let branch_count = text.matches("要么").count()
        + text.matches("或者").count()
        + text.matches("或是").count()
        + text.matches("还是").count();
    if branch_count < 2 {
        return false;
    }
    let action_hits = [
        "继续", "推进", "改去", "换个", "探查", "进入", "检查", "观察", "靠近", "贴近", "前往",
        "转去", "深挖", "追", "查", "走", "去",
    ]
    .iter()
    .filter(|cue| text.contains(**cue))
    .count();
    action_hits >= 2
}

fn chinese_soft_next_action_menu(text: &str) -> bool {
    let Some(tail) = [
        "下一步自然会是",
        "下一步会是",
        "下一步就是",
        "下一步则是",
        "下一步可以是",
        "下一步如果",
        "比较自然的方向会是",
        "自然的方向会是",
        "自然方向会是",
    ]
    .iter()
    .find_map(|cue| text.split_once(cue).map(|(_, rest)| rest)) else {
        return false;
    };
    let branch_count = tail.matches('、').count()
        + tail.matches('；').count()
        + tail.matches(';').count()
        + tail.matches("或者").count()
        + tail.matches("或是").count()
        + tail.matches("还是").count();
    if branch_count == 0 {
        return false;
    }
    let action_hits = [
        "去看",
        "查看",
        "寻找",
        "绕",
        "靠近",
        "贴近",
        "进入",
        "检查",
        "观察",
        "询问",
        "前往",
        "转去",
        "转回去",
        "处理",
        "继续",
        "深挖",
        "查",
        "追",
    ]
    .iter()
    .filter(|cue| tail.contains(**cue))
    .count();
    action_hits >= 2
}

fn chinese_binary_path_action_menu(text: &str) -> bool {
    let has_path_frame = [
        "两条路",
        "两条线",
        "两个方向",
        "两种方向",
        "几条路",
        "几条线",
        "几种方向",
    ]
    .iter()
    .any(|cue| text.contains(cue));
    if !has_path_frame {
        return false;
    }
    let branch_markers = ["一条", "另一条", "一边", "另一边", "一个", "另一个"]
        .iter()
        .filter(|cue| text.contains(**cue))
        .count();
    if branch_markers < 2 {
        return false;
    }
    let has_next_frame = [
        "你接下来",
        "接下来想",
        "接下来要",
        "下一步",
        "往哪边",
        "往哪条",
        "哪边压",
        "哪条线",
        "哪条路",
    ]
    .iter()
    .any(|cue| text.contains(cue));
    let action_hits = [
        "继续", "顺着", "深挖", "直接", "去找", "回到", "前往", "查", "追", "压",
    ]
    .iter()
    .filter(|cue| text.contains(**cue))
    .count();
    has_next_frame && action_hits >= 2
}

fn chinese_ordinal_direction_menu(text: &str) -> bool {
    if !(text.contains("几条")
        && (text.contains("去处") || text.contains("方向") || text.contains("路径")))
    {
        return false;
    }
    let ordinal_hits = ["一是", "二是", "三是", "四是"]
        .iter()
        .filter(|cue| text.contains(**cue))
        .count();
    if ordinal_hits < 2 {
        return false;
    }
    let action_hits = [
        "继续",
        "顺着",
        "转去",
        "带着",
        "直接去",
        "查",
        "看",
        "追",
        "前往",
    ]
    .iter()
    .filter(|cue| text.contains(**cue))
    .count();
    action_hits >= 2
}

pub(crate) fn source_limited_missing_address_repair(
    player_input: &str,
    visible_text: &str,
) -> Option<String> {
    if !player_asks_for_specific_address(player_input) {
        return None;
    }
    let visible_has_concrete_address = contains_concrete_street_address(visible_text);
    if !visible_has_concrete_address && !visible_claims_address_was_delivered(visible_text) {
        return None;
    }
    if visible_has_concrete_address && contains_concrete_street_address(player_input) {
        return None;
    }

    let mut repaired = visible_text
        .replace(
            "**Corbitt House 的完整街道地址**",
            "Corbitt House 的可导航地址线索",
        )
        .replace(
            "**Corbitt House 的完整地址**",
            "Corbitt House 的可导航地址线索",
        )
        .replace("**Corbitt House 的地址**", "Corbitt House 的可导航地址线索")
        .replace("完整街道地址", "可导航地址线索")
        .replace("完整地址", "可导航地址线索")
        .replace("确切街道地址", "可导航地址线索")
        .replace("确切地址", "可导航地址线索")
        .replace("Corbitt House 的地址", "Corbitt House 的可导航地址线索")
        .replace("记下的地址", "记下的可导航地址线索")
        .replace("full street address", "usable address lead")
        .replace(
            "full address of the Corbitt House",
            "usable address lead for the Corbitt House",
        )
        .replace(
            "full address of Corbitt House",
            "usable address lead for Corbitt House",
        )
        .replace(
            "full address of the old Corbitt place",
            "usable address lead for the old Corbitt place",
        )
        .replace(
            "full address of the old Corbitt House",
            "usable address lead for the old Corbitt House",
        )
        .replace(
            "writes down the address of the old Corbitt House",
            "records the usable address lead for the old Corbitt House",
        )
        .replace(
            "writes down the address of the old Corbitt place",
            "records the usable address lead for the old Corbitt place",
        )
        .replace(
            "“The address is here,” he says, writing it down for you",
            "He records the usable address lead for you",
        )
        .replace(
            "\"The address is here,\" he says, writing it down for you",
            "He records the usable address lead for you",
        )
        .replace(
            "The address is here",
            "The address lead is usable for navigation and records work",
        )
        .replace("writing it down", "recording the usable address lead")
        .replace(
            "writes down the house address",
            "records the usable address lead",
        )
        .replace(
            "write down the house address",
            "record the usable address lead",
        )
        .replace("writes down the address", "records the usable address lead")
        .replace("write down the address", "record the usable address lead")
        .replace(
            "The address is certain",
            "The address lead is sufficient for navigation and records work",
        )
        .replace(
            "the address is certain",
            "the address lead is sufficient for navigation and records work",
        )
        .replace("written address", "address lead")
        .replace("house address", "usable address lead")
        .replace("exact street address", "usable address lead")
        .replace("complete street address", "usable address lead")
        .replace("full address", "usable address lead");
    if visible_has_concrete_address {
        repaired = scrub_concrete_street_address_literals(&repaired);
    }
    if repaired == visible_text {
        repaired = visible_text.to_string();
    }
    let boundary = if manifest_repair_prefers_english(visible_text) {
        "\n\nIn Evelyn's notes, this remains a usable address lead for navigation and records work, not a literal street-number line she can quote. She can use it to travel to the Corbitt House and search public files without adding a street number."
    } else {
        "\n\n你的笔记里，这仍是一条足以导航和查档的可用地址线索，而不是可逐字引用的门牌号。后续可以凭这条线索前往科比特宅或查公共档案。"
    };
    if !repaired.contains("而不是可逐字引用的门牌号")
        && !repaired.contains("not a literal street-number line she can quote")
        && !repaired.contains("当前可见资料没有给出可逐字抄下的门牌号")
        && !repaired
            .contains("current player-visible text does not provide a literal street number")
    {
        repaired.push_str(boundary);
    }
    Some(repaired)
}

fn concrete_street_address_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?ix)
            \b
            \d{1,6}
            \s+
            (?:[[:alpha:]'.-]+\s+){0,5}
            (?:street|st\.?|avenue|ave\.?|road|rd\.?|lane|ln\.?|court|ct\.?|place|pl\.?|square|sq\.?|way|boulevard|blvd\.?|drive|dr\.?)
            \b
            (?:\s*,\s*[[:alpha:]'.-]+)?
            ",
        )
        .expect("street-address scrub regex must compile")
    })
}

fn scrub_concrete_street_address_literals(text: &str) -> String {
    concrete_street_address_regex()
        .replace_all(text, "usable address lead")
        .into_owned()
}

fn player_asks_for_specific_address(player_input: &str) -> bool {
    let lower = player_input.to_lowercase();
    if [
        "完整街道地址",
        "完整地址",
        "确切街道地址",
        "确切地址",
        "具体街道地址",
        "具体地址",
        "门牌号",
    ]
    .iter()
    .any(|cue| player_input.contains(cue))
    {
        return true;
    }
    if [
        "full street address",
        "exact street address",
        "complete street address",
        "specific street address",
        "literal street address",
        "full address",
        "exact address",
        "complete address",
        "specific address",
        "literal address",
    ]
    .iter()
    .any(|cue| lower.contains(cue))
    {
        return true;
    }

    let lower = lower
        .replace("exact refusal", "")
        .replace("exact denial", "")
        .replace("exact restriction", "");
    let address_terms = ["address", "street address", "地址", "门牌", "门牌号"];
    let specific_terms = [
        "完整", "确切", "具体", "街道", "门牌", "street", "exact", "complete", "specific", "full",
    ];
    cue_near_any(&lower, player_input, &address_terms, &specific_terms, 96)
}

fn cue_near_any(
    lower: &str,
    original: &str,
    lhs_terms: &[&str],
    rhs_terms: &[&str],
    max_gap_bytes: usize,
) -> bool {
    let mut lhs_positions: Vec<usize> = Vec::new();
    let mut rhs_positions: Vec<usize> = Vec::new();
    for term in lhs_terms {
        lhs_positions.extend(lower.match_indices(term).map(|(idx, _)| idx));
        if term.chars().any(|ch| !ch.is_ascii()) {
            lhs_positions.extend(original.match_indices(term).map(|(idx, _)| idx));
        }
    }
    for term in rhs_terms {
        rhs_positions.extend(lower.match_indices(term).map(|(idx, _)| idx));
        if term.chars().any(|ch| !ch.is_ascii()) {
            rhs_positions.extend(original.match_indices(term).map(|(idx, _)| idx));
        }
    }
    lhs_positions.iter().any(|lhs| {
        rhs_positions
            .iter()
            .any(|rhs| lhs.abs_diff(*rhs) <= max_gap_bytes)
    })
}

fn visible_claims_address_was_delivered(visible_text: &str) -> bool {
    let lower = visible_text.to_lowercase();
    let mentions_address = lower.contains("address") || visible_text.contains("地址");
    if !mentions_address {
        return false;
    }
    if [
        "写下",
        "写清",
        "写了下来",
        "写下来",
        "记下",
        "递给",
        "交给",
        "给你",
        "gives you",
        "gave you",
        "writes down",
        "wrote down",
        "hands you",
        "handed you",
    ]
    .iter()
    .any(|cue| lower.contains(cue) || visible_text.contains(cue))
    {
        return true;
    }
    if lower.contains("full street address beneath")
        || lower.contains("full address beneath")
        || lower.contains("with the full street address")
        || lower.contains("with the full address")
    {
        return true;
    }

    let claims_written_down = ["write", "writes", "wrote", "writing", "written"]
        .iter()
        .any(|verb| lower.contains(verb))
        && lower.contains("down");
    let claims_copied = lower.contains("copy") || lower.contains("copied");

    claims_written_down || claims_copied
}

pub(crate) fn contains_concrete_street_address(text: &str) -> bool {
    let street_words = [
        "street",
        "st.",
        "avenue",
        "ave",
        "road",
        "rd.",
        "lane",
        "ln.",
        "court",
        "ct.",
        "place",
        "pl.",
        "square",
        "sq.",
        "way",
        "boulevard",
        "blvd",
        "drive",
        "dr.",
        "街",
        "路",
        "号",
        "巷",
        "弄",
        "大道",
    ];

    for segment in text.split(|ch: char| {
        matches!(
            ch,
            '\n' | '\r' | ',' | '，' | ';' | '；' | '。' | '！' | '!' | '？' | '?'
        )
    }) {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        let lower_segment = segment.to_lowercase();
        let has_street_word = street_words
            .iter()
            .any(|word| lower_segment.contains(word) || segment.contains(word));
        if !has_street_word {
            continue;
        }
        if segment.chars().any(|ch| ch.is_ascii_digit())
            || segment.chars().any(|ch| ('０'..='９').contains(&ch))
            || segment
                .chars()
                .any(|ch| "一二三四五六七八九十百千零〇".contains(ch))
        {
            return true;
        }
    }
    false
}

fn prose_segments_look_like_action_list(text: &str) -> bool {
    let body = text
        .split_once("你把这些观察串在一起")
        .map(|(_, rest)| rest)
        .or_else(|| {
            text.split_once("You connect these observations")
                .map(|(_, rest)| rest)
        })
        .or_else(|| {
            text.split_once("you connect these observations")
                .map(|(_, rest)| rest)
        })
        .unwrap_or(text);
    let normalized = body
        .replace("，或者", "；")
        .replace("，还是", "；")
        .replace("，或是", "；")
        .replace("或者", "；")
        .replace("还是", "；")
        .replace("或是", "；");
    let segments: Vec<String> = normalized
        .split(['；', ';', '\n'])
        .map(clean_manifest_fragment)
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.len() < 2 {
        return false;
    }
    segments
        .iter()
        .filter(|segment| manifest_item_looks_like_player_action(segment))
        .count()
        >= 2
}

fn manifest_repair_has_prepared_action_menu(text: &str) -> bool {
    let tail = ["接下来你是准备", "接下来你准备"]
        .iter()
        .find_map(|cue| text.split_once(cue).map(|(_, rest)| rest));
    let Some(tail) = tail else {
        return false;
    };
    if !["还是", "或者", "或是"]
        .iter()
        .any(|connector| tail.contains(connector))
    {
        return false;
    }
    tail.matches("还是").count()
        + tail.matches("或者").count()
        + tail.matches("或是").count()
        + tail.matches('；').count()
        + tail.matches(';').count()
        >= 2
}

fn strip_manifest_list_marker(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    for marker in ["- ", "* ", "+ ", "• ", "· "] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return Some(rest.trim_start());
        }
    }

    if let Some(rest) = strip_numbered_marker(trimmed) {
        return Some(rest.trim_start());
    }

    None
}

fn strip_numbered_marker(line: &str) -> Option<&str> {
    let mut digits_end = 0usize;
    for (idx, ch) in line.char_indices() {
        if ch.is_ascii_digit() {
            digits_end = idx + ch.len_utf8();
        } else {
            break;
        }
    }
    if digits_end > 0 {
        let rest = &line[digits_end..];
        for marker in [".", ")", "、"] {
            if let Some(after) = rest.strip_prefix(marker) {
                return Some(after);
            }
        }
    }

    let after_open = line.strip_prefix('(')?;
    let mut inner_digits_end = 0usize;
    for (idx, ch) in after_open.char_indices() {
        if ch.is_ascii_digit() {
            inner_digits_end = idx + ch.len_utf8();
        } else {
            break;
        }
    }
    if inner_digits_end == 0 {
        return None;
    }
    after_open[inner_digits_end..].strip_prefix(')')
}

fn clean_manifest_dump_heading(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    for cue in [
        "关键事实",
        "你已经确认",
        "你已经能确定",
        "能确定的是",
        "现在能确认",
        "你已经知道",
        "你得到的信息",
        "可你已经能确定",
        "你可以确定",
    ] {
        if let Some(rest) = trimmed.strip_prefix(cue) {
            let rest = rest.trim_start_matches(['：', ':', '，', ',', '。', '.', ' ']);
            return Some(rest);
        }
    }
    None
}

fn clean_manifest_fragment(fragment: &str) -> String {
    fragment
        .replace("**", "")
        .replace("__", "")
        .replace('`', "")
        .trim()
        .trim_start_matches(['：', ':', '，', ',', '。', '.', ' '])
        .trim_end_matches(['：', ':', '；', ';', '，', ',', '。', '.', ' '])
        .trim()
        .to_string()
}

fn strip_ascii_case_prefix<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    if head.eq_ignore_ascii_case(prefix) {
        text.get(prefix.len()..)
    } else {
        None
    }
}

fn naturalize_english_manifest_fragment(fragment: &str) -> String {
    let trimmed = fragment.trim();
    let field_specs = [
        ("Location:", "location"),
        ("Sight:", "sight"),
        ("Looks like:", "looks"),
        ("Sound:", "sound"),
        ("Sounds like:", "sounds"),
        ("Smell:", "smell"),
        ("Smells like:", "smells"),
        ("Reachability:", "reachability"),
        ("Safely reachable:", "reachability"),
        ("What it looks like:", "looks"),
        ("What it sounds like:", "sounds"),
        ("What it smells like:", "smells"),
    ];
    for (prefix, kind) in field_specs {
        if let Some(rest) = strip_ascii_case_prefix(trimmed, prefix) {
            let rest = clean_manifest_fragment(rest);
            if rest.is_empty() {
                return String::new();
            }
            return match kind {
                "location" => {
                    let lower = rest.to_ascii_lowercase();
                    if lower.starts_with("at ")
                        || lower.starts_with("in ")
                        || lower.starts_with("on ")
                        || lower.starts_with("near ")
                        || lower.starts_with("directly ")
                    {
                        format!("It is {rest}")
                    } else {
                        format!("It is at {rest}")
                    }
                }
                "sight" => format!("It shows {rest}"),
                "looks" => format!("It looks like {rest}"),
                "sound" | "sounds" => format!("It sounds like {rest}"),
                "smell" | "smells" => format!("It smells like {rest}"),
                "reachability" => {
                    let lower = rest.to_ascii_lowercase();
                    if lower.starts_with("it is ") {
                        rest
                    } else {
                        format!("It is {rest}")
                    }
                }
                _ => rest,
            };
        }
    }
    clean_manifest_fragment(trimmed)
}

fn join_manifest_sentences_with_style(fragments: &[String], english_style: bool) -> String {
    let suffix = if english_style { "." } else { "。" };
    let separator = if english_style { " " } else { "" };
    fragments
        .iter()
        .map(|fragment| {
            if fragment.ends_with(['。', '！', '？', '.', '!', '?']) {
                fragment.to_string()
            } else {
                format!("{fragment}{suffix}")
            }
        })
        .collect::<Vec<_>>()
        .join(separator)
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
