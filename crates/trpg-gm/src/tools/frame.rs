//! 批2 战斗 skill frame 工具（spec §5）：
//! - `open_combat_frame {participants, stakes}` → 开/武装 Combat StateFrame。
//!   frame 即姿态投影（spec §4.1）：BP3 注入走既有 `state_frame_blocks_for_turn`
//!   通道（StateFrame::to_context_block 整体投影 working_state），无需第二条管线。
//! - `close_frame {summary}` → 复用一期 conflict kernel 压缩原语
//!   `CombatAgent::compact_frame` 落 FrameCompaction，再经
//!   `InteractionLifecycleKernel::close_frame` 级联关帧；close 成功是 combat
//!   mode_exit 退出义务的确定性闭合事件（exit_mode 随之放行）。
//! - `novelty_block`：一期 novelty director 的防重复战术数据
//!   （working_state.npc_tactic_memory）渲染为 BP3 尾段事实块——纯函数零 LLM 调用。
//! 零 per-ruleset 硬编码：combat 是姿态（mode 包数据）不是规则集。

use crate::ledger::TurnLedger;
use crate::tools::{GmTool, ToolCtx, ToolError, ToolOutput, ToolSpec};
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use serde_json::{json, Map, Value};
use trpg_combat::CombatAgent;
use trpg_interaction::InteractionLifecycleKernel;
use trpg_model::{FrameKind, FrameStatus, InteractionContextKind, NpcTacticMemory, Scope, ScopeType, StateFrame};
use uuid::Uuid;

fn frame_is_live(frame: &StateFrame) -> bool {
    matches!(frame.status, FrameStatus::Active | FrameStatus::Paused | FrameStatus::Resolving)
}

fn required_str<'v>(args: &'v Value, key: &str, tool: &str) -> Result<&'v str> {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
        .ok_or_else(|| ToolError::recoverable("invalid_arguments", format!("{tool} requires a non-empty '{key}' string"), None))
}

