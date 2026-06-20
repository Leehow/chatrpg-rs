//! Axis-1 「present vs met/engaged」剧透闸（MAT.M4，§7-#6 + architect Q2 DP-2）—— **纯函数**。
//!
//! M1 让场景 `referenced_npc_ids` 进入 `active_npc_ids`，于是 World 可经玩家**未曾见过**
//! 的 NPC 反应。M4 在「在场（present）」与「已会面 / 已接触（met/engaged）」之间加一层
//! 区分（rule 6 / rule 9）：
//!
//! - 仅**在场**（当前场景具名、合格反应者）的 NPC **可**对玩家行动作出反应，但**绝不**
//!   主动（proactive）抛出玩家尚未会面就不该知道的内容（rule 6：World 只给合理反应，不
//!   追高潮）。
//! - **已会面 / 已接触**的 NPC 可被当作主动叙事杠杆（Director leverage）、可主动开口揭示
//!   （在既有 secret 门允许范围内）。
//!
//! # met/engaged 派生口径（不新增持久化）
//!
//! 复用既有**玩家暴露**信号：`Db::list_surfaced_entities`（= `PlayerExposed` + 遗留
//! `EntitySurfaced` 折叠，见 event_fold.rs `PlayerExposureProjection`）。某 active NPC 的
//! id 出现在该集合里 ⇒ 玩家见过 / 听见过它 ⇒ met/engaged；否则仅 present（un-met）。
//! 这条信号天然「玩家 HEARS it 才算」——它绝不含 `ContextSurfaced`（隐藏 context 装载）。
//!
//! # 严格无操作 / 字节级基线（NON-NEGOTIABLE）
//!
//! 本闸只在 [`MaterializationAffordanceMode::Enforce`] 时**进一步收紧**主动行为；
//! `Off` / `Shadow` ⇒ 闸**惰性**（inert）：所有 active NPC 一律按「不受限」处理，
//! 即与 M4 前完全一致（Director 用全 active 集、guidance 不追加约束）。`Off` 下本就没有
//! 派生 active NPC（M1 只在 Enforce 派生），故仍是字节级基线。

use std::collections::HashSet;

use trpg_model::MaterializationAffordanceMode;

/// 玩家暴露集里的 NPC entity_kind 取值（与 truthgraph 写穿口径一致）。
/// 仅按 entity_id 匹配即可（id 全会话唯一），kind 仅作判定辅助，不强制。
const NPC_ENTITY_KIND: &str = "npc";

/// present vs met/engaged 的派生结果（纯数据，确定性）。
///
/// `met` / `unmet` 都是 `active_npc_ids` 的**子集**，保持输入顺序（stable）。
/// 闸惰性（Off/Shadow）时：`unmet` 恒空、`met` = 全 active 集 ——
/// 即「无任何收紧」，与 M4 前等价。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MetEngagedGate {
    /// 已会面 / 已接触：可作主动杠杆、可主动揭示（在 secret 门允许内）。
    pub met: Vec<String>,
    /// 仅在场（un-met）：只能反应式（reactive），绝不主动抛未会面内容。
    pub unmet: Vec<String>,
    /// 闸是否生效（Enforce）。`false` ⇒ 惰性，调用方应走基线分支。
    pub enforced: bool,
}

impl MetEngagedGate {
    /// 惰性闸：所有 active 一律视作不受限（met），无 un-met 收紧。等价 M4 前基线。
    fn inert(active_npc_ids: &[String]) -> Self {
        MetEngagedGate {
            met: active_npc_ids.to_vec(),
            unmet: Vec::new(),
            enforced: false,
        }
    }

    /// 某 active NPC 是否仅在场（un-met）—— 仅在闸生效时可能为真。
    pub fn is_unmet(&self, npc_id: &str) -> bool {
        self.unmet.iter().any(|id| id == npc_id)
    }
}

/// 判断某 entity_id 是否在玩家暴露集里（met/engaged 信号）。
///
/// `exposed` 是 `(entity_id, entity_kind)` 列表（`Db::list_surfaced_entities` 形状）。
/// 按 id 命中即算「玩家见过」；若该行声明了 kind 且 == "npc"，更强一层确认，但 kind 缺失
/// 或不同也不否决（id 全会话唯一，以 id 为准——fail-open 在「已暴露」方向是安全的，因为
/// 这只决定「是否允许主动」，把已暴露误判成未暴露才会过度收紧）。
fn id_is_exposed(npc_id: &str, exposed: &HashSet<(&str, &str)>) -> bool {
    exposed.iter().any(|(id, _kind)| *id == npc_id)
}

