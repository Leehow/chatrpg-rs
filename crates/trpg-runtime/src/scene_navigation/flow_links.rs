//! J3 FLOW-LINK CONSUMER（`TRPG_NAV_FOLLOW_FLOW_LINKS`，默认 OFF）：消费侧——让 runtime
//! 场景导航器读 `link_type` 并跟随 producer（commit 580a9b1，`module_flow_links.rs`）写入图谱的
//! 已授权有向流转脊（sequential/trigger/branch + source_anchor）。
//!
//! 现状根因（F-FIND）：navigator 是 `link_type`-agnostic——`build_nav_exits` 旧内联逻辑按
//! 迭代序、仅凭 `to_node_id` 列出衔接 beat，把 producer 的有向脊与 `apply_bridge_edges` 的泛
//! spatial 实体共享桥同等对待 ⇒ nav-LLM 看不见方向脊 ⇒ 30 回合零转移（J3 RED）。
//!
//! 本模块是 **additive / flag-gated**（区别于 L-C/L-Y/L-AA 的默认 ON，本 flag 默认 OFF，
//! 对标 producer 侧 `flow_links_enabled`）：
//! - C1：`build_nav_exits` 在 flag ON 时把已授权有向脊边排在 spatial 桥**之前**，并给每条
//!   脊边标注 `link_type`（让 nav 看见『已授权 canonical 下一 beat』vs 泛 spatial 桥）。
//! - C2：`with_flow_link_clause` 在 gravity system prompt 之上追加⑥流转脊优先子句——**仅当
//!   玩家已驱动**（沿用 ①②③④⑤ player-driven gate）时才 commit 到脊边；**反铁路**：玩家跑偏/
//!   原地不动 → 绝不强推、绝不 teleport（fail-closed）。
//! - C3：flag OFF ⇒ 两处皆字节等价今日 link_type-agnostic 行为（OFF==baseline）。
//!
//! 复用（绝不重造、不改写 navigator）：父模块 `build_nav_prompt_with_exits` /
//! `validate_transition`（仍 fail-closed，不按 link_type 拒绝 ⇒ 不破坏 spatial 导航/不railroad）/
//! `gravity_nav_system_prompt`（④⑤ L-Y/L-AA 机制）；`ScenarioLink`/`LinkType`。零硬编码、
//! 无 ruleset_id/module_id 分支。

use trpg_model::{LinkType, ScenarioNode};

/// J3 FLOW-LINK CONSUMER 开关：**默认 OFF**（additive、opt-in；区别于 L-C/L-Y/L-AA 默认 ON）。
/// `TRPG_NAV_FOLLOW_FLOW_LINKS=1/true/on/yes` 开。OFF ⇒ exits 列表与 system prompt 均字节等价
/// 今日 link_type-agnostic 基线。对标 producer 侧 `flow_links_enabled` 默认关形态。
pub fn nav_follow_flow_links_enabled() -> bool {
    std::env::var("TRPG_NAV_FOLLOW_FLOW_LINKS")
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "on" | "yes"))
        .unwrap_or(false)
}

/// 判一条 link 是否「已授权有向流转脊」边（producer FLOW-LINK pass 产出）：`link_type` 非
/// `Spatial`（sequential/trigger/branch/timeline）**且**带非空 `source_anchor`（摘自原文的
/// 流转线索）。`apply_bridge_edges` 的 spatial 桥（anchor=None）与 `ensure_entry_connected`
/// 的兜底 Sequential（anchor=None）均判 false。纯、确定。
pub fn is_authored_flow_link(link: &trpg_model::ScenarioLink) -> bool {
    link.link_type != LinkType::Spatial
        && link
            .source_anchor
            .as_deref()
            .map(str::trim)
            .map(|s| !s.is_empty())
            .unwrap_or(false)
}

/// 流转型中文标签（喂给 nav-LLM 让其分辨脊边语义）。纯映射。
fn flow_type_label(lt: LinkType) -> &'static str {
    match lt {
        LinkType::Sequential => "顺序衔接·canonical下一beat",
        LinkType::Trigger => "条件触发",
        LinkType::Branch => "分支选择",
        LinkType::Timeline => "时间线",
        LinkType::Spatial => "空间桥",
    }
}

