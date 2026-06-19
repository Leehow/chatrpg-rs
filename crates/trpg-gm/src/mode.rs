//! 三期姿态框架核心（spec §4）：skill = 姿态（posture），mode = active
//! state_frame 的投影；mode 包 = data/agent/gm_skill/modes/<mode>/
//! {manifest.json + 提示 md}，全 data-driven 零 per-ruleset 硬编码。
//! fail-closed：manifest 缺失/解析失败/mode_id 与文件夹不一致 → 配置错误 Err；
//! mode=None 路径与二期行为字节级一致（prompts 回归测试护）。

use crate::ledger::TurnLedger;
use crate::tools::{GmTool, ToolCtx, ToolError, ToolOutput, ToolSpec};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use trpg_model::{FrameKind, FrameStatus, Scope, ScopeType, StateFrame};
use uuid::Uuid;

/// mode 包 manifest（data/agent/gm_skill/modes/<mode>/manifest.json）。
/// mode_id/frame_kind 必填（缺失 = 解析失败 = fail-closed）；其余 serde default。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeManifest {
    /// "combat" | "downtime"（与文件夹名一致，开放枚举）。
    pub mode_id: String,
    /// 映射 FrameKind 的 snake_case 字符串（"combat"/"downtime"）。
    pub frame_kind: String,
    /// mode 专属工具名（registry 按名组装，未知名 fail-closed 报配置错误）。
    #[serde(default)]
    pub extra_tools: Vec<String>,
    /// 目录注入过滤器（spec §4.5，数据不是代码）。
    #[serde(default)]
    pub catalog_filter: CatalogFilter,
    /// 退出结算义务描述（挂 ObligationLedger 的 mode_exit 债务，spec §4.4）。
    #[serde(default)]
    pub exit_obligations: Vec<String>,
    /// 节拍参数（spec §4.6）。
    #[serde(default)]
    pub tempo: TempoOverrides,
}

/// 任一维度匹配即注入；三个维度全空 = 不过滤。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CatalogFilter {
    #[serde(default)]
    pub kinds: Vec<String>,
    #[serde(default)]
    pub hooks: Vec<String>,
    #[serde(default)]
    pub semantic_tags: Vec<String>,
}

impl CatalogFilter {
    /// 目录条目是否注入：kinds/hooks/semantic_tags 任一命中即真；全空 = 不过滤恒真。
    pub fn matches(&self, kind: &str, hooks: &[String], tags: &[String]) -> bool {
        if self.kinds.is_empty() && self.hooks.is_empty() && self.semantic_tags.is_empty() {
            return true;
        }
        self.kinds.iter().any(|k| k.eq_ignore_ascii_case(kind))
            || self
                .hooks
                .iter()
                .any(|h| hooks.iter().any(|x| x.eq_ignore_ascii_case(h)))
            || self
                .semantic_tags
                .iter()
                .any(|t| tags.iter().any(|x| x.eq_ignore_ascii_case(t)))
    }
}

/// 节拍覆盖（None = 沿用默认 LoopConfig / 二期门控行为）。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct TempoOverrides {
    #[serde(default)]
    pub max_tool_rounds: Option<u8>,
    #[serde(default)]
    pub effect_closure_per_cluster: Option<bool>,
}

fn modes_root(data_dir: &Path) -> PathBuf {
    data_dir.join("agent/gm_skill/modes")
}

/// mode 包 manifest 路径（存在性 = mode 已安装的判据）。
pub fn mode_manifest_path(data_dir: &Path, mode: &str) -> PathBuf {
    modes_root(data_dir).join(mode).join("manifest.json")
}

fn frame_is_live(frame: &StateFrame) -> bool {
    matches!(
        frame.status,
        FrameStatus::Active | FrameStatus::Paused | FrameStatus::Resolving
    )
}

/// active frame → mode_id（无 active frame = 默认叙事姿态 None）。
/// mode_id 即 frame_kind 字符串（mode 包文件夹按 frame_kind 命名，框架约定）；
/// 该 frame 种类是否真是已安装姿态由 active_mode_manifest 判定。
pub fn current_mode(frames: &[StateFrame]) -> Option<String> {
    frames
        .iter()
        .find(|f| frame_is_live(f))
        .map(|f| f.frame_kind.as_str().to_string())
}

