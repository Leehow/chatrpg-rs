//! 倒置三线索 validator(The Alexandrian):入口 BFS 连通性 + 孤岛/死胡同/不可达检测。纯、可单测。
use std::collections::{HashSet, VecDeque};
use trpg_model::ScenarioNode;

#[derive(Debug, Clone, PartialEq)]
pub struct GraphHealth {
    pub total: usize,
    pub reachable: usize,
    pub orphans: Vec<String>,   // 入口不可达
    pub islands: Vec<String>,   // 无入边
    pub dead_ends: Vec<String>, // 无出边
    pub score: f64,             // reachable / total
    pub ok: bool,               // score>=0.6 且 orphans 不过半
}

/// 从 entry_id 沿 links BFS,算连通健康度。entry 不存在或空图 → score 0、ok=false。
pub fn validate_graph(scenes: &[ScenarioNode], entry_id: &str) -> GraphHealth {
    let total = scenes.len();
    let idx: std::collections::HashMap<&str, usize> = scenes
        .iter()
        .enumerate()
        .map(|(i, s)| (s.node_id.as_str(), i))
        .collect();
    // BFS
    let mut seen: HashSet<usize> = HashSet::new();
    if let Some(&start) = idx.get(entry_id) {
        let mut q = VecDeque::from([start]);
        seen.insert(start);
        while let Some(u) = q.pop_front() {
            for l in &scenes[u].links {
                if let Some(&v) = idx.get(l.to_node_id.as_str()) {
                    if seen.insert(v) {
                        q.push_back(v);
                    }
                }
            }
        }
    }
    // 入边集
    let mut has_inbound: HashSet<usize> = HashSet::new();
    for s in scenes {
        for l in &s.links {
            if let Some(&v) = idx.get(l.to_node_id.as_str()) {
                has_inbound.insert(v);
            }
        }
    }
    let orphans: Vec<String> = scenes
        .iter()
        .enumerate()
        .filter(|(i, _)| !seen.contains(i))
        .map(|(_, s)| s.node_id.clone())
        .collect();
    let islands: Vec<String> = scenes
        .iter()
        .enumerate()
        .filter(|(i, _)| !has_inbound.contains(i))
        .map(|(_, s)| s.node_id.clone())
        .collect();
    let dead_ends: Vec<String> = scenes
        .iter()
        .filter(|s| s.links.is_empty())
        .map(|s| s.node_id.clone())
        .collect();
    let score = if total == 0 {
        0.0
    } else {
        seen.len() as f64 / total as f64
    };
    let ok = total > 0 && score >= 0.6 && orphans.len() * 2 <= total;
    GraphHealth {
        total,
        reachable: seen.len(),
        orphans,
        islands,
        dead_ends,
        score,
        ok,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::{LinkType, ScenarioLink};
    fn linked(id: &str, to: &[&str]) -> ScenarioNode {
        let mut n = ScenarioNode::default();
        n.node_id = id.into();
        n.links = to
            .iter()
            .map(|t| ScenarioLink {
                to_node_id: t.to_string(),
                reason: "".into(),
                clue_id: None,
                link_type: LinkType::Spatial,
                source_anchor: None,
            })
            .collect();
        n
    }
    #[test]
    fn connected_graph_is_ok() {
        let scenes = vec![linked("a", &["b"]), linked("b", &["c"]), linked("c", &[])];
        let h = validate_graph(&scenes, "a");
        assert_eq!(h.reachable, 3);
        assert!((h.score - 1.0).abs() < 1e-9);
        assert!(h.ok);
        assert_eq!(h.dead_ends, vec!["c"], "c 无出边=死胡同");
        assert_eq!(h.islands, vec!["a"], "a 无入边=island");
    }
    #[test]
    fn disconnected_flags_orphans_not_ok() {
        // a→b 连通;c,d 孤立(入口不可达)
        let scenes = vec![
            linked("a", &["b"]),
            linked("b", &[]),
            linked("c", &[]),
            linked("d", &[]),
        ];
        let h = validate_graph(&scenes, "a");
        assert_eq!(h.reachable, 2);
        assert_eq!(h.orphans, vec!["c", "d"]);
        assert!(!h.ok, "半数不可达 → 不 ok");
    }
    #[test]
    fn empty_or_missing_entry_not_ok() {
        assert!(!validate_graph(&[], "x").ok);
        assert!(!validate_graph(&[linked("a", &[])], "nope").ok);
    }
}