/// 构建『当前场景直接衔接 beat』exits 文本（喂给 nav-LLM 作软推进上下文）。复用 tiered.rs 旧
/// 内联语义：去重 by `to_node_id`（首现优先）、in-graph、≠current、格式 `"{node_id} | {title}"`。
/// - `follow_flow_links=false` ⇒ 与历史内联逐字节一致（迭代序、无类型标注）= OFF==baseline。
/// - `follow_flow_links=true` ⇒ 已授权有向脊边（`is_authored_flow_link`）排在 spatial 桥之前
///   （组内稳定序），且每条脊边追加 `[已授权流转脊·<type>]` 标注。C1。
pub fn build_nav_exits(
    cur_node: Option<&ScenarioNode>,
    scenes: &[ScenarioNode],
    current: &str,
    follow_flow_links: bool,
) -> String {
    let Some(node) = cur_node else {
        return String::new();
    };
    let mut seen = std::collections::HashSet::new();
    // (is_authored, line)；OFF 走纯迭代序、line 无标注 ⇒ 字节等价历史内联。
    let mut entries: Vec<(bool, String)> = Vec::new();
    for link in &node.links {
        let to = link.to_node_id.trim();
        if to.is_empty() || to == current || !seen.insert(to.to_string()) {
            continue;
        }
        let Some(t) = scenes.iter().find(|s| s.node_id == to) else {
            continue;
        };
        if follow_flow_links && is_authored_flow_link(link) {
            entries.push((
                true,
                format!(
                    "{} | {} [已授权流转脊·{}]",
                    t.node_id,
                    t.title,
                    flow_type_label(link.link_type)
                ),
            ));
        } else {
            entries.push((false, format!("{} | {}", t.node_id, t.title)));
        }
    }
    if !follow_flow_links {
        // OFF：纯迭代序、无标注 ⇒ 与历史内联逐字节一致（OFF==baseline）。
        return entries
            .into_iter()
            .map(|(_, l)| l)
            .collect::<Vec<_>>()
            .join("\n");
    }
    // ON：稳定分区——已授权脊边在前，spatial/其他在后（各自保留相对序）。
    let mut ranked: Vec<String> = entries
        .iter()
        .filter(|(a, _)| *a)
        .map(|(_, l)| l.clone())
        .collect();
    ranked.extend(entries.iter().filter(|(a, _)| !*a).map(|(_, l)| l.clone()));
    ranked.join("\n")
}

/// ⑥流转脊优先子句：仅当玩家已驱动（沿 ①②③④⑤ gate）才 commit 到已授权脊边；反铁路 fail-closed。
const SCENE_NAV_FLOW_LINK_CLAUSE: &str = "⑥ **已授权流转脊优先（仅在玩家已驱动时）**——衔接 beat 列表中标注『已授权流转脊』的出口，是模组作者写定的 canonical 下一 beat（sequential=读完/顺序进入；trigger=剧情条件触发；branch=玩家选择分歧），其优先级**高于**泛 spatial（实体共享）桥。当玩家已按①②③④⑤真正驱动朝某出口推进时，应优先 commit 到与玩家行动语义最匹配的『已授权流转脊』出口（而非 spatial 桥）；trigger/branch 型出口须其授权条件（source_anchor 所摘线索）已被玩家行动或世界状态满足方可触发。**反铁路（绝对、不可违背）**：流转脊只是导演可顺势跟随的引力、不是轨道——玩家若停在原地观察/试探、或朝列表外/spatial 方向去，绝不可把他强推上流转脊、绝不 teleport；此时 moved=false。仍 fail-closed：目标必须是给定衔接 beat 列表中真实存在的 node_id，绝不编造。";