/// fail-closed：manifest 不存在/解析失败/mode_id 与文件夹名不一致 → Err。
pub fn load_mode_manifest(data_dir: &Path, mode: &str) -> Result<ModeManifest> {
    let path = mode_manifest_path(data_dir, mode);
    if !path.exists() {
        return Err(anyhow!(
            "mode manifest missing: {} (mode '{mode}' is not installed under agent/gm_skill/modes)",
            path.display()
        ));
    }
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed reading {}", path.display()))?;
    let manifest: ModeManifest = serde_json::from_str(&raw)
        .with_context(|| format!("invalid mode manifest JSON: {}", path.display()))?;
    if manifest.mode_id != mode {
        return Err(anyhow!("mode manifest mode_id '{}' does not match folder '{mode}' (fail-closed configuration error)", manifest.mode_id));
    }
    Ok(manifest)
}

/// 回合头部单点：active frames → 第一个带已安装 mode 包的 frame 的 manifest。
/// 无 frame / frame 种类无 mode 包 → Ok(None)（默认叙事姿态，旧路径的
/// InvestigationNode 等非姿态 frame 照常不触发 mode）；包存在但解析失败 →
/// Err（fail-closed 配置错误，绝不静默当 None）。
pub fn active_mode_manifest(
    data_dir: &Path,
    frames: &[StateFrame],
) -> Result<Option<ModeManifest>> {
    for frame in frames.iter().filter(|f| frame_is_live(f)) {
        let mode = frame.frame_kind.as_str();
        if mode_manifest_path(data_dir, mode).exists() {
            return load_mode_manifest(data_dir, mode).map(Some);
        }
    }
    Ok(None)
}

fn parse_frame_kind(raw: &str) -> Result<FrameKind> {
    serde_json::from_value::<FrameKind>(Value::String(raw.to_string()))
        .map_err(|_| anyhow!("mode manifest frame_kind '{raw}' is not a known FrameKind (fail-closed configuration error)"))
}

fn required_str<'v>(args: &'v Value, key: &str, tool: &str) -> Result<&'v str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            ToolError::recoverable(
                "invalid_arguments",
                format!("{tool} requires a non-empty '{key}' string"),
                None,
            )
        })
}

fn ctx_data_dir<'a>(ctx: &ToolCtx<'a>, tool: &str) -> Result<&'a Path> {
    ctx.data_dir.ok_or_else(|| {
        ToolError::fatal(
            "internal_error",
            format!("{tool} requires a data_dir on the tool context (assembler wiring bug)"),
        )
    })
}

/// enter_mode {mode, reason}（spec §4.4 双通道之 agent 语义判定）。
/// 成功 = 开一个该 mode frame（姿态 = frame 投影）+ 挂退出结算义务；
/// 提示/工具/节拍从下一回合头部推导生效。
pub struct EnterModeTool;