fn required_str_vec(args: &Value, key: &str, tool: &str) -> Result<Vec<String>> {
    let items = args.get(key).and_then(Value::as_array)
        .ok_or_else(|| ToolError::recoverable("invalid_arguments", format!("{tool} requires '{key}' as an array of actor ids"), None))?;
    let out: Vec<String> = items.iter()
        .filter_map(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
        .collect();
    if out.is_empty() || out.len() != items.len() {
        return Err(ToolError::recoverable("invalid_arguments", format!("{tool} '{key}' must be a non-empty array of non-empty strings"), None));
    }
    Ok(out)
}

/// 一期 novelty 数据复用（spec §5）：live frame 的 working_state.npc_tactic_memory
/// → "[combat_novelty]" 事实块。fail-closed：缺键/空表/形状不对/frame 已关 → None。
pub fn novelty_block(frames: &[StateFrame]) -> Option<String> {
    let mut lines = Vec::new();
    for frame in frames.iter().filter(|f| frame_is_live(f)) {
        let Some(raw) = frame.working_state.get("npc_tactic_memory") else { continue };
        let entries: Vec<NpcTacticMemory> = serde_json::from_value(raw.clone()).unwrap_or_default();
        for e in &entries {
            lines.push(format!("- {}: 已用战术 {}（本帧 ×{}，连续 {} 次）", e.npc_id, e.tactic_id, e.used_count_in_frame, e.consecutive_count));
        }
    }
    if lines.is_empty() { return None; }
    Some(format!(
        "[combat_novelty]\n以下敌方战术本场战斗已经用过（一期 novelty 防重复数据，事实非建议）。不要逐字重复：换战术、换目标或改变压力点。\n{}\n[/combat_novelty]",
        lines.join("\n")
    ))
}

/// open_combat_frame {participants, stakes}（spec §5 frame 工具）。
/// 已有 live Combat frame（如 enter_mode 刚开的姿态 frame）→ 就地武装
/// （merge working_state，绝不开第二个）；没有 → 新建（frame 即姿态：下一回合
/// 头部推导自动进入 combat mode）。已在其它姿态内 → 嵌套拒绝（栈深 1）。
pub struct OpenCombatFrameTool;

#[async_trait]
impl GmTool for OpenCombatFrameTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec { name: "open_combat_frame", schema: json!({"type":"function","function":{"name":"open_combat_frame","description":"Open (or arm) the combat state frame at the start of an engagement: who is in the fight and what is at stake. The frame's participants, stakes and tactic memory project into the GM context every turn. Reuses the existing combat-mode frame instead of duplicating it.","parameters":{"type":"object","properties":{"participants":{"type":"array","items":{"type":"string"},"description":"Actor ids in the engagement, e.g. [\"pc.current\",\"npc.ghoul\"]."},"stakes":{"type":"string","description":"What this fight is about / what is at risk, in one or two sentences."}},"required":["participants","stakes"]}}}) }
    }

    async fn call(&self, ctx: &ToolCtx<'_>, _ledger: &mut TurnLedger, args: Value) -> Result<ToolOutput> {
        let participants = required_str_vec(&args, "participants", "open_combat_frame")?;
        let stakes = required_str(&args, "stakes", "open_combat_frame")?.to_string();
        // 嵌套 fail-closed：已在非 combat 姿态内不得开战斗 frame（栈深 1）。
        if let Some(active) = ctx.current_mode {
            if active != "combat" {
                return Err(ToolError::recoverable(
                    "mode_nesting_unsupported",
                    format!("already in mode '{active}'; open_combat_frame would nest combat inside it (stack depth 1)"),
                    Some("Exit the current mode first via exit_mode, then enter combat.".to_string()),
                ));
            }
        }
        let frames = ctx.engine.db.list_active_state_frames(&ctx.request.session_id, 8).await.unwrap_or_default();
        let existing = frames.into_iter().find(|f| frame_is_live(f) && matches!(f.frame_kind, FrameKind::Combat));
        let now = Utc::now();
        let (mut frame, armed_existing) = match existing {
            Some(frame) => (frame, true),
            None => (StateFrame {
                frame_id: format!("frame_{}", Uuid::new_v4().simple()),
                frame_kind: FrameKind::Combat,
                session_id: ctx.request.session_id.clone(),
                ruleset_id: ctx.request.ruleset_id.clone(),
                module_id: ctx.request.module_id.clone(),
                parent_frame_id: None,
                scope: Scope { scope_type: ScopeType::Session, scope_id: ctx.request.session_id.clone() },
                status: FrameStatus::Active,
                title: "Combat encounter".to_string(),
                objective: stakes.clone(),
                static_refs: vec![],
                working_state: json!({}),
                active_gate_ids: vec![],
                local_clocks: vec![],
                local_facts: vec![],
                local_modifiers: vec![],
                event_count: 0,
                last_event_ids: vec![],
                retention_policy: Default::default(),
                compaction_policy: Default::default(),
                created_at: now,
                updated_at: now,
            }, false),
        };
        // merge：participants/stakes 覆盖，无关键（entered_reason 等）保留；
        // cluster_count / npc_tactic_memory 仅缺省时播种（武装不清零战术记忆）。
        let mut ws: Map<String, Value> = frame.working_state.as_object().cloned().unwrap_or_default();
        ws.insert("mode_id".to_string(), json!("combat"));
        ws.insert("participants".to_string(), json!(participants));
        ws.insert("stakes".to_string(), json!(stakes));
        ws.entry("cluster_count".to_string()).or_insert(json!(0));
        ws.entry("npc_tactic_memory".to_string()).or_insert(json!([]));
        frame.working_state = Value::Object(ws);
        frame.objective = stakes.clone();
        frame.updated_at = now;
        ctx.engine.db.upsert_state_frame(&frame).await
            .map_err(|e| ToolError::fatal("internal_error", format!("failed to open combat frame: {e}")))?;
        // BP3/interaction 投影注册：interaction context 跟帧（与一期 create_frame
        // 同款，失败不阻断——state_frame_blocks_for_turn 不依赖它）。
        let _ = InteractionLifecycleKernel::new(ctx.engine.db.clone())
            .open_context_for_frame(&frame, InteractionContextKind::Frame).await;
        Ok(ToolOutput::ok(json!({
            "frame_id": frame.frame_id,
            "armed_existing_frame": armed_existing,
            "participants": frame.working_state.get("participants"),
            "stakes": stakes,
            "note": "Combat frame is live: participants/stakes/tactic memory project into your context each turn. Settle every engagement cluster via roll_check + apply_effect/change_track before narrating past it; close_frame {summary} when the fight resolves."
        })))
    }
}

