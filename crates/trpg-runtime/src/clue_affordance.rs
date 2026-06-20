//! Axis-2 线索发现能供（DP-3，MAT.M3）—— **提名层纯函数**。
//!
//! 当一次**成功的侦查类检定**（Spot Hidden / 侦查 / Search / observe）其目标解析到
//! 当前场景 `referenced_clue_ids`（model lib.rs:2067）或 `graph.clues`（lib.rs:2104）
//! 里的某条线索时，本模块**提名**揭示**一条 source-backed 线索 FACT**——即该线索的
//! `fact_id`（= 线索 id，与既有 reveal_fact 通道 fact_id=entity_id/node_id 同口径），
//! **不是线索正文逐字**。提名经 gm turn_loop 映射成 `RevealNomination` 推进既有 reveal
//! 通道，由 `presentation_commit_boundary` 在终审 Allow 后统一 commit（codex#8：复用既有
//! 类型，不另造 `ClueRevealNomination`）。
//!
//! # 粒度 / 等级（DP-3）
//!
//! `ContextSurfaced` → 提名 → `PlayerLearnedFact`。线索仅在 PresentationCommit Allow 后
//! 才成 `PlayerLearnedFact`；同回合 surface→提名→commit 是允许的。本模块只产**提名候选**
//! （纯数据），绝不落库、绝不写 DomainEvent——commit 归既有 runtime primitive。
//!
//! # 严格无操作 / 字节级基线
//!
//! 玩家可见效果（揭示线索 fact）仅 [`MaterializationAffordanceMode::Enforce`] 且
//! **reveal-gating ON** 时才产出提名候选：
//! - `Off` / `Shadow` ⇒ 空 Vec（`Off` == 字节级基线；`Shadow` 暂无独立 audit sink，
//!   按 flag 基座契约对玩家可见效果等同非应用）。
//! - gating OFF ⇒ 空 Vec（gating OFF 时 reveal_fact 回退即时 commit = F13 基线；线索能供
//!   绝不在 gating OFF 时偷偷即时揭示，故仅当 Enforce ∧ gating ON 双闸同开才提名）。
//!
//! # 零规则集硬编码（Constitution Rule 11 / §二-⑪）
//!
//! 侦查信号与线索解析全部 data-driven：用 `tested_parameter` / `check_label` /
//! `action_summary` 文本对**当前场景引用的线索**（`graph.clues` 内按 id 取别名）做范围内
//! 匹配——绝不按 `ruleset_id` / `module_id` 分支。范围只限当前场景引用线索（与
//! `truthgraph::player_exposed_events_for_scene_narration` 同口径，避免把未来场景线索误揭）。

use serde_json::Value;
use trpg_model::{MaterializationAffordanceMode, ModuleGraph, ScenarioNode};

/// 一条「成功侦查 → 揭示某 source-backed 线索 FACT」的提名候选（纯数据）。
///
/// `fact_id` = 线索 id（reveal_fact 通道 fact_id=entity_id 同口径），**非**线索正文。
/// gm turn_loop 在 reveal 通道在位时把它映射成 `RevealNomination` 推进既有边界。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClueRevealCandidate {
    /// 揭示的线索 fact id（= 线索 id）。
    pub fact_id: String,
    /// source-backed 解释（哪次检定揭示），写进 RevealNomination.reason。
    pub reason: String,
}

/// 一次已结算检定的最小输入（gm turn_loop 从 CheckResultRecord + CheckContract join 得到）。
///
/// 纯数据 + 借用，避免 runtime 依赖 gm 的 CheckContract 形状以外的东西——保持 DB-free。
pub struct ResolvedCheck<'a> {
    /// 检定是否成功（outcome["success"] == true）。
    pub success: bool,
    /// 该检定测的参数（如 "侦查" / "Spot Hidden" / "Search"）。None = 未填。
    pub tested_parameter: Option<&'a str>,
    /// 检定标签（玩家面文案，常含动作描述）。
    pub check_label: &'a str,
    /// 动作摘要（GM 面意图描述）。
    pub action_summary: &'a str,
}

/// 侦查类检定的通用语义词（中英）。零规则集分支：这是「调查」这一**玩法动作**的通用
/// 词典，不绑定任何具体规则集的技能名（CoC 的 Spot Hidden / DND 的 Investigation 等都落入）。
const INVESTIGATIVE_TERMS: &[&str] = &[
    "spot hidden",
    "search",
    "investigat",
    "perception",
    "observe",
    "examine",
    "scrutin",
    "侦查",
    "搜查",
    "搜索",
    "调查",
    "观察",
    "察觉",
    "检视",
    "勘查",
    "勘察",
    "翻找",
];

