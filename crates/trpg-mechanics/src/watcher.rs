//! MechanicDue 产出单点（spec §5.2 阈值通路）。
//!
//! 阈值穿越检测从 `apply_outcome_resource_tracks` 的结算单点抽出为纯函数
//! `detect_crossings`（行为零变化，金样测试钉死），由两处效果落账点共用：
//! ① `after_check_resolved` → `apply_outcome_resource_tracks`（旧路径 play_turn_sse
//!    经此免费受益）；② `apply_direct_effect` 落账后。
//!
//! 工程红线：本文件无任何规则集名/轨名字面量——阈值词表只认既有 kernel 数据
//! 契约键 `at`/`direction`/`loss_in_one_go`/`consequence` + A4 新写回的
//! `followup_procedure_id`。

use crate::RefereeCombatService;
use anyhow::Result;
use serde_json::json;
use std::collections::HashSet;
use trpg_model::{
    granularity_seconds, CalendarGranularity, CheckContract, CheckResultRecord, DueSource,
    DueStatus, EngineHook, MechanicDue, MechanicEntry, ParameterOperation, RuleKernel,
};
use uuid::Uuid;

/// 阈值穿越中间产物（结算逻辑→watcher 解耦的纯数据）。
///
/// `threshold_at` 是对骨架契约的最小补充字段：cumulative 穿越的 `at` 边界值，
/// 用于把既有 `resource_threshold_consequence` CreateFact 逐字节重建
/// （金样测试 `apply_outcome_threshold_facts_unchanged` 钉死）。
#[derive(Debug, Clone)]
pub struct ThresholdCrossing {
    pub track_id: String,
    pub kind: String, // "cumulative" | "loss_in_one_go"
    pub consequence: String,
    pub followup_procedure_id: Option<String>,
    pub owner_kind: String,
    pub owner_id: String,
    pub before: i32,
    pub after: i32,
    pub threshold_at: Option<i32>,
}