/// 在已构建的 gravity system prompt 之上按 flag 追加⑥流转脊优先子句（纯追加，不改写基线）。
/// `follow=false` ⇒ 原样返回（字节等价；OFF==baseline）。便于单测锁定 OFF 字节等价 + ON 含⑥。
pub fn with_flow_link_clause(base: String, follow: bool) -> String {
    if !follow {
        return base; // OFF==baseline：字节等价。
    }
    let mut s = base;
    s.push('\n');
    s.push_str(SCENE_NAV_FLOW_LINK_CLAUSE);
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::ScenarioLink;

    fn link(to: &str, lt: LinkType, anchor: Option<&str>) -> ScenarioLink {
        ScenarioLink {
            to_node_id: to.to_string(),
            reason: "r".into(),
            clue_id: None,
            link_type: lt,
            source_anchor: anchor.map(str::to_string),
        }
    }

    fn node(id: &str, title: &str, links: Vec<ScenarioLink>) -> ScenarioNode {
        let mut n = ScenarioNode::default();
        n.node_id = id.into();
        n.title = title.into();
        n.links = links;
        n
    }

    /// entry: 一条 spatial 桥(side, anchorless) + 一条已授权 sequential 脊(deep, anchored)。
    fn graph() -> Vec<ScenarioNode> {
        vec![
            node(
                "entry",
                "Warehouse Entry",
                vec![
                    link("side", LinkType::Spatial, None),
                    link("deep", LinkType::Sequential, Some("press deeper along the cable")),
                ],
            ),
            node("deep", "Warehouse Depths", vec![]),
            node("side", "Side Office", vec![]),
        ]
    }

    // C3 OFF==baseline：follow=false ⇒ 与历史内联逐字节一致（迭代序、无标注）。
    #[test]
    fn build_nav_exits_off_is_byte_identical_to_legacy_inline() {
        let g = graph();
        let cur = g.iter().find(|s| s.node_id == "entry");
        let exits = build_nav_exits(cur, &g, "entry", false);
        assert_eq!(
            exits, "side | Side Office\ndeep | Warehouse Depths",
            "OFF 必须与历史内联输出（迭代序、无类型标注）逐字节一致"
        );
    }

    // C3 OFF==baseline：cur_node=None ⇒ 空串（等价历史 unwrap_or_default）。
    #[test]
    fn build_nav_exits_off_none_node_is_empty() {
        let g = graph();
        assert_eq!(build_nav_exits(None, &g, "entry", false), "");
    }

    // C1：follow=true ⇒ 已授权 sequential 脊(deep)排在 spatial 桥(side)之前，且脊边带类型标注。
    #[test]
    fn build_nav_exits_on_ranks_authored_above_spatial_and_annotates() {
        let g = graph();
        let cur = g.iter().find(|s| s.node_id == "entry");
        let exits = build_nav_exits(cur, &g, "entry", true);
        let lines: Vec<&str> = exits.lines().collect();
        assert_eq!(lines.len(), 2, "两个出口都在");
        assert!(
            lines[0].starts_with("deep | Warehouse Depths"),
            "已授权 sequential 脊必须排第一，实际首行: {}",
            lines[0]
        );
        assert!(lines[0].contains("已授权流转脊"), "脊边必须标注，实际: {}", lines[0]);
        assert!(lines[0].contains("顺序衔接"), "sequential 标其类型，实际: {}", lines[0]);
        assert_eq!(lines[1], "side | Side Office", "spatial 桥排后且无标注");
    }

    // C1：trigger / branch 脊边各标其类型；spatial 桥(anchorless)始终在后且无标注。
    #[test]
    fn build_nav_exits_on_labels_trigger_and_branch() {
        let g = vec![
            node(
                "hub",
                "Hub",
                vec![
                    link("br", LinkType::Branch, Some("If the PCs flee, go to…")),
                    link("bridge", LinkType::Spatial, None),
                    link("tr", LinkType::Trigger, Some("Once they hack the server…")),
                ],
            ),
            node("br", "Branch Beat", vec![]),
            node("tr", "Trigger Beat", vec![]),
            node("bridge", "Bridge Beat", vec![]),
        ];
        let cur = g.iter().find(|s| s.node_id == "hub");
        let exits = build_nav_exits(cur, &g, "hub", true);
        let lines: Vec<&str> = exits.lines().collect();
        // 两条脊边(br, tr)在前(稳定序 br 先于 tr)，spatial bridge 最后。
        assert!(lines[0].starts_with("br | Branch Beat") && lines[0].contains("分支选择"));
        assert!(lines[1].starts_with("tr | Trigger Beat") && lines[1].contains("条件触发"));
        assert_eq!(lines[2], "bridge | Bridge Beat", "spatial 桥排最后且无标注");
    }

    // is_authored_flow_link：仅 (非 Spatial) ∧ (非空 anchor) 为真。
    #[test]
    fn is_authored_flow_link_discriminates_correctly() {
        assert!(is_authored_flow_link(&link("x", LinkType::Sequential, Some("cue"))));
        assert!(is_authored_flow_link(&link("x", LinkType::Trigger, Some("cue"))));
        assert!(is_authored_flow_link(&link("x", LinkType::Branch, Some("cue"))));
        // spatial 桥（anchor=None）→ false
        assert!(!is_authored_flow_link(&link("x", LinkType::Spatial, None)));
        // 兜底 Sequential（anchor=None）→ false（ensure_entry_connected 形态）
        assert!(!is_authored_flow_link(&link("x", LinkType::Sequential, None)));
        // 空白 anchor（trim 后空）→ false
        assert!(!is_authored_flow_link(&link("x", LinkType::Sequential, Some("   "))));
        // 即便标了 anchor，spatial 仍 false（脊只认有向型）
        assert!(!is_authored_flow_link(&link("x", LinkType::Spatial, Some("cue"))));
    }

    // flag 默认 OFF（未设时）；显式 1/on 开、0 关。
    #[test]
    fn nav_follow_flow_links_flag_defaults_off_and_parses() {
        let saved = std::env::var("TRPG_NAV_FOLLOW_FLOW_LINKS").ok();
        std::env::remove_var("TRPG_NAV_FOLLOW_FLOW_LINKS");
        assert!(!nav_follow_flow_links_enabled(), "未设 ⇒ 默认 OFF（字节等价基线）");
        std::env::set_var("TRPG_NAV_FOLLOW_FLOW_LINKS", "1");
        assert!(nav_follow_flow_links_enabled(), "\"1\" ⇒ ON");
        std::env::set_var("TRPG_NAV_FOLLOW_FLOW_LINKS", "on");
        assert!(nav_follow_flow_links_enabled(), "\"on\" ⇒ ON");
        std::env::set_var("TRPG_NAV_FOLLOW_FLOW_LINKS", "0");
        assert!(!nav_follow_flow_links_enabled(), "\"0\" ⇒ OFF");
        match saved {
            Some(v) => std::env::set_var("TRPG_NAV_FOLLOW_FLOW_LINKS", v),
            None => std::env::remove_var("TRPG_NAV_FOLLOW_FLOW_LINKS"),
        }
    }

    // C3：with_flow_link_clause(false) ⇒ 字节等价 base（OFF==baseline）。
    #[test]
    fn with_flow_link_clause_off_is_byte_equal() {
        let base = "GRAVITY_BASE_PROMPT".to_string();
        assert_eq!(with_flow_link_clause(base.clone(), false), base);
    }

    // C2：with_flow_link_clause(true) ⇒ 以 base 为前缀追加⑥，含脊优先 + 反铁路 + fail-closed。
    #[test]
    fn with_flow_link_clause_on_appends_anti_railroad_clause() {
        let base = "GRAVITY_BASE_PROMPT".to_string();
        let p = with_flow_link_clause(base.clone(), true);
        assert!(p.starts_with(&base), "ON 必须以 base 为前缀（纯追加）");
        assert!(p.len() > base.len(), "ON 严格更长");
        assert!(p.contains("已授权流转脊"), "应含脊优先指令");
        assert!(p.contains("反铁路"), "必须含反铁路守卫");
        assert!(
            p.contains("绝不 teleport") || p.contains("moved=false"),
            "反铁路：玩家跑偏不可强推/teleport"
        );
        assert!(p.contains("fail-closed") || p.contains("绝不编造"), "保留 fail-closed");
    }
}