/// 判断一次检定文本是否带侦查语义（generic、data-driven）。
pub(crate) fn is_investigative(check: &ResolvedCheck<'_>) -> bool {
    let mut hay = String::new();
    if let Some(p) = check.tested_parameter {
        hay.push_str(p);
        hay.push(' ');
    }
    hay.push_str(check.check_label);
    hay.push(' ');
    hay.push_str(check.action_summary);
    let hay = hay.to_lowercase();
    INVESTIGATIVE_TERMS.iter().any(|t| hay.contains(t))
}

/// 取某线索 Value 的 id（`id` 字段）。脏数据 → None。
fn clue_id(clue: &Value) -> Option<&str> {
    clue.get("id").and_then(Value::as_str).filter(|s| !s.trim().is_empty())
}

/// 取某线索 Value 的可匹配别名（id + name/title/label）。
/// 与 truthgraph::entity_aliases 同立场：在当前场景范围内匹配，避免误揭未来线索。
fn clue_aliases(clue: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |s: &str| {
        let s = s.trim();
        if s.chars().count() >= 2 && !out.iter().any(|e: &String| e == s) {
            out.push(s.to_string());
        }
    };
    if let Some(id) = clue_id(clue) {
        push(id);
    }
    for key in ["name", "title", "label", "display_name"] {
        if let Some(text) = clue.get(key).and_then(Value::as_str) {
            push(text);
        }
    }
    out
}

/// 在 `graph.clues` 内按 id 找一条线索 Value（id 全图谱唯一）。
fn find_clue<'a>(graph: &'a ModuleGraph, clue_id_wanted: &str) -> Option<&'a Value> {
    graph
        .clues
        .iter()
        .find(|c| clue_id(c) == Some(clue_id_wanted))
}