/// 从 `active_npc_ids` + 玩家暴露集派生 present vs met/engaged 闸（DP-2 已会面门）。
///
/// 门控：仅 `mode.is_enforce()` 才真正划分 met/unmet；`Off` / `Shadow` ⇒ 惰性闸
/// （所有 active 视作 met、unmet 空、`enforced=false`）= M4 前基线（无玩家可见收紧）。
///
/// Enforce 下：active NPC 的 id 出现在玩家暴露集 ⇒ `met`；否则 ⇒ `unmet`（仅在场）。
/// 两子集都保持 `active_npc_ids` 输入顺序（stable，确定性）。
pub fn derive_met_engaged_gate(
    active_npc_ids: &[String],
    exposed_entities: &[(String, String)],
    mode: MaterializationAffordanceMode,
) -> MetEngagedGate {
    // Off / Shadow：闸惰性（无玩家可见收紧）→ 严格基线。
    if !mode.is_enforce() {
        return MetEngagedGate::inert(active_npc_ids);
    }
    let exposed: HashSet<(&str, &str)> = exposed_entities
        .iter()
        .map(|(id, kind)| (id.as_str(), kind.as_str()))
        .collect();
    let _ = NPC_ENTITY_KIND; // kind 仅文档化口径；id 唯一即足够判定。
    let mut met = Vec::new();
    let mut unmet = Vec::new();
    for npc_id in active_npc_ids {
        if id_is_exposed(npc_id, &exposed) {
            met.push(npc_id.clone());
        } else {
            unmet.push(npc_id.clone());
        }
    }
    MetEngagedGate {
        met,
        unmet,
        enforced: true,
    }
}

/// Director 杠杆口径：可被当作**主动**叙事杠杆（写进 visible-fact 建议）的 active NPC 集。
///
/// - 惰性闸（Off/Shadow）⇒ 返回**全** active 集（与 M4 前 `state.active_npc_ids` 完全一致
///   → 字节级基线）。
/// - Enforce ⇒ 仅返回 `met` 子集：un-met（仅在场）NPC 不作主动杠杆（rule 6 / §7-#6）。
///   un-met NPC 仍可反应（World 反应路径不被此函数移除），只是不被 Director 主动推。
pub fn director_leverage_npc_ids(gate: &MetEngagedGate) -> Vec<String> {
    // 惰性时 met 已 = 全 active 集；Enforce 时 met = 已会面子集。两路统一取 met。
    gate.met.clone()
}