/// close_frame {summary}（spec §5）：先压缩（一期 conflict kernel 原语）后关帧
/// （InteractionLifecycleKernel 级联），并闭合该姿态的 mode_exit 退出义务。
pub struct CloseFrameTool;

#[async_trait]
impl GmTool for CloseFrameTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec { name: "close_frame", schema: json!({"type":"function","function":{"name":"close_frame","description":"Close the active combat frame when the engagement resolves: compacts the frame into a durable summary (consequences, costs, open hooks) and archives transient events, then closes it. Settles the combat exit obligation so exit_mode can pass. Call only after every cluster's effects are booked.","parameters":{"type":"object","properties":{"summary":{"type":"string","description":"Durable outcome of the fight: consequences, costs, relationship shifts, open hooks."}},"required":["summary"]}}}) }
    }

    async fn call(&self, ctx: &ToolCtx<'_>, _ledger: &mut TurnLedger, args: Value) -> Result<ToolOutput> {
        let summary = required_str(&args, "summary", "close_frame")?.to_string();
        let frames = ctx.engine.db.list_active_state_frames(&ctx.request.session_id, 8).await.unwrap_or_default();
        // 目标 = 当前姿态的 frame：mode_id 即 frame_kind（框架约定，mode.rs 同款）；
        // 头部推导 None（同回合刚开帧）→ 回退首个带已安装 mode 包语义的 live
        // Combat frame（open_combat_frame 只产 Combat）。
        let target = match ctx.current_mode {
            Some(mode) => frames.into_iter().find(|f| frame_is_live(f) && f.frame_kind.as_str() == mode),
            None => frames.into_iter().find(|f| frame_is_live(f) && matches!(f.frame_kind, FrameKind::Combat)),
        };
        let Some(frame) = target else {
            return Err(ToolError::recoverable(
                "frame_not_found",
                "no live posture frame to close in this session",
                Some("Open one with open_combat_frame first; if the fight is already closed, just exit_mode.".to_string()),
            ));
        };
        // 1. 压缩（一期原语复用：耐久后果落 FrameCompaction，事件归档）。
        let compaction = CombatAgent::from_env_or_default(ctx.engine.db.clone())
            .compact_frame(&frame, &format!("close_frame: {summary}"))
            .await
            .map_err(|e| ToolError::fatal("internal_error", format!("frame compaction failed: {e}")))?;
        // 2. 级联关帧（gate/check 终态化 + frame status=completed + generation bump）。
        InteractionLifecycleKernel::new(ctx.engine.db.clone())
            .close_frame(&ctx.request.session_id, &frame.frame_id, format!("close_frame: {summary}"))
            .await
            .map_err(|e| ToolError::fatal("internal_error", format!("failed to close frame: {e}")))?;
        // 3. close 成功 = 该姿态退出义务的确定性闭合事件（exit_mode 放行；
        //    机械债务 blocking() 仍独立门控）。mode_id 即 frame_kind（框架约定）。
        if let Some(cell) = ctx.obligations {
            cell.lock().unwrap_or_else(|p| p.into_inner())
                .clear_mode_exit_obligations(frame.frame_kind.as_str());
        }
        Ok(ToolOutput::ok(json!({
            "closed_frame_id": frame.frame_id,
            "compaction_id": compaction.compaction_id,
            "summary_markdown": compaction.summary_markdown,
            "note": "Frame compacted and closed; exit obligations settled. Call exit_mode to return to the narration posture."
        })))
    }
}

#[cfg(test)]
#[path = "frame_tests.rs"]
mod tests;