/// 计算本次成功侦查应提名揭示的**单条** source-backed 线索 fact 候选（DP-3）。
///
/// 双闸门控：仅当 `mode.is_enforce()` ∧ `gating_on` 才可能产候选；任一关 ⇒ 空 Vec
/// （`Off` 字节级基线；gating OFF 回退 F13 即时 commit 基线，故线索能供不在此偷揭）。
///
/// 解析口径（generic）：检定须 success ∧ 带侦查语义；目标线索须**当前场景引用**
/// （`scene.referenced_clue_ids`），且其别名（id/name/title）出现在检定文本里。命中多条
/// 时取「当前场景引用顺序」首条（确定性、最低意外性），只产 1 条候选（DP-3 单条 fact）。
///
/// fail-closed：无成功 / 非侦查 / 无场景引用线索 / 文本不含任何线索别名 → 空 Vec。
pub fn clue_reveal_candidates(
    check: &ResolvedCheck<'_>,
    scene: &ScenarioNode,
    graph: &ModuleGraph,
    mode: MaterializationAffordanceMode,
    gating_on: bool,
) -> Vec<ClueRevealCandidate> {
    // 双闸：Enforce ∧ gating ON 才产玩家可见提名（Off/Shadow/gating-off ⇒ 严格无操作）。
    if !mode.is_enforce() || !gating_on {
        return Vec::new();
    }
    if !check.success || !is_investigative(check) {
        return Vec::new();
    }
    // 检定文本（小写）——用于在当前场景引用线索的别名上做范围内匹配。
    let mut hay = String::new();
    if let Some(p) = check.tested_parameter {
        hay.push_str(p);
        hay.push(' ');
    }
    hay.push_str(check.check_label);
    hay.push(' ');
    hay.push_str(check.action_summary);
    let hay = hay.to_lowercase();

    // 按当前场景引用顺序找第一条「别名出现在检定文本里」的线索 → 单条候选。
    for clue_ref in &scene.referenced_clue_ids {
        let cid = clue_ref.trim();
        if cid.is_empty() {
            continue;
        }
        // 线索须能在 graph.clues 解析（source-backed）；解析不到 → 跳过（fail-closed）。
        let Some(clue) = find_clue(graph, cid) else {
            continue;
        };
        let hit = clue_aliases(clue)
            .iter()
            .any(|alias| hay.contains(&alias.to_lowercase()));
        if hit {
            return vec![ClueRevealCandidate {
                fact_id: cid.to_string(),
                reason: format!(
                    "successful {} check examined the clue in the current scene",
                    check
                        .tested_parameter
                        .filter(|p| !p.trim().is_empty())
                        .unwrap_or("investigation")
                ),
            }];
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use trpg_model::MaterializationAffordanceMode::{Enforce, Off, Shadow};

    fn scene_with(clues: &[&str]) -> ScenarioNode {
        ScenarioNode {
            node_id: "sc01".into(),
            referenced_clue_ids: clues.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn graph_with(clues: Vec<Value>) -> ModuleGraph {
        ModuleGraph {
            clues,
            ..Default::default()
        }
    }

    fn investig(success: bool, label: &str) -> ResolvedCheck<'static> {
        // leak ok: test-only static strings.
        ResolvedCheck {
            success,
            tested_parameter: Some("侦查"),
            check_label: Box::leak(label.to_string().into_boxed_str()),
            action_summary: "玩家仔细搜查书房",
        }
    }

    // ===== TDD #1: 成功侦查命中场景线索 → 恰好一条 source-backed fact 提名 =====

    #[test]
    fn success_on_scene_clue_target_yields_one_candidate() {
        let scene = scene_with(&["clue_diary"]);
        let graph = graph_with(vec![json!({"id":"clue_diary","name":"血迹斑斑的日记","body":"全文不该逐字揭示"})]);
        let check = investig(true, "侦查血迹斑斑的日记");
        let out = clue_reveal_candidates(&check, &scene, &graph, Enforce, true);
        assert_eq!(out.len(), 1, "成功侦查命中线索应恰好提名 1 条: {out:?}");
        // fact_id 是 source-backed 线索 id，**绝非**线索正文。
        assert_eq!(out[0].fact_id, "clue_diary");
        assert!(!out[0].reason.contains("全文"), "reason 不得含线索正文");
    }

    #[test]
    fn matches_by_clue_id_in_text() {
        // 别名匹配也覆盖 id 本身（GM 检定文本带 clue id 的场景）。
        let scene = scene_with(&["clue_letter"]);
        let graph = graph_with(vec![json!({"id":"clue_letter","name":"撕碎的信"})]);
        let check = ResolvedCheck {
            success: true,
            tested_parameter: Some("Spot Hidden"),
            check_label: "search for clue_letter",
            action_summary: "examine the desk",
        };
        let out = clue_reveal_candidates(&check, &scene, &graph, Enforce, true);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].fact_id, "clue_letter");
    }

    // ===== TDD #2: 检定失败 → 无提名 =====

    #[test]
    fn failed_check_yields_no_candidate() {
        let scene = scene_with(&["clue_diary"]);
        let graph = graph_with(vec![json!({"id":"clue_diary","name":"血迹斑斑的日记"})]);
        let check = investig(false, "侦查血迹斑斑的日记");
        let out = clue_reveal_candidates(&check, &scene, &graph, Enforce, true);
        assert!(out.is_empty(), "失败检定不得提名: {out:?}");
    }

    // ===== TDD #3: Off / Shadow → 严格无操作（字节级基线）=====

    #[test]
    fn off_mode_strict_noop() {
        let scene = scene_with(&["clue_diary"]);
        let graph = graph_with(vec![json!({"id":"clue_diary","name":"血迹斑斑的日记"})]);
        let check = investig(true, "侦查血迹斑斑的日记");
        let out = clue_reveal_candidates(&check, &scene, &graph, Off, true);
        assert!(out.is_empty(), "Off: 严格无操作，字节级基线");
    }

    #[test]
    fn shadow_mode_no_player_visible_effect() {
        let scene = scene_with(&["clue_diary"]);
        let graph = graph_with(vec![json!({"id":"clue_diary","name":"血迹斑斑的日记"})]);
        let check = investig(true, "侦查血迹斑斑的日记");
        let out = clue_reveal_candidates(&check, &scene, &graph, Shadow, true);
        assert!(out.is_empty(), "Shadow: 无玩家可见效果（不提名）");
    }

    // ===== gating 闸：Enforce 但 gating OFF → 无提名（F13 基线，不偷揭）=====

    #[test]
    fn enforce_but_gating_off_yields_no_candidate() {
        let scene = scene_with(&["clue_diary"]);
        let graph = graph_with(vec![json!({"id":"clue_diary","name":"血迹斑斑的日记"})]);
        let check = investig(true, "侦查血迹斑斑的日记");
        let out = clue_reveal_candidates(&check, &scene, &graph, Enforce, false);
        assert!(out.is_empty(), "gating OFF: 回退 F13 即时 commit 基线，线索能供不偷揭");
    }

    // ===== fail-closed：非侦查检定 / 目标不在场景引用 / 解析不到线索 =====

    #[test]
    fn non_investigative_check_yields_no_candidate() {
        // 攻击/运动类检定即使成功也不触发线索能供（generic 语义闸）。
        let scene = scene_with(&["clue_diary"]);
        let graph = graph_with(vec![json!({"id":"clue_diary","name":"血迹斑斑的日记"})]);
        let check = ResolvedCheck {
            success: true,
            tested_parameter: Some("近战格斗"),
            check_label: "挥剑攻击血迹斑斑的日记守卫",
            action_summary: "attack",
        };
        let out = clue_reveal_candidates(&check, &scene, &graph, Enforce, true);
        assert!(out.is_empty(), "非侦查检定不触发线索能供: {out:?}");
    }

    #[test]
    fn clue_not_referenced_by_current_scene_is_not_revealed() {
        // 检定文本提到一条线索，但它不在当前场景引用里 → 不揭（避免误揭未来场景线索）。
        let scene = scene_with(&["clue_other"]);
        let graph = graph_with(vec![
            json!({"id":"clue_other","name":"无关物"}),
            json!({"id":"clue_future","name":"未来真凶日记"}),
        ]);
        let check = investig(true, "侦查未来真凶日记");
        let out = clue_reveal_candidates(&check, &scene, &graph, Enforce, true);
        assert!(out.is_empty(), "线索不在当前场景引用 → 不揭: {out:?}");
    }

    #[test]
    fn clue_ref_unresolvable_in_graph_is_skipped() {
        // 场景引用了一条 id，但 graph.clues 解析不到（脏引用）→ fail-closed 跳过。
        let scene = scene_with(&["clue_ghost"]);
        let graph = graph_with(vec![]);
        let check = investig(true, "侦查 clue_ghost");
        let out = clue_reveal_candidates(&check, &scene, &graph, Enforce, true);
        assert!(out.is_empty(), "线索 graph 解析不到 → fail-closed");
    }

    #[test]
    fn text_mentions_no_clue_alias_yields_no_candidate() {
        // 成功侦查但文本不含任何当前场景线索别名 → 目标未解析到线索 → 不揭。
        let scene = scene_with(&["clue_diary"]);
        let graph = graph_with(vec![json!({"id":"clue_diary","name":"血迹斑斑的日记"})]);
        let check = ResolvedCheck {
            success: true,
            tested_parameter: Some("侦查"),
            check_label: "环视四周",
            action_summary: "玩家四下张望",
        };
        let out = clue_reveal_candidates(&check, &scene, &graph, Enforce, true);
        assert!(out.is_empty(), "无线索别名命中 → 目标未解析到线索");
    }

    // ===== TDD: 多线索命中取场景引用首条（确定性，单条 fact）=====

    #[test]
    fn multiple_matches_takes_first_scene_referenced_clue() {
        let scene = scene_with(&["clue_first", "clue_second"]);
        let graph = graph_with(vec![
            json!({"id":"clue_first","name":"撕碎的信"}),
            json!({"id":"clue_second","name":"皮面日记"}),
        ]);
        // 文本同时含两条线索名字 → 取场景引用顺序首条（clue_first）。
        let check = investig(true, "侦查 撕碎的信 与 皮面日记");
        let out = clue_reveal_candidates(&check, &scene, &graph, Enforce, true);
        assert_eq!(out.len(), 1, "DP-3 单条 fact：命中多条也只产 1 条");
        assert_eq!(out[0].fact_id, "clue_first", "取场景引用顺序首条（确定性）");
    }

    // ===== TDD #6 GENERIC：无任何 ruleset/module 名分支也工作 =====

    #[test]
    fn generic_no_ruleset_or_module_branch_needed() {
        // 不同「规则集」用各自的侦查技能名都应命中——证明逻辑 data-driven、非名分支。
        let scene = scene_with(&["clue_x"]);
        let graph = graph_with(vec![json!({"id":"clue_x","name":"线索X"})]);
        for param in ["Investigation", "Spot Hidden", "侦查", "Perception"] {
            let check = ResolvedCheck {
                success: true,
                tested_parameter: Some(param),
                check_label: "examine 线索X",
                action_summary: "",
            };
            let out = clue_reveal_candidates(&check, &scene, &graph, Enforce, true);
            assert_eq!(out.len(), 1, "侦查参数 {param:?} 应 generic 命中");
            assert_eq!(out[0].fact_id, "clue_x");
        }
    }
}