#[async_trait]
impl GmTool for EnterModeTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "enter_mode",
            schema: json!({"type":"function","function":{"name":"enter_mode","description":"Enter a posture mode (e.g. combat, downtime). The mode's prompt overlay, tool set, catalog filter and tempo take effect from the next turn. Fails when a mode is already active (stack depth 1, no nesting) or the mode is not installed.","parameters":{"type":"object","properties":{"mode":{"type":"string","description":"Mode id, the folder name under agent/gm_skill/modes."},"reason":{"type":"string","description":"Why the fiction calls for this posture now."}},"required":["mode","reason"]}}}),
        }
    }

    async fn call(
        &self,
        ctx: &ToolCtx<'_>,
        _ledger: &mut TurnLedger,
        args: Value,
    ) -> Result<ToolOutput> {
        let mode = required_str(&args, "mode", "enter_mode")?;
        let reason = required_str(&args, "reason", "enter_mode")?.to_string();
        // 嵌套 fail-closed（栈深 1）：回合头部推导的当前姿态优先（无 db 依赖）。
        if let Some(active) = ctx.current_mode {
            return Err(nesting_error(active));
        }
        let data_dir = ctx_data_dir(ctx, "enter_mode")?;
        let manifest = load_mode_manifest(data_dir, mode).map_err(|e| {
            ToolError::recoverable(
                "mode_not_found",
                format!("mode '{mode}' unavailable: {e}"),
                Some("Only installed modes under agent/gm_skill/modes can be entered.".to_string()),
            )
        })?;
        let kind = parse_frame_kind(&manifest.frame_kind)
            .map_err(|e| ToolError::fatal("internal_error", e.to_string()))?;
        // 同回合二次 enter 兜底：头部推导只反映回合开始时刻，db 活跃 frame 再查
        // 一遍（db 失败折空 → 单测/瞬断不阻断；真嵌套由下一回合头部仍可见）。
        let frames = ctx
            .engine
            .db
            .list_active_state_frames(&ctx.request.session_id, 8)
            .await
            .unwrap_or_default();
        if let Some(active) = active_mode_manifest(data_dir, &frames)
            .map_err(|e| ToolError::fatal("internal_error", e.to_string()))?
        {
            return Err(nesting_error(&active.mode_id));
        }
        let now = Utc::now();
        let frame = StateFrame {
            frame_id: format!("frame_{}", Uuid::new_v4().simple()),
            frame_kind: kind,
            session_id: ctx.request.session_id.clone(),
            ruleset_id: ctx.request.ruleset_id.clone(),
            module_id: ctx.request.module_id.clone(),
            parent_frame_id: None,
            scope: Scope {
                scope_type: ScopeType::Session,
                scope_id: ctx.request.session_id.clone(),
            },
            status: FrameStatus::Active,
            title: format!("{} mode", manifest.mode_id),
            objective: reason.clone(),
            static_refs: vec![],
            working_state: json!({"mode_id": manifest.mode_id, "entered_reason": reason, "entered_turn": ctx.request.turn_id}),
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
        };
        ctx.engine
            .db
            .upsert_state_frame(&frame)
            .await
            .map_err(|e| {
                ToolError::fatal("internal_error", format!("failed to open mode frame: {e}"))
            })?;
        // 退出结算义务（spec §4.4）：进入即挂账（只门 exit_mode 不门叙事轮）；
        // 回合头部凭 manifest 幂等 re-seed 兜跨进程。guard 绝不跨 await。
        if let Some(cell) = ctx.obligations {
            let mut obligations = cell.lock().unwrap_or_else(|p| p.into_inner());
            obligations.ensure_mode_exit_obligations(&manifest.mode_id, &manifest.exit_obligations);
        }
        Ok(ToolOutput::ok(json!({
            "entered_mode": manifest.mode_id,
            "frame_id": frame.frame_id,
            "exit_obligations": manifest.exit_obligations,
            "note": "Mode prompt overlay, tool set and tempo take effect from the next turn (turn-head derivation)."
        })))
    }
}

fn nesting_error(active: &str) -> anyhow::Error {
    ToolError::recoverable(
        "mode_nesting_unsupported",
        format!("already in mode '{active}'; mode nesting is unsupported (stack depth 1)"),
        Some("Exit the current mode first via exit_mode (settle exit obligations or waive_obligation with a reason).".to_string()),
    )
}

/// exit_mode {reason}（退出结算义务门控 + waive 通道照常，spec §4.4）。
/// 成功 = 关闭 mode frame（复用 InteractionLifecycleKernel::close_frame 级联
/// 原语；frame 已被 mode 专属工具如 close_frame 关闭 → 幂等成功）+ 清退出义务。
pub struct ExitModeTool;

