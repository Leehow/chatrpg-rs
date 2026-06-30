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

use trpg_model::{DomainEvent, DomainEventKind, MaterializationAffordanceMode};

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

/// MAT.M7 (D2) PURE：玩家本回合**主动接触/对抗**某个 active NPC ⇒ 该 NPC 即「已会面」。
///
/// # 为什么需要这条路（M6 暴露的 met/engaged 死锁）
///
/// `met_engaged` 把「已会面」派生自玩家暴露集（`PlayerExposed`/`EntitySurfaced`）。但此前
/// **唯一**发 `PlayerExposed` 的路径是「NPC 名字/别名已出现在玩家可见念白里」
/// （[`crate::truthgraph::player_exposed_events_for_scene_narration`]）。而 M4 的 un-met 闸
/// （[`restrict_unmet_npc_guidance`]）又禁止未会面 NPC 主动自报身份——于是一个戏剧价值
/// 就在「自报身份」的 NPC（如 cyberpunk `npc.athena`）永远进不了「已会面」：没会面就不能
/// 被点名、不被点名就不会面。M6 实跑 19 回合 `PlayerExposed=0`，Athena reveal 不可达。
///
/// # 解法（DP-2 原意：玩家**选择**接触即是「会面」之举）
///
/// 玩家**故意**对一个在场 active NPC 发起接触/对抗（如对它跑对抗检定 / open-fire / 接触
/// 握手）——这一**玩家主动**的行为本身就是「met」之举（不是 NPC 自报，不破坏剧透闸）。
/// 故在此情形为该 NPC 发一条 `PlayerExposed`，`met_engaged` 即据此把它判为已会面，
/// `restrict_unmet_npc_guidance` 不再约束它，它方可（在既有 secret 门允许内）开口揭示。
///
/// # 信号口径（确定性、source-grounded，非 LLM 猜测）
///
/// `engaged_npc_id` 必须是引擎已**确定性解析**的接触目标——M7 接线点取
/// `opposed_binding.persona.actor_id`（对抗预 pass 已按 id 命中场景 NPC、fail-closed）。
/// 本函数再做一道收口：该 id 必须落在 `active_npc_ids` 集内（present 才谈得上「会面」），
/// 否则不发（绝不凭空把非在场 id 标为已会面）。
///
/// # 严格基线
///
/// 仅 `mode.is_enforce()` 才产事件；Off/Shadow ⇒ 恒空（无新事件 ⇒ 玩家暴露投影不变 ⇒
/// 字节级基线）。事件 id `de_exposed_{session}_{npc}` 与念白路径同口径 ⇒ on-conflict-do-nothing
/// 幂等折叠（同一 NPC 既被念白暴露又被接触暴露，仍是一行）。
pub fn player_engaged_met_events(
    session_id: &str,
    turn_id: &str,
    active_npc_ids: &[String],
    engaged_npc_id: &str,
    mode: MaterializationAffordanceMode,
) -> Vec<DomainEvent> {
    if !mode.is_enforce() {
        return Vec::new(); // Off/Shadow：不产事件 → 字节级基线。
    }
    let engaged = engaged_npc_id.trim();
    if engaged.is_empty() {
        return Vec::new();
    }
    // 收口：接触目标必须是本回合在场（active）NPC——present 才谈得上「会面」。
    if !active_npc_ids.iter().any(|id| id == engaged) {
        return Vec::new();
    }
    let data = serde_json::json!({
        "entity_id": engaged,
        "entity_kind": NPC_ENTITY_KIND,
        "reason": "player deliberately engaged this present NPC (opposed/contact interaction)",
    });
    vec![DomainEvent::new(
        format!("de_exposed_{session_id}_{engaged}"),
        session_id,
        turn_id,
        DomainEventKind::PlayerExposed,
        data,
    )]
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::MaterializationAffordanceMode::{Enforce, Off, Shadow};

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn exposed(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
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
        let out = restrict_unmet_npc_guidance(
            Some("[npc_behavior_guidance npc=npc_unmet]\n…\n[/npc_behavior_guidance]".into()),
            &gate,
        );
        let s = out.unwrap();
        assert!(s.contains("[npc_presence_gate]"), "应追加在场闸约束块");
        assert!(s.contains("npc_unmet"), "约束应点名 un-met NPC id");
        assert!(
            s.contains("only reactively") || s.contains("reactively"),
            "应表达只可反应"
        );
        // 约束只提 id，绝不含 secret 文本。
        assert!(!s.contains("secret_value"));
    }

    #[test]
    fn met_only_set_appends_no_constraint() {
        // 全部 active NPC 已会面 → 无 un-met → 不追加（基线）。
        let active = ids(&["npc_met"]);
        let gate = derive_met_engaged_gate(&active, &exposed(&[("npc_met", "npc")]), Enforce);
        let base =
            Some("[npc_behavior_guidance npc=npc_met]\n…\n[/npc_behavior_guidance]".to_string());
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
        assert!(
            !gate.enforced,
            "Shadow：无独立 audit sink，对 M4 等同非应用"
        );
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

    // ===== MAT.M7 (D2): 玩家主动接触 active NPC → 发 PlayerExposed → 解 met 死锁 =====

    /// TDD #3：Enforce 下，玩家故意接触一个 active 且 un-met 的 NPC ⇒ 发 PlayerExposed；
    /// 该事件喂回 derive_met_engaged_gate ⇒ 该 NPC 现判为 met ⇒ 不再受 un-met 约束。
    #[test]
    fn deliberate_engage_of_active_unmet_npc_emits_player_exposed_then_met() {
        let active = ids(&["npc_athena"]);
        // 接触前：未暴露 ⇒ un-met（M4 闸约束它）。
        let before = derive_met_engaged_gate(&active, &exposed(&[]), Enforce);
        assert!(before.is_unmet("npc_athena"), "接触前应为 un-met");
        assert!(
            restrict_unmet_npc_guidance(Some("BASE".into()), &before)
                .unwrap()
                .contains("[npc_presence_gate]"),
            "接触前 un-met 闸应约束它"
        );

        // 玩家主动接触 ⇒ 发 PlayerExposed。
        let evs = player_engaged_met_events("sess", "t1", &active, "npc_athena", Enforce);
        assert_eq!(evs.len(), 1, "应为被接触的 active NPC 发一条事件");
        assert_eq!(evs[0].kind, trpg_model::DomainEventKind::PlayerExposed);
        assert_eq!(
            evs[0].event_id, "de_exposed_sess_npc_athena",
            "与念白暴露同口径 ⇒ 幂等折叠"
        );
        assert_eq!(
            evs[0].data.get("entity_id").and_then(|v| v.as_str()),
            Some("npc_athena")
        );

        // 把该事件折进玩家暴露集 ⇒ 现判为 met ⇒ 不再受约束。
        let now_exposed = exposed(&[("npc_athena", "npc")]);
        let after = derive_met_engaged_gate(&active, &now_exposed, Enforce);
        assert!(!after.is_unmet("npc_athena"), "接触后应为 met");
        assert_eq!(after.met, ids(&["npc_athena"]));
        assert_eq!(
            restrict_unmet_npc_guidance(Some("BASE".into()), &after),
            Some("BASE".into()),
            "接触后 met ⇒ 不再追加 un-met 约束（恢复基线）"
        );
    }

    /// TDD #4：没有主动接触 ⇒ 仅在场 active NPC 不自动暴露 ⇒ M4 un-met 闸照旧成立。
    #[test]
    fn merely_present_active_npc_is_not_auto_exposed() {
        let active = ids(&["npc_present"]);
        // 接触目标为空 ⇒ 不发事件。
        assert!(player_engaged_met_events("sess", "t1", &active, "", Enforce).is_empty());
        // 接触一个**不在场**的 id ⇒ 不发事件（绝不把非在场标为已会面）。
        assert!(
            player_engaged_met_events("sess", "t1", &active, "npc_offscreen", Enforce).is_empty(),
            "接触目标必须是 active NPC 才发"
        );
        // 未暴露 ⇒ 仍 un-met，M4 闸仍约束。
        let gate = derive_met_engaged_gate(&active, &exposed(&[]), Enforce);
        assert!(gate.is_unmet("npc_present"));
    }

    /// TDD #5：Off / Shadow ⇒ player_engaged_met_events 恒空（字节级基线）。
    #[test]
    fn engage_event_is_noop_under_off_and_shadow() {
        let active = ids(&["npc_athena"]);
        for mode in [Off, Shadow] {
            assert!(
                player_engaged_met_events("sess", "t1", &active, "npc_athena", mode).is_empty(),
                "{mode:?}: 不产暴露事件（基线）"
            );
        }
    }

    /// TDD #6：GENERIC —— 任意 ruleset/module 命名都 data-driven，无名分支。
    #[test]
    fn engage_event_is_generic_no_name_branch() {
        for npc in [
            "coc_keeper_npc",
            "dnd_tavern_npc",
            "生造模组_村长",
            "npc.athena_drone",
        ] {
            let active = ids(&[npc]);
            let evs = player_engaged_met_events("s", "t", &active, npc, Enforce);
            assert_eq!(evs.len(), 1, "id={npc} 应纯 data-driven 发事件");
            assert_eq!(
                evs[0].data.get("entity_id").and_then(|v| v.as_str()),
                Some(npc)
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