/// 为「仅在场（un-met）」的 active NPC 追加一条**反应式约束**到既有 NPC 行为指引块尾。
///
/// 这是**纯渲染追加**：绝不改既有 `to_guidance_block` 字节、绝不改 secret 门、绝不落库。
/// - 惰性闸 / 无 un-met ⇒ 返回 `base` 原样（`None` 仍 `None`）= 字节级基线。
/// - Enforce ∧ 有 un-met ⇒ 在 base 块后追加一段约束，列出 un-met NPC id，要求它们
///   **只可反应、不可主动抛出玩家尚未会面就不该知道的内容**（rule 6：不追高潮）。
///
/// 约束只提 NPC id（与 guidance 块同口径），绝不含任何 GM-only secret 文本 / 未会面内容。
pub fn restrict_unmet_npc_guidance(base: Option<String>, gate: &MetEngagedGate) -> Option<String> {
    if !gate.enforced || gate.unmet.is_empty() {
        return base; // 基线：不追加。
    }
    let block = format!(
        "[npc_presence_gate]\nThe player has NOT yet met/engaged these present NPCs: {}.\nThey MAY react to the player's action, but MUST NOT volunteer information, secrets, \
or plot the player has not yet earned through meeting/engaging them — react reasonably, \
never chase a climax (rule 6). An un-met NPC speaks only reactively.\n[/npc_presence_gate]",
        gate.unmet.join(", ")
    );
    match base {
        Some(b) if !b.trim().is_empty() => Some(format!("{b}\n\n{block}")),
        // base 为空但有 un-met active NPC：约束块本身即指引（dynamic tail 会写它）。
        _ => Some(block),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::MaterializationAffordanceMode::{Enforce, Off, Shadow};

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn exposed(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    // ===== TDD #1+#2 基座：Enforce 下 present 划分 met / unmet =====

    #[test]
    fn enforce_partitions_present_into_met_and_unmet() {
        let active = ids(&["npc_met", "npc_unmet"]);
        let seen = exposed(&[("npc_met", "npc")]);
        let gate = derive_met_engaged_gate(&active, &seen, Enforce);
        assert!(gate.enforced);
        assert_eq!(gate.met, ids(&["npc_met"]));
        assert_eq!(gate.unmet, ids(&["npc_unmet"]));
        assert!(gate.is_unmet("npc_unmet"));
        assert!(!gate.is_unmet("npc_met"));
    }

    #[test]
    fn exposed_match_is_by_id_even_if_kind_differs() {
        // id 全会话唯一：kind 标注不同（或缺失）也按 id 命中（fail-open 于「已暴露」方向）。
        let active = ids(&["npc_x"]);
        let seen = exposed(&[("npc_x", "")]);
        let gate = derive_met_engaged_gate(&active, &seen, Enforce);
        assert_eq!(gate.met, ids(&["npc_x"]), "id 命中即 met，不强制 kind");
        assert!(gate.unmet.is_empty());
    }

    // ===== TDD #2: Director 杠杆只用 met（un-met 不作主动杠杆）；met 仍可 =====

    #[test]
    fn director_leverage_uses_only_met_under_enforce() {
        let active = ids(&["npc_met", "npc_unmet"]);
        let seen = exposed(&[("npc_met", "npc")]);
        let gate = derive_met_engaged_gate(&active, &seen, Enforce);
        // 主动杠杆只含已会面 NPC；仅在场的 npc_unmet 被排除。
        assert_eq!(director_leverage_npc_ids(&gate), ids(&["npc_met"]));
    }

    // ===== TDD #1/#2: un-met NPC 行为指引被追加「只可反应」约束（不主动揭示）=====

    #[test]
    fn unmet_npc_guidance_gets_reactive_only_constraint() {
        let active = ids(&["npc_unmet"]);
        let gate = derive_met_engaged_gate(&active, &exposed(&[]), Enforce);
        let out = restrict_unmet_npc_guidance(Some("[npc_behavior_guidance npc=npc_unmet]\n…\n[/npc_behavior_guidance]".into()), &gate);
        let s = out.unwrap();
        assert!(s.contains("[npc_presence_gate]"), "应追加在场闸约束块");
        assert!(s.contains("npc_unmet"), "约束应点名 un-met NPC id");
        assert!(s.contains("only reactively") || s.contains("reactively"), "应表达只可反应");
        // 约束只提 id，绝不含 secret 文本。
        assert!(!s.contains("secret_value"));
    }

    #[test]
    fn met_only_set_appends_no_constraint() {
        // 全部 active NPC 已会面 → 无 un-met → 不追加（基线）。
        let active = ids(&["npc_met"]);
        let gate = derive_met_engaged_gate(&active, &exposed(&[("npc_met", "npc")]), Enforce);
        let base = Some("[npc_behavior_guidance npc=npc_met]\n…\n[/npc_behavior_guidance]".to_string());
        assert_eq!(
            restrict_unmet_npc_guidance(base.clone(), &gate),
            base,
            "无 un-met：不追加约束（字节不变）"
        );
    }

    // ===== TDD #3: Off / Shadow → 严格无操作（字节级基线）=====

    #[test]
    fn off_mode_inert_no_filtering() {
        let active = ids(&["npc_a", "npc_b"]);
        // 即便玩家暴露集只含 npc_a，Off 也不得划分 / 不得收紧。
        let gate = derive_met_engaged_gate(&active, &exposed(&[("npc_a", "npc")]), Off);
        assert!(!gate.enforced, "Off：闸惰性");
        assert_eq!(gate.met, active, "Off：全 active 视作不受限（= 基线）");
        assert!(gate.unmet.is_empty(), "Off：无 un-met 收紧");
        // Director 杠杆 = 全 active 集（字节级基线）。
        assert_eq!(director_leverage_npc_ids(&gate), active);
        // guidance 不追加约束。
        let base = Some("BASE".to_string());
        assert_eq!(restrict_unmet_npc_guidance(base.clone(), &gate), base);
    }

    #[test]
    fn shadow_mode_inert_no_player_visible_effect() {
        let active = ids(&["npc_a", "npc_b"]);
        let gate = derive_met_engaged_gate(&active, &exposed(&[("npc_a", "npc")]), Shadow);
        assert!(!gate.enforced, "Shadow：无独立 audit sink，对 M4 等同非应用");
        assert_eq!(gate.met, active);
        assert!(gate.unmet.is_empty());
        assert_eq!(director_leverage_npc_ids(&gate), active);
        assert_eq!(
            restrict_unmet_npc_guidance(None, &gate),
            None,
            "Shadow：None 仍 None（无玩家可见效果）"
        );
    }

    #[test]
    fn off_with_empty_active_is_empty_baseline() {
        let gate = derive_met_engaged_gate(&[], &exposed(&[]), Off);
        assert!(gate.met.is_empty() && gate.unmet.is_empty() && !gate.enforced);
        assert!(restrict_unmet_npc_guidance(None, &gate).is_none());
    }

    // ===== TDD #5: GENERIC —— 无任何 ruleset/module 名分支 =====

    #[test]
    fn generic_no_ruleset_or_module_branch_needed() {
        // 任意 NPC id 命名（不同「规则集」）都同样按玩家暴露集 data-driven 划分。
        for (active_id, seen_id, expect_met) in [
            ("coc_keeper_npc", "coc_keeper_npc", true),
            ("dnd_tavern_npc", "someone_else", false),
            ("生造模组_村长", "生造模组_村长", true),
        ] {
            let active = ids(&[active_id]);
            let seen = exposed(&[(seen_id, "npc")]);
            let gate = derive_met_engaged_gate(&active, &seen, Enforce);
            assert_eq!(
                gate.met.contains(&active_id.to_string()),
                expect_met,
                "id={active_id} seen={seen_id} 应纯 data-driven 判定"
            );
        }
    }

    // ===== 确定性：met/unmet 保持 active 输入顺序 =====

    #[test]
    fn partition_preserves_active_input_order() {
        let active = ids(&["c", "a", "b"]);
        let seen = exposed(&[("a", "npc")]);
        let gate = derive_met_engaged_gate(&active, &seen, Enforce);
        assert_eq!(gate.met, ids(&["a"]));
        assert_eq!(gate.unmet, ids(&["c", "b"]), "un-met 保持 active 输入顺序");
    }
}