#[async_trait]
impl GmTool for ExitModeTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "exit_mode",
            schema: json!({"type":"function","function":{"name":"exit_mode","description":"Exit the active posture mode and return to the default narration posture from the next turn. Blocked while unresolved mechanical obligations or mode exit obligations remain (settle them or waive_obligation with a reason).","parameters":{"type":"object","properties":{"reason":{"type":"string","description":"Why the posture ends now."}},"required":["reason"]}}}),
        }
    }

    async fn call(
        &self,
        ctx: &ToolCtx<'_>,
        _ledger: &mut TurnLedger,
        args: Value,
    ) -> Result<ToolOutput> {
        let reason = required_str(&args, "reason", "exit_mode")?.to_string();
        let data_dir = ctx_data_dir(ctx, "exit_mode")?;
        // 当前姿态：回合头部推导优先；头部 None 时查 db（同回合刚 enter 的兜底）。
        let mode_id: String = match ctx.current_mode {
            Some(m) => m.to_string(),
            None => {
                let frames = ctx
                    .engine
                    .db
                    .list_active_state_frames(&ctx.request.session_id, 8)
                    .await
                    .unwrap_or_default();
                match active_mode_manifest(data_dir, &frames).map_err(|e| ToolError::fatal("internal_error", e.to_string()))? {
                    Some(m) => m.mode_id,
                    None => return Err(ToolError::recoverable("no_active_mode", "no posture mode is active; exit_mode only applies inside a mode", Some("In the default narration posture just keep narrating, or enter_mode first.".to_string()))),
                }
            }
        };
        // 退出结算义务门控（spec §4.4）：blocking() ∪ mode_exit 任一未清 → 拦截。
        // J2 修复（downtime frame 泄漏）：除本 mode 自身退出义务外无任何未清
        // 机械项（due/check/debt/cluster 皆空）⇒ 退出义务确定性闭合——"结算表
        // 已落账"在零应结项时为真。无确定性闭合事件的 mode（如 downtime）由此
        // 能正常退出，不再只能靠 waive 逃生（frame 永久 active 的泄漏根源）。
        if let Some(cell) = ctx.obligations {
            let outstanding = {
                cell.lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .exit_blocking()
            };
            let own_exit_prefix = format!("mode_exit.{mode_id}.");
            let only_own_exit = outstanding
                .iter()
                .all(|v| v.kind == "mode_exit" && v.target_id.starts_with(&own_exit_prefix));
            if !outstanding.is_empty() && !only_own_exit {
                let lines = outstanding
                    .iter()
                    .map(|v| format!("- {} {}: {}", v.kind, v.target_id, v.summary))
                    .collect::<Vec<_>>()
                    .join("\n");
                return Err(ToolError::recoverable(
                    "exit_blocked_by_obligations",
                    format!("exit_mode blocked by unresolved obligations:\n{lines}"),
                    Some("Settle each item (roll_check / apply_effect / change_track ...) or waive_obligation with a reason, then call exit_mode again.".to_string()),
                ));
            }
        }
        // 关帧：按 manifest.frame_kind 找活跃 mode frame；找不到（已被 mode 工具
        // 关闭/db 不可达）→ 幂等成功，下一回合头部自然回落叙事姿态。
        let manifest = load_mode_manifest(data_dir, &mode_id).map_err(|e| {
            ToolError::fatal(
                "internal_error",
                format!("active mode '{mode_id}' has no loadable manifest: {e}"),
            )
        })?;
        let frames = ctx
            .engine
            .db
            .list_active_state_frames(&ctx.request.session_id, 8)
            .await
            .unwrap_or_default();
        let live = frames
            .into_iter()
            .find(|f| frame_is_live(f) && f.frame_kind.as_str() == manifest.frame_kind);
        let closed_frame_id = match live {
            Some(frame) => {
                trpg_interaction::InteractionLifecycleKernel::new(ctx.engine.db.clone())
                    .close_frame(
                        &ctx.request.session_id,
                        &frame.frame_id,
                        format!("exit_mode: {reason}"),
                    )
                    .await
                    .map_err(|e| {
                        ToolError::fatal(
                            "internal_error",
                            format!("failed to close mode frame: {e}"),
                        )
                    })?;
                Some(frame.frame_id)
            }
            None => None,
        };
        if let Some(cell) = ctx.obligations {
            cell.lock()
                .unwrap_or_else(|p| p.into_inner())
                .clear_mode_exit_obligations(&mode_id);
        }
        Ok(ToolOutput::ok(json!({
            "exited_mode": mode_id,
            "closed_frame_id": closed_frame_id,
            "note": "Default narration posture resumes from the next turn."
        })))
    }
}

#[cfg(test)]
#[path = "mode_tests.rs"]
mod tests;