/// PURE：阈值穿越检测——`apply_outcome_resource_tracks` 旧 L168–191 的
/// crossed/loss_in_one_go 判定逻辑原样抽出（行为零变化），并读取 A4 新写回的
/// `thresholds[*].followup_procedure_id`。owner_kind/owner_id 由调用方传入
/// （apply_outcome_resource_tracks 的 is_actor 二分产物 / apply_direct_effect
/// 的 target_actor）。track 无 id/name → 空 Vec（fail-closed：无法绑定轨）。
pub fn detect_crossings(
    track: &serde_json::Value,
    owner_kind: &str,
    owner_id: &str,
    before: i32,
    after: i32,
    op: ParameterOperation,
) -> Vec<ThresholdCrossing> {
    let id = match track
        .get("id")
        .and_then(|v| v.as_str())
        .or_else(|| track.get("name").and_then(|v| v.as_str()))
    {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
    let Some(ths) = track.get("thresholds").and_then(|v| v.as_array()) else {
        return out;
    };
    for th in ths {
        let cons = th.get("consequence").and_then(|v| v.as_str()).unwrap_or("threshold reached");
        let followup = th
            .get("followup_procedure_id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        // (a) cumulative crossing: value reaches `at` (edge-triggered).
        if let Some(at) = th.get("at").and_then(|v| v.as_i64()).map(|a| a as i32) {
            let below = th.get("direction").and_then(|v| v.as_str()) == Some("at_or_below");
            let crossed = if below { after <= at && before > at } else { after >= at && before < at };
            if crossed {
                out.push(ThresholdCrossing {
                    track_id: id.clone(),
                    kind: "cumulative".to_string(),
                    consequence: cons.to_string(),
                    followup_procedure_id: followup.clone(),
                    owner_kind: owner_kind.to_string(),
                    owner_id: owner_id.to_string(),
                    before,
                    after,
                    threshold_at: Some(at),
                });
            }
        }
        // (b) single-application magnitude (lose >= n in one roll).
        if let Some(n) = th.get("loss_in_one_go").and_then(|v| v.as_i64()).map(|x| x as i32) {
            let lost = before - after;
            if matches!(op, ParameterOperation::Subtract) && lost >= n {
                out.push(ThresholdCrossing {
                    track_id: id.clone(),
                    kind: "loss_in_one_go".to_string(),
                    consequence: cons.to_string(),
                    followup_procedure_id: followup,
                    owner_kind: owner_kind.to_string(),
                    owner_id: owner_id.to_string(),
                    before,
                    after,
                    threshold_at: None,
                });
            }
        }
    }
    out
}

/// PURE：crossing → 既有 `resource_threshold_consequence` CreateFact 的 fact
/// JSON，逐字节对齐重构前的字面构造（金样钉死，重构是抽函数不是改行为）。
pub fn crossing_fact(c: &ThresholdCrossing) -> serde_json::Value {
    if c.kind == "cumulative" {
        json!({"resource_track": c.track_id, "kind": "cumulative", "threshold_at": c.threshold_at, "value": c.after, "consequence": c.consequence})
    } else {
        json!({"resource_track": c.track_id, "kind": "loss_in_one_go", "lost": c.before - c.after, "value": c.after, "consequence": c.consequence})
    }
}

/// PURE：crossing → MechanicDue（due_id="due_{uuid simple}"，evidence
/// {"before","after","delta"}，status=Open，threshold_desc=crossing.consequence
/// ——无 followup_procedure_id 时 agent 凭 prose 语义处理，fail-closed 但不静默）。
pub fn due_from_crossing(session_id: &str, turn_id: &str, c: &ThresholdCrossing) -> MechanicDue {
    MechanicDue {
        due_id: format!("due_{}", Uuid::new_v4().simple()),
        session_id: session_id.to_string(),
        turn_id: turn_id.to_string(),
        source: DueSource::Threshold,
        source_track: Some(c.track_id.clone()),
        hook_event: None,
        mechanic_id: None,
        threshold_desc: c.consequence.clone(),
        followup_procedure_id: c.followup_procedure_id.clone(),
        owner_kind: c.owner_kind.clone(),
        owner_id: c.owner_id.clone(),
        evidence: json!({"before": c.before, "after": c.after, "delta": c.after - c.before}),
        status: DueStatus::Open,
        created_at: chrono::Utc::now(),
    }
}

// ===== B5：EngineHook 事件点（第三触发通路运行时侧） =====

/// 事件点入参（调用方：turn_loop 头部 TurnStart、NavigateSceneTool SceneEnter、
/// AdvanceTimeTool TimeAdvance+Calendar）。SessionEnd/DevelopmentPhase/
/// CombatStart/CombatEnd 本期仅原语支持，无引擎调用点（A3 finalize 已把无事件
/// 点的钩子降级语义并记 validation_report——缺口可观测，spec §5.2）。
#[derive(Debug, Clone)]
pub enum HookEvent {
    TurnStart,
    SceneEnter { scene_id: String },
    Rest,
    /// calendar 跨界检测也在此分支内完成（不发明第二个事件变体）。
    TimeAdvance { from_tick: i64, to_tick: i64 },
    SessionEnd,
    DevelopmentPhase,
    CombatStart,
    CombatEnd,
}

/// PURE：hook × 事件匹配。Calendar 变体单独走 `calendar_crossed`（恒 false）；
/// 其余变体一一对应（serde tag 值即 due.hook_event 字符串）。
pub fn hook_matches_event(hook: &EngineHook, event: &HookEvent) -> bool {
    matches!(
        (hook, event),
        (EngineHook::TurnStart, HookEvent::TurnStart)
            | (EngineHook::SceneEnter, HookEvent::SceneEnter { .. })
            | (EngineHook::Rest, HookEvent::Rest)
            | (EngineHook::TimeAdvance, HookEvent::TimeAdvance { .. })
            | (EngineHook::SessionEnd, HookEvent::SessionEnd)
            | (EngineHook::DevelopmentPhase, HookEvent::DevelopmentPhase)
            | (EngineHook::CombatStart, HookEvent::CombatStart)
            | (EngineHook::CombatEnd, HookEvent::CombatEnd)
    )
}

/// PURE：日历跨界判定——`granularity_seconds(g)` 为 Some(s) 且 s>0 时
/// `from_tick / s != to_tick / s`（整除地板跨界；world_tick 增量即秒数，
/// tick 非负）。granularity 认不出或算出 0（如 segments_per_day>86400
/// 整除塌成 0，LLM 编译数据可达）→ false（fail-closed：不猜、不发 due、不 panic）。
pub fn calendar_crossed(g: &CalendarGranularity, from_tick: i64, to_tick: i64) -> bool {
    match granularity_seconds(g) {
        Some(s) if s > 0 => from_tick / s != to_tick / s,
        _ => false,
    }
}

/// EngineHook serde tag 值（`#[serde(tag="event", rename_all="snake_case")]` 契约）。
fn hook_tag(hook: &EngineHook) -> &'static str {
    match hook {
        EngineHook::SceneEnter => "scene_enter",
        EngineHook::TurnStart => "turn_start",
        EngineHook::TimeAdvance => "time_advance",
        EngineHook::Rest => "rest",
        EngineHook::CombatStart => "combat_start",
        EngineHook::CombatEnd => "combat_end",
        EngineHook::Calendar { .. } => "calendar",
        EngineHook::SessionEnd => "session_end",
        EngineHook::DevelopmentPhase => "development_phase",
    }
}

/// 钩子型 evidence 形状：{"event":…} + 事件携带的结构化上下文。
fn hook_evidence(hook: &EngineHook, event: &HookEvent) -> serde_json::Value {
    match (hook, event) {
        (EngineHook::Calendar { granularity }, HookEvent::TimeAdvance { from_tick, to_tick }) => json!({
            "event": "calendar", "from_tick": from_tick, "to_tick": to_tick,
            "granularity": serde_json::to_value(granularity).unwrap_or(serde_json::Value::Null),
        }),
        (_, HookEvent::TimeAdvance { from_tick, to_tick }) => {
            json!({"event": hook_tag(hook), "from_tick": from_tick, "to_tick": to_tick})
        }
        (_, HookEvent::SceneEnter { scene_id }) => json!({"event": hook_tag(hook), "scene_id": scene_id}),
        _ => json!({"event": hook_tag(hook)}),
    }
}

/// 单条命中 → MechanicDue（§3.5.1 结构化绑定：mechanic_id=entry.id）。
/// owner：tested_parameter 非空视为 actor 参数 → actor/pc.current，
/// 否则 session/{session_id}（开放枚举原值，不强塞 actor）。
fn due_from_hook(entry: &MechanicEntry, hook: &EngineHook, event: &HookEvent, session_id: &str, turn_id: &str) -> MechanicDue {
    let desc = if entry.when_to_use.trim().is_empty() { entry.description.clone() } else { entry.when_to_use.clone() };
    let (owner_kind, owner_id) = match entry.tested_parameter.as_deref().map(str::trim) {
        Some(p) if !p.is_empty() => ("actor".to_string(), "pc.current".to_string()),
        _ => ("session".to_string(), session_id.to_string()),
    };
    MechanicDue {
        due_id: format!("due_{}", Uuid::new_v4().simple()),
        session_id: session_id.to_string(),
        turn_id: turn_id.to_string(),
        source: DueSource::Hook,
        source_track: None,
        hook_event: Some(hook_tag(hook).to_string()),
        mechanic_id: Some(entry.id.clone()),
        threshold_desc: desc,
        followup_procedure_id: entry.followup_links.first().map(|l| l.procedure_id.clone()),
        owner_kind,
        owner_id,
        evidence: hook_evidence(hook, event),
        status: DueStatus::Open,
        created_at: chrono::Utc::now(),
    }
}

/// PURE：目录 × 事件 → 候选 due（db 抑制留集成层 `dues_for_hook`）。
/// Calendar 钩子只在 `TimeAdvance` 分支内经 `calendar_crossed` 闸；
/// TimeAdvance 钩子同分支直发——同一个事件变体，两种钩子。
pub fn hook_due_candidates(catalog: &[MechanicEntry], event: &HookEvent, session_id: &str, turn_id: &str) -> Vec<MechanicDue> {
    let mut out = Vec::new();
    for entry in catalog {
        for hook in &entry.hooks {
            let fired = match (hook, event) {
                (EngineHook::Calendar { granularity }, HookEvent::TimeAdvance { from_tick, to_tick }) => {
                    calendar_crossed(granularity, *from_tick, *to_tick)
                }
                _ => hook_matches_event(hook, event),
            };
            if fired {
                out.push(due_from_hook(entry, hook, event, session_id, turn_id));
            }
        }
    }
    out
}

/// 抑制键：(mechanic_id, hook_event)。任一缺失 → None（不参与抑制）。
fn due_key(d: &MechanicDue) -> Option<(String, String)> {
    Some((d.mechanic_id.clone()?, d.hook_event.clone()?))
}

impl RefereeCombatService {
    /// EngineHook 事件点查询：遍历 kernel.mechanics_catalog 中挂接该事件的
    /// 条目产 due（检测逻辑单点——所有事件点共用本原语，接线点只构造
    /// `HookEvent`）。抑制规则（防债务刷屏）：① 同 (mechanic_id, hook_event)
    /// 已有 open due → 跳过；② waived 且 waive_scope=="scene" → 跳过（场景内
    /// 豁免有效）；waive_scope=="turn" 不抑制（下回合仍提醒，spec §5.3）。
    pub async fn dues_for_hook(
        &self,
        session_id: &str,
        turn_id: &str,
        ruleset_id: &str,
        event: &HookEvent,
    ) -> Result<Vec<MechanicDue>> {
        let Some(kernel) = self.db.load_rule_kernel(ruleset_id).await? else { return Ok(Vec::new()) };
        if kernel.mechanics_catalog.is_empty() {
            return Ok(Vec::new());
        }
        let candidates = hook_due_candidates(&kernel.mechanics_catalog, event, session_id, turn_id);
        self.admit_dues(session_id, candidates).await
    }

    /// 抑制+落库共用尾段（hook 事件点与语义触发预 pass 两路共用）：
    /// ① 同 (mechanic_id, hook_event) 已有 open due → 跳过；② waived 且
    /// waive_scope=="scene" → 跳过（场景内豁免有效）；waive_scope=="turn" 不
    /// 抑制（下回合仍提醒，spec §5.3）。通过者落库（失败 .ok() 吞——主链绝不
    /// 因 watcher 持久化失败中断）并返回。
    pub async fn admit_dues(
        &self,
        session_id: &str,
        candidates: Vec<MechanicDue>,
    ) -> Result<Vec<MechanicDue>> {
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let open = self.db.list_open_mechanic_dues(session_id).await?;
        let open_keys: HashSet<(String, String)> = open.iter().filter_map(due_key).collect();
        // waive_scope 不在 MechanicDue 模型上（模型已定稿），按行补查；查不到按
        // turn 处理（不抑制——fail-closed 方向是"宁可再催，绝不静默"）。
        let waived = self.db.list_mechanic_dues_with_status(session_id, "waived").await?;
        let mut scene_waived: HashSet<(String, String)> = HashSet::new();
        for d in &waived {
            let Some(key) = due_key(d) else { continue };
            let scope: Option<String> = sqlx::query_scalar::<_, Option<String>>(
                "select waive_scope from mechanic_dues where due_id = $1",
            )
            .bind(&d.due_id)
            .fetch_optional(&self.db.pool)
            .await
            .ok()
            .flatten()
            .flatten();
            if scope.as_deref() == Some("scene") {
                scene_waived.insert(key);
            }
        }
        let mut out = Vec::new();
        for due in candidates {
            if let Some(key) = due_key(&due) {
                if open_keys.contains(&key) || scene_waived.contains(&key) {
                    continue;
                }
            }
            self.db.insert_mechanic_due(&due).await.ok();
            out.push(due);
        }
        Ok(out)
    }

    /// 阈值穿越 → due 落库。接线在结算单点：`apply_outcome_resource_tracks`
    /// 阈值分支与 `apply_direct_effect` 落账后（同一套 `detect_crossings`）。
    /// 落库失败 `.ok()` 吞——结算主链绝不因 watcher 持久化失败中断（与既有
    /// `.ok()` 风格一致）。返回本次结算产生的 dues（roll_check 工具结果同步
    /// 带回给 agent）。`result`/`kernel` 为契约保留参数（band/目录联动归后续任务）。
    pub async fn detect_threshold_dues(
        &self,
        contract: &CheckContract,
        result: &CheckResultRecord,
        kernel: &RuleKernel,
        crossings: &[ThresholdCrossing],
    ) -> Vec<MechanicDue> {
        let _ = (result, kernel);
        let mut dues = Vec::with_capacity(crossings.len());
        for c in crossings {
            let due = due_from_crossing(&contract.session_id, &contract.turn_id, c);
            self.db.insert_mechanic_due(&due).await.ok();
            dues.push(due);
        }
        dues
    }
}
