//! P1-3 ADVANCEMENT-FRONTIER CONSUMER (`TRPG_PROGRESSION_ENGINE`, default OFF).
//! The evolution of the flow-link follower from "only follow edges" to "read the
//! runtime ProgressionState + AdvancementFrontier and let the Director select focus
//! from the *legal* frontier" (设计评审 §七). Rust has already decided what is legal
//! (guards evaluated deterministically by the engine); the LLM only ranks/chooses
//! and decides how to surface it — it never invents what is unlocked, never
//! teleports the player (anti-railroad).
//!
//! Two pure helpers, both byte-identical-baseline when OFF:
//! - [`build_frontier_block`]: render the legal frontier (active beats, due
//!   trackers, open objectives) as a bounded Director context block. Empty frontier
//!   → empty string (no hollow "nothing to do" noise).
//! - [`with_frontier_clause`]: append the frontier-focus clause to the gravity
//!   system prompt (surface-not-teleport, Rust-evaluated guards, fail-closed).
use crate::progression::AdvancementFrontier;
use trpg_model::ScenarioNode;

/// Max items listed per frontier category (keeps the prompt bounded; the Director
/// ranks within the legal set, it does not need the whole book).
const FRONTIER_CAP: usize = 8;

/// Render the legal frontier as a Director context block. `scenes` is used only to
/// annotate an active unit id with its title when it is an in-graph scene. Returns
/// an empty string for an empty frontier (OFF==baseline / no hollow content).
pub fn build_frontier_block(frontier: &AdvancementFrontier, scenes: &[ScenarioNode]) -> String {
    if frontier.is_empty() {
        return String::new();
    }
    let title_of = |id: &str| {
        scenes
            .iter()
            .find(|s| s.node_id == id)
            .map(|s| s.title.as_str())
    };
    let mut out = String::from(
        "【已解锁·可推进内容（运行时 frontier；Rust 已判定合法，导演只在其中选焦/呈现）】\
        \n（内容引力：下列各项是可**就地**送到玩家当前位置的内容功能/Carrier，**非传送目标**——\
        它们不是玩家此刻要去的地点；玩家未自驱前往前，绝不据此改写玩家位置。）",
    );
    let mut section = |label: &str, ids: &[String]| {
        if ids.is_empty() {
            return;
        }
        out.push_str("\n");
        out.push_str(label);
        for id in ids.iter().take(FRONTIER_CAP) {
            match title_of(id) {
                Some(t) => out.push_str(&format!("\n  - {id} | {t}")),
                None => out.push_str(&format!("\n  - {id}")),
            }
        }
    };
    section("待推进 beat:", &frontier.active_units);
    section("到期计时/时钟:", &frontier.due_trackers);
    section("进行中目标:", &frontier.open_objectives);
    out
}

/// ⑦ 进度 frontier 选焦子句：仅当 progression 引擎 ON 时追加（纯追加，不改写基线）。
/// `enabled=false` ⇒ 原样返回 base（字节等价；OFF==baseline）。
pub fn with_frontier_clause(base: String, enabled: bool) -> String {
    if !enabled {
        return base;
    }
    let mut s = base;
    s.push('\n');
    s.push_str(FRONTIER_FOCUS_CLAUSE);
    s
}

/// ⑦ frontier 选焦 + 反铁路子句。运行时 ProgressionEngine 已用确定性 guard 算出『当前合法可
/// 推进内容(frontier)』；导演只在该集合内**排序、选焦、决定如何呈现/搬运内容功能**，绝不另造
/// 解锁项。**反铁路（绝对）**：frontier 是导演可顺势聚焦的引力、不是轨道——只搬内容/Carrier，
/// **不搬玩家位置**；玩家若停在原地/朝 frontier 外去，绝不强推、绝不 teleport（moved=false）。
/// 仍 fail-closed：呈现/切换的目标必须是 frontier 或给定衔接列表里真实存在的 id，绝不编造。
const FRONTIER_FOCUS_CLAUSE: &str = "⑦ **进度 frontier 选焦（仅在引擎 ON 时）**——运行时 ProgressionEngine 已用确定性 guard 算出『当前合法可推进内容(frontier)』（上方【已解锁·可推进内容】块；空则无新解锁）。导演只在该 frontier 集合内**排序、选焦、决定如何呈现**——优先把 frontier 中与玩家行动语义最匹配的待推进 beat/到期计时/进行中目标的**内容功能/Carrier 搬到玩家眼前**，而不是空转复述静态场景；**绝不**凭空新增 frontier 之外的解锁项（解锁由 Rust 评 guard 决定，不由你猜）。**反铁路（绝对、不可违背）**：frontier 是引力不是轨道——只搬内容/Carrier、**不搬玩家位置**；玩家若停在原地观察/试探、或朝 frontier 外去，绝不把他强推上某条进度线、绝不 teleport，此时 moved=false。**叠加而非覆盖（关键）**：本子句叠加在玩家**位置主权**(L-K)与叙述地点忠实之上、绝不凌驾——frontier beat 的**作者地点(locus)≠玩家当前位置**，那是该内容“本来写在哪”，不是玩家此刻所在；**切场(moved=true)仅当玩家本回合输入确实玩家自驱朝该 beat 的 locus 推进**，否则一律 moved=false、按内容引力把该 beat 内容就地送达玩家当前处，绝不把玩家叙述进他没声明的新位置。仍 fail-closed：呈现或切换的目标必须是 frontier 或给定衔接 beat 列表中真实存在的 node_id，绝不编造。";

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::ScenarioNode;

    fn scene(id: &str, title: &str) -> ScenarioNode {
        let mut n = ScenarioNode::default();
        n.node_id = id.into();
        n.title = title.into();
        n
    }

    fn frontier(active: &[&str], trackers: &[&str], objectives: &[&str]) -> AdvancementFrontier {
        AdvancementFrontier {
            active_units: active.iter().map(|s| s.to_string()).collect(),
            due_trackers: trackers.iter().map(|s| s.to_string()).collect(),
            open_objectives: objectives.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn empty_frontier_renders_empty_block() {
        let f = AdvancementFrontier::default();
        assert_eq!(
            build_frontier_block(&f, &[]),
            "",
            "empty frontier → no hollow block"
        );
    }

    #[test]
    fn block_lists_active_unit_with_scene_title() {
        let scenes = vec![scene("beat.escape_condo", "Escape the Condo")];
        let f = frontier(&["beat.escape_condo"], &[], &[]);
        let block = build_frontier_block(&f, &scenes);
        assert!(block.contains("beat.escape_condo"), "lists the unit id");
        assert!(
            block.contains("Escape the Condo"),
            "annotates with the in-graph title"
        );
    }

    #[test]
    fn block_lists_trackers_and_objectives() {
        let f = frontier(&[], &["tracker.scavvs_timer"], &["obj.neutralize_athena"]);
        let block = build_frontier_block(&f, &[]);
        assert!(block.contains("tracker.scavvs_timer"));
        assert!(block.contains("obj.neutralize_athena"));
    }

    #[test]
    fn block_is_bounded_by_cap() {
        let many: Vec<String> = (0..30).map(|i| format!("beat.{i}")).collect();
        let f = AdvancementFrontier {
            active_units: many,
            due_trackers: vec![],
            open_objectives: vec![],
        };
        let listed = build_frontier_block(&f, &[]).matches("beat.").count();
        assert!(
            listed <= FRONTIER_CAP,
            "listed {listed} > cap {FRONTIER_CAP}"
        );
    }

    #[test]
    fn with_frontier_clause_off_is_byte_equal() {
        let base = "GRAVITY_BASE".to_string();
        assert_eq!(with_frontier_clause(base.clone(), false), base);
    }

    #[test]
    fn with_frontier_clause_on_appends_anti_railroad() {
        let base = "GRAVITY_BASE".to_string();
        let p = with_frontier_clause(base.clone(), true);
        assert!(p.starts_with(&base), "pure append");
        assert!(p.len() > base.len());
        assert!(p.contains("frontier"), "names the frontier");
        assert!(p.contains("反铁路"), "anti-railroad guard present");
        assert!(
            p.contains("绝不 teleport") || p.contains("不搬玩家"),
            "surface content, never teleport the player"
        );
    }

    // KICK-BACK item-1: 内容引力硬化。frontier 消费必须**叠加在**玩家位置主权(L-K)/叙述地点
    // 忠实(L-O)/冻结定场压制(L-M)之上,不得覆盖;切场仅当玩家**自驱**朝该 beat 的 locus 推进。
    #[test]
    fn clause_subordinates_to_position_sovereignty_and_gates_transition_on_player_drive() {
        let p = with_frontier_clause("BASE".to_string(), true);
        assert!(
            p.contains("位置主权"),
            "clause must explicitly subordinate to player position sovereignty (L-K)"
        );
        assert!(
            p.contains("作者地点") || p.contains("locus"),
            "clause must separate the beat's authored location from the player's live position"
        );
        assert!(
            p.contains("玩家自驱") || p.contains("玩家驱动"),
            "transition (moved=true) only when the player self-drives toward the locus"
        );
    }

    #[test]
    fn block_frames_items_as_in_place_content_gravity_not_destinations() {
        let f = frontier(&["beat.x"], &[], &[]);
        let block = build_frontier_block(&f, &[]);
        assert!(
            block.contains("就地") || block.contains("搬到玩家当前"),
            "block must frame frontier items as content to deliver in place (content-gravity)"
        );
        assert!(
            block.contains("非传送目标") || block.contains("不是要去的地点"),
            "block must say these are NOT travel destinations"
        );
    }
}
